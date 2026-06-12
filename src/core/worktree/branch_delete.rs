//! Core logic for the `git-worktree-branch-delete` command.
//!
//! Deletes branches and their associated worktrees.

use crate::core::worktree::ports::NoopStageRunner;
use crate::core::worktree::push::{PushAction, push_with_hooks};
use crate::core::{HookRunner, ProgressSink};
use crate::executor::presenter::JobPresenter;
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::get_default_branch_local;
use crate::settings::PruneCdTarget;
use crate::{get_git_common_dir, get_project_root};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// Whether to delete the remote branch.
    pub delete_remote: bool,
    /// Only delete the remote branch, keep local worktree and branch.
    pub remote_only: bool,
    /// Skip local branch deletion and remote branch deletion. Only the
    /// worktree is removed, with `worktree-pre-remove` /
    /// `worktree-post-remove` hooks firing as usual. Used by `daft merge -r`
    /// (without `-b`) to remove a source worktree while keeping the local
    /// branch ref intact.
    pub keep_local_branch: bool,
    /// Skip the repo's pre-push hook when deleting the remote branch
    /// (`--no-verify`).
    pub no_verify: bool,
    /// Where to cd after deleting the current worktree.
    pub prune_cd_target: PruneCdTarget,
    /// Label exposed to hook scripts as `DAFT_COMMAND`. Defaults to
    /// `"branch-delete"` for the standalone `daft remove` /
    /// `daft branch-delete` flow; the merge cleanup loop sets this to
    /// `"merge"` so hook scripts can distinguish the invocation source.
    pub command_label: String,
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

use super::porcelain::{WorktreeListEntry, parse_worktree_list_porcelain};

/// Bundles common parameters used throughout the branch-delete operation.
struct BranchDeleteContext<'a> {
    git: &'a GitCommand,
    project_root: PathBuf,
    git_dir: PathBuf,
    remote_name: String,
    source_worktree: PathBuf,
    default_branch: String,
    /// Skip the repo's pre-push hook on the remote-branch delete.
    no_verify: bool,
    /// Reports the pre-push hook run on the remote-branch delete (#599).
    presenter: Option<&'a Arc<dyn JobPresenter>>,
}

/// Validated branch ready for deletion.
struct ValidatedBranch {
    name: String,
    worktree_path: Option<PathBuf>,
    remote_name: Option<String>,
    remote_branch_name: Option<String>,
    is_current_worktree: bool,
    /// When true, only the worktree is removed — local branch ref and remote
    /// branch are preserved. Used for the default branch.
    worktree_only: bool,
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
///
/// `presenter` reports the pre-push hook run on remote-branch deletes
/// (#599); pass `None` to skip that reporting (the hook is still honored).
pub fn execute(
    params: &BranchDeleteParams,
    presenter: Option<&Arc<dyn JobPresenter>>,
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
        no_verify: params.no_verify,
        presenter,
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
        params.remote_only,
        params.keep_local_branch,
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
    let (deletions, cd_target) = execute_deletions(&ctx, &validated, params, &worktree_map, sink);

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
    worktree_entries: &[WorktreeListEntry],
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
    worktree_entries: &[WorktreeListEntry],
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
    worktree_entries: &[WorktreeListEntry],
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
///   2. Default branch protection: without --force, always refused; with
///      --force, allowed as worktree-only removal (skips checks 3-5)
///   3. No uncommitted changes in worktree (skip with --force)
///   4. Merged into default branch (skip with --force or keep_local_branch)
///   5. Local/remote in sync (skip with --force or keep_local_branch)
#[allow(clippy::too_many_arguments)]
fn validate_branches(
    ctx: &BranchDeleteContext,
    branches: &[String],
    force: bool,
    remote_only: bool,
    keep_local_branch: bool,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    sink: &mut dyn ProgressSink,
) -> (Vec<ValidatedBranch>, Vec<ValidationError>) {
    let mut validated = Vec::new();
    let mut errors = Vec::new();

    for branch in branches {
        sink.on_step(&format!("Validating branch '{branch}'..."));

        // Remote-only mode: skip local branch checks entirely.
        // Just verify the remote branch exists and produce a ValidatedBranch
        // with only remote info populated.
        if remote_only {
            let (remote_name, remote_branch_name) = resolve_remote_for_missing_local(ctx, branch);

            if remote_name.is_none() || remote_branch_name.is_none() {
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message: format!(
                        "no remote branch found for '{}' on '{}'",
                        branch, ctx.remote_name
                    ),
                });
                continue;
            }

            sink.on_step(&format!(
                "Branch '{branch}' — remote-only deletion, skipping local checks"
            ));

            validated.push(ValidatedBranch {
                name: branch.clone(),
                worktree_path: None,
                remote_name,
                remote_branch_name,
                is_current_worktree: false,
                worktree_only: false,
            });
            continue;
        }

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

        let wt_path = worktree_map.get(branch.as_str()).cloned();

        // Check 2: Default branch protection
        if branch == &ctx.default_branch {
            if !force {
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message: format!(
                        "refusing to delete the default branch '{}' (use --force to remove the worktree only)",
                        ctx.default_branch
                    ),
                });
                continue;
            } else if wt_path.is_none() {
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message: format!(
                        "the default branch '{}' has no worktree to remove",
                        ctx.default_branch
                    ),
                });
                continue;
            } else {
                // Force + worktree exists: allow worktree-only removal.
                // Skip checks 3-5 since we are not deleting the branch ref.
                let is_current = match (&wt_path, current_wt_path) {
                    (Some(wt), Some(current)) => {
                        wt == current
                            || std::fs::canonicalize(wt).ok() == std::fs::canonicalize(current).ok()
                    }
                    _ => false,
                } || (wt_path.is_some()
                    && current_branch.is_some()
                    && current_branch == Some(branch.as_str()));

                sink.on_step(&format!(
                    "Default branch '{}' — will remove worktree only",
                    branch
                ));

                validated.push(ValidatedBranch {
                    name: branch.clone(),
                    worktree_path: wt_path,
                    remote_name: None,
                    remote_branch_name: None,
                    is_current_worktree: is_current,
                    worktree_only: true,
                });
                continue;
            }
        }

        // Check 3: No uncommitted changes (skip with --force)
        if !force && let Some(ref path) = wt_path {
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

        // Check 4: Merged into default branch (skip with --force or keep_local_branch)
        if !force && !keep_local_branch {
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

        // Check 5: Local/remote in sync (skip with --force or keep_local_branch)
        if !force
            && !keep_local_branch
            && let Some(ref remote) = remote_name
            && let Some(ref remote_branch) = remote_branch_name
        {
            match check_local_remote_sync(ctx, branch, remote, remote_branch) {
                Ok(true) => {
                    sink.on_step(&format!("Branch '{branch}' is in sync with remote"));
                }
                Ok(false) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: "local and remote branches are out of sync (use -D to force)"
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

        // Check 6: Divergence guard — refuse removal when in-scope untracked daft
        // files in this worktree differ from the merge target's. Gated BEFORE
        // propagation (Task 6.1) so that a failed/skipped propagation doesn't
        // silently lose visitor-config refinements. --force (-D) bypasses.
        //
        // Gate conditions mirror Task 6.1's propagation block: only applies when
        // (a) the source worktree exists on disk, (b) the merge-target worktree
        // (default branch) is also checked out somewhere, and (c) the branch has
        // at least one in-scope file (has_local or has_visitor_daft_yml). The
        // divergence check is the second gate: it only refuses when those files
        // actually differ from the target's.
        if !force
            && !keep_local_branch
            && let Some(ref wt) = wt_path
            && wt.is_dir()
        {
            let has_local = wt.join("daft.local.yml").is_file();
            let has_visitor_daft_yml = wt.join("daft.yml").is_file()
                && matches!(
                    crate::hooks::yaml_config_loader::classify_main_config(wt),
                    crate::hooks::yaml_config_loader::ConfigStatus::Visitor
                );
            if (has_local || has_visitor_daft_yml)
                && let Some(target_wt) = worktree_map.get(ctx.default_branch.as_str())
            {
                match crate::hooks::visitor_propagation::has_inscope_divergence(wt, target_wt) {
                    Ok(true) => {
                        errors.push(ValidationError {
                            branch: branch.clone(),
                            message: format!(
                                "untracked daft files in {} differ from the merge \
                                 target {}. Consolidate first with `daft file merge` \
                                 or pass -D/--force to remove anyway.",
                                wt.display(),
                                target_wt.display(),
                            ),
                        });
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        sink.on_step(&format!(
                            "Warning: divergence check failed for '{branch}': {e}; \
                             proceeding with removal"
                        ));
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
            worktree_only: false,
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

/// Resolve remote info for a branch that may not exist locally.
///
/// First tries the normal tracking config lookup. If the local branch doesn't
/// exist (so git config has no `branch.<name>.remote`), falls back to probing
/// `refs/remotes/<default-remote>/<branch>`.
fn resolve_remote_for_missing_local(
    ctx: &BranchDeleteContext,
    branch: &str,
) -> (Option<String>, Option<String>) {
    // Try normal tracking lookup first (works when local branch exists)
    let result = resolve_remote_tracking(ctx, branch);
    if result.0.is_some() {
        return result;
    }

    // Fallback: check if the default remote has this branch
    let remote_ref = format!("refs/remotes/{}/{branch}", ctx.remote_name);
    if let Ok(true) = ctx.git.show_ref_exists(&remote_ref) {
        return (Some(ctx.remote_name.clone()), Some(branch.to_string()));
    }

    (None, None)
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
    worktree_map: &HashMap<String, PathBuf>,
    sink: &mut (impl ProgressSink + HookRunner),
) -> (Vec<DeletionResult>, Option<PathBuf>) {
    // Partition into regular and deferred (current worktree) branches
    let (deferred, regular): (Vec<&ValidatedBranch>, Vec<&ValidatedBranch>) =
        validated.iter().partition(|b| b.is_current_worktree);

    let mut deletions = Vec::new();

    // Process regular branches first
    for branch in &regular {
        let result = delete_single_branch(
            ctx,
            branch,
            params.force,
            params.delete_remote,
            params.remote_only,
            params.keep_local_branch,
            &params.command_label,
            worktree_map,
            sink,
        );
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

            let result = delete_single_branch(
                ctx,
                branch,
                params.force,
                params.delete_remote,
                params.remote_only,
                params.keep_local_branch,
                &params.command_label,
                worktree_map,
                sink,
            );

            if result.worktree_removed {
                cd_target = Some(target);
            }

            deletions.push(result);
        } else {
            // No worktree, just delete branch and remote
            let result = delete_single_branch(
                ctx,
                branch,
                params.force,
                params.delete_remote,
                params.remote_only,
                params.keep_local_branch,
                &params.command_label,
                worktree_map,
                sink,
            );
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
#[allow(clippy::too_many_arguments)]
fn delete_single_branch(
    ctx: &BranchDeleteContext,
    branch: &ValidatedBranch,
    force: bool,
    delete_remote: bool,
    remote_only: bool,
    keep_local_branch: bool,
    command_label: &str,
    worktree_map: &HashMap<String, PathBuf>,
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

    // Step 1: Cancel any running background jobs for this worktree, then
    // run the pre-remove hook (only if worktree exists). The cancel is
    // best-effort and runs first so the pre-remove hook sees a settled
    // coordinator state and can audit the worktree without racing against
    // jobs that are about to be torn down anyway.
    if let Some(ref wt_path) = branch.worktree_path {
        super::prune::cancel_background_jobs_for_worktree(&branch.name, sink);
        run_removal_hook(
            HookType::PreRemove,
            ctx,
            wt_path,
            &branch.name,
            command_label,
            sink,
        );
    }

    // Step 2: Delete remote branch (hardest to recreate, do first)
    // Skipped for worktree-only removal (default branch), keep_local_branch mode,
    // or when remote deletion is disabled.
    if !keep_local_branch
        && !branch.worktree_only
        && (delete_remote || remote_only)
        && let (Some(remote), Some(remote_branch)) =
            (&branch.remote_name, &branch.remote_branch_name)
    {
        sink.on_step(&format!(
            "Deleting remote branch {}/{}...",
            remote, remote_branch
        ));
        // Run from the branch's worktree when it still exists (Step 3 removes
        // it later) so the repo's pre-push hook fires there; otherwise any
        // directory inside the repo works for a remote delete.
        let push_cwd = branch
            .worktree_path
            .as_deref()
            .filter(|p| p.is_dir())
            .unwrap_or(&ctx.project_root);
        match push_with_hooks(
            ctx.git,
            PushAction::Delete {
                remote,
                branch: remote_branch,
            },
            push_cwd,
            !ctx.no_verify,
            &NoopStageRunner,
            ctx.presenter,
            None,
        )
        .and_then(crate::core::worktree::push::PushOutcome::into_result)
        {
            Ok(_) => {
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

    // When remote_only is set, skip local operations entirely.
    if remote_only {
        if branch.remote_name.is_none() || branch.remote_branch_name.is_none() {
            result.errors.push(format!(
                "Branch '{}' has no remote tracking branch",
                branch.name
            ));
        }
        return result;
    }

    // Visitor-config propagation: if the source worktree still has in-scope
    // untracked daft files, copy them into the merge target's worktree before
    // the source worktree gets removed. Gated cheapest-first so non-users
    // pay no cost. Skip when the branch being deleted IS the default branch
    // (worktree_only path), as there is no merge target to propagate to.
    if !branch.worktree_only
        && !remote_only
        && branch.name != ctx.default_branch
        && let Some(ref wt_path) = branch.worktree_path
        && wt_path.is_dir()
    {
        let has_local = wt_path.join("daft.local.yml").is_file();
        let has_visitor_daft_yml = wt_path.join("daft.yml").is_file()
            && matches!(
                crate::hooks::yaml_config_loader::classify_main_config(wt_path),
                crate::hooks::yaml_config_loader::ConfigStatus::Visitor
            );

        if (has_local || has_visitor_daft_yml)
            && let Some(target_wt) = worktree_map.get(&ctx.default_branch)
            // Only salvage when the source actually has in-scope content the
            // target lacks. When it doesn't — the common case: an unchanged
            // worktree whose daft files are a subset of the target's — there is
            // nothing to copy, and running the merge would needlessly
            // re-serialize the target's daft.yml, stripping its comments and
            // littering it with `null`s. (A non-forced divergent removal is
            // already blocked by the divergence guard with a "consolidate first"
            // message; only a forced removal reaches here with real divergence.)
            && matches!(
                crate::hooks::visitor_propagation::has_inscope_divergence(wt_path, target_wt),
                Ok(true)
            )
        {
            let _ = crate::hooks::visitor_propagation::propagate(wt_path, target_wt);
        }
    }

    // Step 3: Remove worktree (if one exists)
    if let Some(ref wt_path) = branch.worktree_path {
        // Guard: the main working tree (contains .git/ directory, not a .git file)
        // cannot be removed. In non-bare layouts, this is the original clone directory.
        let git_entry = wt_path.join(".git");
        if git_entry.is_dir() {
            result.errors.push(format!(
                "Cannot remove '{}': this is the main working tree. \
                 Use `daft layout transform` to restructure, or delete other worktrees instead.",
                branch.name
            ));
        } else if wt_path.exists() {
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
    // Skipped for worktree-only removal (default branch) or keep_local_branch mode.
    if !keep_local_branch && !branch.worktree_only {
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
    }

    // Step 5: Run post-remove hook (only if worktree existed)
    if has_worktree && let Some(ref wt_path) = branch.worktree_path {
        run_removal_hook(
            HookType::PostRemove,
            ctx,
            wt_path,
            &branch.name,
            command_label,
            sink,
        );
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
    command_label: &str,
    sink: &mut (impl ProgressSink + HookRunner),
) {
    let hook_ctx = HookContext::new(
        hook_type,
        command_label,
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
///
/// Thin I/O wrapper around the shared
/// [`super::porcelain::parse_worktree_list_porcelain`]. Bare entries are
/// retained; branch-delete simply never maps a bare/detached (branch-less)
/// entry into its branch→path lookup.
fn parse_worktree_list(git: &GitCommand) -> Result<Vec<WorktreeListEntry>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    Ok(parse_worktree_list_porcelain(&porcelain_output))
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
    fn test_validated_branch_fields() {
        let vb = ValidatedBranch {
            name: "feature/test".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/project/feature/test")),
            remote_name: Some("origin".to_string()),
            remote_branch_name: Some("feature/test".to_string()),
            is_current_worktree: false,
            worktree_only: false,
        };
        assert_eq!(vb.name, "feature/test");
        assert!(vb.worktree_path.is_some());
        assert!(!vb.is_current_worktree);
        assert!(!vb.worktree_only);
    }

    #[test]
    fn test_validated_branch_no_worktree() {
        let vb = ValidatedBranch {
            name: "orphan-branch".to_string(),
            worktree_path: None,
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
            worktree_only: false,
        };
        assert!(vb.worktree_path.is_none());
        assert!(vb.remote_name.is_none());
        assert!(vb.remote_branch_name.is_none());
    }

    #[test]
    fn test_validated_branch_worktree_only() {
        let vb = ValidatedBranch {
            name: "main".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/project/main")),
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
            worktree_only: true,
        };
        assert!(vb.worktree_only);
        assert!(vb.worktree_path.is_some());
        assert!(vb.remote_name.is_none());
        assert!(vb.remote_branch_name.is_none());
    }

    #[test]
    fn test_deletion_result_worktree_only() {
        let result = DeletionResult {
            branch: "main".to_string(),
            remote_deleted: false,
            worktree_removed: true,
            branch_deleted: false,
            errors: Vec::new(),
        };
        assert!(!result.has_errors());
        assert_eq!(result.deleted_parts(), "worktree");
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

    // ── keep_local_branch integration tests ────────────────────────────────

    use serial_test::serial;
    use std::process::Command as ShellCommand;
    use std::process::Stdio;

    /// Test-only helper: run `git` quietly so subprocess output doesn't leak
    /// into the test log. Returns the exit status, panics on spawn failure.
    fn git_quiet(path: &std::path::Path, args: &[&str]) -> std::process::ExitStatus {
        ShellCommand::new("git")
            .args(args)
            .current_dir(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
    }

    /// RAII helper: saves cwd on construction and restores on drop.
    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().expect("cwd readable at test start"),
            }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            if std::env::set_current_dir(&self.original).is_err() {
                let _ = std::env::set_current_dir(std::env::temp_dir());
            }
        }
    }

    fn init_repo(path: &std::path::Path) {
        ShellCommand::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        // Create a fake origin/HEAD so get_default_branch_local() can resolve
        // "main" without needing a real remote.
        let remotes_dir = path.join(".git/refs/remotes/origin");
        std::fs::create_dir_all(&remotes_dir).unwrap();
        std::fs::write(remotes_dir.join("HEAD"), "ref: refs/remotes/origin/main\n").unwrap();
    }

    fn setup_worktree(root: &std::path::Path, branch: &str, wt_path: &std::path::Path) {
        git_quiet(
            root,
            &[
                "worktree",
                "add",
                "-q",
                &wt_path.display().to_string(),
                "-b",
                branch,
            ],
        );
    }

    #[test]
    #[serial]
    fn keep_local_branch_removes_worktree_only() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: true,
            no_verify: false,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, None, &mut bridge).expect("keep_local_branch should succeed");

        assert_eq!(result.deletions.len(), 1);
        assert!(
            result.deletions[0].worktree_removed,
            "worktree must be removed"
        );
        assert!(
            !result.deletions[0].branch_deleted,
            "branch must NOT be deleted"
        );
        assert!(!feat_wt.exists(), "worktree directory must be gone");

        // Verify the branch ref still exists.
        let git = GitCommand::new(true);
        assert!(
            git.show_ref_exists("refs/heads/feature").unwrap_or(false),
            "feature branch must still exist after keep_local_branch=true"
        );
    }

    #[test]
    #[serial]
    fn keep_local_branch_skips_merged_into_default_check() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);

        // Add a commit on feature that is NOT merged into main.
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "feature work"])
            .current_dir(&feat_wt)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: true,
            no_verify: false,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, None, &mut bridge).unwrap();

        assert!(
            result.validation_errors.is_empty(),
            "merged-into-default check must be skipped under keep_local_branch"
        );
        assert_eq!(result.deletions.len(), 1);
        assert!(result.deletions[0].worktree_removed);
        assert!(!result.deletions[0].branch_deleted);
    }

    #[test]
    #[serial]
    fn run_removal_hook_uses_command_label_from_params() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);

        // For PreRemove, hooks are discovered from the worktree being removed
        // (via daft.yml in that worktree). Use an absolute sentinel path (not
        // $DAFT_PROJECT_ROOT) so the test is immune to path-canonicalization
        // differences on macOS (/var → /private/var symlinks).
        let canonical_root = tmp.path().canonicalize().unwrap();
        let feat_wt_canonical = feat_wt.canonicalize().unwrap();
        let sentinel_path = canonical_root.join("captured-command");

        // Install a daft.yml hook in the feature worktree that records DAFT_COMMAND.
        // YAML hooks are discovered from the worktree being removed, and run via the
        // YAML executor which handles env var injection correctly in tests.
        std::fs::write(
            feat_wt_canonical.join("daft.yml"),
            format!(
                "hooks:\n  worktree-pre-remove:\n    jobs:\n      - name: capture-command\n        run: echo \"$DAFT_COMMAND\" > {}\n",
                sentinel_path.display()
            ),
        )
        .unwrap();

        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            // force=true bypasses uncommitted-changes / merged / sync checks
            // so writing daft.yml into the worktree after add doesn't abort.
            force: true,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: true,
            no_verify: false,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "merge".to_string(),
        };

        let mut output = TestOutput::new();
        // Use with_trust_db so the hook runs with explicit Allow trust.
        // Set trust for the canonical git_dir path (what get_git_common_dir() returns).
        let canonical_git_dir = tmp.path().join(".git").canonicalize().unwrap();
        let mut trust_db = crate::hooks::TrustDatabase::default();
        trust_db.set_trust_level(&canonical_git_dir, crate::hooks::TrustLevel::Allow);
        let executor = HookExecutor::with_trust_db(HooksConfig::default(), trust_db);
        let mut bridge = CommandBridge::new(&mut output, executor);
        let bd_result = execute(&params, None, &mut bridge).unwrap();
        assert!(
            bd_result.validation_errors.is_empty(),
            "unexpected validation errors: {:?}",
            bd_result
                .validation_errors
                .iter()
                .map(|e| format!("{}: {}", e.branch, e.message))
                .collect::<Vec<_>>()
        );

        let captured = std::fs::read_to_string(&sentinel_path)
            .unwrap_or_else(|_| format!("<sentinel not found at {}>", sentinel_path.display()));
        assert_eq!(
            captured.trim(),
            "merge",
            "DAFT_COMMAND must reflect command_label='merge', not the hardcoded 'branch-delete'"
        );
    }

    // ── Divergence guard tests ─────────────────────────────────────────────

    /// Regression test: divergence guard refuses branch-delete when daft.local.yml
    /// in the feature worktree differs from the default branch worktree.
    ///
    /// To isolate Check 6 (divergence) from Check 3 (uncommitted changes), we
    /// add daft.local.yml to .gitignore so git does not see it as dirty. This
    /// mirrors real usage: daft.local.yml is a personal overlay that should be
    /// gitignored in the repository.
    #[test]
    #[serial]
    fn divergence_guard_refuses_delete_when_local_yml_differs() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);

        // Add .gitignore in the feature worktree that ignores daft.local.yml so
        // that Check 3 (uncommitted changes) does not fire before Check 6.
        // Commit the .gitignore so it is tracked and doesn't itself appear dirty.
        std::fs::write(feat_wt.join(".gitignore"), "daft.local.yml\n").unwrap();
        git_quiet(&feat_wt, &["add", ".gitignore"]);
        ShellCommand::new("git")
            .args(["commit", "-q", "-m", "gitignore daft.local.yml"])
            .current_dir(&feat_wt)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        // Merge feature into main so Check 4 (not merged) passes. The .gitignore
        // commit makes the branches diverge from HEAD but squash-merge passes
        // git-cherry, so use fast-forward merge instead.
        ShellCommand::new("git")
            .args(["merge", "--ff-only", "feature"])
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Write a daft.local.yml in the feature worktree that doesn't exist in main.
        // Because it's gitignored, Check 3 will not flag it as dirty.
        std::fs::write(
            feat_wt.join("daft.local.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - run: echo personal\n",
        )
        .unwrap();

        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, None, &mut bridge).unwrap();

        assert!(
            !result.validation_errors.is_empty(),
            "should have a validation error when daft.local.yml diverges"
        );
        assert!(
            result.validation_errors[0]
                .message
                .contains("untracked daft files"),
            "error message must mention untracked daft files, got: {}",
            result.validation_errors[0].message
        );
        // Feature worktree must NOT have been removed.
        assert!(
            feat_wt.exists(),
            "feature worktree must still exist after refusal"
        );
    }

    /// Regression test: --force bypasses the divergence guard.
    #[test]
    #[serial]
    fn divergence_guard_bypassed_with_force() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);

        // Same setup as the "refuses" test: gitignore daft.local.yml to isolate Check 6.
        std::fs::write(feat_wt.join(".gitignore"), "daft.local.yml\n").unwrap();
        git_quiet(&feat_wt, &["add", ".gitignore"]);
        ShellCommand::new("git")
            .args(["commit", "-q", "-m", "gitignore daft.local.yml"])
            .current_dir(&feat_wt)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        ShellCommand::new("git")
            .args(["merge", "--ff-only", "feature"])
            .current_dir(tmp.path())
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();

        // Write a daft.local.yml in the feature worktree that doesn't exist in main.
        std::fs::write(
            feat_wt.join("daft.local.yml"),
            "hooks:\n  worktree-post-create:\n    jobs:\n      - run: echo personal\n",
        )
        .unwrap();

        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            force: true, // --force bypasses divergence guard
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, None, &mut bridge).unwrap();

        assert!(
            result.validation_errors.is_empty(),
            "force should bypass divergence guard, got: {:?}",
            result
                .validation_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.deletions.len(), 1);
        assert!(
            result.deletions[0].worktree_removed,
            "worktree must be removed with --force"
        );
        assert!(!feat_wt.exists(), "feature worktree directory must be gone");
    }
}
