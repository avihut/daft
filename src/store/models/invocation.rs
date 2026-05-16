//! `invocations` row.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationRow {
    pub repo_hash: String,
    pub invocation_id: String,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    pub created_at: DateTime<Utc>,
    pub coordinator_pid: Option<u32>,
}
