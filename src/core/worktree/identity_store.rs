//! Remembering which branch a worktree was created for.
//!
//! Daft knows a worktree's intended branch at the moment it creates it, and
//! every time it later observes the worktree with that branch attached. Git
//! forgets as soon as HEAD detaches for a reason it does not record — a tag or
//! SHA checkout. Recording the fact here is what lets `daft list` still name
//! such a worktree instead of showing an anonymous `(detached)` row.
//!
//! **Derived state always wins.** These records are consulted only after live
//! git state has nothing to say ([`super::identity`]), and cross-checked for
//! drift. A record can be out of date; live state cannot.
//!
//! Everything here is **best-effort**, following the size cache
//! ([`crate::commands::size_cache`]): identity is a display nicety, not
//! correctness, so a missing store, a busy database or a schema from a newer
//! build degrades to "no record" rather than failing a command. Reads never
//! create the store; only writes do.

use crate::store::models::WorktreeIdentityRow;
use crate::store::repos::{WorktreeIdentitiesRepo, with_write_txn};
use crate::store::{Pool, paths};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The private-gitdir id for a worktree — the directory name under
/// `<common-dir>/worktrees/`, which is what records are keyed on.
///
/// Stable across `git worktree move` and branch renames. The main worktree of
/// a non-bare repo has no such directory (its git dir *is* the common dir), so
/// it has no id and is never recorded — no loss, since a repo's main worktree
/// is not something daft creates for a branch.
pub fn worktree_id_for(worktree_path: &Path) -> Option<String> {
    let git_dir = crate::git::op_state::resolve_worktree_git_dir(worktree_path).ok()?;
    // Only linked worktrees live under `worktrees/<id>`.
    if git_dir.parent()?.file_name()? != "worktrees" {
        return None;
    }
    Some(git_dir.file_name()?.to_str()?.to_string())
}

/// Handle on one repo's identity records.
///
/// `None` from [`Self::open`] means "operate without records", never an error
/// the caller has to handle.
pub struct IdentityStore {
    repo_hash: String,
    db_path: PathBuf,
}

impl IdentityStore {
    /// Open the identity records for the repo whose git common dir is
    /// `git_common_dir`, for writing. Creates the store if absent.
    pub fn open(git_common_dir: &Path) -> Option<Self> {
        let repo_hash =
            match crate::core::repo_identity::compute_repo_id_from_common_dir(git_common_dir) {
                Ok(id) => id,
                Err(e) => {
                    crate::log_debug!("worktree identities unavailable (repo identity): {e:#}");
                    return None;
                }
            };
        let db_path = match paths::for_repo(&repo_hash) {
            Ok(p) => p,
            Err(e) => {
                crate::log_debug!("worktree identities unavailable (store path): {e}");
                return None;
            }
        };
        Some(Self { repo_hash, db_path })
    }

    /// Record that `worktree_path` is for `branch`.
    ///
    /// Callers must only report worktrees they have observed *attached* to
    /// that branch, or just created for it. Recording from a detached
    /// observation would let a stale reading overwrite the good record it is
    /// supposed to be the fallback for.
    pub fn record(&self, worktree_path: &Path, branch: &str) {
        let Some(worktree_id) = worktree_id_for(worktree_path) else {
            return;
        };
        let row = WorktreeIdentityRow {
            repo_hash: self.repo_hash.clone(),
            worktree_id,
            branch: branch.to_string(),
            worktree_path: worktree_path.display().to_string(),
            updated_at: chrono::Utc::now(),
        };
        if let Err(e) = self.write(|conn| WorktreeIdentitiesRepo::upsert(conn, &row)) {
            crate::log_debug!("could not record worktree identity: {e}");
        }
    }

    /// Note several worktrees observed attached to their branches, in one
    /// transaction — the shape the list paths need.
    ///
    /// Non-destructive: a worktree with no record gets one, and an existing
    /// record has its path and timestamp refreshed but keeps its branch. An
    /// observation is evidence of what is checked out *now*, which is not the
    /// same claim as what the worktree is *for* — conflating them would let a
    /// listing quietly redefine intent and erase drift before it was reported.
    pub fn observe_all<'a>(&self, observations: impl IntoIterator<Item = (&'a Path, &'a str)>) {
        let now = chrono::Utc::now();
        let rows: Vec<WorktreeIdentityRow> = observations
            .into_iter()
            .filter_map(|(path, branch)| {
                Some(WorktreeIdentityRow {
                    repo_hash: self.repo_hash.clone(),
                    worktree_id: worktree_id_for(path)?,
                    branch: branch.to_string(),
                    worktree_path: path.display().to_string(),
                    updated_at: now,
                })
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        if let Err(e) = self.write(|conn| {
            for row in &rows {
                WorktreeIdentitiesRepo::observe(conn, row)?;
            }
            Ok(())
        }) {
            crate::log_debug!("could not record worktree identities: {e}");
        }
    }

    /// Forget every record for `branch`. Called when a worktree is removed —
    /// removal paths know the branch, not the private-gitdir id, because git
    /// has usually unregistered the worktree by then.
    pub fn forget_branch(&self, branch: &str) {
        if let Err(e) = self.write(|conn| {
            WorktreeIdentitiesRepo::delete_for_branch(conn, &self.repo_hash, branch).map(|_| ())
        }) {
            crate::log_debug!("could not forget worktree identity: {e}");
        }
    }

    fn write<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::store::error::Result<T>,
    ) -> crate::store::error::Result<T> {
        let pool = Pool::open(&self.db_path)?;
        let mut conn = pool.writer()?;
        with_write_txn(&mut conn, f)
    }
}

/// Every recorded identity for a repo, keyed by private-gitdir id.
///
/// A **pure read**: it does not create the store, so a repo that has never
/// recorded anything (or a build that predates the table) yields an empty map
/// rather than materializing a database from a read-only command.
pub fn read_identities(git_common_dir: &Path) -> HashMap<String, WorktreeIdentityRow> {
    read_inner(git_common_dir).unwrap_or_default()
}

fn read_inner(git_common_dir: &Path) -> Option<HashMap<String, WorktreeIdentityRow>> {
    let repo_hash =
        crate::core::repo_identity::compute_repo_id_from_common_dir(git_common_dir).ok()?;
    let state_dir = crate::daft_state_dir().ok()?;
    let db_path = state_dir
        .join(paths::JOBS_SUBDIR)
        .join(&repo_hash)
        .join(paths::COORDINATOR_DB);
    // Don't open (and thereby create) the store just to find it empty. The
    // pool's read-write bootstrap is used rather than a bare read-only
    // connection: a checkpointed coordinator.db with no -wal/-shm sidecar is
    // SQLITE_CANTOPEN under SQLITE_OPEN_READ_ONLY (see size_cache).
    if !db_path.exists() {
        return None;
    }
    let pool = Pool::open(&db_path).ok()?;
    let conn = pool.reader().ok()?;
    let rows = WorktreeIdentitiesRepo::list_for_repo(&conn, &repo_hash).ok()?;
    Some(
        rows.into_iter()
            .map(|row| (row.worktree_id.clone(), row))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Build a linked-worktree shape and return (tempdir, common dir, worktree).
    fn linked_worktree(id: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("repo/.git");
        let private = common.join("worktrees").join(id);
        std::fs::create_dir_all(&private).unwrap();
        let worktree = tmp.path().join(id);
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            String::from("gitdir: ") + private.to_str().unwrap() + "\n",
        )
        .unwrap();
        (tmp, common, worktree)
    }

    #[test]
    fn worktree_id_is_the_private_gitdir_name() {
        let (_tmp, _common, worktree) = linked_worktree("wt-a");
        assert_eq!(worktree_id_for(&worktree).as_deref(), Some("wt-a"));
    }

    /// A non-bare repo's main worktree has no `worktrees/<id>` entry, so it
    /// has no id — and is never recorded.
    #[test]
    fn a_main_worktree_has_no_private_gitdir_id() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        assert_eq!(worktree_id_for(tmp.path()), None);
    }

    #[test]
    fn a_path_that_is_not_a_worktree_has_no_id() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(worktree_id_for(tmp.path()), None);
    }

    #[test]
    #[serial]
    fn records_round_trip_through_the_store() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, worktree) = linked_worktree("wt-a");

        // A pure read on a repo that has recorded nothing must not create
        // the store.
        assert!(read_identities(&common).is_empty());

        let store = IdentityStore::open(&common).expect("store opens");
        store.record(&worktree, "feat/x");

        let found = read_identities(&common);
        assert_eq!(found.len(), 1);
        assert_eq!(found["wt-a"].branch, "feat/x");
        assert_eq!(found["wt-a"].worktree_path, worktree.display().to_string());
    }

    #[test]
    #[serial]
    fn a_later_observation_replaces_the_earlier_one() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, worktree) = linked_worktree("wt-a");
        let store = IdentityStore::open(&common).unwrap();

        store.record(&worktree, "feat/old");
        store.record(&worktree, "feat/new");

        let found = read_identities(&common);
        assert_eq!(found.len(), 1, "the worktree has one identity, not two");
        assert_eq!(found["wt-a"].branch, "feat/new");
    }

    #[test]
    #[serial]
    fn observe_all_writes_every_observation() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, wt_a) = linked_worktree("wt-a");
        // A second worktree under the same common dir.
        let private_b = common.join("worktrees/wt-b");
        std::fs::create_dir_all(&private_b).unwrap();
        let wt_b = common.parent().unwrap().parent().unwrap().join("wt-b");
        std::fs::create_dir_all(&wt_b).unwrap();
        std::fs::write(
            wt_b.join(".git"),
            String::from("gitdir: ") + private_b.to_str().unwrap() + "\n",
        )
        .unwrap();

        let store = IdentityStore::open(&common).unwrap();
        store.observe_all([(wt_a.as_path(), "feat/a"), (wt_b.as_path(), "feat/b")]);

        let found = read_identities(&common);
        assert_eq!(found["wt-a"].branch, "feat/a");
        assert_eq!(found["wt-b"].branch, "feat/b");
    }

    #[test]
    #[serial]
    fn forget_branch_removes_the_record() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, worktree) = linked_worktree("wt-a");
        let store = IdentityStore::open(&common).unwrap();
        store.record(&worktree, "feat/x");

        store.forget_branch("feat/x");
        assert!(read_identities(&common).is_empty());
    }

    /// Paths with no private-gitdir id are silently skipped, not errors —
    /// a main worktree or a vanished directory must not fail a command.
    #[test]
    #[serial]
    fn unrecordable_paths_are_skipped_without_error() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, _worktree) = linked_worktree("wt-a");
        let store = IdentityStore::open(&common).unwrap();

        store.record(Path::new("/nonexistent/worktree"), "feat/x");
        assert!(read_identities(&common).is_empty());
    }
}
