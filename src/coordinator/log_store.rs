use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobMeta {
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub command: String,
    pub working_dir: String,
    pub env: HashMap<String, String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    pub pid: Option<u32>,
    pub background: bool,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    pub needs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvocationMeta {
    pub invocation_id: String,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Manages background job log storage on disk.
///
/// Directory structure:
/// ```text
/// <base_dir>/
///   <invocation-id>/
///     <job-name>/
///       meta.json
///       output.log
/// ```
#[derive(Clone)]
pub struct LogStore {
    pub base_dir: PathBuf,
}

impl LogStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Returns the log store for a specific repository.
    pub fn for_repo(repo_hash: &str) -> Result<Self> {
        let base = crate::daft_state_dir()?.join("jobs").join(repo_hash);
        Ok(Self::new(base))
    }

    pub fn create_job_dir(&self, invocation_id: &str, job_name: &str) -> Result<PathBuf> {
        let dir = self.base_dir.join(invocation_id).join(job_name);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create job log dir: {}", dir.display()))?;
        Ok(dir)
    }

    pub fn write_meta(&self, job_dir: &Path, meta: &JobMeta) -> Result<()> {
        let path = job_dir.join("meta.json");
        let content = serde_json::to_string_pretty(meta)?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write job meta: {}", path.display()))?;
        Ok(())
    }

    pub fn read_meta(&self, job_dir: &Path) -> Result<JobMeta> {
        let path = job_dir.join("meta.json");
        let content = fs::read_to_string(&path)?;
        let meta: JobMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    pub fn log_path(job_dir: &Path) -> PathBuf {
        job_dir.join("output.log")
    }

    /// Write `meta.json` and `output.log` for a completed job atomically.
    ///
    /// Creates the job directory if needed. Used by `BufferingLogSink` (for
    /// foreground jobs) and by `yaml_executor` (for skipped job records).
    pub fn write_job_record(
        &self,
        invocation_id: &str,
        meta: &JobMeta,
        log_bytes: &[u8],
    ) -> Result<PathBuf> {
        let job_dir = self.create_job_dir(invocation_id, &meta.name)?;
        self.write_meta(&job_dir, meta)?;
        fs::write(Self::log_path(&job_dir), log_bytes)
            .with_context(|| format!("Failed to write log file for job: {}", meta.name))?;
        Ok(job_dir)
    }

    pub fn list_job_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = Vec::new();
        if !self.base_dir.exists() {
            return Ok(dirs);
        }
        for inv_entry in fs::read_dir(&self.base_dir)? {
            let inv_entry = inv_entry?;
            if inv_entry.file_type()?.is_dir() {
                for job_entry in fs::read_dir(inv_entry.path())? {
                    let job_entry = job_entry?;
                    if job_entry.file_type()?.is_dir() {
                        dirs.push(job_entry.path());
                    }
                }
            }
        }
        Ok(dirs)
    }

    pub fn clean(&self, max_age: chrono::Duration) -> Result<usize> {
        let cutoff = chrono::Utc::now() - max_age;
        let mut removed = 0;

        for job_dir in self.list_job_dirs()? {
            if let Ok(meta) = self.read_meta(&job_dir) {
                if meta.started_at < cutoff && !matches!(meta.status, JobStatus::Running) {
                    fs::remove_dir_all(&job_dir)?;
                    removed += 1;

                    // Clean up empty invocation dir
                    if let Some(parent) = job_dir.parent() {
                        let _ = fs::remove_dir(parent); // Only succeeds if empty
                    }
                }
            }
        }

        Ok(removed)
    }

    pub fn write_invocation_meta(&self, invocation_id: &str, meta: &InvocationMeta) -> Result<()> {
        let dir = self.base_dir.join(invocation_id);
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create invocation dir: {}", dir.display()))?;
        let path = dir.join("invocation.json");
        let content = serde_json::to_string_pretty(meta)?;
        fs::write(&path, content)?;
        Ok(())
    }

    pub fn read_invocation_meta(&self, invocation_id: &str) -> Result<InvocationMeta> {
        let path = self.base_dir.join(invocation_id).join("invocation.json");
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read invocation meta: {}", path.display()))?;
        let meta: InvocationMeta = serde_json::from_str(&content)?;
        Ok(meta)
    }

    pub fn list_invocations(&self) -> Result<Vec<InvocationMeta>> {
        let mut invocations = Vec::new();
        if !self.base_dir.exists() {
            return Ok(invocations);
        }
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let inv_id = entry.file_name().to_string_lossy().to_string();
                if let Ok(meta) = self.read_invocation_meta(&inv_id) {
                    invocations.push(meta);
                }
            }
        }
        invocations.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(invocations)
    }

    pub fn list_invocations_for_worktree(&self, worktree: &str) -> Result<Vec<InvocationMeta>> {
        let all = self.list_invocations()?;
        Ok(all.into_iter().filter(|m| m.worktree == worktree).collect())
    }

    pub fn find_invocations_by_prefix(
        &self,
        worktree: &str,
        prefix: &str,
    ) -> Result<Vec<InvocationMeta>> {
        let all = self.list_invocations_for_worktree(worktree)?;
        Ok(all
            .into_iter()
            .filter(|m| m.invocation_id.starts_with(prefix))
            .collect())
    }

    pub fn list_jobs_in_invocation(&self, invocation_id: &str) -> Result<Vec<PathBuf>> {
        let inv_dir = self.base_dir.join(invocation_id);
        let mut dirs = Vec::new();
        if !inv_dir.exists() {
            return Ok(dirs);
        }
        for entry in fs::read_dir(&inv_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.path());
            }
        }
        Ok(dirs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    #[test]
    fn test_create_job_log_dir() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("abc123", "warm-build").unwrap();
        assert!(dir.exists());
        assert!(dir.ends_with("abc123/warm-build"));
    }

    #[test]
    fn test_write_and_read_meta() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv1", "job1").unwrap();
        let meta = JobMeta {
            name: "job1".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "/path/to/wt".to_string(),
            command: "cargo build".to_string(),
            working_dir: "/path/to/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(12345),
            background: false,
            finished_at: None,
            needs: vec![],
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert_eq!(loaded.name, "job1");
        assert!(matches!(loaded.status, JobStatus::Running));
    }

    #[test]
    fn test_list_jobs_for_repo() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        store.create_job_dir("inv1", "job-a").unwrap();
        store.create_job_dir("inv1", "job-b").unwrap();
        store.create_job_dir("inv2", "job-c").unwrap();
        let jobs = store.list_job_dirs().unwrap();
        assert_eq!(jobs.len(), 3);
    }

    #[test]
    fn test_clean_old_logs() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("old-inv", "old-job").unwrap();
        let meta = JobMeta {
            name: "old-job".to_string(),
            hook_type: "post-clone".to_string(),
            worktree: "/tmp/wt".to_string(),
            command: "echo old".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now() - chrono::Duration::days(30),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: None,
            needs: vec![],
        };
        store.write_meta(&dir, &meta).unwrap();
        let removed = store.clean(chrono::Duration::days(7)).unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.exists());
    }

    #[test]
    fn test_write_and_read_invocation_meta() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        // Create the invocation directory (normally done by create_job_dir)
        std::fs::create_dir_all(tmp.path().join("inv1")).unwrap();

        let meta = InvocationMeta {
            invocation_id: "inv1".to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/tax-calc".to_string(),
            created_at: chrono::Utc::now(),
        };
        store.write_invocation_meta("inv1", &meta).unwrap();
        let loaded = store.read_invocation_meta("inv1").unwrap();
        assert_eq!(loaded.invocation_id, "inv1");
        assert_eq!(loaded.trigger_command, "worktree-post-create");
        assert_eq!(loaded.worktree, "feature/tax-calc");
    }

    #[test]
    fn test_list_invocations() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        for (inv_id, wt) in &[("inv1", "feature/a"), ("inv2", "feature/b")] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: chrono::Utc::now(),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let all = store.list_invocations().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_list_invocations_for_worktree() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        for (inv_id, wt) in &[
            ("inv1", "feature/a"),
            ("inv2", "feature/b"),
            ("inv3", "feature/a"),
        ] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: chrono::Utc::now(),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let filtered = store.list_invocations_for_worktree("feature/a").unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|m| m.worktree == "feature/a"));
    }

    #[test]
    fn test_list_jobs_in_invocation() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        // Create two jobs under inv1
        store.create_job_dir("inv1", "db-migrate").unwrap();
        store.create_job_dir("inv1", "warm-build").unwrap();
        // Create one job under inv2
        store.create_job_dir("inv2", "db-seed").unwrap();

        let jobs = store.list_jobs_in_invocation("inv1").unwrap();
        assert_eq!(jobs.len(), 2);
        let names: Vec<&str> = jobs
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(names.contains(&"db-migrate"));
        assert!(names.contains(&"warm-build"));
    }

    #[test]
    fn test_job_meta_background_and_finished_at() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv1", "bg-job").unwrap();
        let finished = chrono::Utc::now();
        let meta = JobMeta {
            name: "bg-job".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "echo hi".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: finished - chrono::Duration::seconds(5),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: Some(1234),
            background: true,
            finished_at: Some(finished),
            needs: vec![],
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert!(loaded.background);
        assert!(loaded.finished_at.is_some());
    }

    #[test]
    fn test_list_invocations_empty_store() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let all = store.list_invocations().unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn skipped_status_round_trips_through_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::new(dir.path().to_path_buf());
        let job_dir = store.create_job_dir("inv1", "dbsetup").unwrap();

        let meta = JobMeta {
            name: "dbsetup".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: String::new(),
            working_dir: String::new(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Skipped,
            exit_code: None,
            pid: None,
            background: false,
            finished_at: None,
            needs: vec![],
        };
        store.write_meta(&job_dir, &meta).unwrap();

        let loaded = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded.status, JobStatus::Skipped);
    }

    #[test]
    fn invocation_meta_written_without_coordinator() {
        // Verifies that write_invocation_meta creates the file on its own;
        // no coordinator fork required.
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::new(dir.path().to_path_buf());
        let meta = InvocationMeta {
            invocation_id: "deadbeef".to_string(),
            trigger_command: "worktree-post-create".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            created_at: chrono::Utc::now(),
        };

        store.write_invocation_meta("deadbeef", &meta).unwrap();
        let path = dir.path().join("deadbeef").join("invocation.json");
        assert!(path.exists());

        let loaded = store.read_invocation_meta("deadbeef").unwrap();
        assert_eq!(loaded.trigger_command, "worktree-post-create");
    }

    #[test]
    fn write_job_record_creates_meta_and_log_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = LogStore::new(dir.path().to_path_buf());

        let meta = JobMeta {
            name: "pnpm-install".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "pnpm install".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: Some(chrono::Utc::now()),
            needs: vec![],
        };

        let job_dir = store
            .write_job_record("inv42", &meta, b"installing...\ndone\n")
            .unwrap();

        let loaded_meta = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded_meta.name, "pnpm-install");

        let log_bytes = std::fs::read(LogStore::log_path(&job_dir)).unwrap();
        assert_eq!(log_bytes, b"installing...\ndone\n");
    }

    #[test]
    fn job_meta_needs_round_trips_through_json() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let dir = store.create_job_dir("inv-needs", "seeder").unwrap();
        let meta = JobMeta {
            name: "seeder".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "feature/x".to_string(),
            command: "echo seed".to_string(),
            working_dir: "/tmp".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: None,
            background: false,
            finished_at: None,
            needs: vec!["migrator".to_string()],
        };
        store.write_meta(&dir, &meta).unwrap();
        let loaded = store.read_meta(&dir).unwrap();
        assert_eq!(loaded.needs, vec!["migrator".to_string()]);
    }

    #[test]
    fn job_meta_without_needs_field_deserializes_to_empty_vec() {
        let json = r#"{
            "name": "old-job",
            "hook_type": "worktree-post-create",
            "worktree": "feature/x",
            "command": "echo hi",
            "working_dir": "/tmp",
            "env": {},
            "started_at": "2026-04-11T12:00:00Z",
            "status": "completed",
            "exit_code": 0,
            "pid": null,
            "background": false,
            "finished_at": null
        }"#;
        let meta: JobMeta = serde_json::from_str(json).unwrap();
        assert!(meta.needs.is_empty());
    }

    #[test]
    fn find_invocations_by_prefix_returns_matching() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        let now = chrono::Utc::now();
        for (inv_id, wt, offset) in &[
            ("a3f200000000", "feature/a", 100i64),
            ("a3f200000001", "feature/a", 50),
            ("b7c100000000", "feature/a", 10),
        ] {
            std::fs::create_dir_all(tmp.path().join(inv_id)).unwrap();
            let meta = InvocationMeta {
                invocation_id: inv_id.to_string(),
                trigger_command: "worktree-post-create".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: wt.to_string(),
                created_at: now - chrono::Duration::seconds(*offset),
            };
            store.write_invocation_meta(inv_id, &meta).unwrap();
        }

        let matches = store
            .find_invocations_by_prefix("feature/a", "a3f2")
            .unwrap();
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|m| m.invocation_id.starts_with("a3f2")));

        let matches = store
            .find_invocations_by_prefix("feature/a", "b7c1")
            .unwrap();
        assert_eq!(matches.len(), 1);

        let matches = store
            .find_invocations_by_prefix("feature/a", "zzzz")
            .unwrap();
        assert!(matches.is_empty());
    }
}
