//! Core logic for the `git-worktree-flow-adopt` command.
//!
//! Converts a traditional repository to worktree-based layout.

use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::settings::DaftSettings;
use crate::utils::*;
use crate::{get_current_branch, get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

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

    if is_worktree_layout(&git)? {
        anyhow::bail!(
            "Repository is already in worktree layout.\n\
             Use git-worktree-checkout or git-worktree-checkout -b to create new worktrees."
        );
    }

    let current_branch = get_current_branch().context("Could not determine current branch")?;
    progress.on_step(&format!("Current branch: '{current_branch}'"));

    let git_dir = get_git_common_dir()?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    progress.on_step(&format!("Project root: '{}'", project_root.display()));

    let has_changes = git.has_uncommitted_changes()?;
    if has_changes {
        progress.on_step("Uncommitted changes detected - will preserve them");
    }

    let worktree_path = project_root.join(&current_branch);

    let settings = DaftSettings::load_global()?;
    let repo_display_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository")
        .to_string();

    if params.dry_run {
        progress.on_step("[DRY RUN] Would perform the following actions:");
        progress.on_step(&format!(
            "[DRY RUN] Move all files to '{}'",
            worktree_path.display()
        ));
        progress.on_step("[DRY RUN] Convert .git to bare repository");
        progress.on_step(&format!(
            "[DRY RUN] Register worktree for branch '{current_branch}'"
        ));
        if has_changes {
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

    // Stash changes
    if has_changes {
        progress.on_step("Stashing uncommitted changes...");
        git.stash_push_with_untracked("daft-flow-adopt: temporary stash for conversion")
            .context("Failed to stash changes")?;
    }

    change_directory(&project_root)?;

    // Move files via staging directory
    move_files_via_staging(&project_root, &git_dir, &worktree_path, progress)?;

    // Convert to bare
    progress.on_step("Converting to bare repository...");
    git.config_set("core.bare", "true")
        .context("Failed to set core.bare")?;

    let bare_index = git_dir.join("index");
    if bare_index.exists() {
        fs::remove_file(&bare_index).ok();
    }

    // Setup fetch refspec
    progress.on_step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&settings.remote) {
        progress.on_warning(&format!("Could not set fetch refspec: {e}"));
    }

    // Register worktree
    register_worktree(&git_dir, &worktree_path, &current_branch, progress)?;

    change_directory(&worktree_path)?;

    // Initialize index
    progress.on_step("Initializing worktree index...");
    let reset_result = std::process::Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(&worktree_path)
        .output()
        .context("Failed to initialize worktree index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        progress.on_warning(&format!("git reset warning: {}", stderr.trim()));
    }

    // Restore stashed changes
    let stash_conflict = if has_changes {
        progress.on_step("Restoring uncommitted changes...");
        if let Err(e) = git.stash_pop() {
            progress.on_warning(&format!("Could not restore stashed changes: {e}"));
            progress
                .on_warning("Your changes are still in the stash. Run 'git stash pop' manually.");
            true
        } else {
            false
        }
    } else {
        false
    };

    Ok(AdoptResult {
        project_root,
        git_dir,
        worktree_path,
        current_branch,
        remote_name: settings.remote,
        repo_display_name,
        dry_run: false,
        stash_conflict,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Check if the repository is already in worktree layout.
fn is_worktree_layout(git: &GitCommand) -> Result<bool> {
    if let Ok(Some(bare_value)) = git.config_get("core.bare") {
        if bare_value.to_lowercase() == "true" {
            let worktree_output = git.worktree_list_porcelain()?;
            let worktree_count = worktree_output
                .lines()
                .filter(|line| line.starts_with("worktree "))
                .count();
            return Ok(worktree_count > 0);
        }
    }
    Ok(false)
}

/// Move all files (except .git) via a staging directory to handle path conflicts.
fn move_files_via_staging(
    project_root: &Path,
    git_dir: &Path,
    worktree_path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let entries_to_move: Vec<PathBuf> = fs::read_dir(project_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|name| name != ".git")
                .unwrap_or(false)
        })
        .collect();

    if entries_to_move.is_empty() {
        progress.on_step("No files to move (empty repository)");
    } else {
        progress.on_step(&format!(
            "Moving {} items to worktree...",
            entries_to_move.len()
        ));
    }

    let staging_dir = git_dir.join("daft-adopt-staging");
    fs::create_dir_all(&staging_dir).with_context(|| {
        format!(
            "Failed to create staging directory: {}",
            staging_dir.display()
        )
    })?;

    // Move to staging
    for entry in &entries_to_move {
        let file_name = entry.file_name().context("Could not get file name")?;
        let dest = staging_dir.join(file_name);
        fs::rename(entry, &dest).with_context(|| {
            format!(
                "Failed to move '{}' to staging: {}",
                entry.display(),
                dest.display()
            )
        })?;
    }

    // Create worktree directory
    fs::create_dir_all(worktree_path).with_context(|| {
        format!(
            "Failed to create worktree directory: {}",
            worktree_path.display()
        )
    })?;

    // Move from staging to worktree
    let staged_entries: Vec<PathBuf> = fs::read_dir(&staging_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();

    for entry in &staged_entries {
        let file_name = entry.file_name().context("Could not get file name")?;
        let dest = worktree_path.join(file_name);
        fs::rename(entry, &dest).with_context(|| {
            format!(
                "Failed to move '{}' to '{}'",
                entry.display(),
                dest.display()
            )
        })?;
    }

    fs::remove_dir(&staging_dir).ok();

    Ok(())
}

/// Register the worktree with git's worktree tracking.
fn register_worktree(
    git_dir: &Path,
    worktree_path: &Path,
    current_branch: &str,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step(&format!(
        "Registering worktree for branch '{current_branch}'..."
    ));

    let worktree_git_file = worktree_path.join(".git");

    // Create .git file pointing to worktrees subdirectory
    let worktree_name = current_branch.replace('/', "-");
    let worktrees_dir = git_dir.join("worktrees").join(&worktree_name);
    fs::create_dir_all(&worktrees_dir).context("Failed to create worktrees directory")?;

    // Write gitdir file
    let gitdir_path = worktrees_dir.join("gitdir");
    fs::write(&gitdir_path, format!("{}\n", worktree_git_file.display()))
        .context("Failed to write gitdir file")?;

    // Write HEAD file
    let head_path = worktrees_dir.join("HEAD");
    fs::write(&head_path, format!("ref: refs/heads/{current_branch}\n"))
        .context("Failed to write HEAD file")?;

    // Write commondir file
    let commondir_path = worktrees_dir.join("commondir");
    fs::write(&commondir_path, "../..\n").context("Failed to write commondir file")?;

    // Update .git file in worktree
    let correct_gitdir = format!("gitdir: {}", worktrees_dir.display());
    fs::write(&worktree_git_file, correct_gitdir)
        .context("Failed to update .git file in worktree")?;

    Ok(())
}
