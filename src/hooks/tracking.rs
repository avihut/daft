use serde::{Deserialize, Serialize};

/// Worktree attributes that a hook job can track.
/// When a tracked attribute changes (e.g., during rename or layout transform),
/// the job is re-run with teardown/setup semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TrackedAttribute {
    Path,
    Branch,
}
