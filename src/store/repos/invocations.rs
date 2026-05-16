//! Queries against the `invocations` table.

use crate::store::error::Result;
use crate::store::models::InvocationRow;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

pub struct InvocationsRepo;

impl InvocationsRepo {
    /// Insert or replace an invocation row.
    pub fn upsert(conn: &Connection, row: &InvocationRow) -> Result<()> {
        conn.execute(
            "INSERT INTO invocations
                 (repo_hash, invocation_id, trigger_command, hook_type, worktree, created_at, coordinator_pid)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(repo_hash, invocation_id) DO UPDATE SET
                 trigger_command = excluded.trigger_command,
                 hook_type       = excluded.hook_type,
                 worktree        = excluded.worktree,
                 created_at      = excluded.created_at,
                 coordinator_pid = excluded.coordinator_pid",
            params![
                row.repo_hash,
                row.invocation_id,
                row.trigger_command,
                row.hook_type,
                row.worktree,
                row.created_at.to_rfc3339(),
                row.coordinator_pid,
            ],
        )?;
        Ok(())
    }

    pub fn get(
        conn: &Connection,
        repo_hash: &str,
        invocation_id: &str,
    ) -> Result<Option<InvocationRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, invocation_id, trigger_command, hook_type, worktree, created_at, coordinator_pid
                 FROM invocations
                 WHERE repo_hash = ?1 AND invocation_id = ?2",
                params![repo_hash, invocation_id],
                row_to_invocation,
            )
            .optional()?;
        Ok(row)
    }
}

fn row_to_invocation(row: &rusqlite::Row<'_>) -> rusqlite::Result<InvocationRow> {
    let created_at_str: String = row.get("created_at")?;
    let created_at = parse_rfc3339(&created_at_str, "created_at")?;
    Ok(InvocationRow {
        repo_hash: row.get("repo_hash")?,
        invocation_id: row.get("invocation_id")?,
        trigger_command: row.get("trigger_command")?,
        hook_type: row.get("hook_type")?,
        worktree: row.get("worktree")?,
        created_at,
        coordinator_pid: row.get::<_, Option<u32>>("coordinator_pid")?,
    })
}

pub(crate) fn parse_rfc3339(s: &str, col: &'static str) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(InvalidTimestamp { col, source: e }),
            )
        })
}

#[derive(Debug)]
struct InvalidTimestamp {
    col: &'static str,
    source: chrono::ParseError,
}

impl std::fmt::Display for InvalidTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid RFC3339 timestamp in column `{}`: {}",
            self.col, self.source
        )
    }
}

impl std::error::Error for InvalidTimestamp {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use crate::store::migrate;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        migrate::run(&mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn sample_inv() -> InvocationRow {
        InvocationRow {
            repo_hash: "repohash".into(),
            invocation_id: "inv-1".into(),
            trigger_command: "daft co -b foo".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "feat/foo".into(),
            created_at: Utc::now(),
            coordinator_pid: Some(42),
        }
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let (_tmp, conn) = fresh_db();
        let row = sample_inv();
        InvocationsRepo::upsert(&conn, &row).unwrap();
        let back = InvocationsRepo::get(&conn, &row.repo_hash, &row.invocation_id)
            .unwrap()
            .unwrap();
        // Compare with secondary-precision tolerance: RFC3339 preserves
        // nanos so the full struct is equal.
        assert_eq!(back, row);
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (_tmp, conn) = fresh_db();
        let mut row = sample_inv();
        InvocationsRepo::upsert(&conn, &row).unwrap();
        row.trigger_command = "daft hooks run".into();
        InvocationsRepo::upsert(&conn, &row).unwrap();
        let back = InvocationsRepo::get(&conn, &row.repo_hash, &row.invocation_id)
            .unwrap()
            .unwrap();
        assert_eq!(back.trigger_command, "daft hooks run");
    }

    #[test]
    fn get_missing_returns_none() {
        let (_tmp, conn) = fresh_db();
        let back = InvocationsRepo::get(&conn, "missing", "missing").unwrap();
        assert!(back.is_none());
    }
}
