//! redb-backed durable state for the coordinator.
//!
//! Per-invocation lifecycle: the coordinator process is spawned per hook fire
//! and exits when its queue drains. Persistence here is so the *next*
//! invocation can reconcile what a previous (possibly-crashed) coordinator
//! left behind. Crash recovery marks any `Running`/`Cancelling` row whose
//! process group no longer exists as `Crashed`.
//!
//! Tables and key shape live in [`schema`]; values in [`types`].

use anyhow::{Context, Result, anyhow};
use redb::{Database, ReadableDatabase, ReadableTable, ReadableTableMetadata};
use std::path::Path;
use std::sync::Arc;

use crate::coordinator::log_store::JobStatus;

pub mod schema;
pub mod types;

pub use types::{InvocationRow, JobRow, RepoPolicyRow};

/// Durable coordinator state. Cheap to clone — wraps the redb `Database`
/// in an `Arc`. redb itself serializes concurrent writes.
#[derive(Clone)]
pub struct JobStore {
    db: Arc<Database>,
}

impl std::fmt::Debug for JobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobStore").finish_non_exhaustive()
    }
}

impl JobStore {
    /// Open or create the database at `path`. Verifies schema-version compat
    /// and sets the version sentinel for fresh databases.
    ///
    /// If the stored schema version is *higher* than [`schema::SCHEMA_VERSION`],
    /// `open` returns an error — older binaries refuse to write data they
    /// don't fully understand. Reads of known fields would still succeed if a
    /// caller opened a `ReadTransaction` directly, but that path is
    /// intentionally not exposed.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent of redb at {}", path.display()))?;
        }
        let db = Database::create(path)
            .with_context(|| format!("open coordinator redb at {}", path.display()))?;
        let store = Self { db: Arc::new(db) };
        store.check_schema_version()?;
        Ok(store)
    }

    fn check_schema_version(&self) -> Result<()> {
        let write_txn = self
            .db
            .begin_write()
            .context("begin write txn for schema check")?;
        {
            let mut meta = write_txn
                .open_table(schema::META)
                .context("open meta table")?;
            // Read the value and drop the access guard before any insert.
            let stored = {
                let guard = meta
                    .get(schema::SCHEMA_VERSION_KEY)
                    .context("read schema_version")?;
                guard.map(|g| g.value())
            };
            match stored {
                Some(on_disk) if on_disk > schema::SCHEMA_VERSION => {
                    return Err(anyhow!(
                        "coordinator store on-disk schema version {} is newer than this \
                         binary's {} — refusing to write (upgrade the binary)",
                        on_disk,
                        schema::SCHEMA_VERSION
                    ));
                }
                Some(_) => {
                    // Equal or older — proceed. Lower-version migrations
                    // would run here once we have any.
                }
                None => {
                    meta.insert(schema::SCHEMA_VERSION_KEY, schema::SCHEMA_VERSION)
                        .context("write schema_version")?;
                }
            }
            // Touch the other tables so subsequent read-only transactions
            // (e.g. `read_repo_policy` on a fresh db) don't fail with
            // "Table 'X' does not exist". `open_table` on a WriteTransaction
            // creates the table if absent.
            let _ = write_txn
                .open_table(schema::INVOCATIONS)
                .context("ensure invocations table exists")?;
            let _ = write_txn
                .open_table(schema::JOBS)
                .context("ensure jobs table exists")?;
            let _ = write_txn
                .open_table(schema::REPO_POLICY)
                .context("ensure repo_policy table exists")?;
        }
        write_txn.commit().context("commit schema-version txn")?;
        Ok(())
    }

    /// Convenience: open the redb file inside `<base>/coordinator.redb` for a
    /// `LogStore` base directory, then run the one-shot legacy
    /// `repo-policy.json` migration. Callers outside the coordinator path
    /// (foreground hooks, `daft hooks jobs prune`, cleanup) use this so they
    /// don't have to know the redb filename or the legacy sidecar layout.
    pub fn open_for_repo_base(repo_hash: &str, base: &Path) -> Result<Self> {
        let store = Self::open(&base.join("coordinator.redb"))?;
        store.migrate_repo_policy_from_json(repo_hash, &base.join("repo-policy.json"));
        Ok(store)
    }

    pub fn upsert_invocation(&self, row: &InvocationRow) -> Result<()> {
        let key = types::invocation_key(&row.repo_hash, &row.invocation_id);
        let bytes = bincode::serialize(row).context("serialize InvocationRow")?;
        let write_txn = self.db.begin_write()?;
        {
            let mut t = write_txn.open_table(schema::INVOCATIONS)?;
            t.insert(key.as_str(), bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn read_invocation(
        &self,
        repo_hash: &str,
        invocation_id: &str,
    ) -> Result<Option<InvocationRow>> {
        let key = types::invocation_key(repo_hash, invocation_id);
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(schema::INVOCATIONS)?;
        let Some(v) = t.get(key.as_str())? else {
            return Ok(None);
        };
        let row: InvocationRow = bincode::deserialize(v.value()).context("decode InvocationRow")?;
        Ok(Some(row))
    }

    pub fn upsert_job(&self, row: &JobRow) -> Result<()> {
        let key = types::job_key(&row.repo_hash, &row.invocation_id, &row.name);
        let bytes = bincode::serialize(row).context("serialize JobRow")?;
        let write_txn = self.db.begin_write()?;
        {
            let mut t = write_txn.open_table(schema::JOBS)?;
            t.insert(key.as_str(), bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    pub fn get_job(
        &self,
        repo_hash: &str,
        invocation_id: &str,
        job_name: &str,
    ) -> Result<Option<JobRow>> {
        let key = types::job_key(repo_hash, invocation_id, job_name);
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(schema::JOBS)?;
        let Some(v) = t.get(key.as_str())? else {
            return Ok(None);
        };
        Ok(Some(
            bincode::deserialize(v.value()).context("decode JobRow")?,
        ))
    }

    /// All jobs for a repo across every invocation. Used by `daft hooks jobs ls`.
    pub fn list_jobs_for_repo(&self, repo_hash: &str) -> Result<Vec<JobRow>> {
        let prefix = format!("{repo_hash}:");
        let upper = format!("{repo_hash};"); // ':' is 0x3A, ';' is 0x3B — first char > ':' for prefix scan
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(schema::JOBS)?;
        let mut out = Vec::new();
        for entry in t.range(prefix.as_str()..upper.as_str())? {
            let (_, v) = entry?;
            let row: JobRow = bincode::deserialize(v.value()).context("decode JobRow")?;
            out.push(row);
        }
        Ok(out)
    }

    /// Jobs whose current status is `Running` or `Cancelling`. Used by the
    /// reconciliation pass on coordinator boot.
    pub fn list_active_jobs(&self, repo_hash: &str) -> Result<Vec<JobRow>> {
        Ok(self
            .list_jobs_for_repo(repo_hash)?
            .into_iter()
            .filter(|r| matches!(r.status, JobStatus::Running | JobStatus::Cancelling))
            .collect())
    }

    /// Returns the count of rows in the `JOBS` table. Test/diagnostic helper.
    #[allow(dead_code)]
    pub fn job_count(&self) -> Result<u64> {
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(schema::JOBS)?;
        Ok(t.len()?)
    }

    /// Read the per-repo cleanup policy. Returns `defaults()` when no row
    /// exists — matches the previous JSON sidecar behavior.
    pub fn read_repo_policy(
        &self,
        repo_hash: &str,
    ) -> Result<crate::coordinator::clean_policy::RepoPolicy> {
        let key = types::repo_policy_key(repo_hash);
        let read_txn = self.db.begin_read()?;
        let t = read_txn.open_table(schema::REPO_POLICY)?;
        let Some(v) = t.get(key.as_str())? else {
            return Ok(crate::coordinator::clean_policy::RepoPolicy::defaults());
        };
        let row: RepoPolicyRow = bincode::deserialize(v.value()).context("decode RepoPolicyRow")?;
        Ok(row.to_policy())
    }

    /// Persist the per-repo cleanup policy. Field-merges with the on-disk
    /// values: explicit `Some(_)` in `policy` wins; `None` preserves the
    /// stored value. Mirrors the previous JSON-sidecar behavior so hooks
    /// without a `log:` block (which produce an all-`None` policy) don't
    /// silently wipe persisted tuning.
    pub fn write_repo_policy(
        &self,
        repo_hash: &str,
        policy: &crate::coordinator::clean_policy::RepoPolicy,
    ) -> Result<()> {
        let on_disk = self.read_repo_policy(repo_hash)?;
        let merged = crate::coordinator::clean_policy::RepoPolicy {
            version: policy.version,
            max_total_size_bytes: policy.max_total_size_bytes.or(on_disk.max_total_size_bytes),
            keep_last: policy.keep_last.or(on_disk.keep_last),
            stale_running_after_seconds: policy
                .stale_running_after_seconds
                .or(on_disk.stale_running_after_seconds),
        };
        let row = RepoPolicyRow::from_policy(&merged);
        let bytes = bincode::serialize(&row).context("serialize RepoPolicyRow")?;
        let key = types::repo_policy_key(repo_hash);
        let write_txn = self.db.begin_write()?;
        {
            let mut t = write_txn.open_table(schema::REPO_POLICY)?;
            t.insert(key.as_str(), bytes.as_slice())?;
        }
        write_txn.commit()?;
        Ok(())
    }

    /// One-shot migration of a legacy `repo-policy.json` sidecar into redb.
    /// Safe to call repeatedly: skips when the file is gone or the redb row
    /// already exists. After successful ingest the sidecar is deleted.
    ///
    /// Errors during parse, write, or delete are reported via `eprintln!` and
    /// swallowed — migration is best-effort, the next hook fire will rewrite
    /// the policy anyway.
    pub fn migrate_repo_policy_from_json(&self, repo_hash: &str, json_path: &Path) {
        if !json_path.exists() {
            return;
        }
        // If redb already has a row, don't overwrite it with stale JSON.
        let has_row = match self.read_repo_policy(repo_hash) {
            Ok(p) => p != crate::coordinator::clean_policy::RepoPolicy::defaults(),
            Err(e) => {
                eprintln!(
                    "daft: warning: probing redb repo policy during migration of {}: {e}",
                    json_path.display()
                );
                return;
            }
        };
        if !has_row {
            let json = match std::fs::read_to_string(json_path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "daft: warning: failed to read legacy repo policy {}: {e}",
                        json_path.display()
                    );
                    return;
                }
            };
            let policy: crate::coordinator::clean_policy::RepoPolicy =
                match serde_json::from_str(&json) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!(
                            "daft: warning: failed to parse legacy repo policy {}: {e}",
                            json_path.display()
                        );
                        return;
                    }
                };
            if let Err(e) = self.write_repo_policy(repo_hash, &policy) {
                eprintln!(
                    "daft: warning: failed to migrate repo policy {} into redb: {e}",
                    json_path.display()
                );
                return;
            }
        }
        // File ingested (or skipped because redb already authoritative);
        // delete the sidecar so we never re-ingest it on a later open.
        if let Err(e) = std::fs::remove_file(json_path) {
            eprintln!(
                "daft: warning: failed to remove migrated repo policy {} : {e}",
                json_path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn sample_job(repo: &str, inv: &str, name: &str) -> JobRow {
        JobRow {
            schema_version: schema::SCHEMA_VERSION,
            repo_hash: repo.into(),
            invocation_id: inv.into(),
            name: name.into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/test".into(),
            command: "echo hi".into(),
            working_dir: "/tmp".into(),
            env: HashMap::new(),
            started_at: Utc::now(),
            finished_at: None,
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(12345),
            pgid: Some(12345),
            background: true,
            needs: vec![],
            tags: vec!["slow".into()],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        }
    }

    #[test]
    fn upsert_and_get_job_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();
        let row = sample_job("repohash", "inv1", "build");
        store.upsert_job(&row).unwrap();
        let back = store.get_job("repohash", "inv1", "build").unwrap().unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn list_jobs_for_repo_scopes_by_prefix() {
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();
        store
            .upsert_job(&sample_job("repoA", "inv1", "a1"))
            .unwrap();
        store
            .upsert_job(&sample_job("repoA", "inv1", "a2"))
            .unwrap();
        store
            .upsert_job(&sample_job("repoB", "inv1", "b1"))
            .unwrap();
        let a = store.list_jobs_for_repo("repoA").unwrap();
        let b = store.list_jobs_for_repo("repoB").unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn list_active_jobs_filters_to_running_and_cancelling() {
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();
        let mut running = sample_job("r", "i", "running");
        let mut completed = sample_job("r", "i", "completed");
        completed.status = JobStatus::Completed;
        let mut cancelling = sample_job("r", "i", "cancelling");
        cancelling.status = JobStatus::Cancelling;
        let mut cancelled = sample_job("r", "i", "cancelled");
        cancelled.status = JobStatus::Cancelled;
        let mut crashed = sample_job("r", "i", "crashed");
        crashed.status = JobStatus::Crashed;
        for r in [
            &mut running,
            &mut completed,
            &mut cancelling,
            &mut cancelled,
            &mut crashed,
        ] {
            store.upsert_job(r).unwrap();
        }
        let active = store.list_active_jobs("r").unwrap();
        let mut names: Vec<_> = active.iter().map(|j| j.name.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["cancelling", "running"]);
    }

    #[test]
    fn schema_version_higher_than_binary_refuses_open() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("coordinator.redb");

        // Write a future schema_version directly via redb.
        {
            let db = Database::create(&path).unwrap();
            let write_txn = db.begin_write().unwrap();
            {
                let mut t = write_txn.open_table(schema::META).unwrap();
                t.insert(schema::SCHEMA_VERSION_KEY, schema::SCHEMA_VERSION + 99)
                    .unwrap();
            }
            write_txn.commit().unwrap();
        }

        let err = JobStore::open(&path).unwrap_err();
        assert!(
            err.to_string().contains("newer than this binary"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn invocation_round_trips() {
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();
        let row = InvocationRow {
            schema_version: schema::SCHEMA_VERSION,
            repo_hash: "rh".into(),
            invocation_id: "abc123".into(),
            trigger_command: "daft co -b foo".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/foo".into(),
            created_at: Utc::now(),
            coordinator_pid: Some(999),
        };
        store.upsert_invocation(&row).unwrap();
        let back = store.read_invocation("rh", "abc123").unwrap().unwrap();
        assert_eq!(back, row);
    }

    #[test]
    fn repo_policy_round_trips_through_redb() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();

        let policy = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(7),
            stale_running_after_seconds: Some(120),
        };
        store.write_repo_policy("repoA", &policy).unwrap();
        let back = store.read_repo_policy("repoA").unwrap();
        assert_eq!(back, policy);
    }

    #[test]
    fn repo_policy_missing_returns_defaults() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();
        let p = store.read_repo_policy("repoX").unwrap();
        assert_eq!(p, RepoPolicy::defaults());
    }

    #[test]
    fn write_repo_policy_preserves_unset_fields_from_on_disk() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();

        let first = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(5),
            stale_running_after_seconds: None,
        };
        store.write_repo_policy("r", &first).unwrap();

        // Second write with all-None values (hook without a log: block) must
        // not clobber the persisted tuning.
        let second = RepoPolicy::defaults();
        store.write_repo_policy("r", &second).unwrap();

        let read = store.read_repo_policy("r").unwrap();
        assert_eq!(read.max_total_size_bytes, Some(100 * 1024 * 1024));
        assert_eq!(read.keep_last, Some(5));
    }

    #[test]
    fn write_repo_policy_overrides_explicitly_set_fields() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let store = JobStore::open(&tmp.path().join("coordinator.redb")).unwrap();

        let first = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(5),
            stale_running_after_seconds: None,
        };
        store.write_repo_policy("r", &first).unwrap();

        let second = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(200 * 1024 * 1024),
            keep_last: None,
            stale_running_after_seconds: None,
        };
        store.write_repo_policy("r", &second).unwrap();

        let read = store.read_repo_policy("r").unwrap();
        assert_eq!(read.max_total_size_bytes, Some(200 * 1024 * 1024));
        assert_eq!(read.keep_last, Some(5), "unset preserves on-disk");
    }

    #[test]
    fn repo_policy_migration_imports_json_once_and_deletes_file() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();
        let json_path = base.join("repo-policy.json");

        let legacy = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(50 * 1024 * 1024),
            keep_last: Some(11),
            stale_running_after_seconds: Some(7200),
        };
        std::fs::write(&json_path, serde_json::to_string(&legacy).unwrap()).unwrap();

        {
            let store = JobStore::open_for_repo_base("repohash", base).unwrap();
            assert!(!json_path.exists(), "migration should delete sidecar");
            let back = store.read_repo_policy("repohash").unwrap();
            assert_eq!(back, legacy);
        }

        // Second open with no file is a no-op.
        let store2 = JobStore::open_for_repo_base("repohash", base).unwrap();
        let back2 = store2.read_repo_policy("repohash").unwrap();
        assert_eq!(back2, legacy);
    }

    /// redb v4 enforces a single-writer-and-single-reader file lock at open
    /// time — a second `JobStore::open` on the same file fails while the
    /// first handle is alive. This shapes the CLI reader path: while a
    /// coordinator is alive holding the file, `load_redb_job_meta_index`
    /// returns `None` and `list_jobs()` / `show_logs` fall back to
    /// `meta.json`. The fallback covers the common case (cancel/complete
    /// updates write to both stores) but leaves `Crashed` reconciliation
    /// invisible until the live coordinator drains. Documented as a
    /// known limitation; fixing it would require open-and-close-per-op on
    /// the coordinator side, a different DB, or a coordinator IPC for
    /// metadata reads.
    #[test]
    fn concurrent_open_is_rejected_by_redb_lock() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("concurrent.redb");
        let _first = JobStore::open(&p).unwrap();
        let second = JobStore::open(&p);
        assert!(
            second.is_err(),
            "redb's process-level lock should reject a concurrent open; if this assertion ever \
             fires, the load_redb_job_meta_index fallback rationale needs revisiting"
        );
    }

    #[test]
    fn repo_policy_migration_skips_when_redb_row_present() {
        use crate::coordinator::clean_policy::RepoPolicy;
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        // Seed redb with one policy.
        let redb_policy = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(300 * 1024 * 1024),
            keep_last: Some(9),
            stale_running_after_seconds: None,
        };
        {
            let store = JobStore::open(&base.join("coordinator.redb")).unwrap();
            store.write_repo_policy("repohash", &redb_policy).unwrap();
        }

        // Also drop a (stale) sidecar with different values.
        let json_path = base.join("repo-policy.json");
        let legacy = RepoPolicy {
            version: RepoPolicy::VERSION,
            max_total_size_bytes: Some(1024),
            keep_last: Some(1),
            stale_running_after_seconds: None,
        };
        std::fs::write(&json_path, serde_json::to_string(&legacy).unwrap()).unwrap();

        // Migration via open_for_repo_base must not overwrite redb's row.
        let store = JobStore::open_for_repo_base("repohash", base).unwrap();
        // The sidecar is still removed (we never want to re-ingest it later).
        assert!(!json_path.exists());
        let back = store.read_repo_policy("repohash").unwrap();
        assert_eq!(back, redb_policy, "redb wins over stale sidecar");
    }
}
