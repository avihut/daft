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

pub fn invocation_key(repo_hash: &str, invocation_id: &str) -> String {
    format!("{repo_hash}:{invocation_id}")
}

pub fn job_key(repo_hash: &str, invocation_id: &str, job_name: &str) -> String {
    format!("{repo_hash}:{invocation_id}:{job_name}")
}
