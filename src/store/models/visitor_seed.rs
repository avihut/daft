//! Row model for the `visitor_seeds` table.

use chrono::{DateTime, Utc};

/// The content daft last wrote into one worktree's untracked daft file.
///
/// `content` is the merge base for three-way consolidation and the
/// reference for pristine/refined classification (byte comparison).
/// `seeded_at` is the original provenance timestamp — upserts preserve it;
/// `updated_at` moves on every refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitorSeedRow {
    pub repo_hash: String,
    /// Branch checked out in the seeded worktree, e.g. `feat/x`.
    pub branch_slug: String,
    /// `daft.yml` or `daft.local.yml`.
    pub filename: String,
    pub content: String,
    pub seeded_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
