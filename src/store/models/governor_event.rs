//! `governor_events` row: one action or observation of the sync push
//! resource governor (#678) — throttles, freezes, kill-requeues, timeouts.

use chrono::{DateTime, Utc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernorEventRow {
    /// Assigned by SQLite on insert; `None` for rows not yet persisted.
    pub id: Option<i64>,
    pub repo_hash: String,
    pub occurred_at: DateTime<Utc>,
    /// Event kind: `throttle`, `freeze`, `thaw`, `kill_requeue`, `timeout`.
    pub kind: String,
    /// Branch of the affected push unit, when the event is unit-scoped.
    pub branch: Option<String>,
    /// Duration payload (held/frozen milliseconds), when applicable.
    pub detail_ms: Option<u64>,
    /// Memory payload (peak tree-RSS bytes), when applicable.
    pub rss_bytes: Option<u64>,
}
