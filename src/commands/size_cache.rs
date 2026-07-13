//! Read/write access to the per-repo worktree size cache — the
//! `worktree_sizes` table in the repo's coordinator store. Powers the
//! stale-then-refresh Size column in `daft list --columns +size`: seed each
//! cell from the last-known value (rendered dim), run the bounded walk, then
//! persist the fresh figures for next time.
//!
//! Everything here is **best-effort**. The cache is a display accelerator,
//! never a source of truth (the walk always runs and supersedes it), so every
//! failure — missing store, busy DB, a walk gap — degrades to "no cache"
//! rather than surfacing an error to the user. Reads never materialize the
//! store; only [`persist_worktree_sizes`] creates it.

use crate::store::Pool;
use crate::store::models::WorktreeSizeRow;
use crate::store::paths;
use crate::store::repos::{WorktreeSizesRepo, with_write_txn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The **stat-guard**, shared by the worktree and repo persist paths so the
/// rule can't drift: only persist a freshly-walked size for a path that still
/// exists on disk. Two things can turn a walked size into a lie before it is
/// written — the walk yields `Some(0)` for a path removed in the TOCTOU window
/// between walk and write (which the walker's root-error → `None` can't catch),
/// and a vanished target must never overwrite a good cached value with 0.
/// Callers that hold an `Option<u64>` pair this with their own `Some` filter.
pub(crate) fn should_persist(path: &Path) -> bool {
    path.exists()
}

/// Last-known worktree sizes for `repo_hash`, keyed by branch slug. Empty on
/// any error and — deliberately — when the coordinator store doesn't exist
/// yet (first run), which it does NOT create: seeding is a pure read.
pub fn read_worktree_sizes(repo_hash: &str) -> HashMap<String, u64> {
    read_inner(repo_hash).unwrap_or_default()
}

fn read_inner(repo_hash: &str) -> Option<HashMap<String, u64>> {
    let state_dir = crate::daft_state_dir().ok()?;
    let db_path = state_dir
        .join(paths::JOBS_SUBDIR)
        .join(repo_hash)
        .join(paths::COORDINATOR_DB);
    // Pure read: don't open (and thereby create) the store just to find it
    // empty. First run has no DB — that's a cache miss, not an error.
    if !db_path.exists() {
        return None;
    }
    let pool = Pool::open(&db_path).ok()?;
    let conn = pool.reader().ok()?;
    let rows = WorktreeSizesRepo::list_for_repo(&conn, repo_hash).ok()?;
    Some(
        rows.into_iter()
            .map(|r| (r.branch_slug, r.size_bytes))
            .collect(),
    )
}

/// Persist freshly-walked worktree sizes for `repo_hash` in a single
/// transaction (the writer pool is size 1 — never one write per worktree).
/// `entries` are `(branch_slug, worktree_path, size_bytes)`.
///
/// **Stat-guard:** an entry whose `worktree_path` no longer exists is skipped.
/// The walk returns `Some(0)` for a vanished directory, and persisting that
/// would clobber a good cached value with `0`; dropping it leaves the last
/// real figure in place until the worktree returns or is explicitly evicted.
///
/// Best-effort: creates the store if missing, swallows any write failure.
/// Callers should pass only sizes that were actually measured this run (not
/// stale-seeded values) so `measured_at` stays honest.
pub fn persist_worktree_sizes(
    repo_hash: &str,
    entries: impl IntoIterator<Item = (String, PathBuf, u64)>,
) {
    let measured_at = chrono::Utc::now();
    let rows: Vec<WorktreeSizeRow> = entries
        .into_iter()
        .filter(|(_, path, _)| should_persist(path)) // stat-guard
        .map(|(branch_slug, path, size_bytes)| WorktreeSizeRow {
            repo_hash: repo_hash.to_string(),
            branch_slug,
            worktree_path: path.to_string_lossy().into_owned(),
            size_bytes,
            measured_at,
        })
        .collect();
    if rows.is_empty() {
        return;
    }
    let _ = persist_inner(repo_hash, &rows);
}

fn persist_inner(repo_hash: &str, rows: &[WorktreeSizeRow]) -> anyhow::Result<()> {
    // `for_repo` creates `<state>/jobs/<repo_hash>/` if missing; `Pool::open`
    // creates + migrates `coordinator.db`.
    let db_path = paths::for_repo(repo_hash)?;
    let pool = Pool::open(&db_path)?;
    let mut conn = pool.writer()?;
    with_write_txn(&mut conn, |tx| {
        for row in rows {
            WorktreeSizesRepo::upsert(tx, row)?;
        }
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// End-to-end round trip against an isolated state dir: persist two
    /// worktree sizes, read them back keyed by branch. `#[serial]` +
    /// `IsolatedStateDir` keep the write off the developer's real state dir
    /// (the real-state-guard tripwire).
    #[test]
    #[serial]
    fn persist_then_read_round_trips() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "repo-hash-1";
        let dir = tempfile::tempdir().unwrap();
        let wt_a = dir.path().join("a");
        let wt_b = dir.path().join("b");
        std::fs::create_dir_all(&wt_a).unwrap();
        std::fs::create_dir_all(&wt_b).unwrap();

        persist_worktree_sizes(
            repo,
            [
                ("feat/a".to_string(), wt_a.clone(), 4096),
                ("feat/b".to_string(), wt_b.clone(), 8192),
            ],
        );

        let cached = read_worktree_sizes(repo);
        assert_eq!(cached.get("feat/a"), Some(&4096));
        assert_eq!(cached.get("feat/b"), Some(&8192));
    }

    #[test]
    #[serial]
    fn read_missing_store_is_empty() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        // No persist has run → no coordinator.db → empty, no error, no
        // materialized store.
        assert!(read_worktree_sizes("never-seen").is_empty());
    }

    #[test]
    #[serial]
    fn stat_guard_skips_vanished_path() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "repo-hash-2";
        let dir = tempfile::tempdir().unwrap();
        let present = dir.path().join("present");
        std::fs::create_dir_all(&present).unwrap();
        let vanished = dir.path().join("gone"); // never created

        persist_worktree_sizes(
            repo,
            [
                ("feat/here".to_string(), present, 4096),
                ("feat/gone".to_string(), vanished, 0),
            ],
        );

        let cached = read_worktree_sizes(repo);
        assert_eq!(cached.get("feat/here"), Some(&4096));
        assert!(
            !cached.contains_key("feat/gone"),
            "a vanished path must not be persisted (would clobber with 0)"
        );
    }

    #[test]
    #[serial]
    fn upsert_refreshes_existing_value() {
        let _guard = crate::store::paths::IsolatedStateDir::new();
        let repo = "repo-hash-3";
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();

        persist_worktree_sizes(repo, [("feat/x".to_string(), wt.clone(), 100)]);
        persist_worktree_sizes(repo, [("feat/x".to_string(), wt.clone(), 999)]);

        assert_eq!(read_worktree_sizes(repo).get("feat/x"), Some(&999));
    }
}
