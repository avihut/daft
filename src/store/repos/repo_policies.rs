//! Queries against the `repo_policy` table.

use crate::store::error::Result;
use crate::store::models::RepoPolicyRow;
use rusqlite::{Connection, OptionalExtension, params};

pub struct RepoPoliciesRepo;

impl RepoPoliciesRepo {
    pub fn upsert(conn: &Connection, row: &RepoPolicyRow) -> Result<()> {
        conn.execute(
            "INSERT INTO repo_policy
                 (repo_hash, policy_version, max_total_size_bytes, keep_last, stale_running_after_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(repo_hash) DO UPDATE SET
                 policy_version              = excluded.policy_version,
                 max_total_size_bytes        = excluded.max_total_size_bytes,
                 keep_last                   = excluded.keep_last,
                 stale_running_after_seconds = excluded.stale_running_after_seconds",
            params![
                row.repo_hash,
                row.policy_version,
                row.max_total_size_bytes.map(|n| n as i64),
                row.keep_last.map(|n| n as i64),
                row.stale_running_after_seconds,
            ],
        )?;
        Ok(())
    }

    pub fn get(conn: &Connection, repo_hash: &str) -> Result<Option<RepoPolicyRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, policy_version, max_total_size_bytes, keep_last, stale_running_after_seconds
                 FROM repo_policy
                 WHERE repo_hash = ?1",
                params![repo_hash],
                row_to_repo_policy,
            )
            .optional()?;
        Ok(row)
    }
}

fn row_to_repo_policy(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepoPolicyRow> {
    Ok(RepoPolicyRow {
        repo_hash: row.get("repo_hash")?,
        policy_version: row.get("policy_version")?,
        max_total_size_bytes: row
            .get::<_, Option<i64>>("max_total_size_bytes")?
            .map(|n| n as u64),
        keep_last: row.get::<_, Option<i64>>("keep_last")?.map(|n| n as usize),
        stale_running_after_seconds: row.get::<_, Option<i64>>("stale_running_after_seconds")?,
    })
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

    fn sample(repo: &str) -> RepoPolicyRow {
        RepoPolicyRow {
            repo_hash: repo.into(),
            policy_version: 1,
            max_total_size_bytes: Some(100 * 1024 * 1024),
            keep_last: Some(5),
            stale_running_after_seconds: Some(60),
        }
    }

    #[test]
    fn upsert_and_get_round_trip() {
        let (_tmp, conn) = fresh_db();
        RepoPoliciesRepo::upsert(&conn, &sample("r")).unwrap();
        let back = RepoPoliciesRepo::get(&conn, "r").unwrap().unwrap();
        assert_eq!(back, sample("r"));
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let (_tmp, conn) = fresh_db();
        RepoPoliciesRepo::upsert(&conn, &sample("r")).unwrap();
        let mut updated = sample("r");
        updated.max_total_size_bytes = Some(999);
        RepoPoliciesRepo::upsert(&conn, &updated).unwrap();
        let back = RepoPoliciesRepo::get(&conn, "r").unwrap().unwrap();
        assert_eq!(back.max_total_size_bytes, Some(999));
    }

    #[test]
    fn get_missing_returns_none() {
        let (_tmp, conn) = fresh_db();
        assert!(RepoPoliciesRepo::get(&conn, "missing").unwrap().is_none());
    }
}
