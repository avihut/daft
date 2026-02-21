use super::GitCommand;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

impl GitCommand {
    pub fn worktree_add(&self, path: &Path, branch: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(path).arg(branch);

        let output = cmd
            .output()
            .context("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree add failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_add_orphan(&self, path: &Path, branch_name: &str) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add", "--orphan"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        // --orphan creates a worktree with an unborn branch, which is needed
        // for empty repos where the bare clone's HEAD already references the
        // default branch name (causing -b to fail with "already exists").
        cmd.arg("-b").arg(branch_name).arg(path);

        let output = cmd
            .output()
            .context("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree add failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_add_new_branch(
        &self,
        path: &Path,
        new_branch: &str,
        base_branch: &str,
    ) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        cmd.arg(path).arg("-b").arg(new_branch).arg(base_branch);

        let output = cmd
            .output()
            .context("Failed to execute git worktree add command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree add failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_remove(&self, path: &Path, force: bool) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "remove"]);

        if force {
            cmd.arg("--force");
        }

        cmd.arg(path);

        let output = cmd
            .output()
            .context("Failed to execute git worktree remove command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree remove failed: {}", stderr);
        }

        Ok(())
    }

    pub fn worktree_list_porcelain(&self) -> Result<String> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .output()
            .context("Failed to execute git worktree list command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree list failed: {}", stderr);
        }

        String::from_utf8(output.stdout).context("Failed to parse git worktree list output")
    }

    /// Find the worktree path for a given branch name.
    /// Returns None if no worktree is checked out on that branch.
    pub fn find_worktree_for_branch(
        &self,
        branch_name: &str,
    ) -> Result<Option<std::path::PathBuf>> {
        let porcelain_output = self.worktree_list_porcelain()?;

        let mut current_path: Option<std::path::PathBuf> = None;

        for line in porcelain_output.lines() {
            if let Some(worktree_path) = line.strip_prefix("worktree ") {
                current_path = Some(std::path::PathBuf::from(worktree_path));
            } else if let Some(branch_ref) = line.strip_prefix("branch ") {
                if let Some(branch) = branch_ref.strip_prefix("refs/heads/") {
                    if branch == branch_name {
                        return Ok(current_path.take());
                    }
                }
                current_path = None;
            } else if line.is_empty() {
                current_path = None;
            }
        }

        Ok(None)
    }

    /// Resolve a target (worktree name or branch name) to a worktree path.
    /// Priority: worktree name (directory name) > branch name
    pub fn resolve_worktree_path(
        &self,
        target: &str,
        project_root: &Path,
    ) -> Result<std::path::PathBuf> {
        let porcelain_output = self.worktree_list_porcelain()?;

        // Parse worktree list porcelain output
        // Format:
        // worktree /path/to/worktree
        // HEAD <sha>
        // branch refs/heads/branch-name
        // <blank line>
        let mut worktrees: Vec<(std::path::PathBuf, Option<String>)> = Vec::new();
        let mut current_path: Option<std::path::PathBuf> = None;
        let mut current_branch: Option<String> = None;

        for line in porcelain_output.lines() {
            if let Some(worktree_path) = line.strip_prefix("worktree ") {
                // Save previous worktree if any
                if let Some(path) = current_path.take() {
                    worktrees.push((path, current_branch.take()));
                }
                current_path = Some(std::path::PathBuf::from(worktree_path));
                current_branch = None;
            } else if let Some(branch_ref) = line.strip_prefix("branch ") {
                // Extract branch name from refs/heads/branch-name
                current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
            }
        }
        // Don't forget the last worktree
        if let Some(path) = current_path.take() {
            worktrees.push((path, current_branch.take()));
        }

        // First, check if target is a path relative to project root (most precise)
        let potential_path = project_root.join(target);
        for (path, _) in &worktrees {
            if path == &potential_path {
                return Ok(path.clone());
            }
        }

        // Second, check if target matches a branch name
        for (path, branch) in &worktrees {
            if let Some(branch_name) = branch {
                if branch_name == target {
                    return Ok(path.clone());
                }
            }
        }

        // Third, check if target matches a worktree directory name (convenience shorthand)
        for (path, _) in &worktrees {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == target {
                    return Ok(path.clone());
                }
            }
        }

        anyhow::bail!("No worktree found for '{}'", target)
    }

    /// Move a worktree to a new location.
    pub fn worktree_move(&self, from: &Path, to: &Path) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "move"]);
        cmd.arg(from).arg(to);

        let output = cmd
            .output()
            .context("Failed to execute git worktree move command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Git worktree move failed: {}", stderr);
        }

        Ok(())
    }
}
