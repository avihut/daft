//! Adapter: SQLite implementation of [`JobsStorePort`].
//!
//! Wraps [`store::Pool`] and translates between the store's row models +
//! domain types like [`RepoPolicy`]. Applies the env scrub at the
//! persistence boundary so secrets never reach disk.

use crate::coordinator::clean_policy::RepoPolicy;
use crate::coordinator::ports::{JobsStorePort, SeedsStorePort};
use crate::store::models::{InvocationRow, JobRow, RepoPolicyRow, VisitorSeedRow};
use crate::store::repos::{InvocationsRepo, JobsRepo, RepoPoliciesRepo, VisitorSeedsRepo};
use crate::store::{Pool, env_scrub};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

/// Files left over from the redb-era store. Removed (with a one-line
/// stderr banner) by [`SqliteJobsStore::for_repo_base_with_wipe`] — which is
/// called only from coordinator startup. The completion / CLI-reader path
/// uses [`SqliteJobsStore::for_repo_base`] (no wipe) so the banner cannot
/// surface mid-`Tab` press. Matches the pre-1.0 no-back-compat principle:
/// cleanup is loud, but only where loud is allowed.
const LEGACY_FILES: &[&str] = &["coordinator.redb", "repo-policy.json"];

/// Per-job sidecar files written by the pre-cutover code. Swept inside
/// each invocation directory by the same coordinator-startup pass that
/// removes [`LEGACY_FILES`]. Production code no longer writes
/// `meta.json` — SQLite is the source of truth for job metadata — so
/// any file by that name on disk is leftover from a daft version that
/// dual-wrote during the SQLite migration.
const LEGACY_JOB_FILES: &[&str] = &["meta.json"];

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

    /// Open the per-repo SQLite database at `<base>/coordinator.db`.
    ///
    /// **Trust boundary:** `base` MUST be a per-repo state directory
    /// derived from `daft_state_dir()` (production callers pass
    /// `LogStore::base_dir`, which is constructed as
    /// `<daft_state_dir>/jobs/<repo_hash>`). The path is not canonicalized
    /// here because `daft_state_dir()` isn't reachable from this layer
    /// without coupling it to the application boundary; the assertion that
    /// the per-repo parent stays under the state dir is enforced upstream
    /// by [`crate::store::paths::for_repo_under`] (the canonical path
    /// resolver used by `LogStore::new`). New callers MUST not derive
    /// `base` from user input.
    ///
    /// Does NOT wipe legacy redb-era files — use
    /// [`Self::for_repo_base_with_wipe`] for coordinator-startup paths
    /// where the stderr banner is acceptable. Completion / CLI-reader
    /// paths call this constructor so a `Tab` press cannot leak text into
    /// the user's terminal.
    pub fn for_repo_base(base: &Path) -> Result<Self> {
        Self::open_at(base)
    }

    /// Coordinator-startup variant: opens the per-repo SQLite database
    /// after sweeping any legacy redb-era files (`coordinator.redb`,
    /// `repo-policy.json`, …) that may still be sitting next to it. The
    /// sweep emits a one-line stderr banner per file; called only from
    /// [`run_coordinator`](crate::coordinator::process::run_coordinator),
    /// which already writes diagnostics to stderr at startup time.
    pub fn for_repo_base_with_wipe(base: &Path) -> Result<Self> {
        wipe_legacy_files(base);
        Self::open_at(base)
    }

    fn open_at(base: &Path) -> Result<Self> {
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
    wipe_legacy_job_files(base);
}

/// Walk per-invocation / per-job directories and remove sidecar files
/// listed in [`LEGACY_JOB_FILES`] (currently `meta.json`). Each invocation
/// is one entry directly under `base`; each job is one entry under its
/// invocation. Walk depth is fixed at 2, so we don't recurse into job
/// dirs (which contain user-controlled `output.jsonl`).
fn wipe_legacy_job_files(base: &Path) {
    let Ok(inv_entries) = std::fs::read_dir(base) else {
        return;
    };
    for inv_entry in inv_entries.flatten() {
        if inv_entry.file_type().map(|t| !t.is_dir()).unwrap_or(true) {
            continue;
        }
        let inv_dir = inv_entry.path();
        let Ok(job_entries) = std::fs::read_dir(&inv_dir) else {
            continue;
        };
        for job_entry in job_entries.flatten() {
            if job_entry.file_type().map(|t| !t.is_dir()).unwrap_or(true) {
                continue;
            }
            let job_dir = job_entry.path();
            for name in LEGACY_JOB_FILES {
                let p = job_dir.join(name);
                if !p.exists() {
                    continue;
                }
                match std::fs::remove_file(&p) {
                    Ok(()) => eprintln!(
                        "daft: removed legacy {} at {} (SQLite is now the source of truth)",
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
        JobsRepo::list_by_repo_and_two_statuses(&conn, repo_hash, "running", "cancelling")
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

impl SeedsStorePort for SqliteJobsStore {
    fn record_seed(
        &self,
        repo_hash: &str,
        branch_slug: &str,
        filename: &str,
        content: &str,
    ) -> Result<()> {
        // Timestamps are computed here, at the persistence boundary (same
        // spirit as the env scrub on job rows). The repo's upsert preserves
        // the original `seeded_at` when a row already exists.
        let now = chrono::Utc::now();
        let row = VisitorSeedRow {
            repo_hash: repo_hash.to_string(),
            branch_slug: branch_slug.to_string(),
            filename: filename.to_string(),
            content: content.to_string(),
            seeded_at: now,
            updated_at: now,
        };
        let conn = self.pool.writer().context("checkout writer")?;
        VisitorSeedsRepo::upsert(&conn, &row).map_err(anyhow::Error::from)
    }

    fn get_seed(
        &self,
        repo_hash: &str,
        branch_slug: &str,
        filename: &str,
    ) -> Result<Option<VisitorSeedRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        VisitorSeedsRepo::get(&conn, repo_hash, branch_slug, filename).map_err(anyhow::Error::from)
    }

    fn delete_seed(&self, repo_hash: &str, branch_slug: &str, filename: &str) -> Result<()> {
        let conn = self.pool.writer().context("checkout writer")?;
        VisitorSeedsRepo::delete_one(&conn, repo_hash, branch_slug, filename)
            .map(|_| ())
            .map_err(anyhow::Error::from)
    }

    fn delete_seeds_for_branch(&self, repo_hash: &str, branch_slug: &str) -> Result<usize> {
        let conn = self.pool.writer().context("checkout writer")?;
        VisitorSeedsRepo::delete_for_branch(&conn, repo_hash, branch_slug)
            .map_err(anyhow::Error::from)
    }

    fn list_seeds_for_repo(&self, repo_hash: &str) -> Result<Vec<VisitorSeedRow>> {
        let conn = self.pool.reader().context("checkout reader")?;
        VisitorSeedsRepo::list_for_repo(&conn, repo_hash).map_err(anyhow::Error::from)
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

    #[test]
    fn for_repo_base_with_wipe_removes_legacy_files() {
        // Upgrading from the redb-era + dual-write design leaves three
        // categories of files on disk:
        //   1. `coordinator.redb`        (pre-PR-#508 redb store)
        //   2. `repo-policy.json`        (pre-PR-#508 sidecar)
        //   3. `<inv>/<job>/meta.json`   (PR-#508 transitional dual-write)
        // The coordinator-startup constructor must remove all three (the
        // sweep emits stderr banners in production; this test only
        // asserts the filesystem post-condition).
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        std::fs::write(base.join("coordinator.redb"), b"old redb bytes").unwrap();
        std::fs::write(base.join("repo-policy.json"), b"{}").unwrap();
        // Per-job sidecar from the dual-write era.
        let job_dir = base.join("inv-1").join("build");
        std::fs::create_dir_all(&job_dir).unwrap();
        std::fs::write(job_dir.join("meta.json"), b"{}").unwrap();

        let _store = SqliteJobsStore::for_repo_base_with_wipe(base).unwrap();

        assert!(
            !base.join("coordinator.redb").exists(),
            "legacy coordinator.redb should be wiped on first open"
        );
        assert!(
            !base.join("repo-policy.json").exists(),
            "legacy repo-policy.json should be wiped on first open"
        );
        assert!(
            !job_dir.join("meta.json").exists(),
            "legacy per-job meta.json should be wiped on first open"
        );
        // The job dir itself stays (it may still hold an output.jsonl).
        assert!(job_dir.exists(), "job dir survives the legacy-file sweep");
        // The new SQLite DB landed.
        assert!(
            base.join("coordinator.db").exists(),
            "new coordinator.db should exist after for_repo_base_with_wipe"
        );
    }

    #[test]
    fn for_repo_base_does_not_wipe_legacy_files() {
        // Completion / CLI-reader paths call `for_repo_base` (not
        // `_with_wipe`). Stderr from this path leaks into the user's
        // terminal because `__complete` runs inside `eval`-captured shell
        // code that only captures stdout. Regression guard: any legacy
        // file present must survive the open.
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        std::fs::write(base.join("coordinator.redb"), b"old redb bytes").unwrap();
        std::fs::write(base.join("repo-policy.json"), b"{}").unwrap();
        let job_dir = base.join("inv-1").join("build");
        std::fs::create_dir_all(&job_dir).unwrap();
        std::fs::write(job_dir.join("meta.json"), b"{}").unwrap();

        let _store = SqliteJobsStore::for_repo_base(base).unwrap();

        assert!(
            base.join("coordinator.redb").exists(),
            "non-wiping constructor must not touch legacy coordinator.redb"
        );
        assert!(
            base.join("repo-policy.json").exists(),
            "non-wiping constructor must not touch legacy repo-policy.json"
        );
        assert!(
            job_dir.join("meta.json").exists(),
            "non-wiping constructor must not touch legacy per-job meta.json"
        );
        // The new SQLite DB still landed.
        assert!(
            base.join("coordinator.db").exists(),
            "coordinator.db should exist after for_repo_base"
        );
    }

    #[test]
    fn seed_record_get_refresh_round_trip() {
        let (_tmp, store) = fresh();
        store
            .record_seed("repo", "feat/x", "daft.yml", "v1")
            .unwrap();
        let first = store
            .get_seed("repo", "feat/x", "daft.yml")
            .unwrap()
            .unwrap();
        assert_eq!(first.content, "v1");

        // Refresh: content moves, original seeded_at survives.
        store
            .record_seed("repo", "feat/x", "daft.yml", "v2")
            .unwrap();
        let second = store
            .get_seed("repo", "feat/x", "daft.yml")
            .unwrap()
            .unwrap();
        assert_eq!(second.content, "v2");
        assert_eq!(second.seeded_at, first.seeded_at);
    }

    #[test]
    fn seed_delete_for_branch_scopes_to_branch() {
        let (_tmp, store) = fresh();
        store
            .record_seed("repo", "feat/x", "daft.yml", "x")
            .unwrap();
        store
            .record_seed("repo", "feat/x", "daft.local.yml", "xl")
            .unwrap();
        store
            .record_seed("repo", "feat/y", "daft.yml", "y")
            .unwrap();

        assert_eq!(store.delete_seeds_for_branch("repo", "feat/x").unwrap(), 2);
        assert!(
            store
                .get_seed("repo", "feat/x", "daft.yml")
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_seed("repo", "feat/y", "daft.yml")
                .unwrap()
                .is_some()
        );

        let listed = store.list_seeds_for_repo("repo").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].branch_slug, "feat/y");
    }
}
