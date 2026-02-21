//! Core logic for the `git-worktree-fetch` command.
//!
//! Updates worktree branches by pulling from their remote tracking branches.

use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::utils::*;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Input parameters for the fetch operation.
pub struct FetchParams {
    /// Target worktrees by directory name or branch name.
    pub targets: Vec<String>,
    /// Update all worktrees.
    pub all: bool,
    /// Update even if worktree has uncommitted changes.
    pub force: bool,
    /// Show what would be done without making changes.
    pub dry_run: bool,
    /// Use git pull --rebase.
    pub rebase: bool,
    /// Use git pull --autostash.
    pub autostash: bool,
    /// Only fast-forward (default behavior).
    pub ff_only: bool,
    /// Allow merge commits (disables --ff-only).
    pub no_ff_only: bool,
    /// Additional arguments to pass to git pull.
    pub pull_args: Vec<String>,
    /// Whether to run in quiet mode (suppresses git pull output).
    pub quiet: bool,
}

/// Result of a fetch operation for a single worktree.
#[derive(Debug)]
pub struct WorktreeFetchResult {
    pub worktree_name: String,
    pub success: bool,
    pub message: String,
    pub skipped: bool,
}

/// Aggregated result of fetching all worktrees.
pub struct FetchResult {
    /// Per-worktree results.
    pub results: Vec<WorktreeFetchResult>,
    /// The remote name that was fetched.
    pub remote_name: String,
    /// The remote URL (if available).
    pub remote_url: Option<String>,
    /// The pull arguments used.
    pub pull_args: Vec<String>,
}

impl FetchResult {
    pub fn updated_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.success && !r.skipped)
            .count()
    }

    pub fn skipped_count(&self) -> usize {
        self.results.iter().filter(|r| r.skipped).count()
    }

    pub fn failed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| !r.success && !r.skipped)
            .count()
    }
}

/// Execute the fetch operation.
pub fn execute(
    params: &FetchParams,
    git: &GitCommand,
    project_root: &Path,
    remote_name: &str,
    progress: &mut dyn ProgressSink,
) -> Result<FetchResult> {
    let original_dir = get_current_directory()?;

    // Determine targets
    let targets = determine_targets(params, git, project_root, progress)?;

    if targets.is_empty() {
        return Ok(FetchResult {
            results: Vec::new(),
            remote_name: remote_name.to_string(),
            remote_url: git.remote_get_url(remote_name).ok(),
            pull_args: Vec::new(),
        });
    }

    // Build pull arguments
    let pull_args = build_pull_args(params);

    progress.on_step(&format!("Pull arguments: {}", pull_args.join(" ")));

    // Process each target
    let mut results: Vec<WorktreeFetchResult> = Vec::new();

    for target_path in &targets {
        let worktree_name = target_path
            .strip_prefix(project_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("unknown")
            .to_string();

        let result = process_worktree(
            git,
            target_path,
            &worktree_name,
            &pull_args,
            params,
            progress,
        );
        results.push(result);
    }

    // Return to original directory
    change_directory(&original_dir)?;

    Ok(FetchResult {
        results,
        remote_name: remote_name.to_string(),
        remote_url: git.remote_get_url(remote_name).ok(),
        pull_args,
    })
}

/// Determine which worktrees to update based on arguments.
fn determine_targets(
    params: &FetchParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<Vec<PathBuf>> {
    if params.all {
        get_all_worktrees(git)
    } else if params.targets.is_empty() {
        let current = git.get_current_worktree_path()?;
        Ok(vec![current])
    } else {
        let mut resolved: Vec<PathBuf> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for target in &params.targets {
            match git.resolve_worktree_path(target, project_root) {
                Ok(path) => resolved.push(path),
                Err(e) => errors.push(format!("'{}': {}", target, e)),
            }
        }

        if !errors.is_empty() {
            for error in &errors {
                progress.on_warning(&format!("Failed to resolve target {error}"));
            }
            anyhow::bail!("Failed to resolve {} target(s)", errors.len());
        }

        Ok(resolved)
    }
}

/// Get all non-bare worktrees from git worktree list.
fn get_all_worktrees(git: &GitCommand) -> Result<Vec<PathBuf>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut worktrees: Vec<PathBuf> = Vec::new();
    let mut current_worktree: Option<PathBuf> = None;
    let mut is_bare = false;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            if let Some(path) = current_worktree.take() {
                if !is_bare {
                    worktrees.push(path);
                }
            }
            current_worktree = Some(PathBuf::from(worktree_path));
            is_bare = false;
        } else if line == "bare" {
            is_bare = true;
        }
    }

    if let Some(path) = current_worktree {
        if !is_bare {
            worktrees.push(path);
        }
    }

    Ok(worktrees)
}

/// Build pull arguments from params and settings.
fn build_pull_args(params: &FetchParams) -> Vec<String> {
    let mut pull_args: Vec<String> = Vec::new();

    if params.rebase {
        pull_args.push("--rebase".to_string());
    }

    if !params.no_ff_only && !params.rebase {
        pull_args.push("--ff-only".to_string());
    }

    if params.autostash {
        pull_args.push("--autostash".to_string());
    }

    for arg in &params.pull_args {
        pull_args.push(arg.clone());
    }

    pull_args
}

/// Process a single worktree.
fn process_worktree(
    git: &GitCommand,
    target_path: &Path,
    worktree_name: &str,
    pull_args: &[String],
    params: &FetchParams,
    progress: &mut dyn ProgressSink,
) -> WorktreeFetchResult {
    progress.on_step(&format!("Processing '{worktree_name}'..."));

    // Change to worktree directory
    if let Err(e) = change_directory(target_path) {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: false,
            message: format!("Failed to change to directory: {e}"),
            skipped: false,
        };
    }

    // Check for uncommitted changes
    match git.has_uncommitted_changes_in(target_path) {
        Ok(has_changes) => {
            if has_changes && !params.force {
                progress.on_warning(&format!(
                    "Skipping '{worktree_name}': has uncommitted changes (use --force to update anyway)"
                ));
                return WorktreeFetchResult {
                    worktree_name: worktree_name.to_string(),
                    success: true,
                    message: "Skipped: uncommitted changes".to_string(),
                    skipped: true,
                };
            }
        }
        Err(e) => {
            return WorktreeFetchResult {
                worktree_name: worktree_name.to_string(),
                success: false,
                message: format!("Failed to check status: {e}"),
                skipped: false,
            };
        }
    }

    // Check if branch has an upstream
    if check_has_upstream(git).is_err() {
        progress.on_warning(&format!(
            "Skipping '{worktree_name}': no tracking branch configured"
        ));
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Skipped: no tracking branch".to_string(),
            skipped: true,
        };
    }

    // Dry run mode
    if params.dry_run {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: format!("Dry run: would pull with: git pull {}", pull_args.join(" ")),
            skipped: true,
        };
    }

    // Run git pull
    let pull_args_refs: Vec<&str> = pull_args.iter().map(|s| s.as_str()).collect();

    let pull_result = if params.quiet {
        git.pull(&pull_args_refs).map(|_| ())
    } else {
        git.pull_passthrough(&pull_args_refs)
    };

    match pull_result {
        Ok(()) => WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Updated successfully".to_string(),
            skipped: false,
        },
        Err(e) => WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: false,
            message: format!("Failed: {e}"),
            skipped: false,
        },
    }
}

/// Check if the current branch has an upstream tracking branch.
fn check_has_upstream(git: &GitCommand) -> Result<()> {
    let branch = git.symbolic_ref_short_head()?;
    let remote_key = format!("branch.{}.remote", branch);
    if git.config_get(&remote_key)?.is_none() {
        anyhow::bail!("No upstream configured for branch '{}'", branch);
    }
    Ok(())
}
