use crate::{
    config::counters::{INITIAL_BRANCHES_DELETED, INITIAL_WORKTREES_REMOVED, OPERATION_INCREMENT},
    get_git_common_dir, get_project_root,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, RemovalReason},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::{get_default_branch_local, remote_branch_exists},
    settings::PruneCdTarget,
    DaftSettings, WorktreeConfig, SHELL_WRAPPER_ENV,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "git-worktree-prune")]
#[command(version = crate::VERSION)]
#[command(about = "Remove worktrees and branches for deleted remote branches")]
#[command(long_about = r#"
Removes local branches whose corresponding remote tracking branches have been
deleted, along with any associated worktrees. This is useful for cleaning up
after branches have been merged and deleted on the remote.

The command first fetches from the remote with pruning enabled to update the
list of remote tracking branches. It then identifies local branches that were
tracking now-deleted remote branches, removes their worktrees (if any exist),
and finally deletes the local branches.

If you are currently inside a worktree that is about to be pruned, the command
handles this gracefully. In a bare-repo worktree layout (created by daft), the
current worktree is removed last and the shell is redirected to a safe location
(project root by default, or the default branch worktree if configured via
daft.prune.cdTarget). In a regular repository where the current branch is being
pruned, the command checks out the default branch before deleting the old branch.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    is_bare: bool,
}

/// Bundles common parameters used throughout the prune operation.
struct PruneContext<'a> {
    git: &'a GitCommand,
    project_root: PathBuf,
    git_dir: PathBuf,
    remote_name: String,
    source_worktree: PathBuf,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-prune"));

    // Initialize logging based on verbosity flag
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(false, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_prune(&mut output, &settings)?;
    Ok(())
}

fn run_prune(output: &mut dyn Output, settings: &DaftSettings) -> Result<()> {
    let config = WorktreeConfig::default();
    let git = GitCommand::new(output.is_quiet());
    let ctx = PruneContext {
        git: &git,
        project_root: get_project_root()?,
        git_dir: get_git_common_dir()?,
        remote_name: config.remote_name.clone(),
        source_worktree: std::env::current_dir()?,
    };

    output.step(&format!(
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

    output.step("Identifying local branches whose upstream branch is gone...");

    let mut gone_branches = Vec::new();

    // Method 1: Use git branch -vv to find branches with gone upstream
    let branch_output = git.branch_list_verbose()?;
    for line in branch_output.lines() {
        if line.contains(": gone]") {
            // Extract branch name from the line.
            // git branch -vv prefixes: '*' = current branch, '+' = checked out in linked worktree
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

    // Method 2: Also check for branches that don't exist on remote but have worktrees
    output.step("Checking for branches with worktrees that don't exist on remote...");
    let ref_output = git.for_each_ref("%(refname:short)", "refs/heads")?;

    for line in ref_output.lines() {
        let branch_name = line.trim();
        if branch_name.is_empty() || branch_name == "master" || branch_name == "main" {
            continue;
        }

        if worktree_map.contains_key(branch_name)
            && !remote_branch_exists(&ctx.remote_name, branch_name)?
            && !gone_branches.contains(&branch_name.to_string())
        {
            gone_branches.push(branch_name.to_string());
            output.debug(&format!(
                "Found branch with worktree not on remote: {branch_name}"
            ));
        }
    }

    if gone_branches.is_empty() {
        return Ok(());
    }

    output.step(&format!(
        "Found {} branches to potentially prune",
        gone_branches.len()
    ));
    for branch in &gone_branches {
        output.step(&format!(" - {branch}"));
    }

    // Detect current worktree context
    let current_wt_path = git.get_current_worktree_path().ok();
    let current_branch = git.symbolic_ref_short_head().ok();

    let mut branches_deleted = INITIAL_BRANCHES_DELETED;
    let mut worktrees_removed = INITIAL_WORKTREES_REMOVED;
    let mut deferred_branch: Option<String> = None;

    for branch_name in &gone_branches {
        output.step(&format!("Processing branch: {branch_name}"));

        let wt_info = worktree_map.get(branch_name.as_str()).cloned();

        match wt_info {
            Some((ref wt_path, true)) if !is_bare_layout => {
                // SCENARIO B: Branch is checked out in the main worktree of a regular repo.
                // We can't remove the main worktree, so checkout the default branch first.
                output.step(&format!(
                    "Branch {branch_name} is checked out in the main worktree"
                ));

                let is_current = current_branch.as_deref() == Some(branch_name.as_str());

                if is_current {
                    match get_default_branch_local(&ctx.git_dir, &ctx.remote_name) {
                        Ok(default_branch) => {
                            output
                                .step(&format!("Checking out default branch {default_branch}..."));
                            if let Err(e) = git.checkout(&default_branch) {
                                output.error(&format!(
                                    "Failed to checkout {default_branch}: {e}. \
                                     Skipping deletion of branch {branch_name}."
                                ));
                                continue;
                            }
                        }
                        Err(e) => {
                            output.error(&format!(
                                "Cannot determine default branch: {e}. \
                                 Skipping deletion of branch {branch_name}. \
                                 Try: git remote set-head {remote} --auto",
                                remote = ctx.remote_name
                            ));
                            continue;
                        }
                    }
                } else {
                    // The branch is in the main worktree but isn't current
                    // (shouldn't normally happen, but handle gracefully)
                    output.step(&format!(
                        "Branch {branch_name} has worktree at {} but is not checked out there; removing worktree",
                        wt_path.display()
                    ));
                    if !remove_worktree(&ctx, wt_path, branch_name, output) {
                        continue;
                    }
                    worktrees_removed += OPERATION_INCREMENT;
                }

                // Delete the branch (no worktree removal needed for Scenario B current branch)
                delete_branch(&git, branch_name, output, &mut branches_deleted);
            }
            Some((ref wt_path, _)) if !is_bare_layout => {
                // Linked worktree in a non-bare repo
                let is_current = current_wt_path
                    .as_ref()
                    .map(|p| p == wt_path)
                    .unwrap_or(false);

                if is_current {
                    output.step(&format!(
                        "Deferring {branch_name} (current worktree) to process last"
                    ));
                    deferred_branch = Some(branch_name.clone());
                    continue;
                }

                remove_worktree_and_delete_branch(
                    &ctx,
                    wt_path,
                    branch_name,
                    output,
                    &mut worktrees_removed,
                    &mut branches_deleted,
                );
            }
            Some((ref wt_path, is_main)) => {
                // Bare layout: all worktrees are "linked" except the bare dir itself
                // is_main in a bare layout means the bare .git dir entry (no real worktree)
                if is_main {
                    // The first entry in a bare repo is the bare dir, not a real worktree
                    output.step(&format!("No associated worktree found for {branch_name}"));
                    delete_branch(&git, branch_name, output, &mut branches_deleted);
                    continue;
                }

                let is_current = current_wt_path
                    .as_ref()
                    .map(|p| p == wt_path)
                    .unwrap_or(false);

                if is_current {
                    output.step(&format!(
                        "Deferring {branch_name} (current worktree) to process last"
                    ));
                    deferred_branch = Some(branch_name.clone());
                    continue;
                }

                remove_worktree_and_delete_branch(
                    &ctx,
                    wt_path,
                    branch_name,
                    output,
                    &mut worktrees_removed,
                    &mut branches_deleted,
                );
            }
            None => {
                // No worktree found for this branch
                output.step(&format!("No associated worktree found for {branch_name}"));
                delete_branch(&git, branch_name, output, &mut branches_deleted);
            }
        }
    }

    // Process deferred branch (user's current worktree) last.
    // Track the CD target so we can emit it as the very last output line
    // (the shell wrapper parses stdout for __DAFT_CD__).
    let mut deferred_cd_target: Option<PathBuf> = None;

    if let Some(ref branch_name) = deferred_branch {
        output.step(&format!(
            "Processing deferred branch: {branch_name} (current worktree)"
        ));

        if let Some((ref wt_path, _)) = worktree_map.get(branch_name.as_str()) {
            // Resolve the CD target and change working directory BEFORE removing
            // the worktree. Once the worktree is removed, the CWD is gone and
            // all subsequent git commands would fail with "Unable to read current
            // working directory".
            let cd_target = resolve_prune_cd_target(
                settings.prune_cd_target,
                &ctx.project_root,
                &ctx.git_dir,
                &ctx.remote_name,
                output,
            );

            if let Err(e) = std::env::set_current_dir(&cd_target) {
                output.error(&format!(
                    "Failed to change directory to {}: {e}. \
                     Skipping removal of current worktree {branch_name}.",
                    cd_target.display()
                ));
            } else {
                let removed = remove_worktree_and_delete_branch(
                    &ctx,
                    wt_path,
                    branch_name,
                    output,
                    &mut worktrees_removed,
                    &mut branches_deleted,
                );

                if removed {
                    deferred_cd_target = Some(cd_target);
                }
            }
        }
    }

    // Git-like result message
    if branches_deleted > 0 || worktrees_removed > 0 {
        output.result(&format!(
            "Pruned {} branches, removed {} worktrees",
            branches_deleted, worktrees_removed
        ));
    }

    // Check if any worktrees might need manual pruning
    let worktree_list = git.worktree_list_porcelain()?;
    if worktree_list.contains("prunable") {
        output.warning(
            "Some prunable worktree data may exist. Run 'git worktree prune' to clean up.",
        );
    }

    // Emit the CD marker as the very last output. The shell wrapper captures
    // all stdout and parses for __DAFT_CD__: lines to cd the parent shell.
    // When no shell wrapper is active, tell the user to cd manually.
    if let Some(ref cd_target) = deferred_cd_target {
        if std::env::var(SHELL_WRAPPER_ENV).is_ok() {
            output.cd_path(cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    Ok(())
}

/// Parse `git worktree list --porcelain` into structured entries.
fn parse_worktree_list(git: &GitCommand) -> Result<Vec<WorktreeEntry>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut current_is_bare = false;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            // Save previous entry if any
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
    // Don't forget the last entry
    if let Some(path) = current_path.take() {
        entries.push(WorktreeEntry {
            path,
            branch: current_branch.take(),
            is_bare: current_is_bare,
        });
    }

    Ok(entries)
}

/// Delete a local branch with force, updating counters.
fn delete_branch(
    git: &GitCommand,
    branch_name: &str,
    output: &mut dyn Output,
    branches_deleted: &mut u32,
) {
    output.step(&format!("Deleting local branch {branch_name}..."));
    if let Err(e) = git.branch_delete(branch_name, true) {
        output.error(&format!("Failed to delete branch {branch_name}: {e}"));
    } else {
        output.step(&format!("Branch {branch_name} deleted"));
        *branches_deleted += OPERATION_INCREMENT;
    }
}

/// Remove a worktree (with hooks) and return whether it was successful.
fn remove_worktree(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    output: &mut dyn Output,
) -> bool {
    // Run pre-remove hook
    if let Err(e) = run_hook(
        HookType::PreRemove,
        ctx,
        &wt_path.to_path_buf(),
        branch_name,
        output,
    ) {
        output.warning(&format!("Pre-remove hook failed for {branch_name}: {e}"));
    }

    if wt_path.exists() {
        output.step("Removing worktree...");
        if let Err(e) = ctx.git.worktree_remove(wt_path, true) {
            output.error(&format!(
                "Failed to remove worktree {}: {e}. Skipping deletion of branch {branch_name}.",
                wt_path.display()
            ));
            return false;
        }
        output.step(&format!("Worktree at {} removed", wt_path.display()));
    } else {
        output.warning(&format!(
            "Worktree directory {} not found. Attempting to force remove record.",
            wt_path.display()
        ));
        if let Err(e) = ctx.git.worktree_remove(wt_path, true) {
            output.error(&format!(
                "Failed to remove orphaned worktree record {}: {e}. Skipping deletion of branch {branch_name}.",
                wt_path.display()
            ));
            return false;
        }
        output.step(&format!(
            "Worktree record for {} removed",
            wt_path.display()
        ));
    }

    // Run post-remove hook
    if let Err(e) = run_hook(
        HookType::PostRemove,
        ctx,
        &wt_path.to_path_buf(),
        branch_name,
        output,
    ) {
        output.warning(&format!("Post-remove hook failed for {branch_name}: {e}"));
    }

    // Clean up empty parent directories
    cleanup_empty_parent_dirs(&ctx.project_root, wt_path, output);

    true
}

/// Remove a worktree and delete the associated branch.
/// Returns true if the worktree was successfully removed (branch deletion may still fail).
fn remove_worktree_and_delete_branch(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    output: &mut dyn Output,
    worktrees_removed: &mut u32,
    branches_deleted: &mut u32,
) -> bool {
    output.step(&format!(
        "Found associated worktree for {branch_name} at: {}",
        wt_path.display()
    ));

    if !remove_worktree(ctx, wt_path, branch_name, output) {
        return false;
    }
    *worktrees_removed += OPERATION_INCREMENT;

    delete_branch(ctx.git, branch_name, output, branches_deleted);

    true
}

/// Resolve where to cd after pruning the user's current worktree.
fn resolve_prune_cd_target(
    cd_target: PruneCdTarget,
    project_root: &Path,
    git_dir: &Path,
    remote_name: &str,
    output: &mut dyn Output,
) -> PathBuf {
    match cd_target {
        PruneCdTarget::Root => project_root.to_path_buf(),
        PruneCdTarget::DefaultBranch => match get_default_branch_local(git_dir, remote_name) {
            Ok(default_branch) => {
                let branch_dir = project_root.join(&default_branch);
                if branch_dir.is_dir() {
                    branch_dir
                } else {
                    output.step(&format!(
                        "Default branch worktree directory '{}' not found, falling back to project root",
                        branch_dir.display()
                    ));
                    project_root.to_path_buf()
                }
            }
            Err(e) => {
                output.warning(&format!(
                    "Cannot determine default branch for cd target: {e}. Falling back to project root."
                ));
                project_root.to_path_buf()
            }
        },
    }
}

/// Run a lifecycle hook (pre-remove or post-remove) for a worktree.
fn run_hook(
    hook_type: HookType,
    ctx: &PruneContext,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

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

    executor.execute(&hook_ctx, output)?;

    Ok(())
}

/// Clean up empty parent directories after removing a worktree.
///
/// Walks up the directory tree from the removed worktree's parent directory,
/// removing each directory if empty, until reaching the project root.
/// This handles branches with slashes (e.g., `feature/my-branch`) where
/// removing the worktree leaves empty intermediate directories.
fn cleanup_empty_parent_dirs(
    project_root: &std::path::Path,
    worktree_path: &std::path::Path,
    output: &mut dyn Output,
) {
    let mut current = worktree_path.parent();
    while let Some(dir) = current {
        // Stop at or above the project root
        if dir == project_root || !dir.starts_with(project_root) {
            break;
        }
        // fs::remove_dir only succeeds on empty directories
        match std::fs::remove_dir(dir) {
            Ok(()) => {
                output.step(&format!("Removed empty directory '{}'", dir.display()));
                current = dir.parent();
            }
            Err(_) => break,
        }
    }
}
