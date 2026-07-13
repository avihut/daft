//! Queries against the `governor_events` table (#678).

use crate::store::error::Result;
use crate::store::models::GovernorEventRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, params};

pub struct GovernorEventsRepo;

impl GovernorEventsRepo {
    /// Append one event. `row.id` is ignored (SQLite assigns it).
    pub fn insert(conn: &Connection, row: &GovernorEventRow) -> Result<()> {
        conn.execute(
            "INSERT INTO governor_events
                 (repo_hash, occurred_at, kind, branch, detail_ms, rss_bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.repo_hash,
                row.occurred_at.to_rfc3339(),
                row.kind,
                row.branch,
                row.detail_ms.map(|n| n as i64),
                row.rss_bytes.map(|n| n as i64),
            ],
        )?;
        Ok(())
    }

    /// All events for a repo, oldest first.
    pub fn list_by_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<GovernorEventRow>> {
        let mut stmt = conn.prepare(
            "SELECT id, repo_hash, occurred_at, kind, branch, detail_ms, rss_bytes
             FROM governor_events
             WHERE repo_hash = ?1
             ORDER BY occurred_at ASC, id ASC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash], row_to_event)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<GovernorEventRow> {
    Ok(GovernorEventRow {
        id: row.get(0)?,
        repo_hash: row.get(1)?,
        occurred_at: parse_rfc3339(&row.get::<_, String>(2)?, "occurred_at")?,
        kind: row.get(3)?,
        branch: row.get(4)?,
        detail_ms: row.get::<_, Option<i64>>(5)?.map(|n| n.max(0) as u64),
        rss_bytes: row.get::<_, Option<i64>>(6)?.map(|n| n.max(0) as u64),
    })
}
