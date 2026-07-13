//! Queries against the `repo_sizes` table (cached catalog-repo disk sizes).

use crate::store::error::Result;
use crate::store::models::RepoSizeRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};

pub struct RepoSizesRepo;

impl RepoSizesRepo {
    /// Insert or refresh one repo's cached size — the latest walk wins.
    /// `size_bytes` is stored as SQLite `INTEGER` (i64); directory sizes never
    /// approach `i64::MAX`, so the `u64`↔`i64` cast is lossless.
    pub fn upsert(conn: &Connection, row: &RepoSizeRow) -> Result<()> {
        conn.execute(
            "INSERT INTO repo_sizes (uuid, repo_path, size_bytes, measured_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(uuid) DO UPDATE SET
                 repo_path   = excluded.repo_path,
                 size_bytes  = excluded.size_bytes,
                 measured_at = excluded.measured_at",
            params![
                row.uuid,
                row.repo_path,
                row.size_bytes as i64,
                row.measured_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, uuid: &str) -> Result<Option<RepoSizeRow>> {
        let row = conn
            .query_row(
                "SELECT uuid, repo_path, size_bytes, measured_at
                 FROM repo_sizes WHERE uuid = ?1",
                params![uuid],
                row_to_size,
            )
            .optional()?;
        Ok(row)
    }

    /// Every cached repo size (the catalog is global). Used to seed the repo
    /// list's Size cells with last-known values up front.
    pub fn list_all(conn: &Connection) -> Result<Vec<RepoSizeRow>> {
        let mut stmt = conn.prepare(
            "SELECT uuid, repo_path, size_bytes, measured_at
             FROM repo_sizes ORDER BY uuid ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_size)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Evict a repo's cached size — called when it is removed from the catalog.
    /// Returns the number of rows deleted (0 or 1).
    pub fn delete_for_uuid(conn: &Connection, uuid: &str) -> Result<usize> {
        let n = conn.execute("DELETE FROM repo_sizes WHERE uuid = ?1", params![uuid])?;
        Ok(n)
    }
}

fn row_to_size(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoSizeRow> {
    let measured_at_str: String = row.get("measured_at")?;
    let size_bytes: i64 = row.get("size_bytes")?;
    Ok(RepoSizeRow {
        uuid: row.get("uuid")?,
        repo_path: row.get("repo_path")?,
        size_bytes: size_bytes as u64,
        measured_at: parse_rfc3339(&measured_at_str, "measured_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use crate::store::migrate::{self, catalog_set};
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn fresh_catalog_db() -> (TempDir, Connection) {
        // The catalog lineage, not the coordinator one — mirror
        // catalog_repos::tests::catalog_conn (bring_up + catalog_set), since
        // open_for_test runs the coordinator migrations (a newer user_version).
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("catalog.db");
        let is_fresh = !path.exists();
        let mut conn = Connection::open(&path).unwrap();
        connection::bring_up(
            &mut conn,
            &path,
            connection::WRITER_BUSY_TIMEOUT_MS,
            is_fresh,
            false,
        )
        .unwrap();
        migrate::run_set(&catalog_set(), &mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn sample(uuid: &str, bytes: u64) -> RepoSizeRow {
        RepoSizeRow {
            uuid: uuid.into(),
            repo_path: format!("/repos/{uuid}"),
            size_bytes: bytes,
            measured_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn upsert_and_get_round_trip() {
        let (_tmp, conn) = fresh_catalog_db();
        RepoSizesRepo::upsert(&conn, &sample("u1", 4096)).unwrap();
        let back = RepoSizesRepo::get(&conn, "u1").unwrap().unwrap();
        assert_eq!(back, sample("u1", 4096));
    }

    #[test]
    fn upsert_overwrites_with_latest_walk() {
        let (_tmp, conn) = fresh_catalog_db();
        RepoSizesRepo::upsert(&conn, &sample("u1", 100)).unwrap();
        let mut refreshed = sample("u1", 999);
        refreshed.measured_at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        RepoSizesRepo::upsert(&conn, &refreshed).unwrap();
        assert_eq!(
            RepoSizesRepo::get(&conn, "u1").unwrap().unwrap().size_bytes,
            999
        );
    }

    #[test]
    fn list_all_is_ordered() {
        let (_tmp, conn) = fresh_catalog_db();
        RepoSizesRepo::upsert(&conn, &sample("u2", 2)).unwrap();
        RepoSizesRepo::upsert(&conn, &sample("u1", 1)).unwrap();
        let uuids: Vec<String> = RepoSizesRepo::list_all(&conn)
            .unwrap()
            .into_iter()
            .map(|r| r.uuid)
            .collect();
        assert_eq!(uuids, vec!["u1", "u2"]);
    }

    #[test]
    fn delete_for_uuid_evicts() {
        let (_tmp, conn) = fresh_catalog_db();
        RepoSizesRepo::upsert(&conn, &sample("u1", 1)).unwrap();
        assert_eq!(RepoSizesRepo::delete_for_uuid(&conn, "u1").unwrap(), 1);
        assert!(RepoSizesRepo::get(&conn, "u1").unwrap().is_none());
    }
}
