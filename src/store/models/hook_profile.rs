//! `hook_profiles` row: the learned resource profile of one hook script
//! (#678). Keyed by `(repo_hash, stage, hook_hash)` — the hash is over the
//! resolved hook file's contents, so editing the hook invalidates the
//! profile naturally.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookProfileRow {
    pub repo_hash: String,
    /// Hook stage, e.g. `pre-push`.
    pub stage: String,
    /// Content hash of the resolved hook file (cache key, not security).
    pub hook_hash: String,
    /// Decayed maximum of the hook's process-tree RSS across runs.
    pub peak_rss_bytes: u64,
    /// Exponentially weighted average wall time of one hook run.
    pub wall_ms: u64,
    /// How many runs contributed to this profile.
    pub runs: u32,
    pub updated_at: DateTime<Utc>,
}
