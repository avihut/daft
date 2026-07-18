//! Row model for the `forge_health` singleton table.

use chrono::{DateTime, Utc};

/// The repo's forge-integration health, written by the forge-cache refresh
/// and read by `daft list` to decide whether the default `pr` column shows.
///
/// `healthy = false` means the last refresh hit a *deep* failure — one that
/// keeps failing until the user intervenes (`error_kind` says which) — and
/// hides the default-sourced `pr` column. Transient failures never flip it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeHealthRow {
    pub healthy: bool,
    /// `"missing-tool"`, `"unauthenticated"`, or `"repo-access"` when
    /// `healthy` is false; `None` otherwise.
    pub error_kind: Option<String>,
    /// When the last refresh attempt started — the background-refresh
    /// throttle key.
    pub started_at: Option<DateTime<Utc>>,
    /// When the last refresh attempt concluded (success or failure) — the
    /// live table's refresh-in-flight display state settles when this
    /// advances.
    pub finished_at: Option<DateTime<Utc>>,
    /// When a refresh last succeeded. `None` means no snapshot was ever
    /// taken, which drives the PR column's first-load skeleton.
    pub succeeded_at: Option<DateTime<Utc>>,
}
