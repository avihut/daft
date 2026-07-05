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
                 (repo_hash, invocation_id, trigger_command, hook_type, worktree, created_at, coordinator_pid, status, skip_reason)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(repo_hash, invocation_id) DO UPDATE SET
                 trigger_command = excluded.trigger_command,
                 hook_type       = excluded.hook_type,
                 worktree        = excluded.worktree,
                 created_at      = excluded.created_at,
                 coordinator_pid = excluded.coordinator_pid,
                 status          = excluded.status,
                 skip_reason     = excluded.skip_reason",
            params![
                row.repo_hash,
                row.invocation_id,
                row.trigger_command,
                row.hook_type,
                row.worktree,
                row.created_at.to_rfc3339(),
                row.coordinator_pid,
                row.status,
                row.skip_reason,
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
                "SELECT repo_hash, invocation_id, trigger_command, hook_type, worktree, created_at, coordinator_pid, status, skip_reason
                 FROM invocations
                 WHERE repo_hash = ?1 AND invocation_id = ?2",
                params![repo_hash, invocation_id],
                row_to_invocation,
            )
            .optional()?;
        Ok(row)
    }

    /// All invocations for a repo, oldest first.
    pub fn list_by_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<InvocationRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, invocation_id, trigger_command, hook_type, worktree, created_at, coordinator_pid, status, skip_reason
             FROM invocations
             WHERE repo_hash = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash], row_to_invocation)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// All invocations for a repo with the given status, oldest first.
    pub fn list_by_repo_and_status(
        conn: &Connection,
        repo_hash: &str,
        status: &str,
    ) -> Result<Vec<InvocationRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, invocation_id, trigger_command, hook_type, worktree, created_at, coordinator_pid, status, skip_reason
             FROM invocations
             WHERE repo_hash = ?1 AND status = ?2
             ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash, status], row_to_invocation)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Delete rows matching the natural key `(repo, hook_type, worktree)`
    /// and the given status. Returns the number of rows removed.
    pub fn delete_by_key_and_status(
        conn: &Connection,
        repo_hash: &str,
        hook_type: &str,
        worktree: &str,
        status: &str,
    ) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM invocations
             WHERE repo_hash = ?1 AND hook_type = ?2 AND worktree = ?3 AND status = ?4",
            params![repo_hash, hook_type, worktree, status],
        )?;
        Ok(n)
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
        status: row.get("status")?,
        skip_reason: row.get::<_, Option<String>>("skip_reason")?,
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
            status: crate::store::models::invocation::INVOCATION_STATUS_COMPLETED.into(),
            skip_reason: None,
        }
    }

    fn skipped_inv(id: &str, hook_type: &str, worktree: &str) -> InvocationRow {
        InvocationRow {
            repo_hash: "repohash".into(),
            invocation_id: id.into(),
            trigger_command: "checkout".into(),
            hook_type: hook_type.into(),
            worktree: worktree.into(),
            created_at: Utc::now(),
            coordinator_pid: None,
            status: crate::store::models::invocation::INVOCATION_STATUS_SKIPPED.into(),
            skip_reason: Some(crate::store::models::invocation::SKIP_REASON_UNTRUSTED.into()),
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

    #[test]
    fn list_by_status_filters_and_orders() {
        let (_tmp, conn) = fresh_db();
        InvocationsRepo::upsert(&conn, &sample_inv()).unwrap();
        let mut older = skipped_inv("inv-2", "worktree-post-create", "feat/a");
        older.created_at = Utc::now() - chrono::Duration::hours(1);
        InvocationsRepo::upsert(&conn, &older).unwrap();
        InvocationsRepo::upsert(&conn, &skipped_inv("inv-3", "post-clone", "main")).unwrap();

        let skipped = InvocationsRepo::list_by_repo_and_status(
            &conn,
            "repohash",
            crate::store::models::invocation::INVOCATION_STATUS_SKIPPED,
        )
        .unwrap();
        assert_eq!(skipped.len(), 2);
        // Oldest first; the completed sample row is excluded.
        assert_eq!(skipped[0].invocation_id, "inv-2");
        assert_eq!(skipped[1].invocation_id, "inv-3");

        let other_repo = InvocationsRepo::list_by_repo_and_status(
            &conn,
            "elsewhere",
            crate::store::models::invocation::INVOCATION_STATUS_SKIPPED,
        )
        .unwrap();
        assert!(other_repo.is_empty());
    }

    #[test]
    fn delete_by_key_and_status_scopes_tightly() {
        let (_tmp, conn) = fresh_db();
        InvocationsRepo::upsert(
            &conn,
            &skipped_inv("inv-1", "worktree-post-create", "feat/a"),
        )
        .unwrap();
        InvocationsRepo::upsert(
            &conn,
            &skipped_inv("inv-2", "worktree-post-create", "feat/b"),
        )
        .unwrap();
        InvocationsRepo::upsert(
            &conn,
            &skipped_inv("inv-3", "worktree-pre-create", "feat/a"),
        )
        .unwrap();
        // Completed row sharing the same natural key must survive.
        let mut done = sample_inv();
        done.invocation_id = "inv-4".into();
        done.hook_type = "worktree-post-create".into();
        done.worktree = "feat/a".into();
        InvocationsRepo::upsert(&conn, &done).unwrap();

        let n = InvocationsRepo::delete_by_key_and_status(
            &conn,
            "repohash",
            "worktree-post-create",
            "feat/a",
            crate::store::models::invocation::INVOCATION_STATUS_SKIPPED,
        )
        .unwrap();
        assert_eq!(n, 1);

        let remaining = InvocationsRepo::list_by_repo_and_status(
            &conn,
            "repohash",
            crate::store::models::invocation::INVOCATION_STATUS_SKIPPED,
        )
        .unwrap();
        let ids: Vec<_> = remaining.iter().map(|r| r.invocation_id.as_str()).collect();
        assert_eq!(ids, vec!["inv-2", "inv-3"]);
        assert!(
            InvocationsRepo::get(&conn, "repohash", "inv-4")
                .unwrap()
                .is_some()
        );
    }
}
