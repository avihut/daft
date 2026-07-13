//! Row model for the `worktree_sizes` table.

use chrono::{DateTime, Utc};

/// The last-known on-disk size of one worktree's directory tree, cached so
/// `daft list --columns +size` can render a stale value immediately and refresh
/// it in the background. A display hint, never authoritative: `measured_at`
/// records when the walk ran so the UI can mark the value stale until a fresh
/// walk supersedes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeSizeRow {
    pub repo_hash: String,
    /// Branch checked out in the worktree, e.g. `feat/x`. Keying on this (not
    /// the path) lets a cached size survive worktree moves/renames.
    pub branch_slug: String,
    /// Canonical absolute worktree path — kept for eviction and the
    /// removed-target guard (a vanished path must not overwrite a good size).
    pub worktree_path: String,
    pub size_bytes: u64,
    pub measured_at: DateTime<Utc>,
}
