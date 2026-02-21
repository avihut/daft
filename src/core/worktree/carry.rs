//! Core logic for the `git-worktree-carry` command.
//!
//! Transfers uncommitted changes from the current worktree to one or more
//! target worktrees via git stash.

use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::utils::{change_directory, get_current_directory};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Input parameters for the carry operation.
pub struct CarryParams {
    /// Target worktrees by directory name or branch name.
    pub targets: Vec<String>,
    /// If true, copy changes instead of moving them.
    pub copy: bool,
}

/// A successfully resolved carry target.
pub struct CarryTarget {
    /// Display name (relative to project root).
    pub name: String,
    /// Absolute path to the worktree.
    pub path: PathBuf,
}

/// A target that failed during carry.
pub struct CarryFailure {
    /// Display name of the target.
    pub name: String,
    /// Error description.
    pub error: String,
}

/// Result of a carry operation.
pub struct CarryResult {
    /// Targets where changes were successfully applied.
    pub successes: Vec<CarryTarget>,
    /// Targets that failed.
    pub failures: Vec<CarryFailure>,
    /// Whether copy mode was used (explicit --copy or multiple targets).
    pub copy_mode: bool,
    /// Path to cd into after the operation.
    pub cd_target: PathBuf,
    /// Whether the stash was preserved (due to failures).
    pub stash_preserved: bool,
    /// True if there were no changes to carry.
    pub no_changes: bool,
    /// True if no valid targets remained after resolution.
    pub no_valid_targets: bool,
    /// Errors from target resolution (before any changes were made).
    pub resolution_errors: Vec<String>,
}

/// Execute the carry operation.
///
/// Stashes uncommitted changes from the current worktree and applies them
/// to the specified targets. Returns a structured result describing what
/// happened, without performing any output.
pub fn execute(
    params: &CarryParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<CarryResult> {
    let source_worktree = git.get_current_worktree_path()?;

    // Check for uncommitted changes
    if !git.has_uncommitted_changes()? {
        return Ok(CarryResult {
            successes: Vec::new(),
            failures: Vec::new(),
            copy_mode: false,
            cd_target: source_worktree,
            stash_preserved: false,
            no_changes: true,
            no_valid_targets: false,
            resolution_errors: Vec::new(),
        });
    }

    // Resolve all targets upfront (fail fast if any are invalid)
    let mut resolved_targets: Vec<CarryTarget> = Vec::new();
    let mut resolution_errors: Vec<String> = Vec::new();

    for target in &params.targets {
        match git.resolve_worktree_path(target, project_root) {
            Ok(path) => {
                if path == source_worktree {
                    progress
                        .on_warning(&format!("Skipping '{}': already in this worktree", target));
                    continue;
                }
                let name = path
                    .strip_prefix(project_root)
                    .ok()
                    .and_then(|p| p.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                resolved_targets.push(CarryTarget { name, path });
            }
            Err(e) => {
                resolution_errors.push(format!("'{}': {}", target, e));
            }
        }
    }

    // If there are resolution errors, bail before making changes
    if !resolution_errors.is_empty() {
        return Ok(CarryResult {
            successes: Vec::new(),
            failures: Vec::new(),
            copy_mode: false,
            cd_target: source_worktree,
            stash_preserved: false,
            no_changes: false,
            no_valid_targets: false,
            resolution_errors,
        });
    }

    // If no valid targets remain, exit
    if resolved_targets.is_empty() {
        return Ok(CarryResult {
            successes: Vec::new(),
            failures: Vec::new(),
            copy_mode: false,
            cd_target: source_worktree,
            stash_preserved: false,
            no_changes: false,
            no_valid_targets: true,
            resolution_errors: Vec::new(),
        });
    }

    // Determine copy mode: explicit --copy flag OR multiple targets
    let copy_mode = params.copy || resolved_targets.len() > 1;

    // Stash the changes
    progress.on_step("Stashing uncommitted changes...");
    git.stash_push_with_untracked("daft: carry changes")?;

    // Apply to each target
    let mut successes: Vec<CarryTarget> = Vec::new();
    let mut failures: Vec<CarryFailure> = Vec::new();

    for target in resolved_targets {
        progress.on_step(&format!("Applying changes to '{}'...", target.name));

        if let Err(e) = change_directory(&target.path) {
            failures.push(CarryFailure {
                name: target.name,
                error: format!("Failed to change directory: {}", e),
            });
            continue;
        }

        if let Err(e) = git.stash_apply() {
            failures.push(CarryFailure {
                name: target.name.clone(),
                error: format!(
                    "Failed to apply changes: {}. Resolve with: cd {} && git stash apply",
                    e,
                    target.path.display()
                ),
            });
        } else {
            progress.on_debug(&format!("Changes applied to '{}'", target.name));
            successes.push(target);
        }
    }

    // Handle stash cleanup based on mode
    let stash_preserved;
    if copy_mode {
        progress.on_step("Restoring changes in source worktree...");
        change_directory(&source_worktree)?;
        if let Err(e) = git.stash_pop() {
            progress.on_warning(&format!(
                "Failed to restore stashed changes: {}. Run 'git stash pop' to restore.",
                e
            ));
            stash_preserved = true;
        } else {
            stash_preserved = false;
        }
    } else {
        // Move mode: drop the stash since we moved the changes
        if let Err(e) = git.stash_drop() {
            progress.on_warning(&format!("Failed to drop stash: {}", e));
            stash_preserved = true;
        } else {
            stash_preserved = false;
        }
    }

    // Change to the last successful target, or stay in source
    let last_target_path = successes
        .last()
        .map(|t| t.path.clone())
        .unwrap_or_else(|| source_worktree.clone());

    change_directory(&last_target_path)?;
    let cd_target = get_current_directory()?;
    let has_failures = !failures.is_empty();

    Ok(CarryResult {
        successes,
        failures,
        copy_mode,
        cd_target,
        stash_preserved: stash_preserved || has_failures,
        no_changes: false,
        no_valid_targets: false,
        resolution_errors: Vec::new(),
    })
}
