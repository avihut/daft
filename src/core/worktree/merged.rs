//! Shared "is this branch merged?" detection.
//!
//! Used by branch-delete validation (Check 4) and by prune's
//! gone-but-unmerged guard. A remote branch disappearing does NOT imply the
//! work was merged — abandoned branches get their remotes deleted too — so
//! every removal path that infers "merged" from "gone" must verify with
//! these checks instead.
//!
//! Note: `core::worktree::merge` has its own ancestor-only
//! `is_branch_merged_into` for mid-merge bookkeeping; it intentionally does
//! NOT detect squash merges and must not be unified with this one.

use crate::git::GitCommand;
use anyhow::{Context, Result};

/// Check whether a branch has been merged into the default branch.
///
/// Checks against both the local default branch and its remote tracking
/// branch (which may be ahead of local).
pub fn is_branch_merged(
    git: &GitCommand,
    branch: &str,
    default_branch: &str,
    remote_name: &str,
) -> Result<bool> {
    // Check against local default branch first
    if is_branch_merged_into(git, branch, default_branch)? {
        return Ok(true);
    }

    // Also check against the remote tracking branch, which may be ahead of local
    let remote_ref = format!("{remote_name}/{default_branch}");
    if is_branch_merged_into(git, branch, &remote_ref)? {
        return Ok(true);
    }

    Ok(false)
}

/// Check whether `branch` has been merged into `target`.
///
/// Two-step: `merge-base --is-ancestor` detects regular merges; `git cherry`
/// detects squash merges (all lines start with `-`).
pub fn is_branch_merged_into(git: &GitCommand, branch: &str, target: &str) -> Result<bool> {
    // Step 1: Check if branch is an ancestor of the target (regular merge)
    let is_ancestor = git
        .merge_base_is_ancestor(branch, target)
        .context("merge-base check failed")?;

    if is_ancestor {
        return Ok(true);
    }

    // Step 2: Check for squash merge via git cherry.
    let cherry_output = git
        .cherry(target, branch)
        .context("git cherry check failed")?;

    let lines: Vec<&str> = cherry_output.lines().collect();

    // Empty output means no commits to compare
    if lines.is_empty() {
        return Ok(true);
    }

    // All lines must start with `-` for the branch to be considered squash-merged
    let all_merged = lines.iter().all(|line| line.starts_with('-'));
    Ok(all_merged)
}
