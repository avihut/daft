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

use crate::store::models::{WorktreeIdentityRow, WorktreeKind};
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

    /// Record that `worktree_path` is for `branch` — a deliberate
    /// (re)definition of intent.
    ///
    /// Reserved for the moments that *decide* what a worktree is for:
    /// creation, `daft rename`, and `daft doctor --fix`. Merely seeing a
    /// worktree attached to a branch is not one of them — that is an
    /// observation and goes through [`Self::observe_all`], which never
    /// rewrites the branch. Calling this from an observation path would
    /// erase drift the moment a command touched the worktree, before doctor
    /// could ever report it.
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
            kind: WorktreeKind::Branch,
            source_spelling: None,
            pinned_commit: None,
        };
        if let Err(e) = self.write(|conn| WorktreeIdentitiesRepo::upsert(conn, &row)) {
            crate::log_debug!("could not record worktree identity: {e}");
        }
    }

    /// Record that `worktree_path` is an anonymous sandbox named `dirname`,
    /// created from `source_spelling` and pinned at `pinned_commit`.
    ///
    /// The sandbox counterpart of [`Self::record`]: the directory name goes
    /// where a branch identity would, and `kind` says which meaning the row
    /// carries. `Canonical` rows are what `daft go <commit-ish>` matches on a
    /// revisit; `Fork` rows are never matched by resolution — a fork is
    /// reachable only by its name.
    pub fn record_sandbox(
        &self,
        worktree_path: &Path,
        dirname: &str,
        kind: WorktreeKind,
        source_spelling: &str,
        pinned_commit: &str,
    ) {
        let Some(worktree_id) = worktree_id_for(worktree_path) else {
            return;
        };
        let row = WorktreeIdentityRow {
            repo_hash: self.repo_hash.clone(),
            worktree_id,
            branch: dirname.to_string(),
            worktree_path: worktree_path.display().to_string(),
            updated_at: chrono::Utc::now(),
            kind,
            source_spelling: Some(source_spelling.to_string()),
            pinned_commit: Some(pinned_commit.to_string()),
        };
        if let Err(e) = self.write(|conn| WorktreeIdentitiesRepo::upsert(conn, &row)) {
            crate::log_debug!("could not record sandbox identity: {e}");
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
                    // Observations are of *attached* worktrees, so a fill-in
                    // is a branch identity; on an existing row the repo layer
                    // refreshes only path/timestamp, never the kind.
                    kind: WorktreeKind::Branch,
                    source_spelling: None,
                    pinned_commit: None,
                })
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        if let Err(e) = self.write_fail_fast(|conn| {
            for row in &rows {
                WorktreeIdentitiesRepo::observe(conn, row)?;
            }
            Ok(())
        }) {
            crate::log_debug!("could not record worktree identities: {e}");
        }
    }

    /// Re-point the record for the worktree now sitting at `worktree_path`.
    ///
    /// `daft repo move` relocating a whole repository: the worktree is the same
    /// worktree, so only its path changes. Call it *after* the directory move
    /// and git's linkage repair — the private-gitdir id is read back off the
    /// moved worktree, which matches the row by identity rather than by its old
    /// path. Matching on the old path would be the fragile choice: a
    /// layout-driven move need not preserve any prefix, and git and daft can
    /// spell the same directory differently.
    ///
    /// Best-effort like the rest of this module. A failure costs `daft list` a
    /// name until the next observation, not correctness. Returns whether a
    /// record was updated — `false` also covers a worktree daft never recorded.
    pub fn rewrite_path(&self, worktree_path: &Path) -> bool {
        let Some(worktree_id) = worktree_id_for(worktree_path) else {
            return false;
        };
        let new_path = worktree_path.display().to_string();
        match self.write(|conn| {
            WorktreeIdentitiesRepo::rewrite_path(
                conn,
                &self.repo_hash,
                &worktree_id,
                &new_path,
                chrono::Utc::now(),
            )
        }) {
            Ok(updated) => updated > 0,
            Err(e) => {
                crate::log_debug!("could not re-point worktree identity: {e}");
                false
            }
        }
    }

    /// Forget a removed worktree's record.
    ///
    /// `captured_id` is the private-gitdir id read via [`worktree_id_for`]
    /// *before* the removal — records are keyed on it, so deleting by id
    /// removes the record no matter what branch it names. That is what
    /// removal wants: a drifted or externally-renamed worktree's record must
    /// not outlive its worktree, and git reuses freed ids, so a leftover row
    /// would paint spurious drift on the id's next tenant. When the
    /// directory was already gone before the id could be read, fall back to
    /// [`Self::forget_branch`].
    pub fn forget(&self, captured_id: Option<&str>, branch: &str) {
        let Some(id) = captured_id else {
            self.forget_branch(branch);
            return;
        };
        if let Err(e) =
            self.write(|conn| WorktreeIdentitiesRepo::delete(conn, &self.repo_hash, id).map(|_| ()))
        {
            crate::log_debug!("could not forget worktree identity: {e}");
        }
    }

    /// Forget every record naming `branch` — the fallback for removal paths
    /// that could no longer read the private-gitdir id (the directory was
    /// already gone). Over-broad (two records naming one branch both go) and
    /// blind to drifted rows (the record names the intent, not the checkout),
    /// which is why [`Self::forget`] prefers the id. Branch rows only: a
    /// sandbox sharing the name is a different worktree.
    pub fn forget_branch(&self, branch: &str) {
        if let Err(e) = self.write(|conn| {
            WorktreeIdentitiesRepo::delete_for_branch(conn, &self.repo_hash, branch).map(|_| ())
        }) {
            crate::log_debug!("could not forget worktree identity: {e}");
        }
    }

    /// Forget a removed sandbox's record: by captured private-gitdir id when
    /// one was read before the removal, else by directory name (sandbox rows
    /// only — the mirror of [`Self::forget`]'s branch fallback).
    pub fn forget_sandbox(&self, captured_id: Option<&str>, dirname: &str) {
        if let Some(id) = captured_id {
            self.forget_id(id);
            return;
        }
        if let Err(e) = self.write(|conn| {
            WorktreeIdentitiesRepo::delete_sandbox_by_name(conn, &self.repo_hash, dirname)
                .map(|_| ())
        }) {
            crate::log_debug!("could not forget sandbox identity: {e}");
        }
    }

    /// Forget the record keyed on `worktree_id`, whatever it names. For
    /// callers that discover a stale row themselves (a recorded sandbox whose
    /// worktree is gone) and have no branch or dirname to sweep by.
    pub fn forget_id(&self, worktree_id: &str) {
        if let Err(e) = self.write(|conn| {
            WorktreeIdentitiesRepo::delete(conn, &self.repo_hash, worktree_id).map(|_| ())
        }) {
            crate::log_debug!("could not forget worktree identity: {e}");
        }
    }

    fn write<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::store::error::Result<T>,
    ) -> crate::store::error::Result<T> {
        self.write_inner(false, f)
    }

    /// Like [`Self::write`], but fail fast instead of waiting out the full
    /// writer busy timeout. For writes on otherwise read-only interactive
    /// paths (`daft list`'s observation pass): don't let a display-nicety
    /// write block the prompt for 5s when a coordinator/sync process holds
    /// the coordinator.db write lock — fail with SQLITE_BUSY (swallowed by
    /// the caller) and let the next run observe instead. Same convention as
    /// `size_cache::persist_inner` and the forge_cache writers (review 5).
    /// Deliberate writes (record/forget) keep the patient default: they run
    /// on mutating commands, where completing the write matters more.
    fn write_fail_fast<T>(
        &self,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::store::error::Result<T>,
    ) -> crate::store::error::Result<T> {
        self.write_inner(true, f)
    }

    fn write_inner<T>(
        &self,
        fail_fast: bool,
        f: impl FnOnce(&rusqlite::Transaction<'_>) -> crate::store::error::Result<T>,
    ) -> crate::store::error::Result<T> {
        let pool = Pool::open(&self.db_path)?;
        let mut conn = pool.writer()?;
        if fail_fast {
            conn.busy_timeout(std::time::Duration::from_millis(
                crate::store::connection::READER_BUSY_TIMEOUT_MS as u64,
            ))?;
        }
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

    /// Removal deletes by the captured id, not the branch: a drifted record
    /// does not match the live branch name the removal path knows, and a
    /// second record legitimately naming the same branch must survive.
    /// Regression: the by-branch sweep missed the drifted row (orphaning it
    /// onto git's next reuse of the id) and deleted the innocent one.
    #[test]
    #[serial]
    fn forget_prefers_the_captured_id_and_spares_same_branch_records() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (tmp, common, wt_a) = linked_worktree("wt-a");
        let private_b = common.join("worktrees/wt-b");
        std::fs::create_dir_all(&private_b).unwrap();
        let wt_b = tmp.path().join("wt-b");
        std::fs::create_dir_all(&wt_b).unwrap();
        std::fs::write(
            wt_b.join(".git"),
            String::from("gitdir: ") + private_b.to_str().unwrap() + "\n",
        )
        .unwrap();
        let store = IdentityStore::open(&common).unwrap();

        // wt-a was created for feat/x but has drifted: the branch checked
        // out (and being removed) is now `hotfix`. wt-b is feat/x's new home.
        store.record(&wt_a, "feat/x");
        store.record(&wt_b, "feat/x");

        // The id is captured while the directory still exists…
        let captured = worktree_id_for(&wt_a);
        assert_eq!(captured.as_deref(), Some("wt-a"));
        std::fs::remove_dir_all(&wt_a).unwrap();

        // …so the drifted record dies with its worktree, by id, even though
        // the removal path only knows the live branch name.
        store.forget(captured.as_deref(), "hotfix");

        let found = read_identities(&common);
        assert!(
            !found.contains_key("wt-a"),
            "the removed worktree's record is gone"
        );
        assert_eq!(
            found["wt-b"].branch, "feat/x",
            "a same-branch record for another worktree survives"
        );
    }

    /// When the directory was already gone before the id could be read, the
    /// by-branch sweep is all that is left.
    #[test]
    #[serial]
    fn forget_falls_back_to_the_branch_without_an_id() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, worktree) = linked_worktree("wt-a");
        let store = IdentityStore::open(&common).unwrap();
        store.record(&worktree, "feat/x");

        store.forget(None, "feat/x");
        assert!(read_identities(&common).is_empty());
    }

    #[test]
    #[serial]
    fn sandbox_records_round_trip_with_kind_and_pin() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (_tmp, common, worktree) = linked_worktree("wt-s");
        let store = IdentityStore::open(&common).expect("store opens");

        store.record_sandbox(
            &worktree,
            "v1.18.0",
            WorktreeKind::Canonical,
            "v1.18.0",
            "abc123def4567890abc123def4567890abc123de",
        );

        let found = read_identities(&common);
        let row = &found["wt-s"];
        assert_eq!(row.branch, "v1.18.0", "the dirname is the identity");
        assert_eq!(row.kind, WorktreeKind::Canonical);
        assert_eq!(row.source_spelling.as_deref(), Some("v1.18.0"));
        assert_eq!(
            row.pinned_commit.as_deref(),
            Some("abc123def4567890abc123def4567890abc123de")
        );
    }

    /// Sandbox removal sweeps by dirname when no id was captured — and only
    /// sandbox rows, so a branch sharing the spelling survives.
    #[test]
    #[serial]
    fn forget_sandbox_by_name_spares_the_same_named_branch_record() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let (tmp, common, wt_branch) = linked_worktree("wt-a");
        let private_s = common.join("worktrees/wt-s");
        std::fs::create_dir_all(&private_s).unwrap();
        let wt_s = tmp.path().join("wt-s");
        std::fs::create_dir_all(&wt_s).unwrap();
        std::fs::write(
            wt_s.join(".git"),
            String::from("gitdir: ") + private_s.to_str().unwrap() + "\n",
        )
        .unwrap();
        let store = IdentityStore::open(&common).unwrap();

        store.record(&wt_branch, "scratch");
        store.record_sandbox(
            &wt_s,
            "scratch",
            WorktreeKind::Fork,
            "HEAD",
            &"a".repeat(40),
        );

        store.forget_sandbox(None, "scratch");

        let found = read_identities(&common);
        assert!(!found.contains_key("wt-s"), "the sandbox record is gone");
        assert_eq!(
            found["wt-a"].branch, "scratch",
            "the like-named branch record survives"
        );
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
