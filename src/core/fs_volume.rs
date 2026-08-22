//! Filesystem-boundary facts for moves that must not discover `EXDEV` halfway.
//!
//! `fs::rename` cannot cross a filesystem, and git's `worktree move` is a
//! rename. Anything that relocates a directory decides *at planning time*
//! whether the move is a rename or has to be a copy, so `--dry-run` can say so
//! and a prompt can ask before bytes start flowing.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// `true` when both paths sit on the same device.
#[cfg(unix)]
pub fn same_filesystem(a: &Path, b: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let a_dev = std::fs::metadata(a)
        .with_context(|| String::from("could not stat ") + &a.display().to_string())?
        .dev();
    let b_dev = std::fs::metadata(b)
        .with_context(|| String::from("could not stat ") + &b.display().to_string())?
        .dev();
    Ok(a_dev == b_dev)
}

/// No `st_dev` off unix; the rename's own `EXDEV` is the backstop there.
#[cfg(not(unix))]
pub fn same_filesystem(_a: &Path, _b: &Path) -> Result<bool> {
    Ok(true)
}

/// The nearest ancestor of `path` that exists — the filesystem a
/// yet-to-be-created directory will land on.
pub fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    path.ancestors().find(|p| p.is_dir()).map(Path::to_path_buf)
}

/// How a directory gets from one path to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveStrategy {
    /// A rename — atomic, same volume.
    Rename,
    /// The destination is on another volume: copy (APFS `clonefile` where
    /// available) → fix up git's records → verify → remove the source. The
    /// source is untouched until the copy has been verified.
    CopyThenRemove,
}

/// Decide how `from` reaches `to`. Answers `Rename` whenever the question
/// cannot be answered (a stat failure) — the rename's own error is then the
/// backstop, exactly as before this existed.
pub fn strategy_for(from: &Path, to: &Path) -> MoveStrategy {
    let Some(anchor) = nearest_existing_ancestor(to) else {
        return MoveStrategy::Rename;
    };
    match same_filesystem(from, &anchor) {
        Ok(true) | Err(_) => MoveStrategy::Rename,
        Ok(false) => MoveStrategy::CopyThenRemove,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_volume_is_a_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("a");
        std::fs::create_dir_all(&from).unwrap();
        // A destination whose parents do not exist yet anchors on the tempdir.
        let to = tmp.path().join("deep/nested/b");
        assert_eq!(strategy_for(&from, &to), MoveStrategy::Rename);
        assert!(same_filesystem(&from, tmp.path()).unwrap());
    }

    #[test]
    fn nearest_existing_ancestor_walks_up() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("x/y/z");
        assert_eq!(
            nearest_existing_ancestor(&deep).unwrap(),
            tmp.path().to_path_buf()
        );
    }
}
