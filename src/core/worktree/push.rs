//! Core logic for pushing worktree branches to their remotes.
//!
//! Used by `daft sync --push` to push all branches to their remote
//! tracking branches after updating/rebasing.

use crate::core::worktree::fetch;
use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::utils::*;
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;

/// Input parameters for the push operation.
pub struct PushParams {
    /// Use --force-with-lease when pushing.
    pub force_with_lease: bool,
    /// Name of the remote (e.g. "origin").
    pub remote_name: String,
}

/// Result of pushing a single worktree branch.
#[derive(Debug, Default)]
pub struct WorktreePushResult {
    pub worktree_name: String,
    pub branch_name: String,
    pub success: bool,
    /// "Everything up-to-date" — nothing to push.
    pub up_to_date: bool,
    /// Branch has no remote tracking branch.
    pub no_upstream: bool,
    pub message: String,
}

/// Aggregated result of pushing all worktrees.
pub struct PushResult {
    pub results: Vec<WorktreePushResult>,
    pub remote_name: String,
}

impl PushResult {
    pub fn pushed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.success && !r.up_to_date && !r.no_upstream)
            .count()
    }

    pub fn up_to_date_count(&self) -> usize {
        self.results.iter().filter(|r| r.up_to_date).count()
    }

    pub fn no_upstream_count(&self) -> usize {
        self.results.iter().filter(|r| r.no_upstream).count()
    }

    pub fn failed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| !r.success && !r.no_upstream)
            .count()
    }
}

/// Execute the push operation across all worktrees (sequential path).
pub fn execute(
    params: &PushParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
    exclude_branches: &HashSet<String>,
) -> Result<PushResult> {
    let original_dir = get_current_directory()?;
    let worktrees = fetch::get_all_worktrees_with_branches(git)?;

    let mut results: Vec<WorktreePushResult> = Vec::new();

    for (path, branch) in &worktrees {
        if exclude_branches.contains(branch) {
            continue;
        }

        let worktree_name = path
            .strip_prefix(project_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("unknown")
            .to_string();

        progress.on_step(&format!("Pushing '{worktree_name}'"));

        let result = push_single_worktree(git, path, &worktree_name, branch, params, progress);
        results.push(result);
    }

    change_directory(&original_dir)?;

    Ok(PushResult {
        results,
        remote_name: params.remote_name.clone(),
    })
}

/// Push a single worktree branch to its remote tracking branch.
///
/// Checks for an upstream tracking remote first; skips if none is set.
/// Uses an explicit working directory for thread-safe parallel execution.
pub fn push_single_worktree(
    git: &GitCommand,
    worktree_path: &Path,
    worktree_name: &str,
    branch_name: &str,
    params: &PushParams,
    progress: &mut dyn ProgressSink,
) -> WorktreePushResult {
    // Verify directory exists
    if !worktree_path.is_dir() {
        return WorktreePushResult {
            worktree_name: worktree_name.to_string(),
            branch_name: branch_name.to_string(),
            message: format!("Directory not found: {}", worktree_path.display()),
            ..Default::default()
        };
    }

    // Check if branch has upstream tracking
    match git.get_branch_tracking_remote_from(branch_name, worktree_path) {
        Ok(None) => {
            progress.on_warning(&format!(
                "Skipping '{worktree_name}': no upstream tracking branch"
            ));
            return WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                success: true,
                no_upstream: true,
                message: "No upstream tracking branch".to_string(),
                ..Default::default()
            };
        }
        Err(e) => {
            return WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                message: format!("Failed to check tracking remote: {e}"),
                ..Default::default()
            };
        }
        Ok(Some(_)) => {}
    }

    // Run git push with explicit working directory (thread-safe)
    match git.push_from(
        &params.remote_name,
        branch_name,
        worktree_path,
        params.force_with_lease,
    ) {
        Ok(output) => {
            let up_to_date = output.contains("Everything up-to-date")
                || output.contains("everything up-to-date");
            WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                success: true,
                up_to_date,
                message: if up_to_date {
                    "Already up to date".to_string()
                } else {
                    "Pushed successfully".to_string()
                },
                ..Default::default()
            }
        }
        Err(e) => {
            let msg = format!("{e}");
            progress.on_warning(&format!("Failed to push '{worktree_name}': {msg}"));
            WorktreePushResult {
                worktree_name: worktree_name.to_string(),
                branch_name: branch_name.to_string(),
                message: msg,
                ..Default::default()
            }
        }
    }
}
