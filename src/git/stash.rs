use super::oxide;
use super::GitCommand;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

impl GitCommand {
    /// Check if a specific worktree path has uncommitted or untracked changes.
    pub fn has_uncommitted_changes_in(&self, worktree_path: &Path) -> Result<bool> {
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(worktree_path)
            .output()
            .context("Failed to execute git status command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git status failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git status output")?;
        Ok(!stdout.trim().is_empty())
    }

    /// Check if working directory has uncommitted or untracked changes
    pub fn has_uncommitted_changes(&self) -> Result<bool> {
        if self.use_gitoxide {
            let repo = self.gix_repo()?;
            // Fall back to subprocess if the cached repo is bare (no workdir).
            // This happens when the repo was discovered from the project root in
            // a bare-repo worktree layout (e.g., flow-eject changes CWD to the
            // project root, then later CDs into individual worktrees).
            if repo.workdir().is_some() {
                return oxide::has_uncommitted_changes(&repo);
            }
        }
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .context("Failed to execute git status command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git status failed: {}", stderr);
        }

        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse git status output")?;
        Ok(!stdout.trim().is_empty())
    }

    /// Stash all changes including untracked files
    pub fn stash_push_with_untracked(&self, message: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "push", "-u", "-m", message]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash push command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash push failed: {}", stderr);
        }

        Ok(())
    }

    /// Pop the most recent stash
    pub fn stash_pop(&self) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "pop"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash pop command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash pop failed: {}", stderr);
        }

        Ok(())
    }

    /// Apply the top stash without removing it
    pub fn stash_apply(&self) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "apply"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash apply command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash apply failed: {}", stderr);
        }

        Ok(())
    }

    /// Drop the top stash entry
    pub fn stash_drop(&self) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["stash", "drop"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        let output = cmd
            .output()
            .context("Failed to execute git stash drop command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git stash drop failed: {}", stderr);
        }

        Ok(())
    }
}
