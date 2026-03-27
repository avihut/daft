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
        fs::write(&path, content)?;
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
        };
        store.write_meta(&dir, &meta).unwrap();
        let removed = store.clean(chrono::Duration::days(7)).unwrap();
        assert_eq!(removed, 1);
        assert!(!dir.exists());
    }
}
