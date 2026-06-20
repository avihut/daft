//! Queries against the `visitor_seeds` table.

use crate::store::error::Result;
use crate::store::models::VisitorSeedRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};

pub struct VisitorSeedsRepo;

impl VisitorSeedsRepo {
    /// Insert or refresh a seed. On conflict the original `seeded_at` is
    /// preserved — it records when the file first flowed into the worktree —
    /// while `content` and `updated_at` take the new values.
    pub fn upsert(conn: &Connection, row: &VisitorSeedRow) -> Result<()> {
        conn.execute(
            "INSERT INTO visitor_seeds
                 (repo_hash, branch_slug, filename, content, seeded_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(repo_hash, branch_slug, filename) DO UPDATE SET
                 content    = excluded.content,
                 updated_at = excluded.updated_at",
            params![
                row.repo_hash,
                row.branch_slug,
                row.filename,
                row.content,
                row.seeded_at.to_rfc3339(),
                row.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get(
        conn: &Connection,
        repo_hash: &str,
        branch_slug: &str,
        filename: &str,
    ) -> Result<Option<VisitorSeedRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, branch_slug, filename, content, seeded_at, updated_at
                 FROM visitor_seeds
                 WHERE repo_hash = ?1 AND branch_slug = ?2 AND filename = ?3",
                params![repo_hash, branch_slug, filename],
                row_to_seed,
            )
            .optional()?;
        Ok(row)
    }

    /// Remove one file's seed (e.g. after a consolidation deleted the
    /// source file). Returns the number of rows deleted (0 or 1).
    pub fn delete_one(
        conn: &Connection,
        repo_hash: &str,
        branch_slug: &str,
        filename: &str,
    ) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM visitor_seeds
             WHERE repo_hash = ?1 AND branch_slug = ?2 AND filename = ?3",
            params![repo_hash, branch_slug, filename],
        )?;
        Ok(n)
    }

    /// Remove every seed for a branch — called when its worktree/branch is
    /// removed. Returns the number of rows deleted.
    pub fn delete_for_branch(
        conn: &Connection,
        repo_hash: &str,
        branch_slug: &str,
    ) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM visitor_seeds WHERE repo_hash = ?1 AND branch_slug = ?2",
            params![repo_hash, branch_slug],
        )?;
        Ok(n)
    }

    /// Every seed for a repo, ordered for stable display (`daft __dump-store`
    /// and doctor-style auditing).
    pub fn list_for_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<VisitorSeedRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, branch_slug, filename, content, seeded_at, updated_at
             FROM visitor_seeds
             WHERE repo_hash = ?1
             ORDER BY branch_slug ASC, filename ASC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash], row_to_seed)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn row_to_seed(row: &rusqlite::Row<'_>) -> rusqlite::Result<VisitorSeedRow> {
    let seeded_at_str: String = row.get("seeded_at")?;
    let updated_at_str: String = row.get("updated_at")?;
    Ok(VisitorSeedRow {
        repo_hash: row.get("repo_hash")?,
        branch_slug: row.get("branch_slug")?,
        filename: row.get("filename")?,
        content: row.get("content")?,
        seeded_at: parse_rfc3339(&seeded_at_str, "seeded_at")?,
        updated_at: parse_rfc3339(&updated_at_str, "updated_at")?,
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

    fn sample(branch: &str, content: &str) -> VisitorSeedRow {
        VisitorSeedRow {
            repo_hash: "repo".into(),
            branch_slug: branch.into(),
            filename: "daft.yml".into(),
            content: content.into(),
            seeded_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn upsert_and_get_round_trip() {
        let (_tmp, conn) = fresh_db();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/x", "hooks: {}")).unwrap();
        let back = VisitorSeedsRepo::get(&conn, "repo", "feat/x", "daft.yml")
            .unwrap()
            .unwrap();
        assert_eq!(back, sample("feat/x", "hooks: {}"));
    }

    #[test]
    fn upsert_refreshes_content_but_preserves_seeded_at() {
        let (_tmp, conn) = fresh_db();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/x", "v1")).unwrap();
        let mut refreshed = sample("feat/x", "v2");
        refreshed.seeded_at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        refreshed.updated_at = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        VisitorSeedsRepo::upsert(&conn, &refreshed).unwrap();

        let back = VisitorSeedsRepo::get(&conn, "repo", "feat/x", "daft.yml")
            .unwrap()
            .unwrap();
        assert_eq!(back.content, "v2");
        assert_eq!(back.seeded_at, sample("feat/x", "v1").seeded_at);
        assert_eq!(back.updated_at, refreshed.updated_at);
    }

    #[test]
    fn get_missing_returns_none() {
        let (_tmp, conn) = fresh_db();
        assert!(
            VisitorSeedsRepo::get(&conn, "repo", "feat/x", "daft.yml")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn slashed_branch_slug_round_trips() {
        let (_tmp, conn) = fresh_db();
        VisitorSeedsRepo::upsert(&conn, &sample("daft-1/fix/deep/name", "x")).unwrap();
        assert!(
            VisitorSeedsRepo::get(&conn, "repo", "daft-1/fix/deep/name", "daft.yml")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn delete_for_branch_removes_only_that_branch() {
        let (_tmp, conn) = fresh_db();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/x", "x")).unwrap();
        let mut local = sample("feat/x", "l");
        local.filename = "daft.local.yml".into();
        VisitorSeedsRepo::upsert(&conn, &local).unwrap();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/y", "y")).unwrap();

        let deleted = VisitorSeedsRepo::delete_for_branch(&conn, "repo", "feat/x").unwrap();
        assert_eq!(deleted, 2);
        assert!(
            VisitorSeedsRepo::get(&conn, "repo", "feat/x", "daft.yml")
                .unwrap()
                .is_none()
        );
        assert!(
            VisitorSeedsRepo::get(&conn, "repo", "feat/y", "daft.yml")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn delete_one_removes_single_file_seed() {
        let (_tmp, conn) = fresh_db();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/x", "x")).unwrap();
        assert_eq!(
            VisitorSeedsRepo::delete_one(&conn, "repo", "feat/x", "daft.yml").unwrap(),
            1
        );
        assert_eq!(
            VisitorSeedsRepo::delete_one(&conn, "repo", "feat/x", "daft.yml").unwrap(),
            0
        );
    }

    #[test]
    fn list_for_repo_is_ordered_and_scoped() {
        let (_tmp, conn) = fresh_db();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/y", "y")).unwrap();
        VisitorSeedsRepo::upsert(&conn, &sample("feat/x", "x")).unwrap();
        let mut other = sample("feat/z", "z");
        other.repo_hash = "other-repo".into();
        VisitorSeedsRepo::upsert(&conn, &other).unwrap();

        let rows = VisitorSeedsRepo::list_for_repo(&conn, "repo").unwrap();
        let branches: Vec<&str> = rows.iter().map(|r| r.branch_slug.as_str()).collect();
        assert_eq!(branches, vec!["feat/x", "feat/y"]);
    }
}
