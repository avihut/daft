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
