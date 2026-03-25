//! Core layout transformation operations.
//!
//! Contains the low-level git operations for converting between bare (worktree)
//! and non-bare (traditional) repository layouts. These functions are the
//! canonical implementation — both `daft layout transform` and the deprecated
//! `daft adopt`/`daft eject` commands delegate here.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::move_hooks::{run_setup_hooks, run_teardown_hooks, MoveHookParams};
use crate::hooks::tracking::TrackedAttribute;
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::get_default_branch_from_remote_head;
use crate::settings::DaftSettings;
use crate::utils::*;
use crate::{get_current_branch, get_git_common_dir};

// ── Convert to bare ────────────────────────────────────────────────────────

/// Parameters for converting a non-bare repo to bare (worktree layout).
pub struct ConvertToBareParams {
    pub use_gitoxide: bool,
}

/// Result of converting to bare layout.
pub struct ConvertToBareResult {
    pub project_root: PathBuf,
    pub git_dir: PathBuf,
    pub worktree_path: PathBuf,
    pub current_branch: String,
    pub remote_name: String,
    pub repo_display_name: String,
    pub stash_conflict: bool,
}

/// Convert a non-bare repository to bare (worktree) layout.
///
/// Assumes the caller has already validated that:
/// - The current directory is inside a valid git repository
/// - The repository is not already in bare worktree layout
/// - The current directory is the main repo root (not a linked worktree)
pub fn convert_to_bare(
    params: &ConvertToBareParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<ConvertToBareResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    let current_branch = get_current_branch().context("Could not determine current branch")?;
    sink.on_step(&format!("Current branch: '{current_branch}'"));

    let git_dir = get_git_common_dir()?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    sink.on_step(&format!("Project root: '{}'", project_root.display()));

    let has_changes = git.has_uncommitted_changes()?;
    if has_changes {
        sink.on_step("Uncommitted changes detected - will preserve them");
    }

    let worktree_path = project_root.join(&current_branch);

    let settings = DaftSettings::load_global()?;
    let repo_display_name = project_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repository")
        .to_string();

    // Stash changes
    if has_changes {
        sink.on_step("Stashing uncommitted changes...");
        git.stash_push_with_untracked("daft-layout-transform: temporary stash for conversion")
            .context("Failed to stash changes")?;
    }

    change_directory(&project_root)?;

    // Collect linked worktree paths so we can exclude them from the move.
    // Linked worktrees (e.g., .worktrees/test/) must stay in place — they'll
    // be relocated separately by relocate_worktrees() after the conversion.
    let linked_wt_paths: Vec<PathBuf> = parse_worktrees(&git)?
        .into_iter()
        .filter(|wt| !wt.is_bare && wt.path != project_root)
        .map(|wt| wt.path)
        .collect();

    // Run teardown hooks before the filesystem relocation
    let move_params = MoveHookParams {
        old_worktree_path: project_root.clone(),
        new_worktree_path: worktree_path.clone(),
        old_branch_name: current_branch.clone(),
        new_branch_name: current_branch.clone(),
        project_root: project_root.clone(),
        git_dir: git_dir.clone(),
        remote: settings.remote.clone(),
        source_worktree: project_root.clone(),
        command: "adopt".to_string(),
        changed_attributes: HashSet::from([TrackedAttribute::Path]),
    };
    run_teardown_hooks(&move_params, sink);

    // Move files via staging directory
    move_files_to_worktree(
        &project_root,
        &git_dir,
        &worktree_path,
        &linked_wt_paths,
        sink,
    )?;

    // Run setup hooks after the filesystem relocation
    run_setup_hooks(&move_params, sink);

    // Convert to bare
    sink.on_step("Converting to bare repository...");
    git.config_set("core.bare", "true")
        .context("Failed to set core.bare")?;

    let bare_index = git_dir.join("index");
    if bare_index.exists() {
        fs::remove_file(&bare_index).ok();
    }

    // Setup fetch refspec
    sink.on_step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&settings.remote) {
        sink.on_warning(&format!("Could not set fetch refspec: {e}"));
    }

    // Register worktree
    register_worktree(&git_dir, &worktree_path, &current_branch, sink)?;

    change_directory(&worktree_path)?;

    // Initialize index
    sink.on_step("Initializing worktree index...");
    let reset_result = std::process::Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(&worktree_path)
        .output()
        .context("Failed to initialize worktree index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        sink.on_warning(&format!("git reset warning: {}", stderr.trim()));
    }

    // Restore stashed changes
    let stash_conflict = if has_changes {
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

    Ok(ConvertToBareResult {
        project_root,
        git_dir,
        worktree_path,
        current_branch,
        remote_name: settings.remote,
        repo_display_name,
        stash_conflict,
    })
}

// ── Convert to non-bare ────────────────────────────────────────────────────

/// Parameters for converting a bare repo to non-bare (traditional layout).
pub struct ConvertToNonBareParams {
    /// Branch to keep (None = auto-detect default).
    pub branch: Option<String>,
    /// Force deletion of dirty worktrees.
    pub force: bool,
    pub use_gitoxide: bool,
    pub is_quiet: bool,
    pub remote_name: String,
}

/// Result of converting to non-bare layout.
pub struct ConvertToNonBareResult {
    pub project_root: PathBuf,
    pub target_branch: String,
    pub stash_conflict: bool,
}

/// Convert a bare (worktree) repository to non-bare (traditional) layout.
///
/// Assumes the caller has already validated that:
/// - The current directory is inside a valid git repository
/// - The repository is in bare worktree layout
pub fn convert_to_non_bare(
    params: &ConvertToNonBareParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<ConvertToNonBareResult> {
    let git_dir = get_git_common_dir().context("Not inside a Git repository")?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;
    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    change_directory(&project_root)?;

    let git = GitCommand::new(params.is_quiet).with_gitoxide(params.use_gitoxide);

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

    // Stash changes in target worktree if any
    if target_has_changes {
        sink.on_step("Stashing changes in target worktree...");
        change_directory(&target_worktree.path)?;
        git.stash_push_with_untracked("daft-layout-transform: temporary stash for conversion")
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
    move_files_from_worktree(&target_worktree.path, &project_root, &git_dir, sink)?;

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

    Ok(ConvertToNonBareResult {
        project_root,
        target_branch,
        stash_conflict,
    })
}

// ── Collapse bare to non-bare (for layout transform) ───────────────────────

/// Parameters for collapsing a bare repo to non-bare during layout transform.
pub struct CollapseBareParams {
    pub use_gitoxide: bool,
    pub remote_name: String,
}

/// Result of collapsing a bare repo to non-bare.
pub struct CollapseBareResult {
    pub project_root: PathBuf,
    pub default_branch: String,
}

/// Collapse a bare (worktree) repo into a non-bare repo, keeping linked worktrees.
///
/// Unlike [`convert_to_non_bare`] (which removes all non-target worktrees for
/// the eject command), this function preserves every linked worktree. Only the
/// default branch's worktree is dissolved — its files become the main working
/// tree of the now-non-bare repo.
///
/// Handles both clone-style bare repos (git metadata at root) and adopt-style
/// bare repos (git metadata in `.git/` subdirectory).
pub fn collapse_bare_to_non_bare(
    params: &CollapseBareParams,
    progress: &mut dyn ProgressSink,
) -> Result<CollapseBareResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);
    let git_common_dir = get_git_common_dir()?;
    let git_common_dir = std::fs::canonicalize(&git_common_dir).with_context(|| {
        format!(
            "Could not canonicalize git dir: {}",
            git_common_dir.display()
        )
    })?;

    // Determine repo structure: clone-style bare (metadata at root) vs
    // adopt-style bare (metadata in .git/ subdirectory).
    let is_clone_style = git_common_dir.file_name().is_none_or(|n| n != ".git");
    let project_root = if is_clone_style {
        // Clone-style: git_common_dir IS the project root
        git_common_dir.clone()
    } else {
        // Adopt-style: git_common_dir is <root>/.git, parent is project root
        git_common_dir
            .parent()
            .context("Could not determine project root")?
            .to_path_buf()
    };

    change_directory(&project_root)?;

    // Parse worktrees to find the default branch worktree
    let worktrees = parse_worktrees(&git)?;
    let non_bare_worktrees: Vec<_> = worktrees.iter().filter(|wt| !wt.is_bare).collect();

    if non_bare_worktrees.is_empty() {
        anyhow::bail!(
            "No worktrees found. Cannot convert to non-bare without at least one worktree."
        );
    }

    // Pick the default branch worktree (same logic as resolve_target_worktree)
    let default_branch = crate::remote::get_default_branch_local(
        &git_common_dir,
        &params.remote_name,
        params.use_gitoxide,
    )
    .unwrap_or_else(|_| "main".to_string());
    let find_wt = |name: &str| -> Option<&WorktreeInfo> {
        non_bare_worktrees
            .iter()
            .find(|wt| wt.branch.as_deref() == Some(name))
            .copied()
    };
    let (target_branch, target_wt) = [Some(default_branch.as_str()), Some("main"), Some("master")]
        .into_iter()
        .flatten()
        .find_map(|b| find_wt(b).map(|wt| (b.to_string(), wt)))
        .unwrap_or_else(|| {
            let wt = non_bare_worktrees[0];
            let b = wt.branch.clone().unwrap_or_else(|| "unknown".to_string());
            (b, wt)
        });

    progress.on_step(&format!("Default branch: '{target_branch}'"));

    // For clone-style bare repos, restructure git metadata into .git/ subdir
    if is_clone_style {
        progress.on_step("Restructuring bare repository...");
        restructure_bare_to_dotgit(&project_root, &worktrees, progress)?;
    }

    let git_dir = project_root.join(".git");

    // Move default worktree files to project root via staging
    progress.on_step(&format!(
        "Moving '{}' files to project root...",
        target_branch
    ));
    move_files_from_worktree(&target_wt.path, &project_root, &git_dir, progress)?;

    // Remove only the default worktree's registration (keep others)
    let wt_name = target_branch.replace('/', "-");
    let wt_reg = git_dir.join("worktrees").join(&wt_name);
    if wt_reg.exists() {
        progress.on_step(&format!(
            "Removing worktree registration for '{target_branch}'..."
        ));
        fs::remove_dir_all(&wt_reg).ok();
    }

    // If no more worktree registrations, clean up the worktrees/ dir
    let worktrees_dir = git_dir.join("worktrees");
    if worktrees_dir.exists()
        && worktrees_dir
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
    {
        fs::remove_dir(&worktrees_dir).ok();
    }

    // Convert to non-bare
    progress.on_step("Setting core.bare=false...");
    // Need a new GitCommand since we restructured the repo
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);
    git.config_set("core.bare", "false")
        .context("Failed to set core.bare to false")?;

    // Set HEAD and reset index
    initialize_index(&project_root, &target_branch, progress)?;

    Ok(CollapseBareResult {
        project_root,
        default_branch: target_branch,
    })
}

/// Restructure a clone-style bare repo by moving git metadata into `.git/`.
///
/// Identifies worktree directories (from `git worktree list`) and moves
/// everything else (HEAD, config, refs/, objects/, etc.) into a new `.git/`
/// subdirectory. Updates each worktree's `.git` file to point to the new
/// worktrees registration path.
fn restructure_bare_to_dotgit(
    project_root: &Path,
    worktrees: &[WorktreeInfo],
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let dotgit = project_root.join(".git");
    fs::create_dir(&dotgit).context("Failed to create .git directory")?;

    // Collect worktree directory paths (canonicalized for comparison)
    let wt_paths: Vec<PathBuf> = worktrees
        .iter()
        .filter(|wt| !wt.is_bare)
        .map(|wt| wt.path.canonicalize().unwrap_or_else(|_| wt.path.clone()))
        .collect();

    // Move everything except .git/ and worktree directories into .git/
    for entry in fs::read_dir(project_root)? {
        let entry = entry?;
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip the .git dir we just created
        if name == ".git" {
            continue;
        }

        // Skip worktree directories
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if wt_paths.contains(&canonical) {
            continue;
        }

        let dest = dotgit.join(&name);
        fs::rename(&path, &dest)
            .with_context(|| format!("Failed to move '{}' into .git/: {}", name, path.display()))?;
    }

    // Update each worktree's .git file to point to new location
    for wt in worktrees.iter().filter(|wt| !wt.is_bare) {
        let gitlink = wt.path.join(".git");
        if gitlink.exists() {
            let content = fs::read_to_string(&gitlink)?;
            if let Some(old_path) = content.trim().strip_prefix("gitdir: ") {
                // Insert .git/ segment: <root>/worktrees/<name> → <root>/.git/worktrees/<name>
                let old_path = PathBuf::from(old_path);
                if let Ok(rel) = old_path.strip_prefix(project_root) {
                    let new_path = project_root.join(".git").join(rel);
                    fs::write(&gitlink, format!("gitdir: {}", new_path.display())).with_context(
                        || format!("Failed to update .git file in {}", wt.path.display()),
                    )?;
                }
            }
        }
    }

    progress.on_step("Moved git metadata into .git/ subdirectory");

    Ok(())
}

// ── Shared utilities ───────────────────────────────────────────────────────

/// Check if the repository is in bare worktree layout (core.bare=true + worktrees).
pub fn is_bare_worktree_layout(git: &GitCommand) -> Result<bool> {
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

/// Parsed worktree information from `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_bare: bool,
}

/// Parse `git worktree list --porcelain` output into structured data.
pub fn parse_worktrees(git: &GitCommand) -> Result<Vec<WorktreeInfo>> {
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

// ── Private helpers ────────────────────────────────────────────────────────

/// Move all files (except .git) from project root to a worktree subdirectory
/// via a staging directory to handle path conflicts.
fn move_files_to_worktree(
    project_root: &Path,
    git_dir: &Path,
    worktree_path: &Path,
    skip_paths: &[PathBuf],
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let entries_to_move: Vec<PathBuf> = fs::read_dir(project_root)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            // Skip .git directory
            if path.file_name().and_then(|n| n.to_str()) == Some(".git") {
                return false;
            }
            // Skip directories that contain linked worktrees (they're
            // relocated separately). Check both directions: the entry could BE
            // a worktree path, or it could be an ancestor directory containing one.
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            !skip_paths.iter().any(|skip| {
                let skip_canonical = skip.canonicalize().unwrap_or_else(|_| skip.clone());
                canonical == skip_canonical
                    || canonical.starts_with(&skip_canonical)
                    || skip_canonical.starts_with(&canonical)
            })
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
pub(crate) fn register_worktree(
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

/// Determine which branch/worktree to keep during non-bare conversion.
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

/// Move files from the target worktree back to the project root via staging.
fn move_files_from_worktree(
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
