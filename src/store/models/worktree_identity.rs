//! Row model for the `worktree_identities` table.

use chrono::{DateTime, Utc};

/// The branch a worktree was created for.
///
/// Read only when live git state cannot name the branch — a plain detached
/// checkout — and as a cross-check for drift. Derived state always wins: this
/// row can be out of date, and an in-progress operation never can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentityRow {
    pub repo_hash: String,
    /// The worktree's private-gitdir id: the directory name under
    /// `<common-dir>/worktrees/`. Stable across `git worktree move` and
    /// branch renames, which is why it is the key rather than the path.
    pub worktree_id: String,
    /// The branch this worktree is for, e.g. `feat/x`.
    pub branch: String,
    /// Absolute worktree path, for display and eviction.
    pub worktree_path: String,
    pub updated_at: DateTime<Utc>,
}
