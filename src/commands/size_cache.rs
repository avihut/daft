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

/// Last-known worktree sizes for `repo_hash`, keyed by branch slug. Each value
/// is `(stored_worktree_path, size_bytes)`: the path lets the seed reject a
/// size recorded for a *different* worktree that has since reused the slug (a
/// pruned-then-recreated branch), so a stale figure never surfaces. Empty on
/// any error and — deliberately — when the coordinator store doesn't exist yet
/// (first run), which it does NOT create: seeding is a pure read.
pub fn read_worktree_sizes(repo_hash: &str) -> HashMap<String, (PathBuf, u64)> {
    read_inner(repo_hash).unwrap_or_default()
}

fn read_inner(repo_hash: &str) -> Option<HashMap<String, (PathBuf, u64)>> {
    let state_dir = crate::daft_state_dir().ok()?;
    let db_path = state_dir
        .join(paths::JOBS_SUBDIR)
        .join(repo_hash)
        .join(paths::COORDINATOR_DB);
    // Pure read. Don't open (and thereby create) the store just to find it
    // empty — first run has no DB, a cache miss, not an error. The query runs
    // on the reader pool (300ms busy_timeout), so it fails fast instead of
    // blocking the shell when a coordinator holds the write lock. We
    // deliberately do NOT use a bare read-only connection here: a checkpointed
    // coordinator.db with no -wal/-shm sidecar (the common idle state) is
    // SQLITE_CANTOPEN under SQLITE_OPEN_READ_ONLY, so the pool's read-write
    // bootstrap — which recreates the sidecars without materializing a *new*
    // db (guarded just above) — is the WAL-safe way to read it. (review 6)
    if !db_path.exists() {
        return None;
    }
    let pool = Pool::open(&db_path).ok()?;
    let conn = pool.reader().ok()?;
    let rows = WorktreeSizesRepo::list_for_repo(&conn, repo_hash).ok()?;
    Some(
        rows.into_iter()
            .map(|r| {
                (
                    r.branch_slug,
                    (PathBuf::from(r.worktree_path), r.size_bytes),
                )
            })
            .collect(),
    )
}

/// Seed each worktree's Size cell from `cached` (the caller renders it stale),
/// but only when the cached row's stored path still matches the worktree's
/// current path. Skips sandboxes (never cached) and any slug now checked out at
/// a different path — so a reused branch name can't surface the previous
/// worktree's size. Split out from the command glue so the path-guard is
/// unit-testable.
pub(crate) fn seed_worktree_sizes(
    infos: &mut [crate::core::worktree::list::WorktreeInfo],
    cached: &HashMap<String, (PathBuf, u64)>,
) {
    for info in infos.iter_mut() {
        let Some(path) = info.path.as_deref() else {
            continue; // local-branch stubs etc. have no worktree to size
        };
        if info.is_sandbox {
            continue; // detached sandboxes collide on one slug — never cached
        }
        if let Some((cached_path, bytes)) = cached.get(&info.name)
            && path == cached_path.as_path()
        {
            info.size_bytes = Some(*bytes);
        }
    }
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
    // `daft list` is otherwise read-only. Don't let this display-cache write
    // block the interactive prompt for the full 5s writer timeout when a
    // coordinator/sync process holds the coordinator.db write lock: a short
    // busy_timeout means we fail fast (SQLITE_BUSY, swallowed by the caller)
    // and simply skip persisting this run — the walk already ran, and the next
    // run refreshes the cache. (review 5)
    conn.busy_timeout(std::time::Duration::from_millis(
        crate::store::connection::READER_BUSY_TIMEOUT_MS as u64,
    ))?;
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
        assert_eq!(cached.get("feat/a").map(|(_, b)| *b), Some(4096));
        assert_eq!(cached.get("feat/b").map(|(_, b)| *b), Some(8192));
        // The stored worktree path round-trips too (the seed's path-guard key).
        assert_eq!(cached.get("feat/a").map(|(p, _)| p.clone()), Some(wt_a));
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
        assert_eq!(cached.get("feat/here").map(|(_, b)| *b), Some(4096));
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

        assert_eq!(
            read_worktree_sizes(repo).get("feat/x").map(|(_, b)| *b),
            Some(999)
        );
    }

    /// The seed's path-guard: a cached size is applied only when its stored
    /// path matches the worktree's current path, and sandboxes are never
    /// seeded. Pure (no store), so no `IsolatedStateDir`/`serial` needed.
    #[test]
    fn seed_path_guards_reused_slug_and_skips_sandbox() {
        use crate::core::worktree::list::WorktreeInfo;

        let mut infos = vec![
            {
                let mut i = WorktreeInfo::empty("feat/reused");
                i.path = Some(PathBuf::from("/repo/new-location"));
                i
            },
            {
                let mut i = WorktreeInfo::empty("feat/stable");
                i.path = Some(PathBuf::from("/repo/stable"));
                i
            },
            {
                let mut i = WorktreeInfo::empty("(detached)");
                i.path = Some(PathBuf::from("/repo/sandbox"));
                i.is_sandbox = true;
                i
            },
        ];

        let mut cached = HashMap::new();
        // Same slug, but recorded at a DIFFERENT (old) path — must not seed.
        cached.insert("feat/reused".to_string(), (PathBuf::from("/repo/old"), 500));
        // Path matches the current worktree — seeds.
        cached.insert(
            "feat/stable".to_string(),
            (PathBuf::from("/repo/stable"), 42),
        );
        // A stale detached row — a sandbox must never be seeded.
        cached.insert(
            "(detached)".to_string(),
            (PathBuf::from("/repo/sandbox"), 999),
        );

        seed_worktree_sizes(&mut infos, &cached);

        assert_eq!(
            infos[0].size_bytes, None,
            "a slug reused at a new path must not surface the old size"
        );
        assert_eq!(
            infos[1].size_bytes,
            Some(42),
            "a matching stored path seeds the cached size"
        );
        assert_eq!(infos[2].size_bytes, None, "sandboxes are never seeded");
    }
}
