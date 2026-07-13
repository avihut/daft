//! Queries against the `worktree_sizes` table (cached worktree disk sizes).

use crate::store::error::Result;
use crate::store::models::WorktreeSizeRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};

pub struct WorktreeSizesRepo;

impl WorktreeSizesRepo {
    /// Insert or refresh one worktree's cached size — the latest walk wins
    /// (there is no first-seen to preserve; a size is only ever a fresh hint).
    /// `size_bytes` is stored as SQLite `INTEGER` (i64); directory sizes never
    /// approach `i64::MAX`, so the `u64`↔`i64` cast is lossless.
    pub fn upsert(conn: &Connection, row: &WorktreeSizeRow) -> Result<()> {
        conn.execute(
            "INSERT INTO worktree_sizes
                 (repo_hash, branch_slug, worktree_path, size_bytes, measured_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(repo_hash, branch_slug) DO UPDATE SET
                 worktree_path = excluded.worktree_path,
                 size_bytes    = excluded.size_bytes,
                 measured_at   = excluded.measured_at",
            params![
                row.repo_hash,
                row.branch_slug,
                row.worktree_path,
                row.size_bytes as i64,
                row.measured_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get(
        conn: &Connection,
        repo_hash: &str,
        branch_slug: &str,
    ) -> Result<Option<WorktreeSizeRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, branch_slug, worktree_path, size_bytes, measured_at
                 FROM worktree_sizes
                 WHERE repo_hash = ?1 AND branch_slug = ?2",
                params![repo_hash, branch_slug],
                row_to_size,
            )
            .optional()?;
        Ok(row)
    }

    /// Every cached size for a repo, ordered by branch for stable display.
    /// Used to seed the list's Size cells with last-known values up front.
    pub fn list_for_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<WorktreeSizeRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, branch_slug, worktree_path, size_bytes, measured_at
             FROM worktree_sizes
             WHERE repo_hash = ?1
             ORDER BY branch_slug ASC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash], row_to_size)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Evict a branch's cached size. The store primitive for explicit
    /// eviction; the seed is already path-guarded
    /// ([`crate::commands::size_cache::seed_worktree_sizes`]) so a stale row
    /// from a removed-then-recreated worktree never surfaces even before it is
    /// deleted. Wiring this into the worktree-removal / prune path (the twin of
    /// the repo side's [`crate::catalog::service::Catalog::mark_removed`]
    /// eviction) is a deliberate follow-up. Returns rows deleted (0 or 1).
    pub fn delete_for_branch(
        conn: &Connection,
        repo_hash: &str,
        branch_slug: &str,
    ) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM worktree_sizes WHERE repo_hash = ?1 AND branch_slug = ?2",
            params![repo_hash, branch_slug],
        )?;
        Ok(n)
    }
}

fn row_to_size(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorktreeSizeRow> {
    let measured_at_str: String = row.get("measured_at")?;
    let size_bytes: i64 = row.get("size_bytes")?;
    Ok(WorktreeSizeRow {
        repo_hash: row.get("repo_hash")?,
        branch_slug: row.get("branch_slug")?,
        worktree_path: row.get("worktree_path")?,
        size_bytes: size_bytes as u64,
        measured_at: parse_rfc3339(&measured_at_str, "measured_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use crate::store::migrate;
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        migrate::run(&mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn sample(branch: &str, bytes: u64) -> WorktreeSizeRow {
        WorktreeSizeRow {
            repo_hash: "repo".into(),
            branch_slug: branch.into(),
            worktree_path: format!("/tmp/wt/{branch}"),
            size_bytes: bytes,
            measured_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn upsert_and_get_round_trip() {
        let (_tmp, conn) = fresh_db();
        WorktreeSizesRepo::upsert(&conn, &sample("feat/x", 4096)).unwrap();
        let back = WorktreeSizesRepo::get(&conn, "repo", "feat/x")
            .unwrap()
            .unwrap();
        assert_eq!(back, sample("feat/x", 4096));
    }

    #[test]
    fn upsert_overwrites_with_latest_walk() {
        let (_tmp, conn) = fresh_db();
        WorktreeSizesRepo::upsert(&conn, &sample("feat/x", 100)).unwrap();
        let mut refreshed = sample("feat/x", 999);
        refreshed.measured_at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        WorktreeSizesRepo::upsert(&conn, &refreshed).unwrap();
        let back = WorktreeSizesRepo::get(&conn, "repo", "feat/x")
            .unwrap()
            .unwrap();
        assert_eq!(back.size_bytes, 999);
        assert_eq!(back.measured_at, refreshed.measured_at);
    }

    #[test]
    fn large_size_round_trips_losslessly() {
        let (_tmp, conn) = fresh_db();
        let big = 40 * 1024 * 1024 * 1024_u64; // 40 GiB
        WorktreeSizesRepo::upsert(&conn, &sample("feat/big", big)).unwrap();
        let back = WorktreeSizesRepo::get(&conn, "repo", "feat/big")
            .unwrap()
            .unwrap();
        assert_eq!(back.size_bytes, big);
    }

    #[test]
    fn slashed_branch_slug_round_trips() {
        let (_tmp, conn) = fresh_db();
        WorktreeSizesRepo::upsert(&conn, &sample("daft-1/fix/deep/name", 1)).unwrap();
        assert!(
            WorktreeSizesRepo::get(&conn, "repo", "daft-1/fix/deep/name")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn get_missing_returns_none() {
        let (_tmp, conn) = fresh_db();
        assert!(
            WorktreeSizesRepo::get(&conn, "repo", "feat/x")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn list_for_repo_is_ordered_and_scoped() {
        let (_tmp, conn) = fresh_db();
        WorktreeSizesRepo::upsert(&conn, &sample("feat/y", 2)).unwrap();
        WorktreeSizesRepo::upsert(&conn, &sample("feat/x", 1)).unwrap();
        let mut other = sample("feat/z", 3);
        other.repo_hash = "other-repo".into();
        WorktreeSizesRepo::upsert(&conn, &other).unwrap();

        let rows = WorktreeSizesRepo::list_for_repo(&conn, "repo").unwrap();
        let branches: Vec<&str> = rows.iter().map(|r| r.branch_slug.as_str()).collect();
        assert_eq!(branches, vec!["feat/x", "feat/y"]);
    }

    #[test]
    fn delete_for_branch_evicts_only_that_branch() {
        let (_tmp, conn) = fresh_db();
        WorktreeSizesRepo::upsert(&conn, &sample("feat/x", 1)).unwrap();
        WorktreeSizesRepo::upsert(&conn, &sample("feat/y", 2)).unwrap();
        assert_eq!(
            WorktreeSizesRepo::delete_for_branch(&conn, "repo", "feat/x").unwrap(),
            1
        );
        assert!(
            WorktreeSizesRepo::get(&conn, "repo", "feat/x")
                .unwrap()
                .is_none()
        );
        assert!(
            WorktreeSizesRepo::get(&conn, "repo", "feat/y")
                .unwrap()
                .is_some()
        );
    }
}
