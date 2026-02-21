//! Core logic for the `git-worktree-prune` command.
//!
//! Removes worktrees and branches for deleted remote branches.

use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::{get_default_branch_local, remote_branch_exists};
use crate::settings::PruneCdTarget;
use crate::{get_git_common_dir, get_project_root};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Input parameters for the prune operation.
pub struct PruneParams {
    /// Force removal of worktrees with uncommitted changes.
    pub force: bool,
    /// Whether to use gitoxide.
    pub use_gitoxide: bool,
    /// Whether output is in quiet mode.
    pub is_quiet: bool,
    /// Remote name (from settings).
    pub remote_name: String,
    /// Where to cd after pruning the current worktree.
    pub prune_cd_target: PruneCdTarget,
}

/// Result of a prune operation.
pub struct PruneResult {
    pub remote_name: String,
    pub remote_url: Option<String>,
    pub branches_deleted: u32,
    pub worktrees_removed: u32,
    pub has_prunable: bool,
    /// Where to cd if the current worktree was removed.
    pub cd_target: Option<PathBuf>,
    /// True if no branches were found to prune.
    pub nothing_to_prune: bool,
}

/// Parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    is_bare: bool,
}

/// Bundles common state used throughout the prune operation.
struct PruneContext<'a> {
    git: &'a GitCommand,
    project_root: PathBuf,
    git_dir: PathBuf,
    remote_name: String,
    source_worktree: PathBuf,
}

/// Result of removing a single worktree + deleting its branch.
struct SinglePruneResult {
    worktree_removed: bool,
    branch_deleted: bool,
}

/// Execute the prune operation.
pub fn execute(
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<PruneResult> {
    let git = GitCommand::new(params.is_quiet).with_gitoxide(params.use_gitoxide);
    let ctx = PruneContext {
        git: &git,
        project_root: get_project_root()?,
        git_dir: get_git_common_dir()?,
        remote_name: params.remote_name.clone(),
        source_worktree: std::env::current_dir()?,
    };

    sink.on_step(&format!(
        "Fetching from remote {} and pruning stale remote-tracking branches...",
        ctx.remote_name
    ));
    git.fetch(&ctx.remote_name, true)
        .context("git fetch failed")?;

    // Parse worktree list once upfront
    let worktree_entries = parse_worktree_list(&git)?;
    let is_bare_layout = worktree_entries.first().map(|e| e.is_bare).unwrap_or(false);

    // Build a map: branch_name -> (worktree_path, is_main_worktree)
    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    // Identify gone branches
    let gone_branches = identify_gone_branches(
        &git,
        &worktree_map,
        &ctx.remote_name,
        params.use_gitoxide,
        sink,
    )?;

    let remote_url = git.remote_get_url(&ctx.remote_name).ok();

    if gone_branches.is_empty() {
        return Ok(PruneResult {
            remote_name: ctx.remote_name,
            remote_url,
            branches_deleted: 0,
            worktrees_removed: 0,
            has_prunable: false,
            cd_target: None,
            nothing_to_prune: true,
        });
    }

    sink.on_step(&format!(
        "Found {} branches to potentially prune",
        gone_branches.len()
    ));
    for branch in &gone_branches {
        sink.on_step(&format!(" - {branch}"));
    }

    // Detect current worktree context
    let current_wt_path = git.get_current_worktree_path().ok();
    let current_branch = git.symbolic_ref_short_head().ok();

    let mut branches_deleted: u32 = 0;
    let mut worktrees_removed: u32 = 0;
    let mut deferred_branch: Option<String> = None;

    for branch_name in &gone_branches {
        sink.on_step(&format!("Processing branch: {branch_name}"));

        let wt_info = worktree_map.get(branch_name.as_str()).cloned();

        match wt_info {
            Some((ref wt_path, true)) if !is_bare_layout => {
                // Branch is checked out in the main worktree of a regular repo
                process_main_worktree_branch(
                    &ctx,
                    wt_path,
                    branch_name,
                    &current_branch,
                    params,
                    sink,
                    &mut branches_deleted,
                    &mut worktrees_removed,
                )?;
            }
            Some((ref wt_path, _)) if !is_bare_layout => {
                // Linked worktree in a non-bare repo
                process_linked_worktree_branch(
                    &ctx,
                    wt_path,
                    branch_name,
                    &current_wt_path,
                    params.force,
                    sink,
                    &mut branches_deleted,
                    &mut worktrees_removed,
                    &mut deferred_branch,
                );
            }
            Some((ref wt_path, is_main)) => {
                // Bare layout
                process_bare_layout_branch(
                    &ctx,
                    wt_path,
                    branch_name,
                    is_main,
                    &current_wt_path,
                    params.force,
                    sink,
                    &mut branches_deleted,
                    &mut worktrees_removed,
                    &mut deferred_branch,
                );
            }
            None => {
                // No worktree for this branch
                sink.on_step(&format!("No associated worktree found for {branch_name}"));
                if delete_branch(&git, branch_name, sink) {
                    branches_deleted += 1;
                    sink.on_step(&format!(" * [pruned] {}/{branch_name}", ctx.remote_name));
                }
            }
        }
    }

    // Process deferred branch (user's current worktree) last
    let cd_target = process_deferred_branch(
        &ctx,
        &deferred_branch,
        &worktree_map,
        params,
        sink,
        &mut branches_deleted,
        &mut worktrees_removed,
    );

    // Check for prunable worktrees
    let worktree_list = git.worktree_list_porcelain()?;
    let has_prunable = worktree_list.contains("prunable");

    Ok(PruneResult {
        remote_name: ctx.remote_name,
        remote_url,
        branches_deleted,
        worktrees_removed,
        has_prunable,
        cd_target,
        nothing_to_prune: false,
    })
}

// ── Branch identification ──────────────────────────────────────────────────

/// Identify local branches whose upstream has been deleted.
fn identify_gone_branches(
    git: &GitCommand,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    remote_name: &str,
    use_gitoxide: bool,
    sink: &mut dyn ProgressSink,
) -> Result<Vec<String>> {
    sink.on_step("Identifying local branches whose upstream branch is gone...");
    let mut gone_branches = Vec::new();

    // Method 1: git branch -vv to find branches with gone upstream
    let branch_output = git.branch_list_verbose()?;
    for line in branch_output.lines() {
        if line.contains(": gone]") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let branch_name = match parts.first() {
                Some(&"*") | Some(&"+") => parts.get(1).copied(),
                _ => parts.first().copied(),
            };
            if let Some(name) = branch_name {
                if !name.is_empty() {
                    gone_branches.push(name.to_string());
                }
            }
        }
    }

    // Method 2: Check for branches with worktrees that don't exist on remote
    sink.on_step("Checking for branches with worktrees that don't exist on remote...");
    let ref_output = git.for_each_ref("%(refname:short)", "refs/heads")?;

    for line in ref_output.lines() {
        let branch_name = line.trim();
        if branch_name.is_empty() || branch_name == "master" || branch_name == "main" {
            continue;
        }

        if worktree_map.contains_key(branch_name)
            && !remote_branch_exists(remote_name, branch_name, use_gitoxide)?
            && !gone_branches.contains(&branch_name.to_string())
        {
            gone_branches.push(branch_name.to_string());
            sink.on_debug(&format!(
                "Found branch with worktree not on remote: {branch_name}"
            ));
        }
    }

    Ok(gone_branches)
}

// ── Per-branch processing ──────────────────────────────────────────────────

/// Process a branch checked out in the main worktree of a non-bare repo.
#[allow(clippy::too_many_arguments)]
fn process_main_worktree_branch(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    current_branch: &Option<String>,
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
    branches_deleted: &mut u32,
    worktrees_removed: &mut u32,
) -> Result<()> {
    sink.on_step(&format!(
        "Branch {branch_name} is checked out in the main worktree"
    ));

    let is_current = current_branch.as_deref() == Some(branch_name);
    let mut wt_removed = false;

    if is_current {
        match get_default_branch_local(&ctx.git_dir, &ctx.remote_name, params.use_gitoxide) {
            Ok(default_branch) => {
                sink.on_step(&format!("Checking out default branch {default_branch}..."));
                if let Err(e) = ctx.git.checkout(&default_branch) {
                    sink.on_warning(&format!(
                        "Failed to checkout {default_branch}: {e}. \
                         Skipping deletion of branch {branch_name}."
                    ));
                    return Ok(());
                }
            }
            Err(e) => {
                sink.on_warning(&format!(
                    "Cannot determine default branch: {e}. \
                     Skipping deletion of branch {branch_name}. \
                     Try: git remote set-head {} --auto",
                    ctx.remote_name
                ));
                return Ok(());
            }
        }
    } else {
        sink.on_step(&format!(
            "Branch {branch_name} has worktree at {} but is not checked out there; removing worktree",
            wt_path.display()
        ));
        if !remove_worktree(ctx, wt_path, branch_name, params.force, sink) {
            return Ok(());
        }
        wt_removed = true;
        *worktrees_removed += 1;
    }

    if delete_branch(ctx.git, branch_name, sink) {
        *branches_deleted += 1;
        let annotation = if wt_removed {
            " (worktree removed)"
        } else {
            ""
        };
        sink.on_step(&format!(
            " * [pruned] {}/{branch_name}{annotation}",
            ctx.remote_name
        ));
    }

    Ok(())
}

/// Process a linked worktree in a non-bare repo.
#[allow(clippy::too_many_arguments)]
fn process_linked_worktree_branch(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    current_wt_path: &Option<PathBuf>,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
    branches_deleted: &mut u32,
    worktrees_removed: &mut u32,
    deferred_branch: &mut Option<String>,
) {
    let is_current = current_wt_path
        .as_ref()
        .map(|p| p == wt_path)
        .unwrap_or(false);

    if is_current {
        sink.on_step(&format!(
            "Deferring {branch_name} (current worktree) to process last"
        ));
        *deferred_branch = Some(branch_name.to_string());
        return;
    }

    let result = remove_worktree_and_delete_branch(ctx, wt_path, branch_name, force, sink);
    if result.worktree_removed {
        *worktrees_removed += 1;
    }
    if result.branch_deleted {
        *branches_deleted += 1;
        let annotation = if result.worktree_removed {
            " (worktree removed)"
        } else {
            ""
        };
        sink.on_step(&format!(
            " * [pruned] {}/{branch_name}{annotation}",
            ctx.remote_name
        ));
    }
}

/// Process a branch in a bare-layout repo.
#[allow(clippy::too_many_arguments)]
fn process_bare_layout_branch(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    is_main: bool,
    current_wt_path: &Option<PathBuf>,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
    branches_deleted: &mut u32,
    worktrees_removed: &mut u32,
    deferred_branch: &mut Option<String>,
) {
    if is_main {
        // The first entry in a bare repo is the bare dir, not a real worktree
        sink.on_step(&format!("No associated worktree found for {branch_name}"));
        if delete_branch(ctx.git, branch_name, sink) {
            *branches_deleted += 1;
            sink.on_step(&format!(" * [pruned] {}/{branch_name}", ctx.remote_name));
        }
        return;
    }

    let is_current = current_wt_path
        .as_ref()
        .map(|p| p == wt_path)
        .unwrap_or(false);

    if is_current {
        sink.on_step(&format!(
            "Deferring {branch_name} (current worktree) to process last"
        ));
        *deferred_branch = Some(branch_name.to_string());
        return;
    }

    let result = remove_worktree_and_delete_branch(ctx, wt_path, branch_name, force, sink);
    if result.worktree_removed {
        *worktrees_removed += 1;
    }
    if result.branch_deleted {
        *branches_deleted += 1;
        let annotation = if result.worktree_removed {
            " (worktree removed)"
        } else {
            ""
        };
        sink.on_step(&format!(
            " * [pruned] {}/{branch_name}{annotation}",
            ctx.remote_name
        ));
    }
}

// ── Deferred branch ────────────────────────────────────────────────────────

/// Process the deferred branch (current worktree) after all others.
#[allow(clippy::too_many_arguments)]
fn process_deferred_branch(
    ctx: &PruneContext,
    deferred_branch: &Option<String>,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
    branches_deleted: &mut u32,
    worktrees_removed: &mut u32,
) -> Option<PathBuf> {
    let branch_name = deferred_branch.as_ref()?;

    sink.on_step(&format!(
        "Processing deferred branch: {branch_name} (current worktree)"
    ));

    let (wt_path, _) = worktree_map.get(branch_name.as_str())?;

    let cd_target = resolve_prune_cd_target(
        params.prune_cd_target,
        &ctx.project_root,
        &ctx.git_dir,
        &ctx.remote_name,
        params.use_gitoxide,
        sink,
    );

    if let Err(e) = std::env::set_current_dir(&cd_target) {
        sink.on_warning(&format!(
            "Failed to change directory to {}: {e}. \
             Skipping removal of current worktree {branch_name}.",
            cd_target.display()
        ));
        return None;
    }

    let result = remove_worktree_and_delete_branch(ctx, wt_path, branch_name, params.force, sink);

    let mut deferred_cd = None;
    if result.worktree_removed {
        *worktrees_removed += 1;
        deferred_cd = Some(cd_target);
    }
    if result.branch_deleted {
        *branches_deleted += 1;
        let annotation = if result.worktree_removed {
            " (worktree removed)"
        } else {
            ""
        };
        sink.on_step(&format!(
            " * [pruned] {}/{branch_name}{annotation}",
            ctx.remote_name
        ));
    }

    deferred_cd
}

// ── Worktree operations ────────────────────────────────────────────────────

/// Remove a worktree (with hooks and dirty checks). Returns true on success.
fn remove_worktree(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
) -> bool {
    // Check for uncommitted changes
    if wt_path.exists() && !force {
        match ctx.git.has_uncommitted_changes_in(wt_path) {
            Ok(true) => {
                sink.on_warning(&format!(
                    "Skipping {branch_name}: worktree has uncommitted changes or untracked files (use --force to override)"
                ));
                return false;
            }
            Ok(false) => {}
            Err(e) => {
                sink.on_warning(&format!(
                    "Skipping {branch_name}: failed to check for uncommitted changes: {e} (use --force to override)"
                ));
                return false;
            }
        }
    }

    // Pre-remove hook
    run_removal_hook(HookType::PreRemove, ctx, wt_path, branch_name, sink);

    if wt_path.exists() {
        sink.on_step("Removing worktree...");
        if let Err(e) = ctx.git.worktree_remove(wt_path, force) {
            sink.on_warning(&format!(
                "Failed to remove worktree {}: {e}. Skipping deletion of branch {branch_name}.",
                wt_path.display()
            ));
            return false;
        }
        sink.on_step(&format!("Removed worktree '{branch_name}'"));
    } else {
        sink.on_warning(&format!(
            "Worktree directory {} not found. Attempting to force remove record.",
            wt_path.display()
        ));
        if let Err(e) = ctx.git.worktree_remove(wt_path, true) {
            sink.on_warning(&format!(
                "Failed to remove orphaned worktree record {}: {e}. Skipping deletion of branch {branch_name}.",
                wt_path.display()
            ));
            return false;
        }
        sink.on_step(&format!("Removed worktree '{branch_name}'"));
    }

    // Post-remove hook
    run_removal_hook(HookType::PostRemove, ctx, wt_path, branch_name, sink);

    // Clean up empty parent directories
    cleanup_empty_parent_dirs(&ctx.project_root, wt_path, sink);

    true
}

/// Remove a worktree and delete its branch.
fn remove_worktree_and_delete_branch(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
) -> SinglePruneResult {
    sink.on_step(&format!(
        "Found associated worktree for {branch_name} at: {}",
        wt_path.display()
    ));

    if !remove_worktree(ctx, wt_path, branch_name, force, sink) {
        return SinglePruneResult {
            worktree_removed: false,
            branch_deleted: false,
        };
    }

    let branch_deleted = delete_branch(ctx.git, branch_name, sink);

    SinglePruneResult {
        worktree_removed: true,
        branch_deleted,
    }
}

/// Run a pre-remove or post-remove hook for a worktree.
fn run_removal_hook(
    hook_type: HookType,
    ctx: &PruneContext,
    worktree_path: &Path,
    branch_name: &str,
    sink: &mut (impl ProgressSink + HookRunner),
) {
    let hook_ctx = HookContext::new(
        hook_type,
        "prune",
        &ctx.project_root,
        &ctx.git_dir,
        &ctx.remote_name,
        &ctx.source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::RemoteDeleted);

    if let Err(e) = sink.run_hook(&hook_ctx) {
        sink.on_warning(&format!(
            "{} hook failed for {branch_name}: {e}",
            match hook_type {
                HookType::PreRemove => "Pre-remove",
                HookType::PostRemove => "Post-remove",
                _ => "Hook",
            }
        ));
    }
}

// ── Branch operations ──────────────────────────────────────────────────────

/// Delete a local branch with force. Returns true on success.
fn delete_branch(git: &GitCommand, branch_name: &str, sink: &mut dyn ProgressSink) -> bool {
    sink.on_step(&format!("Deleting local branch {branch_name}..."));
    if let Err(e) = git.branch_delete(branch_name, true) {
        sink.on_warning(&format!("Failed to delete branch {branch_name}: {e}"));
        false
    } else {
        sink.on_step(&format!("Branch {branch_name} deleted"));
        true
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Parse `git worktree list --porcelain` into structured entries.
fn parse_worktree_list(git: &GitCommand) -> Result<Vec<WorktreeEntry>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut current_is_bare = false;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: current_branch.take(),
                    is_bare: current_is_bare,
                });
            }
            current_path = Some(PathBuf::from(worktree_path));
            current_branch = None;
            current_is_bare = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            current_is_bare = true;
        }
    }
    if let Some(path) = current_path.take() {
        entries.push(WorktreeEntry {
            path,
            branch: current_branch.take(),
            is_bare: current_is_bare,
        });
    }

    Ok(entries)
}

/// Resolve where to cd after pruning the user's current worktree.
fn resolve_prune_cd_target(
    cd_target: PruneCdTarget,
    project_root: &Path,
    git_dir: &Path,
    remote_name: &str,
    use_gitoxide: bool,
    sink: &mut dyn ProgressSink,
) -> PathBuf {
    match cd_target {
        PruneCdTarget::Root => project_root.to_path_buf(),
        PruneCdTarget::DefaultBranch => {
            match get_default_branch_local(git_dir, remote_name, use_gitoxide) {
                Ok(default_branch) => {
                    let branch_dir = project_root.join(&default_branch);
                    if branch_dir.is_dir() {
                        branch_dir
                    } else {
                        sink.on_step(&format!(
                            "Default branch worktree directory '{}' not found, falling back to project root",
                            branch_dir.display()
                        ));
                        project_root.to_path_buf()
                    }
                }
                Err(e) => {
                    sink.on_warning(&format!(
                        "Cannot determine default branch for cd target: {e}. Falling back to project root."
                    ));
                    project_root.to_path_buf()
                }
            }
        }
    }
}

/// Clean up empty parent directories after removing a worktree.
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
