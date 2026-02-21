//! Core logic for the `git-worktree-flow-eject` command.
//!
//! Converts a worktree-based repository back to traditional layout.

use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::get_default_branch_from_remote_head;
use crate::utils::*;
use crate::{get_git_common_dir, is_git_repository};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

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

/// Parsed worktree information from `git worktree list --porcelain`.
#[derive(Debug, Clone)]
struct WorktreeInfo {
    path: PathBuf,
    branch: Option<String>,
    is_bare: bool,
}

/// Execute the eject operation.
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

    if !is_worktree_layout(&git)? {
        anyhow::bail!(
            "Repository is not in worktree layout.\n\
             Use git-worktree-flow-adopt to convert a traditional repository to worktree layout."
        );
    }

    // Parse worktrees
    let worktrees = parse_worktrees(&git)?;
    let non_bare_worktrees: Vec<_> = worktrees.iter().filter(|wt| !wt.is_bare).collect();

    if non_bare_worktrees.is_empty() {
        anyhow::bail!(
            "No worktrees found. Cannot convert to traditional layout without at least one worktree."
        );
    }

    sink.on_step(&format!("Found {} worktrees", non_bare_worktrees.len()));
    for wt in &non_bare_worktrees {
        let branch_display = wt.branch.as_deref().unwrap_or("(detached)");
        sink.on_step(&format!("  - {} ({})", wt.path.display(), branch_display));
    }

    // Determine target branch and worktree
    let (target_branch, target_worktree) = resolve_target_worktree(
        &params.branch,
        &non_bare_worktrees,
        &params.remote_name,
        params.use_gitoxide,
    )?;

    sink.on_step(&format!("Target branch to keep: '{target_branch}'"));
    sink.on_step(&format!(
        "Target worktree: '{}'",
        target_worktree.path.display()
    ));

    // Check for dirty worktrees (excluding target)
    check_dirty_worktrees(&git, &non_bare_worktrees, &target_worktree, params.force)?;

    // Check if target worktree has changes
    let prev_dir = get_current_directory()?;
    change_directory(&target_worktree.path)?;
    let target_has_changes = git.has_uncommitted_changes()?;
    change_directory(&prev_dir)?;

    if target_has_changes {
        sink.on_step("Target worktree has uncommitted changes - will preserve them");
    }

    if params.dry_run {
        report_dry_run(&non_bare_worktrees, &target_worktree, &project_root, sink);

        return Ok(EjectResult {
            project_root,
            target_branch,
            dry_run: true,
            stash_conflict: false,
        });
    }

    // Stash changes in target worktree if any
    if target_has_changes {
        sink.on_step("Stashing changes in target worktree...");
        change_directory(&target_worktree.path)?;
        git.stash_push_with_untracked("daft-flow-eject: temporary stash for conversion")
            .context("Failed to stash changes")?;
        change_directory(&project_root)?;
    }

    // Remove non-target worktrees (with hooks)
    remove_worktrees(
        &git,
        &non_bare_worktrees,
        &target_worktree,
        &project_root,
        &git_dir,
        &params.remote_name,
        params.force,
        sink,
    )?;

    // Move files from target worktree to project root
    move_files_via_staging(&target_worktree.path, &project_root, &git_dir, sink)?;

    // Remove worktree registrations
    let worktrees_dir = git_dir.join("worktrees");
    if worktrees_dir.exists() {
        sink.on_step("Cleaning up worktree registrations...");
        fs::remove_dir_all(&worktrees_dir).ok();
    }

    // Convert to non-bare
    sink.on_step("Converting to non-bare repository...");
    git.config_set("core.bare", "false")
        .context("Failed to set core.bare to false")?;

    // Set HEAD and reset index
    initialize_index(&project_root, &target_branch, sink)?;

    // Restore stashed changes
    let stash_conflict = if target_has_changes {
        sink.on_step("Restoring uncommitted changes...");
        if let Err(e) = git.stash_pop() {
            sink.on_warning(&format!("Could not restore stashed changes: {e}"));
            sink.on_warning("Your changes are still in the stash. Run 'git stash pop' manually.");
            true
        } else {
            false
        }
    } else {
        false
    };

    change_directory(&project_root)?;

    Ok(EjectResult {
        project_root,
        target_branch,
        dry_run: false,
        stash_conflict,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Check if the repository is in worktree layout.
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

/// Parse `git worktree list --porcelain` output into structured data.
fn parse_worktrees(git: &GitCommand) -> Result<Vec<WorktreeInfo>> {
    let output = git.worktree_list_porcelain()?;
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                worktrees.push(WorktreeInfo {
                    path,
                    branch: current_branch.take(),
                    is_bare,
                });
            }
            current_path = Some(PathBuf::from(path_str));
            current_branch = None;
            is_bare = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // Don't forget the last worktree
    if let Some(path) = current_path.take() {
        worktrees.push(WorktreeInfo {
            path,
            branch: current_branch.take(),
            is_bare,
        });
    }

    Ok(worktrees)
}

/// Determine which branch/worktree to keep.
fn resolve_target_worktree(
    branch: &Option<String>,
    non_bare_worktrees: &[&WorktreeInfo],
    remote_name: &str,
    use_gitoxide: bool,
) -> Result<(String, WorktreeInfo)> {
    let find_worktree = |branch: &str| -> Option<WorktreeInfo> {
        non_bare_worktrees
            .iter()
            .find(|wt| wt.branch.as_ref().is_some_and(|b| b == branch))
            .map(|wt| (*wt).clone())
    };

    if let Some(ref branch) = branch {
        match find_worktree(branch) {
            Some(wt) => Ok((branch.clone(), wt)),
            None => {
                let available: Vec<_> = non_bare_worktrees
                    .iter()
                    .filter_map(|wt| wt.branch.as_ref())
                    .collect();
                anyhow::bail!(
                    "No worktree found for branch '{}'. Available branches: {}",
                    branch,
                    available
                        .iter()
                        .map(|b| format!("'{b}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    } else {
        let default_branch = get_default_branch_from_remote_head(remote_name, use_gitoxide).ok();

        // Try remote default, then main, then master, then first available
        let candidates: Vec<Option<&str>> =
            vec![default_branch.as_deref(), Some("main"), Some("master")];

        for candidate in candidates.into_iter().flatten() {
            if let Some(wt) = find_worktree(candidate) {
                return Ok((candidate.to_string(), wt));
            }
        }

        // Fall back to first available
        let first_wt = non_bare_worktrees.first().unwrap();
        let branch = first_wt
            .branch
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        Ok((branch, (*first_wt).clone()))
    }
}

/// Check that no non-target worktrees have uncommitted changes (unless --force).
fn check_dirty_worktrees(
    git: &GitCommand,
    non_bare_worktrees: &[&WorktreeInfo],
    target_worktree: &WorktreeInfo,
    force: bool,
) -> Result<()> {
    let mut dirty_worktrees = Vec::new();

    for wt in non_bare_worktrees {
        if wt.path == target_worktree.path {
            continue;
        }
        let prev_dir = get_current_directory()?;
        if change_directory(&wt.path).is_ok() && git.has_uncommitted_changes().unwrap_or(false) {
            dirty_worktrees.push(*wt);
        }
        change_directory(&prev_dir).ok();
    }

    if !dirty_worktrees.is_empty() && !force {
        let dirty_list: Vec<String> = dirty_worktrees
            .iter()
            .map(|wt| {
                let branch = wt.branch.as_deref().unwrap_or("(detached)");
                format!("  - {} ({})", wt.path.display(), branch)
            })
            .collect();

        anyhow::bail!(
            "The following worktrees have uncommitted changes:\n{}\n\n\
             Use --force to delete these worktrees anyway (changes will be lost!).\n\
             Or commit/stash changes in these worktrees first.",
            dirty_list.join("\n")
        );
    }

    Ok(())
}

/// Report what the dry run would do.
fn report_dry_run(
    non_bare_worktrees: &[&WorktreeInfo],
    target_worktree: &WorktreeInfo,
    project_root: &Path,
    sink: &mut dyn ProgressSink,
) {
    sink.on_step("[DRY RUN] Would perform the following actions:");
    for wt in non_bare_worktrees {
        if wt.path != target_worktree.path {
            let branch = wt.branch.as_deref().unwrap_or("(detached)");
            sink.on_step(&format!(
                "[DRY RUN] Remove worktree '{}' ({})",
                wt.path.display(),
                branch
            ));
        }
    }
    sink.on_step(&format!(
        "[DRY RUN] Move files from '{}' to '{}'",
        target_worktree.path.display(),
        project_root.display()
    ));
    sink.on_step("[DRY RUN] Convert to non-bare repository");
}

/// Remove all worktrees except the target, running pre/post-remove hooks.
#[allow(clippy::too_many_arguments)]
fn remove_worktrees(
    git: &GitCommand,
    non_bare_worktrees: &[&WorktreeInfo],
    target_worktree: &WorktreeInfo,
    project_root: &Path,
    git_dir: &Path,
    remote_name: &str,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<()> {
    for wt in non_bare_worktrees {
        if wt.path == target_worktree.path {
            continue;
        }

        let branch = wt.branch.as_deref().unwrap_or("unknown");

        // Pre-remove hook
        let pre_ctx = HookContext::new(
            HookType::PreRemove,
            "eject",
            project_root,
            git_dir,
            remote_name,
            &target_worktree.path,
            &wt.path,
            branch,
        )
        .with_removal_reason(RemovalReason::Ejecting);

        if let Err(e) = sink.run_hook(&pre_ctx) {
            sink.on_warning(&format!("Pre-remove hook failed for {branch}: {e}"));
        }

        sink.on_step(&format!(
            "Removing worktree '{}' ({})...",
            wt.path.display(),
            branch
        ));

        if let Err(e) = git.worktree_remove(&wt.path, force) {
            sink.on_warning(&format!(
                "Failed to remove worktree '{}': {}",
                wt.path.display(),
                e
            ));
            // Try to clean up directory manually
            if wt.path.exists() {
                if let Err(e) = fs::remove_dir_all(&wt.path) {
                    sink.on_warning(&format!("Could not remove worktree directory: {e}"));
                }
            }
        } else {
            sink.on_step(&format!("Removed worktree '{branch}'"));
        }

        // Post-remove hook
        let post_ctx = HookContext::new(
            HookType::PostRemove,
            "eject",
            project_root,
            git_dir,
            remote_name,
            &target_worktree.path,
            &wt.path,
            branch,
        )
        .with_removal_reason(RemovalReason::Ejecting);

        if let Err(e) = sink.run_hook(&post_ctx) {
            sink.on_warning(&format!("Post-remove hook failed for {branch}: {e}"));
        }
    }

    Ok(())
}

/// Move files from the target worktree to the project root via a staging directory.
fn move_files_via_staging(
    worktree_path: &Path,
    project_root: &Path,
    git_dir: &Path,
    sink: &mut dyn ProgressSink,
) -> Result<()> {
    sink.on_step(&format!(
        "Moving files from '{}' to '{}'...",
        worktree_path.display(),
        project_root.display()
    ));

    let entries_to_move: Vec<PathBuf> = fs::read_dir(worktree_path)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .map(|name| name != ".git")
                .unwrap_or(false)
        })
        .collect();

    let staging_dir = git_dir.join("daft-eject-staging");
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

    // Remove .git file from worktree
    let worktree_git_file = worktree_path.join(".git");
    if worktree_git_file.exists() {
        fs::remove_file(&worktree_git_file).ok();
    }

    // Remove the now-empty worktree directory
    if worktree_path.exists() {
        sink.on_step(&format!(
            "Removing worktree directory '{}'...",
            worktree_path.display()
        ));
        fs::remove_dir_all(worktree_path).ok();
    }

    // Move from staging to project root
    let staged_entries: Vec<PathBuf> = fs::read_dir(&staging_dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();

    for entry in &staged_entries {
        let file_name = entry.file_name().context("Could not get file name")?;
        let dest = project_root.join(file_name);
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

/// Set HEAD to the target branch and reset the index.
fn initialize_index(
    project_root: &Path,
    target_branch: &str,
    sink: &mut dyn ProgressSink,
) -> Result<()> {
    sink.on_step(&format!("Setting up index for branch '{target_branch}'..."));

    let head_result = std::process::Command::new("git")
        .args([
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{target_branch}"),
        ])
        .current_dir(project_root)
        .output()
        .context("Failed to set HEAD")?;

    if !head_result.status.success() {
        let stderr = String::from_utf8_lossy(&head_result.stderr);
        sink.on_warning(&format!("git symbolic-ref warning: {}", stderr.trim()));
    }

    let reset_result = std::process::Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(project_root)
        .output()
        .context("Failed to reset index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        sink.on_warning(&format!("git reset warning: {}", stderr.trim()));
    }

    Ok(())
}
