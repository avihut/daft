//! Core logic for the `git-worktree-init` command.
//!
//! Initializes a new Git repository in the worktree-based directory structure.

use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::multi_remote::path::calculate_worktree_path;
use crate::resolve_initial_branch;
use crate::utils::*;
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Input parameters for the init operation.
pub struct InitParams {
    /// Name for the new repository directory.
    pub repository_name: String,
    /// Create only the bare repository; do not create an initial worktree.
    pub bare: bool,
    /// Use <name> as the initial branch.
    pub initial_branch: Option<String>,
    /// Organize worktree under this remote folder.
    pub remote: Option<String>,
    /// Whether multi-remote is globally enabled.
    pub multi_remote_enabled: bool,
    /// Default remote name for multi-remote.
    pub multi_remote_default: String,
}

/// Result of an init operation.
pub struct InitResult {
    /// Name of the created repository.
    pub repository_name: String,
    /// The initial branch name.
    pub initial_branch: String,
    /// Whether bare mode was used.
    pub bare_mode: bool,
    /// Path to cd into (if a worktree was created).
    pub cd_target: Option<PathBuf>,
}

/// Execute the init operation.
///
/// Creates the repository directory structure and returns a structured result.
/// Does not run exec commands or write cd_path — those are command-layer concerns.
pub fn execute(
    params: &InitParams,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<InitResult> {
    validate_repo_name(&params.repository_name)?;

    let initial_branch = resolve_initial_branch(&params.initial_branch);
    if initial_branch.is_empty() {
        anyhow::bail!("Initial branch name cannot be empty");
    }

    // Determine if we should use multi-remote mode
    let use_multi_remote = params.remote.is_some() || params.multi_remote_enabled;
    let remote_for_path = params
        .remote
        .clone()
        .unwrap_or_else(|| params.multi_remote_default.clone());

    let parent_dir = PathBuf::from(&params.repository_name);
    let worktree_dir = calculate_worktree_path(
        &parent_dir,
        &initial_branch,
        &remote_for_path,
        use_multi_remote,
    );

    progress.on_step(&format!(
        "Target repository directory: './{}'",
        parent_dir.display()
    ));

    if !params.bare {
        progress.on_step(&format!(
            "Initial worktree will be in: './{}'",
            worktree_dir.display()
        ));
    } else {
        progress.on_step("Bare mode: Only bare repository will be created");
    }

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    progress.on_step("Creating repository directory...");
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");

    progress.on_step(&format!(
        "Initializing bare repository in './{}'...",
        git_dir.display()
    ));

    if let Err(e) = git.init_bare(&git_dir, &initial_branch) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git init failed"));
    }

    if !params.bare {
        progress.on_step(&format!(
            "Changing directory to './{}'",
            parent_dir.display()
        ));
        change_directory(&parent_dir)?;

        // Set multi-remote config if --remote was provided
        if params.remote.is_some() {
            progress.on_step("Enabling multi-remote mode for this repository...");
            crate::multi_remote::config::set_multi_remote_enabled(git, true)?;
            crate::multi_remote::config::set_multi_remote_default(git, &remote_for_path)?;
        }

        // Calculate the relative worktree path from parent_dir
        let relative_worktree_path = if use_multi_remote {
            PathBuf::from(&remote_for_path).join(&initial_branch)
        } else {
            PathBuf::from(&initial_branch)
        };

        progress.on_step(&format!(
            "Creating initial worktree for branch '{}' at '{}'...",
            initial_branch,
            relative_worktree_path.display()
        ));

        // Ensure parent directory exists (for multi-remote mode)
        if let Some(parent) = relative_worktree_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }
        }

        if let Err(e) = git.worktree_add_orphan(&relative_worktree_path, &initial_branch) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            remove_directory(&parent_dir).ok();
            return Err(e.context("Failed to create initial worktree"));
        }

        progress.on_step(&format!(
            "Changing directory to worktree: './{}'",
            relative_worktree_path.display()
        ));

        if let Err(e) = change_directory(&relative_worktree_path) {
            change_directory(parent_dir.parent().unwrap_or(&PathBuf::from("."))).ok();
            return Err(e);
        }

        let current_dir = get_current_directory()?;

        Ok(InitResult {
            repository_name: params.repository_name.clone(),
            initial_branch,
            bare_mode: false,
            cd_target: Some(current_dir),
        })
    } else {
        Ok(InitResult {
            repository_name: params.repository_name.clone(),
            initial_branch,
            bare_mode: true,
            cd_target: None,
        })
    }
}
