//! Previous worktree state for `daft go -` navigation.
//!
//! Stores the absolute path of the last worktree the user switched away from,
//! enabling `cd -`–style toggling between two worktrees. State is persisted
//! per-repository at `<git-common-dir>/.daft/previous-worktree`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const STATE_DIR: &str = ".daft";
const STATE_FILE: &str = "previous-worktree";

/// Load the previously visited worktree path, if any.
///
/// Returns `Ok(None)` when no previous worktree has been recorded (file
/// missing or empty). Returns `Err` only on unexpected I/O failures.
pub fn load(git_common_dir: &Path) -> Result<Option<PathBuf>> {
    let file = git_common_dir.join(STATE_DIR).join(STATE_FILE);

    match std::fs::read_to_string(&file) {
        Ok(content) => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(PathBuf::from(trimmed)))
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| {
            format!(
                "Failed to read previous worktree state from {}",
                file.display()
            )
        }),
    }
}

/// Save the worktree path as the "previous" for later `daft go -` use.
///
/// Creates the `.daft/` directory inside the git common dir if it does not
/// already exist.
pub fn save(git_common_dir: &Path, worktree_path: &Path) -> Result<()> {
    let dir = git_common_dir.join(STATE_DIR);
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create state directory {}", dir.display()))?;
    }

    let file = dir.join(STATE_FILE);
    std::fs::write(&file, worktree_path.to_string_lossy().as_bytes()).with_context(|| {
        format!(
            "Failed to write previous worktree state to {}",
            file.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_roundtrip() {
        let dir = tempdir().unwrap();
        let worktree = PathBuf::from("/projects/repo/feature-x");

        save(dir.path(), &worktree).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, Some(worktree));
    }

    #[test]
    fn test_no_file() {
        let dir = tempdir().unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, None);
    }

    #[test]
    fn test_empty_file() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join(STATE_DIR);
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join(STATE_FILE), "").unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, None);
    }

    #[test]
    fn test_whitespace_only_file() {
        let dir = tempdir().unwrap();
        let state_dir = dir.path().join(STATE_DIR);
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(state_dir.join(STATE_FILE), "  \n  ").unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, None);
    }

    #[test]
    fn test_overwrite() {
        let dir = tempdir().unwrap();
        let first = PathBuf::from("/projects/repo/main");
        let second = PathBuf::from("/projects/repo/develop");

        save(dir.path(), &first).unwrap();
        save(dir.path(), &second).unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, Some(second));
    }

    #[test]
    fn test_creates_daft_directory() {
        let dir = tempdir().unwrap();
        let daft_dir = dir.path().join(STATE_DIR);
        assert!(!daft_dir.exists());

        save(dir.path(), &PathBuf::from("/some/path")).unwrap();
        assert!(daft_dir.exists());
    }
}
