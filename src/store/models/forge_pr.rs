//! Row model for the `forge_prs` table.

use chrono::{DateTime, Utc};

/// One pull/merge request on the repo's forge, snapshotted by the last cache
/// refresh. A display/completion accelerator, never authoritative — the forge
/// is; `fetched_at` records when the snapshot was taken.
///
/// `kind`, `state`, and `ci_status` are stored as TEXT and surfaced here as
/// plain strings; interpreting them into domain types happens in consumers
/// (the store layer stays a pure data-access layer — see `models/mod.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgePrRow {
    pub repo_hash: String,
    /// `"pr"` (GitHub) or `"mr"` (GitLab).
    pub kind: String,
    /// Stored as SQLite `INTEGER` (i64); forge PR numbers never approach
    /// `u32::MAX`, so the cast is lossless.
    pub number: u32,
    /// Sanitized before persistence (control characters stripped) — readers
    /// render it into terminals and completion streams and trust the store.
    pub title: String,
    /// `"open"`, `"merged"`, or `"closed"`.
    pub state: String,
    /// The PR's source branch name — the outbound-match key against local
    /// branches.
    pub head_branch: String,
    /// The head lives in a fork. Outbound matching requires `false` so a
    /// stranger's fork branch with a colliding name can't label a local one.
    pub is_cross_repo: bool,
    /// `"pass"`, `"fail"`, `"pending"`, or `None` when the PR has no CI.
    pub ci_status: Option<String>,
    pub url: String,
    pub author: String,
    pub fetched_at: DateTime<Utc>,
}
