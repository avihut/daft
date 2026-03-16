//! Temporary worktree management for operations on local-only branches.
//!
//! Creates short-lived worktrees in `.daft-tmp/` for rebase operations on
//! branches that don't have a persistent worktree. Includes aggressive
//! cleanup: Drop guard, stale sweep on startup, and signal handling.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Sanitize a branch name for use as a directory name.
/// Replaces `/` with `--` to produce flat directory names.
fn sanitize_branch_name(branch: &str) -> String {
    branch.replace('/', "--")
}

/// Get the `.daft-tmp` directory path under the bare repo root.
pub fn tmp_dir(bare_root: &Path) -> PathBuf {
    bare_root.join(".daft-tmp")
}

/// Path for a specific branch's temp worktree.
pub fn worktree_path(bare_root: &Path, branch: &str) -> PathBuf {
    tmp_dir(bare_root).join(sanitize_branch_name(branch))
}

/// Create a temporary worktree for the given branch.
pub fn create(bare_root: &Path, branch: &str) -> Result<PathBuf> {
    let path = worktree_path(bare_root, branch);
    if path.exists() {
        // Stale from a previous crash — clean it up first.
        remove(&path)?;
    }
    std::fs::create_dir_all(path.parent().unwrap())
        .context("Failed to create .daft-tmp directory")?;

    let output = Command::new("git")
        .args(["worktree", "add", path.to_str().unwrap(), branch])
        .current_dir(bare_root)
        .output()
        .context("Failed to create temp worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {stderr}");
    }

    Ok(path)
}

/// Remove a temporary worktree using `git worktree remove`.
pub fn remove(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(["worktree", "remove", "--force", path.to_str().unwrap_or("")])
        .output()
        .context("Failed to remove temp worktree")?;

    if !output.status.success() {
        // Fallback: try rm -rf + git worktree prune
        let _ = std::fs::remove_dir_all(path);
        let _ = Command::new("git").args(["worktree", "prune"]).output();
    }

    Ok(())
}

/// Clean up all stale temp worktrees in `.daft-tmp/`.
/// Called at the start of sync/prune to handle leftovers from crashes.
pub fn cleanup_stale(bare_root: &Path) -> Result<()> {
    let tmp = tmp_dir(bare_root);
    if !tmp.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(&tmp).context("Failed to read .daft-tmp directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let _ = remove(&path);
        }
    }

    // Remove the .daft-tmp directory itself if empty.
    let _ = std::fs::remove_dir(&tmp);

    Ok(())
}

/// RAII guard that removes a temp worktree on drop.
pub struct TempWorktreeGuard {
    path: Option<PathBuf>,
}

impl TempWorktreeGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Return the worktree path.
    pub fn path(&self) -> &Path {
        self.path.as_ref().expect("guard already consumed")
    }

    /// Consume the guard without removing the worktree.
    pub fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for TempWorktreeGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(sanitize_branch_name("feat/login"), "feat--login");
        assert_eq!(
            sanitize_branch_name("feat/nested/deep"),
            "feat--nested--deep"
        );
        assert_eq!(sanitize_branch_name("simple"), "simple");
    }

    #[test]
    fn tmp_dir_path() {
        let root = Path::new("/repo");
        assert_eq!(tmp_dir(root), PathBuf::from("/repo/.daft-tmp"));
    }

    #[test]
    fn worktree_path_sanitizes() {
        let root = Path::new("/repo");
        assert_eq!(
            worktree_path(root, "feat/login"),
            PathBuf::from("/repo/.daft-tmp/feat--login")
        );
    }
}
