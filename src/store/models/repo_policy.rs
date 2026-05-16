//! `repo_policy` row.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPolicyRow {
    pub repo_hash: String,
    pub policy_version: u32,
    pub max_total_size_bytes: Option<u64>,
    pub keep_last: Option<usize>,
    pub stale_running_after_seconds: Option<i64>,
}
