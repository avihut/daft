//! Value types stored in redb tables.
//!
//! Each row carries `schema_version` so partially-migrated databases stay
//! self-describing. Values are bincode-encoded.

use crate::coordinator::log_store::JobStatus;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvocationRow {
    pub schema_version: u64,
    pub repo_hash: String,
    pub invocation_id: String,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    pub created_at: DateTime<Utc>,
    pub coordinator_pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobRow {
    pub schema_version: u64,
    pub repo_hash: String,
    pub invocation_id: String,
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub command: String,
    pub working_dir: String,
    pub env: HashMap<String, String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: JobStatus,
    pub exit_code: Option<i32>,
    /// PID of the spawned child process. Equal to `pgid` after `process_group(0)`.
    pub pid: Option<u32>,
    /// Process-group leader. `killpg(pgid, SIGTERM)` reaches descendants too.
    pub pgid: Option<u32>,
    pub background: bool,
    pub needs: Vec<String>,
    /// User-supplied labels from `JobDef.tags`. Filters for richer cancel.
    pub tags: Vec<String>,
    pub retention_seconds: Option<i64>,
    pub max_log_size_bytes: Option<u64>,
    pub log_truncated: bool,
    pub original_size_bytes: Option<u64>,
}

impl JobRow {
    /// Project a row into the `JobMeta` shape used by readers that haven't
    /// been migrated off the legacy `meta.json` path. Keeps display code
    /// uniform regardless of the source.
    pub fn to_job_meta(&self) -> crate::coordinator::log_store::JobMeta {
        crate::coordinator::log_store::JobMeta {
            name: self.name.clone(),
            hook_type: self.hook_type.clone(),
            worktree: self.worktree.clone(),
            command: self.command.clone(),
            working_dir: self.working_dir.clone(),
            env: self.env.clone(),
            started_at: self.started_at,
            status: self.status.clone(),
            exit_code: self.exit_code,
            pid: self.pid,
            background: self.background,
            finished_at: self.finished_at,
            needs: self.needs.clone(),
            retention_seconds: self.retention_seconds,
            max_log_size_bytes: self.max_log_size_bytes,
            log_truncated: self.log_truncated,
            original_size_bytes: self.original_size_bytes,
        }
    }
}

/// Per-repo cleanup policy. Wire/disk shape of
/// [`crate::coordinator::clean_policy::RepoPolicy`] with a `schema_version`
/// tag for forward-compat. Round-trips field-for-field via `From` impls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoPolicyRow {
    pub schema_version: u64,
    pub policy_version: u32,
    pub max_total_size_bytes: Option<u64>,
    pub keep_last: Option<usize>,
    pub stale_running_after_seconds: Option<i64>,
}

impl RepoPolicyRow {
    pub fn from_policy(policy: &crate::coordinator::clean_policy::RepoPolicy) -> Self {
        Self {
            schema_version: super::schema::SCHEMA_VERSION,
            policy_version: policy.version,
            max_total_size_bytes: policy.max_total_size_bytes,
            keep_last: policy.keep_last,
            stale_running_after_seconds: policy.stale_running_after_seconds,
        }
    }

    pub fn to_policy(&self) -> crate::coordinator::clean_policy::RepoPolicy {
        crate::coordinator::clean_policy::RepoPolicy {
            version: self.policy_version,
            max_total_size_bytes: self.max_total_size_bytes,
            keep_last: self.keep_last,
            stale_running_after_seconds: self.stale_running_after_seconds,
        }
    }
}

pub fn invocation_key(repo_hash: &str, invocation_id: &str) -> String {
    format!("{repo_hash}:{invocation_id}")
}

pub fn job_key(repo_hash: &str, invocation_id: &str, job_name: &str) -> String {
    format!("{repo_hash}:{invocation_id}:{job_name}")
}

pub fn repo_policy_key(repo_hash: &str) -> String {
    repo_hash.to_string()
}
