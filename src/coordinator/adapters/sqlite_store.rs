//! Adapter: SQLite implementation of [`JobsStorePort`].
//!
//! Wraps [`store::Pool`] and translates between the store's row models +
//! domain types like [`RepoPolicy`]. Applies the env scrub at the
//! persistence boundary so secrets never reach disk.

use crate::coordinator::clean_policy::RepoPolicy;
use crate::coordinator::ports::JobsStorePort;
use crate::store::models::{InvocationRow, JobRow, RepoPolicyRow};
use crate::store::repos::{InvocationsRepo, JobsRepo, RepoPoliciesRepo};
use crate::store::{Pool, env_scrub};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

/// Files left over from the redb-era store. Removed (with a one-line
/// stderr banner) the first time `for_repo_base` opens the per-repo state
/// directory. Matches the pre-1.0 no-back-compat principle: cleanup is
/// loud, not silent.
const LEGACY_FILES: &[&str] = &["coordinator.redb", "repo-policy.json"];

/// SQLite-backed JobsStorePort. Cheap to clone — shares the underlying
/// [`Pool`] via `Arc`.
#[derive(Clone)]
pub struct SqliteJobsStore {
    pool: Arc<Pool>,
}

impl SqliteJobsStore {
    pub fn new(pool: Pool) -> Self {
        Self {
            pool: Arc::new(pool),
        }
    }

    /// Convenience: open the per-repo SQLite database that sits inside
    /// `<base>/coordinator.db` and remove any legacy redb-era files that
    /// are still sitting next to it (`coordinator.redb`,
    /// `repo-policy.json`). Counterpart to the old
    /// `JobStore::open_for_repo_base`.
    pub fn for_repo_base(base: &Path) -> Result<Self> {
        wipe_legacy_files(base);
        let db_path = base.join(crate::store::paths::COORDINATOR_DB);
        let pool = Pool::open(&db_path)
            .with_context(|| format!("open coordinator store at {}", db_path.display()))?;
        Ok(Self::new(pool))
    }

    /// Borrow the underlying pool. Useful for code paths that legitimately
    /// need a raw reader checkout (today: the tab-completion hot path).
    /// Domain code must NOT call this — use the port methods instead.
    pub fn pool(&self) -> &Pool {
        &self.pool
    }
}

fn wipe_legacy_files(base: &Path) {
    for name in LEGACY_FILES {
        let p = base.join(name);
        if !p.exists() {
            continue;
        }
        match std::fs::remove_file(&p) {
            Ok(()) => eprintln!(
                "daft: removed legacy {} at {} (state format changed; see CHANGELOG)",
                name,
                p.display()
            ),
            Err(e) => eprintln!(
                "daft: warning: failed to remove legacy {}: {e}",
                p.display()
            ),
        }
    }
}

impl std::fmt::Debug for SqliteJobsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteJobsStore")
            .field("path", &self.pool.path())
            .finish()
    }
}

impl JobsStorePort for SqliteJobsStore {
    fn upsert_invocation(&self, row: &InvocationRow) -> Result<()> {
        let conn = self.pool.writer().context("checkout writer")?;
        InvocationsRepo::upsert(&conn, row).map_err(anyhow::Error::from)
    }

    fn get_invocation(
        &self,
        repo_hash: &str,
        invocation_id: &str,
    ) -> Result<Option<InvocationRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        InvocationsRepo::get(&conn, repo_hash, invocation_id).map_err(anyhow::Error::from)
    }

    fn upsert_job(&self, row: &JobRow) -> Result<()> {
        // Scrub secrets before they touch disk. This is the canonical
        // persistence boundary for job rows; every JobsStorePort write goes
        // through here.
        let scrubbed_env = env_scrub::scrub(&row.env);
        let mut to_write = row.clone();
        to_write.env = scrubbed_env;
        let conn = self.pool.writer().context("checkout writer")?;
        JobsRepo::upsert(&conn, &to_write).map_err(anyhow::Error::from)
    }

    fn get_job(&self, repo_hash: &str, invocation_id: &str, name: &str) -> Result<Option<JobRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        JobsRepo::get(&conn, repo_hash, invocation_id, name).map_err(anyhow::Error::from)
    }

    fn list_jobs_for_repo(&self, repo_hash: &str) -> Result<Vec<JobRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        JobsRepo::list_by_repo(&conn, repo_hash).map_err(anyhow::Error::from)
    }

    fn list_jobs_for_invocation(
        &self,
        repo_hash: &str,
        invocation_id: &str,
    ) -> Result<Vec<JobRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        JobsRepo::list_by_invocation(&conn, repo_hash, invocation_id).map_err(anyhow::Error::from)
    }

    fn list_active_jobs(&self, repo_hash: &str) -> Result<Vec<JobRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        JobsRepo::list_by_repo_and_statuses(&conn, repo_hash, &["running", "cancelling"])
            .map_err(anyhow::Error::from)
    }

    fn read_repo_policy(&self, repo_hash: &str) -> Result<RepoPolicy> {
        let conn = self.pool.reader().context("checkout reader")?;
        let row = RepoPoliciesRepo::get(&conn, repo_hash)?;
        Ok(row.map(row_to_policy).unwrap_or_else(RepoPolicy::defaults))
    }

    fn write_repo_policy(&self, repo_hash: &str, policy: &RepoPolicy) -> Result<()> {
        // Merge-write semantics: pull existing values first so `None`
        // fields in `policy` don't wipe persisted tuning.
        let on_disk = self.read_repo_policy(repo_hash)?;
        let merged = RepoPolicy {
            version: policy.version,
            max_total_size_bytes: policy.max_total_size_bytes.or(on_disk.max_total_size_bytes),
            keep_last: policy.keep_last.or(on_disk.keep_last),
            stale_running_after_seconds: policy
                .stale_running_after_seconds
                .or(on_disk.stale_running_after_seconds),
        };
        let row = policy_to_row(repo_hash, &merged);
        let conn = self.pool.writer().context("checkout writer")?;
        RepoPoliciesRepo::upsert(&conn, &row).map_err(anyhow::Error::from)
    }
}

fn row_to_policy(row: RepoPolicyRow) -> RepoPolicy {
    RepoPolicy {
        version: row.policy_version,
        max_total_size_bytes: row.max_total_size_bytes,
        keep_last: row.keep_last,
        stale_running_after_seconds: row.stale_running_after_seconds,
    }
}

fn policy_to_row(repo_hash: &str, policy: &RepoPolicy) -> RepoPolicyRow {
    RepoPolicyRow {
        repo_hash: repo_hash.to_string(),
        policy_version: policy.version,
        max_total_size_bytes: policy.max_total_size_bytes,
        keep_last: policy.keep_last,
        stale_running_after_seconds: policy.stale_running_after_seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, SqliteJobsStore) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let pool = Pool::open(&path).unwrap();
        (tmp, SqliteJobsStore::new(pool))
    }

    fn sample_inv() -> InvocationRow {
        InvocationRow {
            repo_hash: "r".into(),
            invocation_id: "inv".into(),
            trigger_command: "daft co -b foo".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/foo".into(),
            created_at: Utc::now(),
            coordinator_pid: Some(42),
        }
    }

    fn sample_job_with_env(env: HashMap<String, String>) -> JobRow {
        JobRow {
            repo_hash: "r".into(),
            invocation_id: "inv".into(),
            name: "build".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/foo".into(),
            command: "echo hi".into(),
            working_dir: "/tmp".into(),
            env,
            started_at: Utc::now(),
            finished_at: None,
            status: "running".into(),
            exit_code: None,
            pid: Some(1234),
            pgid: Some(1234),
            background: true,
            needs: vec![],
            tags: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
        }
    }

    #[test]
    fn upsert_job_strips_secrets_from_env() {
        let (_tmp, store) = fresh();
        store.upsert_invocation(&sample_inv()).unwrap();
        let env = HashMap::from([
            ("GH_TOKEN".to_string(), "secret".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ]);
        store.upsert_job(&sample_job_with_env(env)).unwrap();
        let back = store.get_job("r", "inv", "build").unwrap().unwrap();
        assert!(
            !back.env.contains_key("GH_TOKEN"),
            "scrub failed: {:?}",
            back.env
        );
        assert_eq!(back.env.get("PATH"), Some(&"/usr/bin".to_string()));
    }

    #[test]
    fn read_repo_policy_returns_defaults_when_missing() {
        let (_tmp, store) = fresh();
        let p = store.read_repo_policy("r").unwrap();
        assert_eq!(p, RepoPolicy::defaults());
    }

    #[test]
    fn write_repo_policy_merges_none_fields() {
        let (_tmp, store) = fresh();
        let first = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(7),
            stale_running_after_seconds: None,
        };
        store.write_repo_policy("r", &first).unwrap();
        let second = RepoPolicy::defaults();
        store.write_repo_policy("r", &second).unwrap();
        let back = store.read_repo_policy("r").unwrap();
        assert_eq!(back.max_total_size_bytes, Some(100 * 1024 * 1024));
        assert_eq!(back.keep_last, Some(7));
    }

    #[test]
    fn list_active_jobs_filters_to_running_and_cancelling() {
        let (_tmp, store) = fresh();
        store.upsert_invocation(&sample_inv()).unwrap();
        for status in ["running", "completed", "cancelling", "cancelled", "crashed"] {
            let mut job = sample_job_with_env(HashMap::new());
            job.name = status.into();
            job.status = status.into();
            store.upsert_job(&job).unwrap();
        }
        let active = store.list_active_jobs("r").unwrap();
        let mut names: Vec<_> = active.iter().map(|j| j.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["cancelling", "running"]);
    }

    #[test]
    fn concurrent_open_allowed() {
        // This replaces the redb-era `concurrent_open_is_rejected_by_redb_lock`
        // test. SQLite + WAL is the whole reason we cut redb: two pools can
        // open the same file and the reader sees committed writes mid-flight.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let writer = SqliteJobsStore::new(Pool::open(&path).unwrap());
        writer.upsert_invocation(&sample_inv()).unwrap();
        let mut job = sample_job_with_env(HashMap::new());
        job.name = "first".into();
        writer.upsert_job(&job).unwrap();
        // Second store opens while the first is alive.
        let reader = SqliteJobsStore::new(Pool::open(&path).unwrap());
        let rows = reader.list_jobs_for_repo("r").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "first");
    }
}
