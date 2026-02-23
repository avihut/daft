//! Core logic for the `git-worktree-rename` command.
//!
//! Renames a branch and its associated worktree directory, optionally
//! updating the remote branch as well.

use crate::core::multi_remote::path::{
    calculate_worktree_path, extract_remote_from_path, resolve_remote_for_branch,
};
use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::{get_git_common_dir, get_project_root};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Input parameters for the rename operation.
pub struct RenameParams {
    /// Source branch name or worktree path.
    pub source: String,
    /// New branch name.
    pub new_branch: String,
    /// Skip remote branch rename.
    pub no_remote: bool,
    /// Preview changes without executing.
    pub dry_run: bool,
    /// Whether to use gitoxide.
    pub use_gitoxide: bool,
    /// Whether output is in quiet mode.
    pub is_quiet: bool,
    /// Remote name (from settings).
    pub remote_name: String,
    /// Whether multi-remote mode is enabled.
    pub multi_remote_enabled: bool,
    /// Default remote for multi-remote mode.
    pub multi_remote_default: String,
}

/// Result of a rename operation.
pub struct RenameResult {
    /// The old branch name.
    pub old_branch: String,
    /// The new branch name.
    pub new_branch: String,
    /// The old worktree path.
    pub old_path: PathBuf,
    /// The new worktree path.
    pub new_path: PathBuf,
    /// Whether the local branch was renamed.
    pub branch_renamed: bool,
    /// Whether the worktree was moved.
    pub worktree_moved: bool,
    /// Whether the remote branch was renamed.
    pub remote_renamed: bool,
    /// Where to cd if CWD was inside the source worktree.
    pub cd_target: Option<PathBuf>,
    /// Whether this was a dry run.
    pub dry_run: bool,
    /// Non-fatal warnings collected during the operation.
    pub warnings: Vec<String>,
}

/// Parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Execute the rename operation.
pub fn execute(params: &RenameParams, sink: &mut dyn ProgressSink) -> Result<RenameResult> {
    let git = GitCommand::new(params.is_quiet).with_gitoxide(params.use_gitoxide);
    let project_root = get_project_root()?;
    let _git_dir = get_git_common_dir()?;

    // Step 1: Resolve source to branch name + worktree path.
    let worktree_entries = parse_worktree_list(&git)?;
    let (old_branch, old_path) =
        resolve_source(&params.source, &worktree_entries, &project_root, sink)?;

    sink.on_step(&format!(
        "Resolved source to branch '{}' at '{}'",
        old_branch,
        old_path.display()
    ));

    // Step 2: Validate.
    // New branch must not already exist.
    let new_ref = format!("refs/heads/{}", params.new_branch);
    if git.show_ref_exists(&new_ref)? {
        anyhow::bail!(
            "Branch '{}' already exists. Choose a different name.",
            params.new_branch
        );
    }

    // Calculate new worktree path.
    let remote_for_path = if params.multi_remote_enabled {
        // Preserve the remote prefix from the existing path.
        extract_remote_from_path(&project_root, &old_path)
            .unwrap_or_else(|| params.multi_remote_default.clone())
    } else {
        params.remote_name.clone()
    };

    let new_path = calculate_worktree_path(
        &project_root,
        &params.new_branch,
        &remote_for_path,
        params.multi_remote_enabled,
    );

    // New path must not already exist on disk.
    if new_path.exists() {
        anyhow::bail!(
            "Destination path '{}' already exists on disk.",
            new_path.display()
        );
    }

    // Step 3: Check if CWD is inside the source worktree.
    let cwd = std::env::current_dir().ok();
    let cwd_inside_source = cwd.as_ref().is_some_and(|cwd| {
        let canonical_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.clone());
        let canonical_old = std::fs::canonicalize(&old_path).unwrap_or_else(|_| old_path.clone());
        canonical_cwd.starts_with(&canonical_old)
    });

    // Step 4: Dry run — report planned actions and return.
    if params.dry_run {
        sink.on_step(&format!(
            "Would rename branch '{}' to '{}'",
            old_branch, params.new_branch
        ));
        sink.on_step(&format!(
            "Would move worktree from '{}' to '{}'",
            old_path.display(),
            new_path.display()
        ));

        if !params.no_remote {
            let remote_info =
                resolve_remote_for_branch(&git, &old_branch, None, &params.remote_name);
            if let Ok(remote) = remote_info {
                if git
                    .show_ref_exists(&format!("refs/remotes/{remote}/{old_branch}"))
                    .unwrap_or(false)
                {
                    sink.on_step(&format!(
                        "Would push '{}/{}' and delete '{}/{}'",
                        remote, params.new_branch, remote, old_branch
                    ));
                }
            }
        }

        if cwd_inside_source {
            sink.on_step(&format!("Would cd to '{}'", new_path.display()));
        }

        let cd_target = if cwd_inside_source {
            Some(new_path.clone())
        } else {
            None
        };

        return Ok(RenameResult {
            old_branch,
            new_branch: params.new_branch.clone(),
            old_path,
            new_path,
            branch_renamed: false,
            worktree_moved: false,
            remote_renamed: false,
            cd_target,
            dry_run: true,
            warnings: Vec::new(),
        });
    }

    let mut warnings = Vec::new();

    // Step 5: Rename the local branch.
    sink.on_step(&format!(
        "Renaming branch '{}' to '{}'...",
        old_branch, params.new_branch
    ));
    git.branch_rename(&old_branch, &params.new_branch)
        .with_context(|| {
            format!(
                "Failed to rename branch '{}' to '{}'",
                old_branch, params.new_branch
            )
        })?;

    // Step 6: Create parent dirs if needed, then move the worktree.
    if let Some(parent) = new_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory '{}'", parent.display()))?;
        }
    }

    sink.on_step(&format!(
        "Moving worktree from '{}' to '{}'...",
        old_path.display(),
        new_path.display()
    ));
    git.worktree_move(&old_path, &new_path).with_context(|| {
        format!(
            "Failed to move worktree from '{}' to '{}'",
            old_path.display(),
            new_path.display()
        )
    })?;

    // Step 7: Remote operations (if applicable and not --no-remote).
    let mut remote_renamed = false;
    if !params.no_remote {
        let remote =
            resolve_remote_for_branch(&git, &params.new_branch, None, &params.remote_name).ok();

        if let Some(ref remote) = remote {
            // Check if the old branch exists on the remote.
            let old_remote_ref = format!("refs/remotes/{remote}/{old_branch}");
            let has_remote = git.show_ref_exists(&old_remote_ref).unwrap_or(false);

            if has_remote {
                // Push new branch name from within the new worktree directory.
                sink.on_step(&format!(
                    "Pushing '{}/{}' to remote...",
                    remote, params.new_branch
                ));
                match git.push_set_upstream_from(remote, &params.new_branch, &new_path) {
                    Ok(()) => {
                        // Delete old remote branch.
                        sink.on_step(&format!(
                            "Deleting old remote branch '{}/{}'...",
                            remote, old_branch
                        ));
                        match git.push_delete(remote, &old_branch) {
                            Ok(()) => {
                                remote_renamed = true;
                                sink.on_step("Remote branch renamed successfully");
                            }
                            Err(e) => {
                                warnings.push(format!(
                                    "Failed to delete old remote branch '{}/{}': {e}",
                                    remote, old_branch
                                ));
                                sink.on_warning(&format!(
                                    "Could not delete old remote branch '{}/{}': {e}",
                                    remote, old_branch
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        warnings.push(format!("Failed to push new branch to remote: {e}"));
                        sink.on_warning(&format!(
                            "Could not push new branch to '{}/{}': {e}",
                            remote, params.new_branch
                        ));
                    }
                }
            }
        }
    }

    // Step 8: Clean up empty parent directories.
    cleanup_empty_parent_dirs(&project_root, &old_path, sink);

    // Step 9: Set cd_target if CWD was inside the source worktree.
    let cd_target = if cwd_inside_source {
        Some(new_path.clone())
    } else {
        None
    };

    Ok(RenameResult {
        old_branch,
        new_branch: params.new_branch.clone(),
        old_path,
        new_path,
        branch_renamed: true,
        worktree_moved: true,
        remote_renamed,
        cd_target,
        dry_run: false,
        warnings,
    })
}

// ── Source resolution ──────────────────────────────────────────────────────

/// Resolve the source argument to a (branch_name, worktree_path) tuple.
///
/// The source can be:
/// - A branch name (looks up associated worktree)
/// - A path to a worktree (absolute or relative)
fn resolve_source(
    source: &str,
    worktree_entries: &[WorktreeEntry],
    project_root: &Path,
    sink: &mut dyn ProgressSink,
) -> Result<(String, PathBuf)> {
    // Try as a path first (absolute, relative to cwd, or relative to project root).
    let candidate_paths: Vec<PathBuf> = {
        let mut paths = Vec::new();
        let path = PathBuf::from(source);
        if path.is_absolute() {
            paths.push(path);
        } else {
            if let Ok(cwd) = std::env::current_dir() {
                paths.push(cwd.join(source));
            }
            paths.push(project_root.join(source));
        }
        paths
    };

    for candidate in &candidate_paths {
        if let Ok(canonical) = std::fs::canonicalize(candidate) {
            for entry in worktree_entries {
                let entry_canonical =
                    std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());
                if canonical == entry_canonical {
                    return match &entry.branch {
                        Some(branch) => {
                            sink.on_step(&format!(
                                "Resolved path '{}' to branch '{}'",
                                source, branch
                            ));
                            Ok((branch.clone(), entry.path.clone()))
                        }
                        None => {
                            anyhow::bail!(
                                "Worktree at '{}' has a detached HEAD; specify a branch name instead",
                                entry.path.display()
                            );
                        }
                    };
                }
            }
        }
    }

    // Try as a branch name.
    for entry in worktree_entries {
        if entry.branch.as_deref() == Some(source) {
            return Ok((source.to_string(), entry.path.clone()));
        }
    }

    anyhow::bail!(
        "No worktree found for '{}'. The source must be a branch with an associated worktree.",
        source
    )
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Parse `git worktree list --porcelain` into structured entries.
fn parse_worktree_list(git: &GitCommand) -> Result<Vec<WorktreeEntry>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: current_branch.take(),
                });
            }
            current_path = Some(PathBuf::from(worktree_path));
            current_branch = None;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        }
    }
    if let Some(path) = current_path.take() {
        entries.push(WorktreeEntry {
            path,
            branch: current_branch.take(),
        });
    }

    Ok(entries)
}

/// Clean up empty parent directories after moving a worktree.
fn cleanup_empty_parent_dirs(
    project_root: &Path,
    worktree_path: &Path,
    sink: &mut dyn ProgressSink,
) {
    let mut current = worktree_path.parent();
    while let Some(dir) = current {
        if dir == project_root || !dir.starts_with(project_root) {
            break;
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => {
                sink.on_step(&format!("Removed empty directory '{}'", dir.display()));
                current = dir.parent();
            }
            Err(_) => break,
        }
    }
}
