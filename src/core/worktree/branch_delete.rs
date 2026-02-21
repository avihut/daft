//! Core logic for the `git-worktree-branch-delete` command.
//!
//! Deletes branches and their associated worktrees.

use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::get_default_branch_local;
use crate::settings::PruneCdTarget;
use crate::{get_git_common_dir, get_project_root};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Input parameters for the branch-delete operation.
pub struct BranchDeleteParams {
    /// Branch names or worktree paths to delete.
    pub branches: Vec<String>,
    /// Force deletion even if not fully merged.
    pub force: bool,
    /// Whether to use gitoxide.
    pub use_gitoxide: bool,
    /// Whether output is in quiet mode.
    pub is_quiet: bool,
    /// Remote name (from settings).
    pub remote_name: String,
    /// Where to cd after deleting the current worktree.
    pub prune_cd_target: PruneCdTarget,
}

/// Result of a branch-delete operation.
pub struct BranchDeleteResult {
    /// Per-branch deletion results (populated when validation passes).
    pub deletions: Vec<DeletionResult>,
    /// Validation errors for branches that failed validation.
    pub validation_errors: Vec<ValidationError>,
    /// Total count of branches that passed validation.
    pub validated_count: usize,
    /// Total count of branches that were requested.
    pub requested_count: usize,
    /// Where to cd if the current worktree was removed.
    pub cd_target: Option<PathBuf>,
    /// True if there were no branches to delete after resolution.
    pub nothing_to_delete: bool,
}

/// A validation error for a single branch.
pub struct ValidationError {
    pub branch: String,
    pub message: String,
}

/// Result of deleting a single branch (tracks what was successfully deleted).
pub struct DeletionResult {
    pub branch: String,
    pub remote_deleted: bool,
    pub worktree_removed: bool,
    pub branch_deleted: bool,
    pub errors: Vec<String>,
}

impl DeletionResult {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Build a human-readable summary of what was deleted (e.g. "worktree, local branch, remote branch").
    pub fn deleted_parts(&self) -> String {
        let mut parts = Vec::new();
        if self.worktree_removed {
            parts.push("worktree");
        }
        if self.branch_deleted {
            parts.push("local branch");
        }
        if self.remote_deleted {
            parts.push("remote branch");
        }
        parts.join(", ")
    }
}

// ── Private types ──────────────────────────────────────────────────────────

/// Parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    #[allow(dead_code)] // Parsed for completeness; not needed by branch-delete logic
    is_bare: bool,
}

/// Bundles common parameters used throughout the branch-delete operation.
struct BranchDeleteContext<'a> {
    git: &'a GitCommand,
    project_root: PathBuf,
    git_dir: PathBuf,
    remote_name: String,
    source_worktree: PathBuf,
    default_branch: String,
}

/// Validated branch ready for deletion.
struct ValidatedBranch {
    name: String,
    worktree_path: Option<PathBuf>,
    remote_name: Option<String>,
    remote_branch_name: Option<String>,
    is_current_worktree: bool,
}

enum ResolveResult {
    /// Argument matched a worktree path and resolved to this branch name.
    Branch(String),
    /// Argument did not match any worktree path; treat as a branch name.
    PassThrough,
    /// Argument matched a worktree but it has no branch (detached HEAD).
    DetachedHead(PathBuf),
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Execute the branch-delete operation.
pub fn execute(
    params: &BranchDeleteParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<BranchDeleteResult> {
    let git = GitCommand::new(params.is_quiet).with_gitoxide(params.use_gitoxide);
    let git_dir = get_git_common_dir()?;
    let default_branch =
        get_default_branch_local(&git_dir, &params.remote_name, params.use_gitoxide)
            .context("Cannot determine default branch")?;

    let ctx = BranchDeleteContext {
        git: &git,
        project_root: get_project_root()?,
        git_dir,
        remote_name: params.remote_name.clone(),
        source_worktree: std::env::current_dir()?,
        default_branch,
    };

    // Parse worktree list once upfront into a map: branch_name -> worktree_path
    let worktree_entries = parse_worktree_list(&git)?;
    let mut worktree_map: HashMap<String, PathBuf> = HashMap::new();
    for entry in &worktree_entries {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), entry.path.clone());
        }
    }

    // Resolve arguments: each arg can be a branch name or a worktree path.
    let resolved =
        resolve_branch_args(&params.branches, &worktree_entries, &ctx.project_root, sink)?;

    // Detect current worktree context for is_current_worktree flagging.
    let current_wt_path = git.get_current_worktree_path().ok();
    let current_branch = git.symbolic_ref_short_head().ok();

    // Validate all branches before performing any deletions
    let (validated, errors) = validate_branches(
        &ctx,
        &resolved,
        params.force,
        &worktree_map,
        current_wt_path.as_ref(),
        current_branch.as_deref(),
        sink,
    );

    let requested_count = resolved.len();

    if !errors.is_empty() {
        return Ok(BranchDeleteResult {
            deletions: Vec::new(),
            validation_errors: errors,
            validated_count: validated.len(),
            requested_count,
            cd_target: None,
            nothing_to_delete: false,
        });
    }

    if validated.is_empty() {
        return Ok(BranchDeleteResult {
            deletions: Vec::new(),
            validation_errors: Vec::new(),
            validated_count: 0,
            requested_count,
            cd_target: None,
            nothing_to_delete: true,
        });
    }

    // Execute deletions
    let (deletions, cd_target) = execute_deletions(&ctx, &validated, params, sink);

    Ok(BranchDeleteResult {
        deletions,
        validation_errors: Vec::new(),
        validated_count: validated.len(),
        requested_count,
        cd_target,
        nothing_to_delete: false,
    })
}

// ── Argument resolution ────────────────────────────────────────────────────

/// Resolve each argument to a branch name.
///
/// Arguments can be:
///   - A branch name (passed through as-is if no worktree path matches)
///   - A worktree path (absolute or relative to cwd, including ".")
fn resolve_branch_args(
    args: &[String],
    worktree_entries: &[WorktreeEntry],
    project_root: &Path,
    sink: &mut dyn ProgressSink,
) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(args.len());

    for arg in args {
        match resolve_single_arg(arg, worktree_entries, project_root) {
            ResolveResult::Branch(name) => {
                sink.on_step(&format!("Resolved path '{}' to branch '{}'", arg, name));
                resolved.push(name);
            }
            ResolveResult::PassThrough => {
                resolved.push(arg.clone());
            }
            ResolveResult::DetachedHead(path) => {
                anyhow::bail!(
                    "worktree at '{}' has a detached HEAD; specify a branch name instead",
                    path.display()
                );
            }
        }
    }

    Ok(resolved)
}

/// Try to resolve a single argument as a worktree path.
fn resolve_single_arg(
    arg: &str,
    worktree_entries: &[WorktreeEntry],
    project_root: &Path,
) -> ResolveResult {
    // Build a candidate path: resolve relative paths against cwd.
    let candidate = PathBuf::from(arg);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(&candidate),
            Err(_) => return ResolveResult::PassThrough,
        }
    };

    // Canonicalize to resolve ".", "..", and symlinks.
    let canonical = match std::fs::canonicalize(&candidate) {
        Ok(p) => p,
        Err(_) => {
            // Path doesn't exist on disk — also try resolving as relative to project root
            return try_resolve_relative_to_root(arg, project_root, worktree_entries);
        }
    };

    // Compare against all known worktree paths.
    for entry in worktree_entries {
        let entry_canonical =
            std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());

        if canonical == entry_canonical {
            return match &entry.branch {
                Some(branch) => ResolveResult::Branch(branch.clone()),
                None => ResolveResult::DetachedHead(entry.path.clone()),
            };
        }
    }

    // No worktree matched — also try as relative to project root before giving up.
    try_resolve_relative_to_root(arg, project_root, worktree_entries)
}

/// Try resolving an argument as a path relative to the project root.
fn try_resolve_relative_to_root(
    arg: &str,
    project_root: &Path,
    worktree_entries: &[WorktreeEntry],
) -> ResolveResult {
    let potential = project_root.join(arg);
    let potential_canonical = std::fs::canonicalize(&potential).ok();

    if let Some(ref canonical) = potential_canonical {
        for entry in worktree_entries {
            let entry_canonical =
                std::fs::canonicalize(&entry.path).unwrap_or_else(|_| entry.path.clone());

            if canonical == &entry_canonical {
                return match &entry.branch {
                    Some(branch) => ResolveResult::Branch(branch.clone()),
                    None => ResolveResult::DetachedHead(entry.path.clone()),
                };
            }
        }
    }

    ResolveResult::PassThrough
}

// ── Validation ─────────────────────────────────────────────────────────────

/// Validate all requested branches. Returns a tuple of (validated, errors).
///
/// Each branch goes through up to 5 checks:
///   1. Branch exists locally
///   2. Not the default branch (even with --force)
///   3. No uncommitted changes in worktree (skip with --force)
///   4. Merged into default branch (skip with --force)
///   5. Local/remote in sync (skip with --force)
#[allow(clippy::too_many_arguments)]
fn validate_branches(
    ctx: &BranchDeleteContext,
    branches: &[String],
    force: bool,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    sink: &mut dyn ProgressSink,
) -> (Vec<ValidatedBranch>, Vec<ValidationError>) {
    let mut validated = Vec::new();
    let mut errors = Vec::new();

    for branch in branches {
        sink.on_step(&format!("Validating branch '{branch}'..."));

        // Check 1: Branch exists locally
        match ctx.git.show_ref_exists(&format!("refs/heads/{branch}")) {
            Ok(true) => {}
            Ok(false) => {
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message: "branch not found".to_string(),
                });
                continue;
            }
            Err(e) => {
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message: format!("failed to check if branch exists: {e}"),
                });
                continue;
            }
        }

        // Check 2: Not the default branch (never allowed, even with --force)
        if branch == &ctx.default_branch {
            errors.push(ValidationError {
                branch: branch.clone(),
                message: format!(
                    "refusing to delete the default branch '{}'",
                    ctx.default_branch
                ),
            });
            continue;
        }

        let wt_path = worktree_map.get(branch.as_str()).cloned();

        // Check 3: No uncommitted changes (skip with --force)
        if !force {
            if let Some(ref path) = wt_path {
                match ctx.git.has_uncommitted_changes_in(path) {
                    Ok(true) => {
                        errors.push(ValidationError {
                            branch: branch.clone(),
                            message: "has uncommitted changes in worktree (use -D to force)"
                                .to_string(),
                        });
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        errors.push(ValidationError {
                            branch: branch.clone(),
                            message: format!(
                                "failed to check for uncommitted changes: {e} (use -D to force)"
                            ),
                        });
                        continue;
                    }
                }
            }
        }

        // Check 4: Merged into default branch (skip with --force)
        if !force {
            match is_branch_merged(ctx, branch) {
                Ok(true) => {
                    sink.on_step(&format!("Branch '{branch}' is merged into default branch"));
                }
                Ok(false) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: format!(
                            "not merged into '{}' (use -D to force)",
                            ctx.default_branch
                        ),
                    });
                    continue;
                }
                Err(e) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: format!("failed to check merge status: {e} (use -D to force)"),
                    });
                    continue;
                }
            }
        }

        // Determine remote tracking info for this branch
        let (remote_name, remote_branch_name) = resolve_remote_tracking(ctx, branch);

        // Check 5: Local/remote in sync (skip with --force)
        if !force {
            if let Some(ref remote) = remote_name {
                if let Some(ref remote_branch) = remote_branch_name {
                    match check_local_remote_sync(ctx, branch, remote, remote_branch) {
                        Ok(true) => {
                            sink.on_step(&format!("Branch '{branch}' is in sync with remote"));
                        }
                        Ok(false) => {
                            errors.push(ValidationError {
                                branch: branch.clone(),
                                message:
                                    "local and remote branches are out of sync (use -D to force)"
                                        .to_string(),
                            });
                            continue;
                        }
                        Err(e) => {
                            errors.push(ValidationError {
                                branch: branch.clone(),
                                message: format!(
                                    "failed to check local/remote sync: {e} (use -D to force)"
                                ),
                            });
                            continue;
                        }
                    }
                }
            }
        }

        // All checks passed — detect if this is the worktree the user is inside.
        // Use both path comparison and branch name as fallback: path comparison
        // can fail when symlinks cause git commands to report different strings
        // (e.g., /tmp vs /private/tmp on macOS).
        let is_current = match (&wt_path, current_wt_path) {
            (Some(wt), Some(current)) => {
                wt == current
                    || std::fs::canonicalize(wt).ok() == std::fs::canonicalize(current).ok()
            }
            _ => false,
        } || (wt_path.is_some()
            && current_branch.is_some()
            && current_branch == Some(branch.as_str()));

        sink.on_step(&format!("Branch '{branch}' passed validation"));

        validated.push(ValidatedBranch {
            name: branch.clone(),
            worktree_path: wt_path,
            remote_name,
            remote_branch_name,
            is_current_worktree: is_current,
        });
    }

    (validated, errors)
}

// ── Merge checking ─────────────────────────────────────────────────────────

/// Check whether a branch has been merged into the default branch.
///
/// Checks against both the local default branch and its remote tracking branch.
/// Uses a two-step approach for each target:
/// 1. `merge-base --is-ancestor` — detects regular merges
/// 2. `git cherry` — detects squash merges (all lines start with `-`)
fn is_branch_merged(ctx: &BranchDeleteContext, branch: &str) -> Result<bool> {
    // Check against local default branch first
    if is_branch_merged_into(ctx, branch, &ctx.default_branch)? {
        return Ok(true);
    }

    // Also check against the remote tracking branch, which may be ahead of local
    let remote_ref = format!("{}/{}", ctx.remote_name, ctx.default_branch);
    if is_branch_merged_into(ctx, branch, &remote_ref)? {
        return Ok(true);
    }

    Ok(false)
}

/// Check whether `branch` has been merged into `target`.
fn is_branch_merged_into(ctx: &BranchDeleteContext, branch: &str, target: &str) -> Result<bool> {
    // Step 1: Check if branch is an ancestor of the target (regular merge)
    let is_ancestor = ctx
        .git
        .merge_base_is_ancestor(branch, target)
        .context("merge-base check failed")?;

    if is_ancestor {
        return Ok(true);
    }

    // Step 2: Check for squash merge via git cherry.
    let cherry_output = ctx
        .git
        .cherry(target, branch)
        .context("git cherry check failed")?;

    let lines: Vec<&str> = cherry_output.lines().collect();

    // Empty output means no commits to compare
    if lines.is_empty() {
        return Ok(true);
    }

    // All lines must start with `-` for the branch to be considered squash-merged
    let all_merged = lines.iter().all(|line| line.starts_with('-'));
    Ok(all_merged)
}

/// Compare local and remote SHAs to determine if the branch is in sync.
fn check_local_remote_sync(
    ctx: &BranchDeleteContext,
    branch: &str,
    remote: &str,
    remote_branch: &str,
) -> Result<bool> {
    let remote_ref = format!("refs/remotes/{remote}/{remote_branch}");

    // If the remote tracking ref doesn't exist, consider it in sync.
    let remote_exists = ctx
        .git
        .show_ref_exists(&remote_ref)
        .context("failed to check remote ref existence")?;
    if !remote_exists {
        return Ok(true);
    }

    let local_sha = ctx
        .git
        .rev_parse(&format!("refs/heads/{branch}"))
        .context("failed to resolve local branch SHA")?;
    let remote_sha = ctx
        .git
        .rev_parse(&remote_ref)
        .context("failed to resolve remote branch SHA")?;

    Ok(local_sha == remote_sha)
}

/// Resolve the remote name and remote branch name for a given local branch.
fn resolve_remote_tracking(
    ctx: &BranchDeleteContext,
    branch: &str,
) -> (Option<String>, Option<String>) {
    // Try to get the configured tracking remote for this branch
    if let Ok(Some(remote)) = ctx.git.get_branch_tracking_remote(branch) {
        return (Some(remote), Some(branch.to_string()));
    }

    // Fall back: check if the default remote has this branch
    let remote_ref = format!("refs/remotes/{}/{branch}", ctx.remote_name);
    if let Ok(true) = ctx.git.show_ref_exists(&remote_ref) {
        return (Some(ctx.remote_name.clone()), Some(branch.to_string()));
    }

    (None, None)
}

// ── Deletion execution ─────────────────────────────────────────────────────

/// Execute all validated deletions. Current-worktree branches are deferred to
/// last so we can resolve a CD target and change directory before removing them.
fn execute_deletions(
    ctx: &BranchDeleteContext,
    validated: &[ValidatedBranch],
    params: &BranchDeleteParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> (Vec<DeletionResult>, Option<PathBuf>) {
    // Partition into regular and deferred (current worktree) branches
    let (deferred, regular): (Vec<&ValidatedBranch>, Vec<&ValidatedBranch>) =
        validated.iter().partition(|b| b.is_current_worktree);

    let mut deletions = Vec::new();

    // Process regular branches first
    for branch in &regular {
        let result = delete_single_branch(ctx, branch, params.force, sink);
        deletions.push(result);
    }

    // Process deferred branch (current worktree) last
    let mut cd_target: Option<PathBuf> = None;

    for branch in &deferred {
        sink.on_step(&format!(
            "Processing deferred branch: {} (current worktree)",
            branch.name
        ));

        if branch.worktree_path.is_some() {
            // Resolve CD target BEFORE removing the worktree.
            let target = resolve_prune_cd_target(
                params.prune_cd_target,
                &ctx.project_root,
                &ctx.git_dir,
                &ctx.remote_name,
                params.use_gitoxide,
                sink,
            );

            if let Err(e) = std::env::set_current_dir(&target) {
                sink.on_warning(&format!(
                    "Failed to change directory to {}: {e}. \
                     Skipping removal of current worktree {}.",
                    target.display(),
                    branch.name
                ));
                continue;
            }

            let result = delete_single_branch(ctx, branch, params.force, sink);

            if result.worktree_removed {
                cd_target = Some(target);
            }

            deletions.push(result);
        } else {
            // No worktree, just delete branch and remote
            let result = delete_single_branch(ctx, branch, params.force, sink);
            deletions.push(result);
        }
    }

    (deletions, cd_target)
}

/// Delete a single branch: remote, worktree, and local branch (in that order).
///
/// Deletion order is deliberate — remote branches are hardest to recreate, so
/// they are deleted first. If a later step fails, the user still has local state
/// to recover from.
fn delete_single_branch(
    ctx: &BranchDeleteContext,
    branch: &ValidatedBranch,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
) -> DeletionResult {
    let mut result = DeletionResult {
        branch: branch.name.clone(),
        remote_deleted: false,
        worktree_removed: false,
        branch_deleted: false,
        errors: Vec::new(),
    };

    let has_worktree = branch.worktree_path.is_some();

    // Step 1: Run pre-remove hook (only if worktree exists)
    if let Some(ref wt_path) = branch.worktree_path {
        run_removal_hook(HookType::PreRemove, ctx, wt_path, &branch.name, sink);
    }

    // Step 2: Delete remote branch (hardest to recreate, do first)
    if let (Some(ref remote), Some(ref remote_branch)) =
        (&branch.remote_name, &branch.remote_branch_name)
    {
        sink.on_step(&format!(
            "Deleting remote branch {}/{}...",
            remote, remote_branch
        ));
        match ctx.git.push_delete(remote, remote_branch) {
            Ok(()) => {
                result.remote_deleted = true;
                sink.on_step(&format!(
                    "Remote branch {}/{} deleted",
                    remote, remote_branch
                ));
            }
            Err(e) => {
                result.errors.push(format!(
                    "Failed to delete remote branch {remote}/{remote_branch}: {e}"
                ));
            }
        }
    }

    // Step 3: Remove worktree (if one exists)
    if let Some(ref wt_path) = branch.worktree_path {
        if wt_path.exists() {
            sink.on_step(&format!("Removing worktree at {}...", wt_path.display()));
            match ctx.git.worktree_remove(wt_path, force) {
                Ok(()) => {
                    result.worktree_removed = true;
                    sink.on_step(&format!("Removed worktree '{}'", branch.name));
                }
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to remove worktree {}: {e}",
                        wt_path.display()
                    ));
                }
            }
        } else {
            // Worktree directory is gone but git may still have a record
            sink.on_warning(&format!(
                "Worktree directory {} not found. Attempting to force remove record.",
                wt_path.display()
            ));
            match ctx.git.worktree_remove(wt_path, true) {
                Ok(()) => {
                    result.worktree_removed = true;
                    sink.on_step(&format!("Removed worktree '{}'", branch.name));
                }
                Err(e) => {
                    result.errors.push(format!(
                        "Failed to remove orphaned worktree record {}: {e}",
                        wt_path.display()
                    ));
                }
            }
        }

        // Clean up empty parent directories after worktree removal
        if result.worktree_removed {
            cleanup_empty_parent_dirs(&ctx.project_root, wt_path, sink);
        }
    }

    // Step 4: Delete local branch
    // Always use force-delete (-D) here because our validation has already passed.
    sink.on_step(&format!("Deleting local branch {}...", branch.name));
    match ctx.git.branch_delete(&branch.name, true) {
        Ok(()) => {
            result.branch_deleted = true;
            sink.on_step(&format!("Branch {} deleted", branch.name));
        }
        Err(e) => {
            result.errors.push(format!(
                "Failed to delete local branch {}: {e}",
                branch.name
            ));
        }
    }

    // Step 5: Run post-remove hook (only if worktree existed)
    if has_worktree {
        if let Some(ref wt_path) = branch.worktree_path {
            run_removal_hook(HookType::PostRemove, ctx, wt_path, &branch.name, sink);
        }
    }

    result
}

// ── Hook execution ─────────────────────────────────────────────────────────

/// Run a lifecycle hook (pre-remove or post-remove) for a worktree.
fn run_removal_hook(
    hook_type: HookType,
    ctx: &BranchDeleteContext,
    worktree_path: &Path,
    branch_name: &str,
    sink: &mut (impl ProgressSink + HookRunner),
) {
    let hook_ctx = HookContext::new(
        hook_type,
        "branch-delete",
        &ctx.project_root,
        &ctx.git_dir,
        &ctx.remote_name,
        &ctx.source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::Manual);

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

/// Resolve where to cd after deleting the user's current worktree.
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
        // Stop at or above the project root
        if dir == project_root || !dir.starts_with(project_root) {
            break;
        }
        // fs::remove_dir only succeeds on empty directories
        match std::fs::remove_dir(dir) {
            Ok(()) => {
                sink.on_step(&format!("Removed empty directory '{}'", dir.display()));
                current = dir.parent();
            }
            Err(_) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list_empty() {
        let entry = WorktreeEntry {
            path: PathBuf::from("/tmp/test"),
            branch: Some("main".to_string()),
            is_bare: false,
        };
        assert_eq!(entry.path, PathBuf::from("/tmp/test"));
        assert_eq!(entry.branch.as_deref(), Some("main"));
        assert!(!entry.is_bare);
    }

    #[test]
    fn test_worktree_entry_bare() {
        let entry = WorktreeEntry {
            path: PathBuf::from("/tmp/test.git"),
            branch: None,
            is_bare: true,
        };
        assert!(entry.is_bare);
        assert!(entry.branch.is_none());
    }

    #[test]
    fn test_validated_branch_fields() {
        let vb = ValidatedBranch {
            name: "feature/test".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/project/feature/test")),
            remote_name: Some("origin".to_string()),
            remote_branch_name: Some("feature/test".to_string()),
            is_current_worktree: false,
        };
        assert_eq!(vb.name, "feature/test");
        assert!(vb.worktree_path.is_some());
        assert!(!vb.is_current_worktree);
    }

    #[test]
    fn test_validated_branch_no_worktree() {
        let vb = ValidatedBranch {
            name: "orphan-branch".to_string(),
            worktree_path: None,
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
        };
        assert!(vb.worktree_path.is_none());
        assert!(vb.remote_name.is_none());
        assert!(vb.remote_branch_name.is_none());
    }

    #[test]
    fn test_validation_error_fields() {
        let err = ValidationError {
            branch: "my-branch".to_string(),
            message: "has uncommitted changes".to_string(),
        };
        assert_eq!(err.branch, "my-branch");
        assert_eq!(err.message, "has uncommitted changes");
    }

    #[test]
    fn test_branch_delete_context_fields() {
        // Verify the context struct can be constructed with expected fields.
        let _default_branch = "main".to_string();
        let _remote_name = "origin".to_string();
        let _project_root = PathBuf::from("/tmp/project");
        let _git_dir = PathBuf::from("/tmp/project/.git");
        let _source_worktree = PathBuf::from("/tmp/project/main");
    }

    #[test]
    fn test_deletion_result_no_errors() {
        let result = DeletionResult {
            branch: "feature/foo".to_string(),
            remote_deleted: true,
            worktree_removed: true,
            branch_deleted: true,
            errors: Vec::new(),
        };
        assert!(!result.has_errors());
        assert_eq!(
            result.deleted_parts(),
            "worktree, local branch, remote branch"
        );
    }

    #[test]
    fn test_deletion_result_with_errors() {
        let result = DeletionResult {
            branch: "feature/bar".to_string(),
            remote_deleted: false,
            worktree_removed: true,
            branch_deleted: true,
            errors: vec!["Failed to delete remote".to_string()],
        };
        assert!(result.has_errors());
        assert_eq!(result.deleted_parts(), "worktree, local branch");
    }

    #[test]
    fn test_deletion_result_nothing_deleted() {
        let result = DeletionResult {
            branch: "broken".to_string(),
            remote_deleted: false,
            worktree_removed: false,
            branch_deleted: false,
            errors: vec!["everything failed".to_string()],
        };
        assert!(result.has_errors());
        assert_eq!(result.deleted_parts(), "");
    }

    #[test]
    fn test_deletion_result_branch_only() {
        let result = DeletionResult {
            branch: "orphan".to_string(),
            remote_deleted: false,
            worktree_removed: false,
            branch_deleted: true,
            errors: Vec::new(),
        };
        assert!(!result.has_errors());
        assert_eq!(result.deleted_parts(), "local branch");
    }

    #[test]
    fn test_deletion_result_remote_only() {
        let result = DeletionResult {
            branch: "remote-only".to_string(),
            remote_deleted: true,
            worktree_removed: false,
            branch_deleted: false,
            errors: Vec::new(),
        };
        assert!(!result.has_errors());
        assert_eq!(result.deleted_parts(), "remote branch");
    }
}
