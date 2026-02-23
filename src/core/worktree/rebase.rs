//! Core logic for rebasing worktree branches onto a base branch.
//!
//! Used by `daft sync --rebase <BRANCH>` to rebase all worktree branches
//! onto a common base after updating from remote.

use crate::core::worktree::fetch;
use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::utils::*;
use anyhow::Result;
use std::path::Path;

/// Input parameters for the rebase operation.
pub struct RebaseParams {
    /// The branch to rebase onto.
    pub base_branch: String,
    /// Rebase even if worktree has uncommitted changes.
    pub force: bool,
    /// Suppress verbose output.
    pub quiet: bool,
}

/// Result of rebasing a single worktree.
#[derive(Debug, Default)]
pub struct WorktreeRebaseResult {
    pub worktree_name: String,
    pub success: bool,
    pub skipped: bool,
    pub conflict: bool,
    pub already_rebased: bool,
    pub message: String,
}

/// Aggregated result of rebasing all worktrees.
pub struct RebaseResult {
    pub results: Vec<WorktreeRebaseResult>,
    pub base_branch: String,
}

impl RebaseResult {
    pub fn rebased_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.success && !r.skipped && !r.already_rebased)
            .count()
    }

    pub fn already_rebased_count(&self) -> usize {
        self.results.iter().filter(|r| r.already_rebased).count()
    }

    pub fn conflict_count(&self) -> usize {
        self.results.iter().filter(|r| r.conflict).count()
    }

    pub fn skipped_count(&self) -> usize {
        self.results.iter().filter(|r| r.skipped).count()
    }
}

/// Execute the rebase operation across all worktrees.
pub fn execute(
    params: &RebaseParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<RebaseResult> {
    let original_dir = get_current_directory()?;
    let worktrees = fetch::get_all_worktrees_with_branches(git)?;

    let mut results: Vec<WorktreeRebaseResult> = Vec::new();

    for (path, branch) in &worktrees {
        // Skip the base branch itself
        if branch == &params.base_branch {
            continue;
        }

        let worktree_name = path
            .strip_prefix(project_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("unknown")
            .to_string();

        progress.on_step(&format!(
            "Rebasing '{worktree_name}' onto {}",
            params.base_branch
        ));

        // Change to worktree directory
        if let Err(e) = change_directory(path) {
            results.push(WorktreeRebaseResult {
                worktree_name,
                message: format!("Failed to change to directory: {e}"),
                ..Default::default()
            });
            continue;
        }

        // Check for uncommitted changes
        match git.has_uncommitted_changes_in(path) {
            Ok(true) if !params.force => {
                progress.on_warning(&format!(
                    "Skipping '{worktree_name}': has uncommitted changes (use --force to rebase anyway)"
                ));
                results.push(WorktreeRebaseResult {
                    worktree_name,
                    success: true,
                    skipped: true,
                    message: "Skipped: uncommitted changes".to_string(),
                    ..Default::default()
                });
                continue;
            }
            Err(e) => {
                results.push(WorktreeRebaseResult {
                    worktree_name,
                    message: format!("Failed to check status: {e}"),
                    ..Default::default()
                });
                continue;
            }
            _ => {}
        }

        // Run git rebase
        match git.rebase(&params.base_branch) {
            Ok(output) => {
                let already_up_to_date =
                    output.contains("is up to date") || output.contains("up to date");
                results.push(WorktreeRebaseResult {
                    worktree_name,
                    success: true,
                    already_rebased: already_up_to_date,
                    message: if already_up_to_date {
                        "Already up to date".to_string()
                    } else {
                        "Rebased successfully".to_string()
                    },
                    ..Default::default()
                });
            }
            Err(_) => {
                // Abort the failed rebase to leave the worktree clean
                if let Err(abort_err) = git.rebase_abort() {
                    progress.on_warning(&format!(
                        "Failed to abort rebase in '{worktree_name}': {abort_err}"
                    ));
                }
                results.push(WorktreeRebaseResult {
                    worktree_name,
                    conflict: true,
                    message: "Rebase conflict — aborted".to_string(),
                    ..Default::default()
                });
            }
        }
    }

    // Return to original directory
    change_directory(&original_dir)?;

    Ok(RebaseResult {
        results,
        base_branch: params.base_branch.clone(),
    })
}
