use super::GitCommand;
use crate::utils::git_command_at;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One `git worktree list --porcelain` stanza, in the shape this layer's
/// branch→worktree lookups need.
///
/// git-layer: parsed here rather than via `core::worktree::porcelain` — the
/// `git` adapter sits below `core` and must not depend on it (core → git,
/// never the reverse). Deliberately minimal; `core`'s parser is the richer
/// one every consumer above this layer uses.
struct PorcelainEntry {
    path: PathBuf,
    /// Short branch name, from a `branch refs/heads/<name>` line.
    branch: Option<String>,
    /// The worktree has a detached HEAD (`detached` line). Bare entries are
    /// neither detached nor branched.
    detached: bool,
}

fn parse_porcelain_entries(porcelain: &str) -> Vec<PorcelainEntry> {
    let mut entries: Vec<PorcelainEntry> = Vec::new();

    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            entries.push(PorcelainEntry {
                path: PathBuf::from(path),
                branch: None,
                detached: false,
            });
        } else if let Some(branch_ref) = line.strip_prefix("branch refs/heads/") {
            if let Some(entry) = entries.last_mut() {
                entry.branch = Some(branch_ref.to_string());
            }
        } else if line == "detached"
            && let Some(entry) = entries.last_mut()
        {
            entry.detached = true;
        }
    }

    entries
}

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
        no_track: bool,
    ) -> Result<()> {
        let mut cmd = Command::new("git");
        cmd.args(["worktree", "add"]);

        if self.quiet {
            cmd.arg("--quiet");
        }

        if no_track {
            cmd.arg("--no-track");
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

    /// Enumerate this directory's worktrees.
    ///
    /// Runs through [`git_command_at`] rather than a bare `git`: an inherited
    /// `GIT_DIR` — daft invoked from inside a git hook, or from a wrapper that
    /// exports it — otherwise wins repo discovery and enumerates *that* repo's
    /// worktrees instead of the current directory's. Every branch→worktree
    /// lookup in daft funnels through here, so a retargeted list silently
    /// reports "no worktree" and callers fall back to the invoking directory
    /// (CLAUDE.md's Test Hygiene rule; the `daft push` hook cwd depends on it).
    pub fn worktree_list_porcelain(&self) -> Result<String> {
        let cwd = std::env::current_dir().context("Could not determine the current directory")?;
        let output = git_command_at(&cwd)
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
    ///
    /// A worktree paused mid-rebase reports no branch — git detaches HEAD to
    /// replay commits — so a porcelain-only match loses it, and callers fall
    /// back to "this branch has no worktree" while its worktree sits right
    /// there. Attached checkouts are matched first and win outright; only
    /// then are detached entries asked what operation they are under
    /// ([`crate::git::op_state`]).
    pub fn find_worktree_for_branch(
        &self,
        branch_name: &str,
    ) -> Result<Option<std::path::PathBuf>> {
        let entries = parse_porcelain_entries(&self.worktree_list_porcelain()?);

        if let Some(entry) = entries
            .iter()
            .find(|e| e.branch.as_deref() == Some(branch_name))
        {
            return Ok(Some(entry.path.clone()));
        }

        Ok(entries
            .iter()
            .filter(|e| e.detached)
            .find(|e| {
                crate::git::op_state::recovered_branch(&e.path).as_deref() == Some(branch_name)
            })
            .map(|e| e.path.clone()))
    }

    /// Resolve a target (worktree name or branch name) to a worktree path.
    ///
    /// Priority: path under the project root > checked-out branch > branch
    /// recovered from an in-progress operation > worktree directory name.
    /// The recovered tier sits with the other branch matching and below every
    /// real checkout, so a paused rebase can never shadow an attached
    /// worktree — it only rescues a lookup that would otherwise fail.
    pub fn resolve_worktree_path(
        &self,
        target: &str,
        project_root: &Path,
    ) -> Result<std::path::PathBuf> {
        let worktrees = parse_porcelain_entries(&self.worktree_list_porcelain()?);

        // First, check if target is a path relative to project root (most precise)
        let potential_path = project_root.join(target);
        if let Some(entry) = worktrees.iter().find(|e| e.path == potential_path) {
            return Ok(entry.path.clone());
        }

        // Second, check if target matches a branch name
        if let Some(entry) = worktrees
            .iter()
            .find(|e| e.branch.as_deref() == Some(target))
        {
            return Ok(entry.path.clone());
        }

        // Third, the same question for worktrees git reports as detached
        // because an operation is replaying commits in them.
        if let Some(entry) = worktrees
            .iter()
            .filter(|e| e.detached)
            .find(|e| crate::git::op_state::recovered_branch(&e.path).as_deref() == Some(target))
        {
            return Ok(entry.path.clone());
        }

        // Fourth, check if target matches a worktree directory name (convenience shorthand)
        if let Some(entry) = worktrees
            .iter()
            .find(|e| e.path.file_name().and_then(|n| n.to_str()) == Some(target))
        {
            return Ok(entry.path.clone());
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
