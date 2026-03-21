//! Deprecated: Core logic for the `git-worktree-flow-eject` command.
//!
//! This module is a thin wrapper around [`crate::core::layout::transform`].
//! New code should use `layout::transform::convert_to_non_bare()` directly.

use crate::core::layout::transform;
use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::utils::*;
use crate::{get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Input parameters for the eject operation.
pub struct EjectParams {
    /// Path to the repository to convert (None = current directory).
    pub repository_path: Option<PathBuf>,
    /// Branch to keep (None = auto-detect default).
    pub branch: Option<String>,
    /// Force deletion of dirty worktrees.
    pub force: bool,
    /// Show what would be done without making changes.
    pub dry_run: bool,
    /// Whether to use gitoxide.
    pub use_gitoxide: bool,
    /// Whether output is in quiet mode.
    pub is_quiet: bool,
    /// Remote name (from settings).
    pub remote_name: String,
}

/// Result of an eject operation.
pub struct EjectResult {
    pub project_root: PathBuf,
    pub target_branch: String,
    pub dry_run: bool,
    pub stash_conflict: bool,
}

/// Execute the eject operation.
///
/// This is a compatibility wrapper. It performs eject-specific validation
/// (repo path, is-git-repo, not-in-layout checks, dry-run) then delegates
/// the actual conversion to [`transform::convert_to_non_bare()`].
///
/// Hooks (pre-remove, post-remove) are called via `HookRunner` for each
/// worktree removal, so the caller must supply a bridge that can execute hooks.
pub fn execute(
    params: &EjectParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<EjectResult> {
    // Change to repository path if provided
    if let Some(ref repo_path) = params.repository_path {
        if !repo_path.exists() {
            anyhow::bail!("Repository path does not exist: {}", repo_path.display());
        }
        change_directory(repo_path)?;
    }

    let git_dir = get_git_common_dir().context("Not inside a Git repository")?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    change_directory(&project_root)?;

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let git = GitCommand::new(params.is_quiet).with_gitoxide(params.use_gitoxide);

    if !transform::is_bare_worktree_layout(&git)? {
        anyhow::bail!(
            "Repository is not in worktree layout.\n\
             Use git-worktree-flow-adopt to convert a traditional repository to worktree layout."
        );
    }

    // Dry-run: report what would happen without making changes
    if params.dry_run {
        let worktrees = transform::parse_worktrees(&git)?;
        let non_bare_worktrees: Vec<_> = worktrees.iter().filter(|wt| !wt.is_bare).collect();

        if non_bare_worktrees.is_empty() {
            anyhow::bail!(
                "No worktrees found. Cannot convert to traditional layout without at least one worktree."
            );
        }

        sink.on_step("[DRY RUN] Would perform the following actions:");
        for wt in &non_bare_worktrees {
            let branch = wt.branch.as_deref().unwrap_or("(detached)");
            sink.on_step(&format!(
                "[DRY RUN] Found worktree '{}' ({})",
                wt.path.display(),
                branch
            ));
        }
        sink.on_step("[DRY RUN] Would remove non-target worktrees");
        sink.on_step("[DRY RUN] Would move target worktree files to project root");
        sink.on_step("[DRY RUN] Would convert to non-bare repository");

        return Ok(EjectResult {
            project_root,
            target_branch: "unknown".to_string(),
            dry_run: true,
            stash_conflict: false,
        });
    }

    // Delegate the actual conversion to the layout transform module
    let convert_params = transform::ConvertToNonBareParams {
        branch: params.branch.clone(),
        force: params.force,
        use_gitoxide: params.use_gitoxide,
        is_quiet: params.is_quiet,
        remote_name: params.remote_name.clone(),
    };
    let result = transform::convert_to_non_bare(&convert_params, sink)?;

    Ok(EjectResult {
        project_root: result.project_root,
        target_branch: result.target_branch,
        dry_run: false,
        stash_conflict: result.stash_conflict,
    })
}
