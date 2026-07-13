//! Row model for the `repo_sizes` table (catalog lineage).

use chrono::{DateTime, Utc};

/// The last-known on-disk size of one catalog repo's directory tree, cached so
/// `daft repo list --columns +size` can render a stale value immediately and
/// refresh it in the background. A display hint, never authoritative:
/// `measured_at` records when the walk ran so the UI can mark it stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSizeRow {
    /// Catalog primary key (`catalog_repos.uuid`). Survives rename/move.
    pub uuid: String,
    /// Repo path that was walked — kept for the removed/moved-target guard.
    pub repo_path: String,
    pub size_bytes: u64,
    pub measured_at: DateTime<Utc>,
}
