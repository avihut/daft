//! `catalog_repos` row.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRepoRow {
    pub uuid: String,
    pub name: String,
    /// Project root the user interacts with (canonical).
    pub path: String,
    /// Git common dir — what trust, hooks, and `daft-id` key on (canonical).
    pub git_common_dir: String,
    /// Remote URL as configured; display and re-clone form.
    pub remote_url: Option<String>,
    /// Normalized match key for relations-manifest resolution.
    pub remote_url_normalized: Option<String>,
    pub default_branch: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// `None` = live. Removed entries are retained for log addressing and
    /// re-clone-by-name.
    pub removed_at: Option<DateTime<Utc>>,
}
