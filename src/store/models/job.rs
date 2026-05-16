//! `jobs` row.
//!
//! `status` is a free string at the storage boundary — domain enums
//! serialize as their lowercase variant name. Forward-compat: future
//! statuses round-trip even on older binaries (the adapter maps unknown
//! statuses to a domain `Unknown` variant).

use chrono::{DateTime, Utc};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    pub repo_hash: String,
    pub invocation_id: String,
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub command: String,
    pub working_dir: String,
    /// Already env-scrubbed at adapter boundary.
    pub env: HashMap<String, String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub status: String,
    pub exit_code: Option<i32>,
    /// PID of the spawned child process. Equal to `pgid` after
    /// `process_group(0)`.
    pub pid: Option<u32>,
    /// Process-group leader. `killpg(pgid, SIGTERM)` reaches descendants
    /// too.
    pub pgid: Option<u32>,
    pub background: bool,
    pub needs: Vec<String>,
    /// User-supplied labels from `JobDef.tags`. Filters for richer cancel.
    pub tags: Vec<String>,
    pub retention_seconds: Option<i64>,
    pub max_log_size_bytes: Option<u64>,
}
