//! Deprecated: Core logic for the `git-worktree-flow-adopt` command.
//!
//! This module is a thin wrapper around [`crate::core::layout::transform`].
//! New code should use `layout::transform::convert_to_bare()` directly.

use crate::core::layout::transform;
use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::utils::*;
use crate::{get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// Input parameters for the adopt operation.
pub struct AdoptParams {
    /// Path to the repository to convert (None = current directory).
    pub repository_path: Option<PathBuf>,
    /// Show what would be done without making changes.
    pub dry_run: bool,
    /// Whether to use gitoxide.
    pub use_gitoxide: bool,
}

/// Result of an adopt operation.
pub struct AdoptResult {
    pub project_root: PathBuf,
    pub git_dir: PathBuf,
    pub worktree_path: PathBuf,
    pub current_branch: String,
    pub remote_name: String,
    pub repo_display_name: String,
    pub dry_run: bool,
    pub stash_conflict: bool,
}

/// Execute the adopt operation.
///
/// This is a compatibility wrapper. It performs adopt-specific validation
/// (repo path, is-git-repo, already-in-layout checks, dry-run) then
/// delegates the actual conversion to [`transform::convert_to_bare()`].
///
/// Hooks are NOT run here -- the command layer handles them due to trust
/// semantics (--trust-hooks, --no-hooks).
pub fn execute(params: &AdoptParams, progress: &mut dyn ProgressSink) -> Result<AdoptResult> {
    // Change to repository path if provided
    if let Some(ref repo_path) = params.repository_path {
        if !repo_path.exists() {
            anyhow::bail!("Repository path does not exist: {}", repo_path.display());
        }
        change_directory(repo_path)?;
    }

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    if transform::is_bare_worktree_layout(&git)? {
        anyhow::bail!(
            "Repository is already in worktree layout.\n\
             Use git-worktree-checkout or git-worktree-checkout -b to create new worktrees."
        );
    }

    // Dry-run: report what would happen without making changes
    if params.dry_run {
        let current_branch =
            crate::get_current_branch().context("Could not determine current branch")?;
        let git_dir = get_git_common_dir()?;
        let git_dir = std::fs::canonicalize(&git_dir)
            .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
        let project_root = git_dir
            .parent()
            .context("Could not determine project root")?
            .to_path_buf();
        let worktree_path = project_root.join(&current_branch);
        let settings = crate::settings::DaftSettings::load_global()?;
        let repo_display_name = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("repository")
            .to_string();

        progress.on_step("[DRY RUN] Would perform the following actions:");
        progress.on_step(&format!(
            "[DRY RUN] Move all files to '{}'",
            worktree_path.display()
        ));
        progress.on_step("[DRY RUN] Convert .git to bare repository");
        progress.on_step(&format!(
            "[DRY RUN] Register worktree for branch '{current_branch}'"
        ));
        if git.has_uncommitted_changes()? {
            progress.on_step("[DRY RUN] Restore uncommitted changes in new worktree");
        }

        return Ok(AdoptResult {
            project_root,
            git_dir,
            worktree_path,
            current_branch,
            remote_name: settings.remote,
            repo_display_name,
            dry_run: true,
            stash_conflict: false,
        });
    }

    // Delegate the actual conversion to the layout transform module
    let convert_params = transform::ConvertToBareParams {
        use_gitoxide: params.use_gitoxide,
    };
    let result = transform::convert_to_bare(&convert_params, progress)?;

    Ok(AdoptResult {
        project_root: result.project_root,
        git_dir: result.git_dir,
        worktree_path: result.worktree_path,
        current_branch: result.current_branch,
        remote_name: result.remote_name,
        repo_display_name: result.repo_display_name,
        dry_run: false,
        stash_conflict: result.stash_conflict,
    })
}
