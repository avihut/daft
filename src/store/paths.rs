//! Canonical path resolution for store files.
//!
//! Every store path is derived from `daft_state_dir()` so callers cannot
//! point the store at arbitrary filesystem locations through user input.
//! `for_repo_under` canonicalizes and asserts the resolved path stays inside
//! the given base — a symlink or `..` segment outside the base is rejected
//! at open time.

use crate::store::error::{Result, StoreError};
use std::path::{Path, PathBuf};

/// Subdirectory under `daft_state_dir()` that holds per-repo coordinator DBs.
pub const JOBS_SUBDIR: &str = "jobs";

/// Filename of the SQLite database inside a per-repo state dir.
pub const COORDINATOR_DB: &str = "coordinator.db";

/// Subdirectory under `daft_data_dir()` that holds the global repo catalog.
/// A dedicated parent (rather than the bare data dir) keeps `tighten_perms`
/// / `verify_perms`'s 0700-parent invariant away from the shared data dir,
/// which also hosts centralized-layout worktrees.
pub const CATALOG_SUBDIR: &str = "catalog";

/// Filename of the global repo-catalog SQLite database.
pub const CATALOG_DB: &str = "catalog.db";

/// Resolve the per-repo coordinator DB path under the daft state dir.
pub fn for_repo(repo_hash: &str) -> Result<PathBuf> {
    let state_dir = crate::daft_state_dir().map_err(|e| StoreError::Io {
        path: PathBuf::from("daft_state_dir"),
        source: std::io::Error::other(e.to_string()),
    })?;
    for_repo_under(&state_dir, repo_hash)
}

/// Resolve the per-repo coordinator DB path under an explicit base. Used by
/// callers that already know the state dir and by tests.
///
/// Creates the parent dir if missing so canonicalization succeeds. After
/// canonicalizing the parent, asserts it sits beneath `base` — rejects
/// symlink-escape attempts.
pub fn for_repo_under(base: &Path, repo_hash: &str) -> Result<PathBuf> {
    let parent = base.join(JOBS_SUBDIR).join(repo_hash);
    std::fs::create_dir_all(&parent).map_err(|source| StoreError::Io {
        path: parent.clone(),
        source,
    })?;

    let canonical_parent = parent.canonicalize().map_err(|source| StoreError::Io {
        path: parent.clone(),
        source,
    })?;
    let canonical_base = base.canonicalize().map_err(|source| StoreError::Io {
        path: base.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(&canonical_base) {
        return Err(StoreError::PathOutsideStateDir(canonical_parent));
    }

    Ok(canonical_parent.join(COORDINATOR_DB))
}

/// Per-repo parent directory (without the DB filename). Useful for callers
/// that need to clean up siblings (e.g. legacy file auto-wipe).
pub fn parent_for_repo_under(base: &Path, repo_hash: &str) -> Result<PathBuf> {
    Ok(for_repo_under(base, repo_hash)?
        .parent()
        .expect("for_repo_under always returns a path with a parent")
        .to_path_buf())
}

/// Resolve the global repo-catalog DB path under the daft data dir,
/// creating `catalog/` if missing. Use for read-write opens.
pub fn catalog_db() -> Result<PathBuf> {
    let data_dir = crate::daft_data_dir().map_err(|e| StoreError::Io {
        path: PathBuf::from("daft_data_dir"),
        source: std::io::Error::other(e.to_string()),
    })?;
    catalog_db_under(&data_dir)
}

/// Resolve the catalog DB path under an explicit base (tests). Creates the
/// parent dir so canonicalization succeeds, then asserts containment like
/// [`for_repo_under`].
pub fn catalog_db_under(base: &Path) -> Result<PathBuf> {
    let parent = base.join(CATALOG_SUBDIR);
    std::fs::create_dir_all(&parent).map_err(|source| StoreError::Io {
        path: parent.clone(),
        source,
    })?;

    let canonical_parent = parent.canonicalize().map_err(|source| StoreError::Io {
        path: parent.clone(),
        source,
    })?;
    let canonical_base = base.canonicalize().map_err(|source| StoreError::Io {
        path: base.to_path_buf(),
        source,
    })?;
    if !canonical_parent.starts_with(&canonical_base) {
        return Err(StoreError::PathOutsideStateDir(canonical_parent));
    }

    Ok(canonical_parent.join(CATALOG_DB))
}

/// Non-creating catalog DB path for read-only probes (completion hot
/// path). Returns `None` when the data dir cannot be resolved; performs no
/// filesystem writes — callers stat the result themselves.
pub fn catalog_db_probe() -> Option<PathBuf> {
    let data_dir = crate::daft_data_dir().ok()?;
    Some(data_dir.join(CATALOG_SUBDIR).join(CATALOG_DB))
}

/// Verify a file is at most user-read-write (mode 0o600) and its parent is
/// at most user-rwx (mode 0o700). Returns `PermissionsTooOpen` for either
/// failure. Called once per process per opened DB.
#[cfg(unix)]
pub fn verify_perms(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let file_meta = std::fs::metadata(path).map_err(|source| StoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file_mode = file_meta.permissions().mode() & 0o777;
    if file_mode & 0o077 != 0 {
        return Err(StoreError::PermissionsTooOpen {
            path: path.to_path_buf(),
            mode: file_mode,
        });
    }
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir_meta = std::fs::metadata(parent).map_err(|source| StoreError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let dir_mode = dir_meta.permissions().mode() & 0o777;
    if dir_mode & 0o077 != 0 {
        return Err(StoreError::PermissionsTooOpen {
            path: parent.to_path_buf(),
            mode: dir_mode,
        });
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn verify_perms(_path: &Path) -> Result<()> {
    Ok(())
}

/// Tighten `path` and its parent to `0600` / `0700` respectively. Called
/// after creating the DB file so subsequent `verify_perms` calls succeed
/// even on systems with a permissive umask.
#[cfg(unix)]
pub fn tighten_perms(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        StoreError::Io {
            path: path.to_path_buf(),
            source,
        }
    })?;
    if let Some(parent) = path.parent() {
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).map_err(
            |source| StoreError::Io {
                path: parent.to_path_buf(),
                source,
            },
        )?;
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn tighten_perms(_path: &Path) -> Result<()> {
    Ok(())
}

/// Test-only RAII guard that redirects the daft *state* dir to a fresh temp
/// dir for the duration of a test, restoring the previous `DAFT_STATE_DIR` on
/// drop. Tests that exercise hook / visitor-seed code paths open the
/// coordinator store via [`for_repo`], which resolves `daft_state_dir()`;
/// without this guard they write the developer's real `~/.local/state/daft/`
/// (#697 — the same isolation-leak class as #478/#669).
///
/// Callers MUST be `#[serial]`: the `DAFT_STATE_DIR` mutation is process-global,
/// so it is only safe when no other test runs concurrently.
#[cfg(test)]
pub(crate) struct IsolatedStateDir {
    _tmp: tempfile::TempDir,
    prev: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl IsolatedStateDir {
    pub(crate) fn new() -> Self {
        let tmp = tempfile::tempdir().expect("create temp state dir");
        let prev = std::env::var_os(crate::STATE_DIR_ENV);
        // SAFETY: `set_var` is `unsafe fn` in edition 2024 (process-global, not
        // thread-safe). Callers are `#[serial]`, serializing the mutation.
        unsafe { std::env::set_var(crate::STATE_DIR_ENV, tmp.path()) };
        Self { _tmp: tmp, prev }
    }
}

#[cfg(test)]
impl Drop for IsolatedStateDir {
    fn drop(&mut self) {
        // SAFETY: as in `new` — serialized by `#[serial]`.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(crate::STATE_DIR_ENV, v),
                None => std::env::remove_var(crate::STATE_DIR_ENV),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_repo_under_returns_path_under_base() {
        let dir = tempfile::tempdir().unwrap();
        let p = for_repo_under(dir.path(), "repohash").unwrap();
        // macOS resolves tempdir symlinks (`/var → /private/var`); compare
        // against the canonical base so the assertion is platform-agnostic.
        let canonical_base = dir.path().canonicalize().unwrap();
        assert!(
            p.starts_with(&canonical_base),
            "{p:?} not under {canonical_base:?}"
        );
        assert!(p.ends_with(format!("{JOBS_SUBDIR}/repohash/{COORDINATOR_DB}")));
    }

    #[test]
    #[cfg(unix)]
    fn for_repo_under_rejects_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let jobs_dir = base.path().join(JOBS_SUBDIR);
        std::fs::create_dir_all(&jobs_dir).unwrap();
        // Plant a symlink that points outside `base`.
        let evil_link = jobs_dir.join("evil");
        std::os::unix::fs::symlink(outside.path(), &evil_link).unwrap();

        let err = for_repo_under(base.path(), "evil").unwrap_err();
        assert!(matches!(err, StoreError::PathOutsideStateDir(_)));
    }

    #[test]
    fn catalog_db_under_returns_path_in_catalog_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let p = catalog_db_under(dir.path()).unwrap();
        let canonical_base = dir.path().canonicalize().unwrap();
        assert!(
            p.starts_with(&canonical_base),
            "{p:?} not under {canonical_base:?}"
        );
        assert!(p.ends_with(format!("{CATALOG_SUBDIR}/{CATALOG_DB}")));
        assert!(p.parent().unwrap().is_dir(), "catalog/ dir was created");
    }

    #[test]
    #[cfg(unix)]
    fn catalog_db_under_rejects_symlink_escape() {
        let outside = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        let evil_link = base.path().join(CATALOG_SUBDIR);
        std::os::unix::fs::symlink(outside.path(), &evil_link).unwrap();

        let err = catalog_db_under(base.path()).unwrap_err();
        assert!(matches!(err, StoreError::PathOutsideStateDir(_)));
    }

    /// #697 regression: both the read-write catalog path and the read-only
    /// completion probe must resolve under the sandboxed `DAFT_DATA_DIR`, never
    /// the real data dir. This locks the `catalog_db()` / `catalog_db_probe()`
    /// → `daft_data_dir()` composition to the override so a future refactor
    /// can't silently retarget the developer's real `catalog.db` (the leak this
    /// ticket cleaned up). `#[serial]` serializes it against the other
    /// `DAFT_*_DIR` override tests, which mutate the same process-global env.
    #[test]
    #[serial_test::serial]
    fn catalog_paths_resolve_under_data_dir_override() {
        let sandbox = tempfile::tempdir().unwrap();
        // SAFETY: `set_var`/`remove_var` are `unsafe fn` in edition 2024
        // (process-global, not thread-safe). `#[serial]` serializes this test
        // against every other env-mutating test; CLAUDE.md permits unsafe in
        // tests. Restore the env *before* asserting so a panic can't leak it.
        unsafe {
            std::env::set_var(crate::DATA_DIR_ENV, sandbox.path());
        }
        let db = catalog_db();
        let probe = catalog_db_probe();
        unsafe {
            std::env::remove_var(crate::DATA_DIR_ENV);
        }

        // Read-write path: canonicalized, so compare against the canonical
        // sandbox (macOS resolves `/var` → `/private/var`).
        let db = db.expect("catalog_db() under DAFT_DATA_DIR override");
        let canonical_sandbox = sandbox.path().canonicalize().unwrap();
        assert!(
            db.starts_with(&canonical_sandbox),
            "catalog_db() {db:?} escaped the DAFT_DATA_DIR sandbox {canonical_sandbox:?}"
        );
        assert!(db.ends_with(format!("{CATALOG_SUBDIR}/{CATALOG_DB}")));

        // Read-only probe: non-creating and non-canonicalizing, so it starts
        // with the raw override path.
        let probe = probe.expect("catalog_db_probe() under DAFT_DATA_DIR override");
        assert!(
            probe.starts_with(sandbox.path()),
            "catalog_db_probe() {probe:?} escaped the DAFT_DATA_DIR sandbox {:?}",
            sandbox.path()
        );
    }

    #[test]
    #[cfg(unix)]
    fn verify_perms_accepts_owner_only_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        verify_perms(&path).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn verify_perms_rejects_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        std::fs::write(&path, b"").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let err = verify_perms(&path).unwrap_err();
        assert!(matches!(err, StoreError::PermissionsTooOpen { .. }));
    }
}
