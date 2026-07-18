//! Queries against the `forge_prs` table (cached forge pull/merge requests).

use crate::store::error::Result;
use crate::store::models::ForgePrRow;
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};

pub struct ForgePrsRepo;

impl ForgePrsRepo {
    /// Insert or refresh one PR — the write-through path when a `daft go pr:N`
    /// resolve already holds fresh data for exactly that PR. The latest fetch
    /// wins on every field (a snapshot is only ever a fresh hint).
    pub fn upsert(conn: &Connection, row: &ForgePrRow) -> Result<()> {
        conn.execute(
            "INSERT INTO forge_prs
                 (repo_hash, kind, number, title, state, head_branch,
                  is_cross_repo, ci_status, url, author, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(repo_hash, kind, number) DO UPDATE SET
                 title         = excluded.title,
                 state         = excluded.state,
                 head_branch   = excluded.head_branch,
                 is_cross_repo = excluded.is_cross_repo,
                 ci_status     = excluded.ci_status,
                 url           = excluded.url,
                 author        = excluded.author,
                 fetched_at    = excluded.fetched_at",
            params![
                row.repo_hash,
                row.kind,
                i64::from(row.number),
                row.title,
                row.state,
                row.head_branch,
                row.is_cross_repo,
                row.ci_status,
                row.url,
                row.author,
                row.fetched_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Replace the whole snapshot for one `(repo, kind)` in the caller's
    /// transaction — the wholesale-refresh path. Scoped by `kind` so a GitHub
    /// refresh can't wipe cached GitLab MRs (mixed-remote repos) and vice
    /// versa. Delete-then-insert keeps snapshot semantics: PRs that vanished
    /// from the forge (merged + pruned from the listing) vanish here too.
    pub fn replace_snapshot(
        tx: &rusqlite::Transaction<'_>,
        repo_hash: &str,
        kind: &str,
        rows: &[ForgePrRow],
    ) -> Result<()> {
        tx.execute(
            "DELETE FROM forge_prs WHERE repo_hash = ?1 AND kind = ?2",
            params![repo_hash, kind],
        )?;
        for row in rows {
            Self::upsert(tx, row)?;
        }
        Ok(())
    }

    /// Every cached PR/MR for a repo, open ones first, newest number first
    /// within a state — the completion listing order.
    pub fn list_for_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<ForgePrRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, kind, number, title, state, head_branch,
                    is_cross_repo, ci_status, url, author, fetched_at
             FROM forge_prs
             WHERE repo_hash = ?1
             ORDER BY (state = 'open') DESC, number DESC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash], row_to_pr)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// The open same-repo PR whose head is `branch`, if any — the outbound
    /// match for `daft list --columns +pr`. Cross-repo rows are excluded so a
    /// fork branch with a colliding name can't label a local one; non-open
    /// rows are excluded so a merged PR stops decorating a reused branch.
    pub fn by_head_branch(
        conn: &Connection,
        repo_hash: &str,
        branch: &str,
    ) -> Result<Option<ForgePrRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, kind, number, title, state, head_branch,
                        is_cross_repo, ci_status, url, author, fetched_at
                 FROM forge_prs
                 WHERE repo_hash = ?1 AND head_branch = ?2
                   AND state = 'open' AND is_cross_repo = 0
                 ORDER BY number DESC
                 LIMIT 1",
                params![repo_hash, branch],
                row_to_pr,
            )
            .optional()?;
        Ok(row)
    }

    /// One PR by its identity — the inbound match: a worktree checked out via
    /// `daft go pr:N` knows its `(kind, number)` from `branch.<b>.merge`, and
    /// this lookup supplies the CI status regardless of open/closed state.
    pub fn by_number(
        conn: &Connection,
        repo_hash: &str,
        kind: &str,
        number: u32,
    ) -> Result<Option<ForgePrRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, kind, number, title, state, head_branch,
                        is_cross_repo, ci_status, url, author, fetched_at
                 FROM forge_prs
                 WHERE repo_hash = ?1 AND kind = ?2 AND number = ?3",
                params![repo_hash, kind, i64::from(number)],
                row_to_pr,
            )
            .optional()?;
        Ok(row)
    }
}

fn row_to_pr(row: &rusqlite::Row<'_>) -> rusqlite::Result<ForgePrRow> {
    let number: i64 = row.get("number")?;
    let fetched_at_str: String = row.get("fetched_at")?;
    Ok(ForgePrRow {
        repo_hash: row.get("repo_hash")?,
        kind: row.get("kind")?,
        number: number as u32,
        title: row.get("title")?,
        state: row.get("state")?,
        head_branch: row.get("head_branch")?,
        is_cross_repo: row.get("is_cross_repo")?,
        ci_status: row.get("ci_status")?,
        url: row.get("url")?,
        author: row.get("author")?,
        fetched_at: parse_rfc3339(&fetched_at_str, "fetched_at")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection;
    use crate::store::migrate;
    use crate::store::repos::with_write_txn;
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, Connection) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("db.sqlite");
        let mut conn = connection::open_for_test(&path).unwrap();
        migrate::run(&mut conn, &path).unwrap();
        (tmp, conn)
    }

    fn sample(number: u32, head: &str) -> ForgePrRow {
        ForgePrRow {
            repo_hash: "repo".into(),
            kind: "pr".into(),
            number,
            title: format!("feat: change {number}"),
            state: "open".into(),
            head_branch: head.into(),
            is_cross_repo: false,
            ci_status: Some("pass".into()),
            url: format!("https://github.com/acme/widget/pull/{number}"),
            author: "octocat".into(),
            fetched_at: Utc.with_ymd_and_hms(2026, 7, 18, 0, 0, 0).unwrap(),
        }
    }

    #[test]
    fn upsert_and_by_number_round_trip() {
        let (_tmp, conn) = fresh_db();
        ForgePrsRepo::upsert(&conn, &sample(7, "feat/x")).unwrap();
        let back = ForgePrsRepo::by_number(&conn, "repo", "pr", 7)
            .unwrap()
            .unwrap();
        assert_eq!(back, sample(7, "feat/x"));
    }

    #[test]
    fn upsert_refreshes_every_field() {
        let (_tmp, conn) = fresh_db();
        ForgePrsRepo::upsert(&conn, &sample(7, "feat/x")).unwrap();
        let mut fresh = sample(7, "feat/x-renamed");
        fresh.ci_status = Some("fail".into());
        fresh.state = "merged".into();
        ForgePrsRepo::upsert(&conn, &fresh).unwrap();
        let back = ForgePrsRepo::by_number(&conn, "repo", "pr", 7)
            .unwrap()
            .unwrap();
        assert_eq!(back, fresh);
    }

    #[test]
    fn by_head_branch_matches_open_same_repo_only() {
        let (_tmp, conn) = fresh_db();
        ForgePrsRepo::upsert(&conn, &sample(7, "feat/x")).unwrap();

        let mut cross = sample(8, "feat/x");
        cross.is_cross_repo = true;
        ForgePrsRepo::upsert(&conn, &cross).unwrap();

        let mut merged = sample(6, "feat/done");
        merged.state = "merged".into();
        ForgePrsRepo::upsert(&conn, &merged).unwrap();

        // Same-repo open PR matches.
        let hit = ForgePrsRepo::by_head_branch(&conn, "repo", "feat/x")
            .unwrap()
            .unwrap();
        assert_eq!(hit.number, 7, "the same-repo PR wins over the fork PR");

        // A merged PR no longer decorates its branch.
        assert!(
            ForgePrsRepo::by_head_branch(&conn, "repo", "feat/done")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn by_head_branch_missing_returns_none() {
        let (_tmp, conn) = fresh_db();
        assert!(
            ForgePrsRepo::by_head_branch(&conn, "repo", "feat/none")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn replace_snapshot_is_scoped_by_kind() {
        let (_tmp, mut conn) = fresh_db();
        ForgePrsRepo::upsert(&conn, &sample(7, "feat/x")).unwrap();
        let mut mr = sample(3, "feat/glab");
        mr.kind = "mr".into();
        ForgePrsRepo::upsert(&conn, &mr).unwrap();

        // A fresh GitHub snapshot without #7 drops it — but the MR survives.
        with_write_txn(&mut conn, |tx| {
            ForgePrsRepo::replace_snapshot(tx, "repo", "pr", &[sample(9, "feat/y")])
        })
        .unwrap();

        assert!(
            ForgePrsRepo::by_number(&conn, "repo", "pr", 7)
                .unwrap()
                .is_none(),
            "vanished PRs leave the snapshot"
        );
        assert!(
            ForgePrsRepo::by_number(&conn, "repo", "pr", 9)
                .unwrap()
                .is_some()
        );
        assert!(
            ForgePrsRepo::by_number(&conn, "repo", "mr", 3)
                .unwrap()
                .is_some(),
            "a GitHub refresh must not wipe cached GitLab MRs"
        );
    }

    #[test]
    fn list_for_repo_orders_open_first_and_scopes_by_repo() {
        let (_tmp, conn) = fresh_db();
        let mut merged = sample(9, "feat/done");
        merged.state = "merged".into();
        ForgePrsRepo::upsert(&conn, &merged).unwrap();
        ForgePrsRepo::upsert(&conn, &sample(3, "feat/a")).unwrap();
        ForgePrsRepo::upsert(&conn, &sample(5, "feat/b")).unwrap();
        let mut other = sample(1, "feat/other");
        other.repo_hash = "other-repo".into();
        ForgePrsRepo::upsert(&conn, &other).unwrap();

        let rows = ForgePrsRepo::list_for_repo(&conn, "repo").unwrap();
        let numbers: Vec<u32> = rows.iter().map(|r| r.number).collect();
        assert_eq!(numbers, vec![5, 3, 9], "open first, then newest-first");
    }

    #[test]
    fn null_ci_status_round_trips() {
        let (_tmp, conn) = fresh_db();
        let mut row = sample(7, "feat/x");
        row.ci_status = None;
        ForgePrsRepo::upsert(&conn, &row).unwrap();
        let back = ForgePrsRepo::by_number(&conn, "repo", "pr", 7)
            .unwrap()
            .unwrap();
        assert_eq!(back.ci_status, None);
    }
}
