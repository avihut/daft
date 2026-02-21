//! Core logic for the `git-worktree-clone` command.
//!
//! Clones a repository into a worktree-based directory structure.

use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::multi_remote::path::calculate_worktree_path;
use crate::remote::{get_default_branch_remote, get_remote_branches, is_remote_empty};
use crate::resolve_initial_branch;
use crate::utils::*;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Input parameters for the clone operation.
pub struct CloneParams {
    /// The repository URL to clone.
    pub repository_url: String,
    /// Check out this branch instead of the remote's default.
    pub branch: Option<String>,
    /// Perform a bare clone only.
    pub no_checkout: bool,
    /// Create a worktree for each remote branch.
    pub all_branches: bool,
    /// Remote for worktree organization (multi-remote mode).
    pub remote: Option<String>,
    /// Remote name (from settings).
    pub remote_name: String,
    /// Whether multi-remote mode is enabled.
    pub multi_remote_enabled: bool,
    /// Default remote name for multi-remote.
    pub multi_remote_default: String,
    /// Whether to set upstream tracking (from settings).
    pub checkout_upstream: bool,
    /// Whether to use gitoxide.
    pub use_gitoxide: bool,
}

/// Result of a clone operation.
pub struct CloneResult {
    pub repo_name: String,
    pub target_branch: String,
    pub default_branch: String,
    pub parent_dir: PathBuf,
    pub git_dir: PathBuf,
    pub remote_name: String,
    pub repository_url: String,
    /// Where to cd into after the operation (None for bare/no-checkout).
    pub cd_target: Option<PathBuf>,
    /// The worktree that was created (for hook context).
    pub worktree_dir: Option<PathBuf>,
    /// True if no worktree was created because the branch doesn't exist.
    pub branch_not_found: bool,
    /// True if the repository was empty (no commits).
    pub is_empty: bool,
    /// True if in no-checkout mode.
    pub no_checkout: bool,
}

/// Execute the clone operation.
///
/// Handles the entire clone workflow: detect branches, clone bare, create
/// worktrees, fetch tracking refs, and set upstream. Does NOT run hooks --
/// the command layer handles that due to clone-specific trust semantics.
pub fn execute(params: &CloneParams, progress: &mut dyn ProgressSink) -> Result<CloneResult> {
    let repo_name = crate::extract_repo_name(&params.repository_url)?;
    progress.on_step(&format!("Repository name detected: '{repo_name}'"));

    // Detect remote branches and determine targets
    let (default_branch, target_branch, branch_exists, is_empty) =
        detect_branches(params, progress)?;

    let parent_dir = PathBuf::from(&repo_name);
    let use_multi_remote = params.remote.is_some() || params.multi_remote_enabled;
    let remote_for_path = params
        .remote
        .clone()
        .unwrap_or_else(|| params.multi_remote_default.clone());

    let worktree_dir = calculate_worktree_path(
        &parent_dir,
        &target_branch,
        &remote_for_path,
        use_multi_remote,
    );

    report_plan(
        params,
        &parent_dir,
        &worktree_dir,
        branch_exists,
        is_empty,
        progress,
    );

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    progress.on_step("Creating repository directory...");
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    progress.on_step(&format!(
        "Cloning bare repository into './{}'...",
        git_dir.display()
    ));

    if let Err(e) = git.clone_bare(&params.repository_url, &git_dir) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git clone failed"));
    }

    progress.on_step(&format!(
        "Changing directory to './{}'",
        parent_dir.display()
    ));
    change_directory(&parent_dir)?;

    // Set up fetch refspec for bare repo
    progress.on_step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&params.remote_name) {
        progress.on_warning(&format!("Could not set fetch refspec: {e}"));
    }

    // Set multi-remote config if --remote was provided
    if params.remote.is_some() {
        progress.on_step("Enabling multi-remote mode for this repository...");
        crate::multi_remote::config::set_multi_remote_enabled(&git, true)?;
        crate::multi_remote::config::set_multi_remote_default(&git, &remote_for_path)?;
    }

    let should_create_worktree = !params.no_checkout && (branch_exists || is_empty);

    if should_create_worktree {
        let relative_worktree_path = if use_multi_remote {
            PathBuf::from(&remote_for_path).join(&target_branch)
        } else {
            PathBuf::from(&target_branch)
        };

        if params.all_branches {
            if is_empty {
                anyhow::bail!(
                    "Cannot use --all-branches with an empty repository (no branches exist)"
                );
            }
            create_all_worktrees(
                &git,
                &params.remote_name,
                use_multi_remote,
                &remote_for_path,
                params.use_gitoxide,
                progress,
            )?;
        } else if is_empty {
            create_orphan_worktree(&git, &target_branch, &relative_worktree_path, progress)?;
        } else {
            create_single_worktree(&git, &target_branch, &relative_worktree_path, progress)?;
        }

        progress.on_step(&format!(
            "Changing directory to worktree: './{}'",
            relative_worktree_path.display()
        ));

        if let Err(e) = change_directory(&relative_worktree_path) {
            change_directory(parent_dir.parent().unwrap_or(Path::new("."))).ok();
            return Err(e);
        }

        // Skip fetch and upstream setup for empty repos
        if !is_empty {
            setup_tracking(
                &git,
                &params.remote_name,
                &target_branch,
                params.checkout_upstream,
                progress,
            );
        }

        let current_dir = get_current_directory()?;

        Ok(CloneResult {
            repo_name,
            target_branch,
            default_branch,
            parent_dir,
            git_dir,
            remote_name: params.remote_name.clone(),
            repository_url: params.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: Some(get_current_directory()?),
            branch_not_found: false,
            is_empty,
            no_checkout: false,
        })
    } else if !params.no_checkout && !branch_exists {
        let current_dir = get_current_directory()?;

        Ok(CloneResult {
            repo_name,
            target_branch,
            default_branch,
            parent_dir,
            git_dir,
            remote_name: params.remote_name.clone(),
            repository_url: params.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: None,
            branch_not_found: true,
            is_empty,
            no_checkout: false,
        })
    } else {
        Ok(CloneResult {
            repo_name,
            target_branch,
            default_branch,
            parent_dir,
            git_dir,
            remote_name: params.remote_name.clone(),
            repository_url: params.repository_url.clone(),
            cd_target: None,
            worktree_dir: None,
            branch_not_found: false,
            is_empty,
            no_checkout: true,
        })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Detect the default branch, target branch, and whether the repo is empty.
fn detect_branches(
    params: &CloneParams,
    progress: &mut dyn ProgressSink,
) -> Result<(String, String, bool, bool)> {
    match get_default_branch_remote(&params.repository_url, params.use_gitoxide) {
        Ok(default_branch) => {
            progress.on_step(&format!("Default branch detected: '{default_branch}'"));

            let (target_branch, branch_exists) = if let Some(ref specified) = params.branch {
                progress.on_step(&format!(
                    "Checking if branch '{specified}' exists on remote..."
                ));
                let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);
                let exists = git
                    .ls_remote_branch_exists(&params.repository_url, specified)
                    .unwrap_or(false);
                if exists {
                    progress.on_step(&format!("Branch '{specified}' found on remote"));
                } else {
                    progress.on_warning(&format!("Branch '{specified}' does not exist on remote"));
                }
                (specified.clone(), exists)
            } else {
                (default_branch.clone(), true)
            };

            Ok((default_branch, target_branch, branch_exists, false))
        }
        Err(e) => {
            if is_remote_empty(&params.repository_url, params.use_gitoxide).unwrap_or(false) {
                let local_default = resolve_initial_branch(&params.branch);
                progress.on_step(&format!(
                    "Empty repository detected, using branch: '{local_default}'"
                ));
                Ok((local_default.clone(), local_default, false, true))
            } else {
                Err(e.context("Failed to determine default branch"))
            }
        }
    }
}

/// Report the plan before executing.
fn report_plan(
    params: &CloneParams,
    parent_dir: &Path,
    worktree_dir: &Path,
    branch_exists: bool,
    is_empty: bool,
    progress: &mut dyn ProgressSink,
) {
    progress.on_step(&format!(
        "Target repository directory: './{}'",
        parent_dir.display()
    ));

    if !params.no_checkout {
        if params.all_branches {
            progress.on_step("Worktrees will be created for all remote branches");
        } else if branch_exists || is_empty {
            progress.on_step(&format!(
                "Initial worktree will be in: './{}'",
                worktree_dir.display()
            ));
        } else {
            progress.on_step("Worktree creation will be skipped (branch does not exist)");
        }
    } else {
        progress.on_step("No-checkout mode: Only bare repository will be created");
    }
}

fn create_single_worktree(
    git: &GitCommand,
    branch: &str,
    worktree_path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step(&format!(
        "Creating initial worktree for branch '{}' at '{}'...",
        branch,
        worktree_path.display()
    ));
    ensure_parent_dir(worktree_path)?;
    git.worktree_add(worktree_path, branch)
        .context("Failed to create initial worktree")?;
    Ok(())
}

fn create_orphan_worktree(
    git: &GitCommand,
    branch: &str,
    worktree_path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step(&format!(
        "Creating initial worktree for empty repository at '{}'...",
        worktree_path.display()
    ));
    ensure_parent_dir(worktree_path)?;
    git.worktree_add_orphan(worktree_path, branch)
        .context("Failed to create initial worktree for empty repository")?;
    Ok(())
}

fn create_all_worktrees(
    git: &GitCommand,
    remote_name: &str,
    use_multi_remote: bool,
    remote_for_path: &str,
    use_gitoxide: bool,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step("Fetching all remote branches...");
    git.fetch(remote_name, false)?;

    let remote_branches =
        get_remote_branches(remote_name, use_gitoxide).context("Failed to get remote branches")?;

    if remote_branches.is_empty() {
        anyhow::bail!("No remote branches found");
    }

    for branch in &remote_branches {
        let worktree_path = if use_multi_remote {
            PathBuf::from(remote_for_path).join(branch)
        } else {
            PathBuf::from(branch)
        };

        progress.on_step(&format!(
            "Creating worktree for branch '{}' at '{}'...",
            branch,
            worktree_path.display()
        ));

        if let Some(parent) = worktree_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    progress.on_warning(&format!("creating directory '{}': {e}", parent.display()));
                    continue;
                }
            }
        }

        if let Err(e) = git.worktree_add(&worktree_path, branch) {
            progress.on_warning(&format!("creating worktree for branch '{branch}': {e}"));
            continue;
        }
    }

    Ok(())
}

/// Ensure parent directory exists (for multi-remote mode).
fn ensure_parent_dir(worktree_path: &Path) -> Result<()> {
    if let Some(parent) = worktree_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }
    Ok(())
}

/// Set up remote tracking refs and upstream after worktree creation.
fn setup_tracking(
    git: &GitCommand,
    remote_name: &str,
    target_branch: &str,
    checkout_upstream: bool,
    progress: &mut dyn ProgressSink,
) {
    progress.on_step(&format!(
        "Fetching from '{remote_name}' to set up remote tracking..."
    ));
    if let Err(e) = git.fetch(remote_name, false) {
        progress.on_warning(&format!("Could not fetch from remote: {e}"));
    }

    if let Err(e) = git.remote_set_head_auto(remote_name) {
        progress.on_warning(&format!("Could not set remote HEAD: {e}"));
    }

    if checkout_upstream {
        progress.on_step(&format!(
            "Setting upstream to '{remote_name}/{target_branch}'..."
        ));
        if let Err(e) = git.set_upstream(remote_name, target_branch) {
            progress.on_warning(&format!(
                "Could not set upstream tracking: {e}. You may need to set it manually."
            ));
        }
    } else {
        progress.on_step("Skipping upstream setup (disabled in config)");
    }
}
