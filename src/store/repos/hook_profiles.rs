//! Queries against the `hook_profiles` table (#678).

use crate::store::error::Result;
use crate::store::models::HookProfileRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};

pub struct HookProfilesRepo;

impl HookProfilesRepo {
    /// Insert or replace the profile for one hook script.
    pub fn upsert(conn: &Connection, row: &HookProfileRow) -> Result<()> {
        conn.execute(
            "INSERT INTO hook_profiles
                 (repo_hash, stage, hook_hash, peak_rss_bytes, wall_ms, runs, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(repo_hash, stage, hook_hash) DO UPDATE SET
                 peak_rss_bytes = excluded.peak_rss_bytes,
                 wall_ms        = excluded.wall_ms,
                 runs           = excluded.runs,
                 updated_at     = excluded.updated_at",
            params![
                row.repo_hash,
                row.stage,
                row.hook_hash,
                row.peak_rss_bytes as i64,
                row.wall_ms as i64,
                i64::from(row.runs),
                row.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// The profile for one hook script, if any run recorded one.
    pub fn get(
        conn: &Connection,
        repo_hash: &str,
        stage: &str,
        hook_hash: &str,
    ) -> Result<Option<HookProfileRow>> {
        conn.query_row(
            "SELECT repo_hash, stage, hook_hash, peak_rss_bytes, wall_ms, runs, updated_at
             FROM hook_profiles
             WHERE repo_hash = ?1 AND stage = ?2 AND hook_hash = ?3",
            params![repo_hash, stage, hook_hash],
            row_to_profile,
        )
        .optional()
        .map_err(Into::into)
    }
}

fn row_to_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<HookProfileRow> {
    Ok(HookProfileRow {
        repo_hash: row.get(0)?,
        stage: row.get(1)?,
        hook_hash: row.get(2)?,
        peak_rss_bytes: row.get::<_, i64>(3)?.max(0) as u64,
        wall_ms: row.get::<_, i64>(4)?.max(0) as u64,
        runs: row.get::<_, i64>(5)?.clamp(0, i64::from(u32::MAX)) as u32,
        updated_at: parse_rfc3339(&row.get::<_, String>(6)?, "updated_at")?,
    })
}
