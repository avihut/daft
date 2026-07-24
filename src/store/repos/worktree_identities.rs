//! Queries against the `worktree_identities` table (a worktree's intended branch).

use crate::store::error::Result;
use crate::store::models::{WorktreeIdentityRow, WorktreeKind};
use crate::store::repos::invocations::parse_rfc3339;
use rusqlite::{Connection, OptionalExtension, params};

pub struct WorktreeIdentitiesRepo;

impl WorktreeIdentitiesRepo {
    /// Record what a worktree is for. The latest observation wins: a
    /// worktree's intent changes when it is renamed or re-purposed, and
    /// there is no first-seen worth preserving. Rewrites the whole row —
    /// including `kind` — so re-purposing a sandbox into a branch worktree
    /// (or the reverse) leaves no stale sandbox metadata behind.
    pub fn upsert(conn: &Connection, row: &WorktreeIdentityRow) -> Result<()> {
        conn.execute(
            "INSERT INTO worktree_identities
                 (repo_hash, worktree_id, branch, worktree_path, updated_at,
                  kind, source_spelling, pinned_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(repo_hash, worktree_id) DO UPDATE SET
                 branch          = excluded.branch,
                 worktree_path   = excluded.worktree_path,
                 updated_at      = excluded.updated_at,
                 kind            = excluded.kind,
                 source_spelling = excluded.source_spelling,
                 pinned_commit   = excluded.pinned_commit",
            params![
                row.repo_hash,
                row.worktree_id,
                row.branch,
                row.worktree_path,
                row.updated_at.to_rfc3339(),
                row.kind.as_str(),
                row.source_spelling,
                row.pinned_commit,
            ],
        )?;
        Ok(())
    }

    /// Record an *observation* of a worktree attached to a branch.
    ///
    /// Fills in a worktree that has no record yet — which is how worktrees
    /// daft did not create acquire one — and refreshes the path and timestamp
    /// of one that does. It deliberately never rewrites `branch`: the record
    /// holds what the worktree is *for*, and seeing a different branch checked
    /// out is the definition of drift, not a correction of it. Silently
    /// adopting the new branch would erase the disagreement before anyone
    /// could be told about it.
    ///
    /// [`Self::upsert`] is the deliberate path — creation, `daft rename`, and
    /// `daft doctor --fix` — where the caller means to redefine the intent.
    pub fn observe(conn: &Connection, row: &WorktreeIdentityRow) -> Result<()> {
        conn.execute(
            "INSERT INTO worktree_identities
                 (repo_hash, worktree_id, branch, worktree_path, updated_at,
                  kind, source_spelling, pinned_commit)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(repo_hash, worktree_id) DO UPDATE SET
                 worktree_path = excluded.worktree_path,
                 updated_at    = excluded.updated_at",
            params![
                row.repo_hash,
                row.worktree_id,
                row.branch,
                row.worktree_path,
                row.updated_at.to_rfc3339(),
                row.kind.as_str(),
                row.source_spelling,
                row.pinned_commit,
            ],
        )?;
        Ok(())
    }

    pub fn get(
        conn: &Connection,
        repo_hash: &str,
        worktree_id: &str,
    ) -> Result<Option<WorktreeIdentityRow>> {
        let row = conn
            .query_row(
                "SELECT repo_hash, worktree_id, branch, worktree_path, updated_at,
                        kind, source_spelling, pinned_commit
                 FROM worktree_identities
                 WHERE repo_hash = ?1 AND worktree_id = ?2",
                params![repo_hash, worktree_id],
                row_to_identity,
            )
            .optional()?;
        Ok(row)
    }

    /// Every recorded identity for a repo, ordered by id for stable output.
    /// The list reads the whole set once rather than querying per worktree.
    pub fn list_for_repo(conn: &Connection, repo_hash: &str) -> Result<Vec<WorktreeIdentityRow>> {
        let mut stmt = conn.prepare(
            "SELECT repo_hash, worktree_id, branch, worktree_path, updated_at,
                    kind, source_spelling, pinned_commit
             FROM worktree_identities
             WHERE repo_hash = ?1
             ORDER BY worktree_id ASC",
        )?;
        let rows = stmt
            .query_map(params![repo_hash], row_to_identity)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Forget a worktree's identity, when the worktree itself is gone.
    /// Returns rows deleted (0 or 1).
    pub fn delete(conn: &Connection, repo_hash: &str, worktree_id: &str) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM worktree_identities WHERE repo_hash = ?1 AND worktree_id = ?2",
            params![repo_hash, worktree_id],
        )?;
        Ok(n)
    }

    /// Forget every identity recorded for a branch, whichever worktree held
    /// it. Removal paths know the branch they are deleting, not the
    /// private-gitdir id — git has usually already unregistered the worktree
    /// by the time they can clean up.
    ///
    /// Branch rows only: a sandbox whose directory name happens to match a
    /// deleted branch is a different worktree and must survive the sweep.
    pub fn delete_for_branch(conn: &Connection, repo_hash: &str, branch: &str) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM worktree_identities
             WHERE repo_hash = ?1 AND branch = ?2 AND kind = 'branch'",
            params![repo_hash, branch],
        )?;
        Ok(n)
    }

    /// Forget a sandbox by its directory-name identity — the removal
    /// fallback when the private-gitdir id could no longer be read. The
    /// mirror image of [`Self::delete_for_branch`]: sandbox rows only, so a
    /// branch named like the sandbox survives.
    pub fn delete_sandbox_by_name(conn: &Connection, repo_hash: &str, name: &str) -> Result<usize> {
        let n = conn.execute(
            "DELETE FROM worktree_identities
             WHERE repo_hash = ?1 AND branch = ?2 AND kind IN ('canonical', 'fork')",
            params![repo_hash, name],
        )?;
        Ok(n)
    }
}

fn row_to_identity(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorktreeIdentityRow> {
    let updated_at_str: String = row.get("updated_at")?;
    let kind_str: String = row.get("kind")?;
    Ok(WorktreeIdentityRow {
        repo_hash: row.get("repo_hash")?,
        worktree_id: row.get("worktree_id")?,
        branch: row.get("branch")?,
        worktree_path: row.get("worktree_path")?,
        updated_at: parse_rfc3339(&updated_at_str, "updated_at")?,
        kind: WorktreeKind::parse(&kind_str),
        source_spelling: row.get("source_spelling")?,
        pinned_commit: row.get("pinned_commit")?,
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

    fn sample(worktree_id: &str, branch: &str) -> WorktreeIdentityRow {
        WorktreeIdentityRow {
            repo_hash: "repo".into(),
            worktree_id: worktree_id.into(),
            branch: branch.into(),
            // Deliberately not `format!`: the repo layer bans that macro so
            // no SQL can ever be built by interpolation (mise-tasks/lint/
            // repos-no-format), and the grep does not exempt test code.
            worktree_path: String::from("/tmp/wt/") + worktree_id,
            updated_at: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            kind: WorktreeKind::Branch,
            source_spelling: None,
            pinned_commit: None,
        }
    }

    fn sandbox(worktree_id: &str, name: &str, kind: WorktreeKind) -> WorktreeIdentityRow {
        let mut row = sample(worktree_id, name);
        row.kind = kind;
        row.source_spelling = Some("v1.18.0".into());
        row.pinned_commit = Some("a".repeat(40));
        row
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let (_tmp, conn) = fresh_db();
        let row = sample("wt-a", "feat/x");
        WorktreeIdentitiesRepo::upsert(&conn, &row).unwrap();
        assert_eq!(
            WorktreeIdentitiesRepo::get(&conn, "repo", "wt-a").unwrap(),
            Some(row)
        );
    }

    #[test]
    fn get_returns_none_for_an_unknown_worktree() {
        let (_tmp, conn) = fresh_db();
        assert_eq!(
            WorktreeIdentitiesRepo::get(&conn, "repo", "nope").unwrap(),
            None
        );
    }

    /// A worktree renamed to another branch keeps its key — the private-gitdir
    /// id survives a rename — and the row is updated, not duplicated.
    #[test]
    fn a_rename_updates_in_place_under_the_same_key() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "feat/old")).unwrap();

        let mut renamed = sample("wt-a", "feat/new");
        renamed.worktree_path = "/tmp/wt/renamed".into();
        renamed.updated_at = Utc.with_ymd_and_hms(2026, 2, 2, 0, 0, 0).unwrap();
        WorktreeIdentitiesRepo::upsert(&conn, &renamed).unwrap();

        assert_eq!(
            WorktreeIdentitiesRepo::list_for_repo(&conn, "repo").unwrap(),
            vec![renamed],
            "the rename must replace the record, not add a second one"
        );
    }

    #[test]
    fn repos_are_isolated_from_each_other() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "feat/x")).unwrap();
        let mut other = sample("wt-a", "other/branch");
        other.repo_hash = "other-repo".into();
        WorktreeIdentitiesRepo::upsert(&conn, &other).unwrap();

        // Same worktree id, different repo — both survive, each visible only
        // to its own repo.
        assert_eq!(
            WorktreeIdentitiesRepo::get(&conn, "repo", "wt-a")
                .unwrap()
                .map(|r| r.branch),
            Some("feat/x".to_string())
        );
        assert_eq!(
            WorktreeIdentitiesRepo::list_for_repo(&conn, "other-repo").unwrap(),
            vec![other]
        );
    }

    #[test]
    fn list_for_repo_is_ordered_and_scoped() {
        let (_tmp, conn) = fresh_db();
        for id in ["wt-c", "wt-a", "wt-b"] {
            WorktreeIdentitiesRepo::upsert(&conn, &sample(id, "feat/x")).unwrap();
        }
        let ids: Vec<String> = WorktreeIdentitiesRepo::list_for_repo(&conn, "repo")
            .unwrap()
            .into_iter()
            .map(|r| r.worktree_id)
            .collect();
        assert_eq!(ids, vec!["wt-a", "wt-b", "wt-c"]);
    }

    #[test]
    fn delete_removes_only_the_named_worktree() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "feat/x")).unwrap();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-b", "feat/y")).unwrap();

        assert_eq!(
            WorktreeIdentitiesRepo::delete(&conn, "repo", "wt-a").unwrap(),
            1
        );
        assert_eq!(
            WorktreeIdentitiesRepo::delete(&conn, "repo", "wt-a").unwrap(),
            0,
            "deleting twice is not an error"
        );
        let remaining: Vec<String> = WorktreeIdentitiesRepo::list_for_repo(&conn, "repo")
            .unwrap()
            .into_iter()
            .map(|r| r.worktree_id)
            .collect();
        assert_eq!(remaining, vec!["wt-b"]);
    }

    /// Observation fills in what is missing and refreshes the path, but the
    /// recorded branch is intent — only a deliberate write redefines it.
    /// Without this, drift would erase itself on the next `daft list`.
    #[test]
    fn observation_never_rewrites_the_recorded_branch() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "feat/x")).unwrap();

        let mut observed = sample("wt-a", "hotfix/urgent");
        observed.worktree_path = "/tmp/wt/moved".into();
        WorktreeIdentitiesRepo::observe(&conn, &observed).unwrap();

        let row = WorktreeIdentitiesRepo::get(&conn, "repo", "wt-a")
            .unwrap()
            .unwrap();
        assert_eq!(row.branch, "feat/x", "intent survives an observation");
        assert_eq!(row.worktree_path, "/tmp/wt/moved", "but the path refreshes");

        // A deliberate write is what changes intent.
        WorktreeIdentitiesRepo::upsert(&conn, &observed).unwrap();
        assert_eq!(
            WorktreeIdentitiesRepo::get(&conn, "repo", "wt-a")
                .unwrap()
                .unwrap()
                .branch,
            "hotfix/urgent"
        );
    }

    #[test]
    fn observation_records_a_worktree_that_has_none() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::observe(&conn, &sample("wt-new", "feat/new")).unwrap();
        assert_eq!(
            WorktreeIdentitiesRepo::get(&conn, "repo", "wt-new")
                .unwrap()
                .map(|r| r.branch),
            Some("feat/new".to_string())
        );
    }

    /// Removal paths know the branch, not the private-gitdir id: git has
    /// usually unregistered the worktree before daft can clean up.
    #[test]
    fn delete_for_branch_forgets_every_worktree_of_that_branch() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "feat/x")).unwrap();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-b", "feat/y")).unwrap();

        assert_eq!(
            WorktreeIdentitiesRepo::delete_for_branch(&conn, "repo", "feat/x").unwrap(),
            1
        );
        let remaining: Vec<String> = WorktreeIdentitiesRepo::list_for_repo(&conn, "repo")
            .unwrap()
            .into_iter()
            .map(|r| r.branch)
            .collect();
        assert_eq!(remaining, vec!["feat/y"]);
    }

    /// A sandbox and a branch row round-trip their kind and sandbox fields.
    #[test]
    fn sandbox_rows_round_trip_kind_spelling_and_pin() {
        let (_tmp, conn) = fresh_db();
        let row = sandbox("wt-s", "v1.18.0", WorktreeKind::Canonical);
        WorktreeIdentitiesRepo::upsert(&conn, &row).unwrap();
        assert_eq!(
            WorktreeIdentitiesRepo::get(&conn, "repo", "wt-s").unwrap(),
            Some(row)
        );
    }

    /// The gate this migration exists for: removing a branch named like a
    /// sandbox's directory must not take the sandbox's record with it.
    #[test]
    fn delete_for_branch_spares_a_same_named_sandbox() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "scratch")).unwrap();
        WorktreeIdentitiesRepo::upsert(&conn, &sandbox("wt-s", "scratch", WorktreeKind::Fork))
            .unwrap();

        assert_eq!(
            WorktreeIdentitiesRepo::delete_for_branch(&conn, "repo", "scratch").unwrap(),
            1,
            "only the branch row goes"
        );
        let row = WorktreeIdentitiesRepo::get(&conn, "repo", "wt-s")
            .unwrap()
            .expect("the sandbox record survives");
        assert_eq!(row.kind, WorktreeKind::Fork);
    }

    /// And the mirror image: forgetting a sandbox by name spares a branch
    /// that happens to share the spelling.
    #[test]
    fn delete_sandbox_by_name_spares_a_same_named_branch() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-a", "scratch")).unwrap();
        WorktreeIdentitiesRepo::upsert(&conn, &sandbox("wt-s", "scratch", WorktreeKind::Canonical))
            .unwrap();

        assert_eq!(
            WorktreeIdentitiesRepo::delete_sandbox_by_name(&conn, "repo", "scratch").unwrap(),
            1
        );
        let row = WorktreeIdentitiesRepo::get(&conn, "repo", "wt-a")
            .unwrap()
            .expect("the branch record survives");
        assert_eq!(row.kind, WorktreeKind::Branch);
    }

    /// Observation refreshes path/timestamp but never rewrites what the row
    /// *is* — including its kind and sandbox metadata.
    #[test]
    fn observation_never_rewrites_kind_or_sandbox_fields() {
        let (_tmp, conn) = fresh_db();
        let row = sandbox("wt-s", "v1.18.0", WorktreeKind::Canonical);
        WorktreeIdentitiesRepo::upsert(&conn, &row).unwrap();

        let mut observed = sample("wt-s", "some-branch");
        observed.worktree_path = "/tmp/wt/moved".into();
        WorktreeIdentitiesRepo::observe(&conn, &observed).unwrap();

        let read = WorktreeIdentitiesRepo::get(&conn, "repo", "wt-s")
            .unwrap()
            .unwrap();
        assert_eq!(read.kind, WorktreeKind::Canonical, "kind is intent");
        assert_eq!(read.branch, "v1.18.0", "so is the name");
        assert_eq!(read.source_spelling.as_deref(), Some("v1.18.0"));
        assert!(read.pinned_commit.is_some());
        assert_eq!(read.worktree_path, "/tmp/wt/moved", "the path refreshes");
    }

    /// A deliberate upsert re-purposes a sandbox into a branch worktree and
    /// leaves no sandbox metadata behind (doctor --fix relies on this).
    #[test]
    fn upsert_repurposes_a_sandbox_into_a_branch_row() {
        let (_tmp, conn) = fresh_db();
        WorktreeIdentitiesRepo::upsert(&conn, &sandbox("wt-s", "v1.18.0", WorktreeKind::Fork))
            .unwrap();
        WorktreeIdentitiesRepo::upsert(&conn, &sample("wt-s", "feat/adopted")).unwrap();

        let read = WorktreeIdentitiesRepo::get(&conn, "repo", "wt-s")
            .unwrap()
            .unwrap();
        assert_eq!(read.kind, WorktreeKind::Branch);
        assert_eq!(read.branch, "feat/adopted");
        assert_eq!(read.source_spelling, None);
        assert_eq!(read.pinned_commit, None);
    }
}
