//! Core logic for the `git-worktree-branch-delete` command.
//!
//! Deletes branches and their associated worktrees.

use crate::core::{
    ConflictSide, ConsolidationChoice, ConsolidationPrompter, ConsolidationRequest, HookRunner,
    ProgressSink, RefinedFileSummary,
};
use crate::git::GitCommand;
use crate::hooks::visitor_seeds::{self, FileClass, SeedClass, SeedsContext};
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
    /// Where to cd after deleting the current worktree.
    pub prune_cd_target: PruneCdTarget,
    /// Label exposed to hook scripts as `DAFT_COMMAND`. Defaults to
    /// `"branch-delete"` for the standalone `daft remove` /
    /// `daft branch-delete` flow; the merge cleanup loop sets this to
    /// `"merge"` so hook scripts can distinguish the invocation source.
    pub command_label: String,
    /// Skip Check 4 (merged into default branch) and Check 5 (local/remote
    /// sync). Set only by the `daft merge` cleanup loop, whose planner has
    /// already validated reachability against the *actual* merge target —
    /// the default-branch checks here would false-refuse cross-target
    /// merges. Unlike `force`, this does NOT bypass the dirty check or the
    /// daft-file provenance guard.
    pub skip_merge_validation: bool,
    /// How the invoking command spells its force flag — used verbatim in
    /// refusal messages (`daft remove` says `-f/--force`, the branch-delete
    /// forms say `-D/--force`).
    pub force_flag_label: String,
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
    /// What to do with the worktree's untracked daft files before removal.
    daft_files: DaftFilePlan,
}

/// Resolved-at-validation decision for the worktree's untracked daft files.
/// Pristine/subsumed copies need no plan (`Nothing`); refined copies were
/// either consolidated interactively (resolved content carried here so
/// execution cannot re-ask) or marked for discard (forced, or the user
/// chose to).
enum DaftFilePlan {
    /// Nothing to preserve — delete the worktree, touch nothing else.
    Nothing,
    /// Write `(filename, resolved content)` into the default-branch worktree
    /// before removal.
    Consolidate(Vec<(String, String)>),
    /// Stash `filename`s under `.daft/discarded/<branch>/` before removal.
    /// The target is never written.
    Discard(Vec<String>),
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
    sink: &mut (impl ProgressSink + HookRunner + ConsolidationPrompter),
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
        params,
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
    params: &BranchDeleteParams,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    sink: &mut (impl ProgressSink + ConsolidationPrompter),
) -> (Vec<ValidatedBranch>, Vec<ValidationError>) {
    let force = params.force;
    let remote_only = params.remote_only;
    let keep_local_branch = params.keep_local_branch;
    let skip_merge_validation = params.skip_merge_validation;
    // One store handle for the whole validation pass; `None` degrades every
    // classification to NoSeed (protective) without blocking anything.
    let seeds = SeedsContext::open(&ctx.git_dir);

    let mut validated = Vec::new();
    let mut errors = Vec::new();

    'branches: for branch in branches {
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
                daft_files: DaftFilePlan::Nothing,
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
                    // Removing the default branch's own worktree: it IS the
                    // consolidation target, so there is nothing to preserve
                    // elsewhere.
                    worktree_only: true,
                    daft_files: DaftFilePlan::Nothing,
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

        // Check 4: Merged into default branch (skip with --force,
        // keep_local_branch, or the merge cleanup's own validation)
        if !force && !keep_local_branch && !skip_merge_validation {
            match is_branch_merged(ctx, branch) {
                Ok(true) => {
                    sink.on_step(&format!("Branch '{branch}' is merged into default branch"));
                }
                Ok(false) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: format!(
                            "not merged into '{}' (use {} to force)",
                            ctx.default_branch, params.force_flag_label
                        ),
                    });
                    continue;
                }
                Err(e) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: format!(
                            "failed to check merge status: {e} (use {} to force)",
                            params.force_flag_label
                        ),
                    });
                    continue;
                }
            }
        }

        // Determine remote tracking info for this branch
        let (remote_name, remote_branch_name) = resolve_remote_tracking(ctx, branch);

        // Check 5: Local/remote in sync (skip with --force, keep_local_branch,
        // or the merge cleanup's own validation)
        if !force
            && !keep_local_branch
            && !skip_merge_validation
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

        // Check 6: Daft-file provenance guard. Classify the worktree's
        // untracked daft files against their recorded seeds: pristine or
        // already-subsumed copies pass silently (deleting them loses
        // nothing — including the stale-but-untouched copy a moved-on
        // target used to false-refuse). Refined copies are real user data:
        // forced removals plan a stash-discard, unforced ones go through
        // the consolidation prompt (non-interactive contexts answer Abort
        // and produce the refusal). The plan is resolved HERE, during
        // validation, so execution never prompts and all-or-nothing
        // validation semantics are preserved.
        //
        // Unlike the old divergence guard, `keep_local_branch` does NOT
        // exempt: the worktree directory is deleted either way, so its
        // refined files are equally at stake.
        let mut daft_files = DaftFilePlan::Nothing;
        if let Some(ref wt) = wt_path
            && wt.is_dir()
        {
            let target_wt = worktree_map.get(ctx.default_branch.as_str());
            let classes = visitor_seeds::classify_in_scope_files(
                seeds.as_ref(),
                branch,
                wt,
                target_wt.map(PathBuf::as_path),
            );
            let blocking: Vec<FileClass> = visitor_seeds::blocking_files(&classes)
                .into_iter()
                .cloned()
                .collect();

            if !blocking.is_empty() {
                if force {
                    daft_files = DaftFilePlan::Discard(
                        blocking.iter().map(|c| c.filename.clone()).collect(),
                    );
                } else {
                    match plan_refined_files(ctx, branch, wt, target_wt, &blocking, params, sink) {
                        Ok(plan) => daft_files = plan,
                        Err(message) => {
                            errors.push(ValidationError {
                                branch: branch.clone(),
                                message,
                            });
                            continue 'branches;
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
            worktree_only: false,
            daft_files,
        });
    }

    (validated, errors)
}

/// Build the consolidation/discard plan for a branch whose daft files are
/// refined (or provenance-less) and not subsumed by the target. Returns the
/// refusal message as `Err` when the user (or a non-interactive context)
/// aborts.
fn plan_refined_files(
    ctx: &BranchDeleteContext,
    branch: &str,
    wt: &Path,
    target_wt: Option<&PathBuf>,
    blocking: &[FileClass],
    params: &BranchDeleteParams,
    sink: &mut (impl ProgressSink + ConsolidationPrompter),
) -> std::result::Result<DaftFilePlan, String> {
    let refusal = |target_display: &str| {
        let example = blocking
            .first()
            .map(|c| c.filename.as_str())
            .unwrap_or("daft.yml");
        format!(
            "worktree '{}' has refined daft files ({}); consolidate with \
             `daft file merge {}/{example} {}/{example}` or re-run with {} to discard",
            wt.display(),
            blocking
                .iter()
                .map(|c| c.filename.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            target_display,
            wt.display(),
            params.force_flag_label,
        )
    };

    // No target worktree: nothing to consolidate into — the only options
    // are refusing or discarding, and discard requires the explicit force.
    let Some(target_wt) = target_wt else {
        return Err(format!(
            "worktree '{}' has refined daft files ({}) and the default branch \
             '{}' has no worktree to consolidate into; check it out first or \
             re-run with {} to discard",
            wt.display(),
            blocking
                .iter()
                .map(|c| c.filename.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            ctx.default_branch,
            params.force_flag_label,
        ));
    };

    // Dry-run the consolidation per file so the prompt can show exactly
    // what would happen.
    let prepared: Vec<PreparedConsolidation> = blocking
        .iter()
        .map(|class| prepare_consolidation(ctx, branch, wt, target_wt, class))
        .collect();

    let request = ConsolidationRequest {
        branch: branch.to_string(),
        worktree_display: wt.display().to_string(),
        target_display: target_wt.display().to_string(),
        files: prepared.iter().map(|p| p.summary.clone()).collect(),
    };

    match sink.on_refined(&request) {
        ConsolidationChoice::Abort => Err(refusal(&target_wt.display().to_string())),
        ConsolidationChoice::Discard => Ok(DaftFilePlan::Discard(
            blocking.iter().map(|c| c.filename.clone()).collect(),
        )),
        ConsolidationChoice::Consolidate => {
            let mut resolved_files = Vec::new();
            for prepared in prepared {
                let content = match prepared.resolution {
                    ConsolidationResolution::Resolved(content) => content,
                    ConsolidationResolution::NeedsSide {
                        target_priority,
                        source_priority,
                    } => {
                        match sink.on_conflicts(
                            &prepared.summary.filename,
                            &prepared.summary.conflict_keys,
                        ) {
                            ConflictSide::Target => target_priority,
                            ConflictSide::Source => source_priority,
                            ConflictSide::Abort => {
                                return Err(refusal(&target_wt.display().to_string()));
                            }
                        }
                    }
                };
                resolved_files.push((prepared.summary.filename.clone(), content));
            }
            Ok(DaftFilePlan::Consolidate(resolved_files))
        }
    }
}

/// A file's dry-run consolidation: the prompt summary plus the resolved
/// content (or both side-resolutions when conflicted keys need a choice).
struct PreparedConsolidation {
    summary: RefinedFileSummary,
    resolution: ConsolidationResolution,
}

enum ConsolidationResolution {
    Resolved(String),
    NeedsSide {
        target_priority: String,
        source_priority: String,
    },
}

/// Compute what consolidating one file would write into the target.
///
/// With a seed: a real three-way merge (`merge3`) — adopted keys and
/// conflicts reported per key path. Without a usable base (NoSeed,
/// unparseable YAML): whole-file mode — the legacy two-way source-wins
/// overlay, labelled as such in the summary so the user knows what they
/// are accepting.
fn prepare_consolidation(
    ctx: &BranchDeleteContext,
    branch: &str,
    wt: &Path,
    target_wt: &Path,
    class: &FileClass,
) -> PreparedConsolidation {
    use crate::hooks::config_merge::{merge_configs, merge3};
    use crate::hooks::yaml_config_loader::parse_yaml_config_str;

    let filename = &class.filename;
    let source_str = std::fs::read_to_string(wt.join(filename)).unwrap_or_default();
    let target_path = target_wt.join(filename);

    // Target has no such file: consolidation is a verbatim copy — comments
    // and formatting preserved, nothing to merge into.
    if !target_path.is_file() {
        return PreparedConsolidation {
            summary: RefinedFileSummary {
                filename: filename.clone(),
                adopt_keys: vec!["(entire file — target has none)".to_string()],
                conflict_keys: Vec::new(),
                whole_file: false,
            },
            resolution: ConsolidationResolution::Resolved(source_str),
        };
    }

    let target_str = std::fs::read_to_string(&target_path).unwrap_or_default();
    let seed_content = (class.class == SeedClass::Refined)
        .then(|| {
            SeedsContext::open(&ctx.git_dir)
                .and_then(|seeds| seeds.get_seed(branch, filename))
                .map(|row| row.content)
        })
        .flatten();

    let parsed = (
        seed_content.as_deref().map(parse_yaml_config_str),
        parse_yaml_config_str(&source_str),
        parse_yaml_config_str(&target_str),
    );

    if let (Some(Ok(base)), Ok(source), Ok(target)) = parsed {
        // Three-way: ours = target (it survives), theirs = source.
        let outcome = merge3(&base, &target, &source);
        if outcome.conflicts.is_empty() {
            let content =
                serde_yaml::to_string(&outcome.merged).unwrap_or_else(|_| source_str.clone());
            return PreparedConsolidation {
                summary: RefinedFileSummary {
                    filename: filename.clone(),
                    adopt_keys: outcome.took_from_theirs,
                    conflict_keys: Vec::new(),
                    whole_file: false,
                },
                resolution: ConsolidationResolution::Resolved(content),
            };
        }
        // Conflicted: pre-compute both side-resolutions. Swapping ours and
        // theirs flips which side wins the conflicted keys while one-sided
        // changes from both sides still flow through.
        let target_priority =
            serde_yaml::to_string(&outcome.merged).unwrap_or_else(|_| target_str.clone());
        let source_priority = serde_yaml::to_string(&merge3(&base, &source, &target).merged)
            .unwrap_or_else(|_| source_str.clone());
        return PreparedConsolidation {
            summary: RefinedFileSummary {
                filename: filename.clone(),
                adopt_keys: outcome.took_from_theirs,
                conflict_keys: outcome.conflicts,
                whole_file: false,
            },
            resolution: ConsolidationResolution::NeedsSide {
                target_priority,
                source_priority,
            },
        };
    }

    // No usable base: whole-file mode (legacy two-way source-wins).
    let content = match (
        parse_yaml_config_str(&target_str),
        parse_yaml_config_str(&source_str),
    ) {
        (Ok(target), Ok(source)) => serde_yaml::to_string(&merge_configs(target, source))
            .unwrap_or_else(|_| source_str.clone()),
        _ => source_str.clone(),
    };
    PreparedConsolidation {
        summary: RefinedFileSummary {
            filename: filename.clone(),
            adopt_keys: Vec::new(),
            conflict_keys: Vec::new(),
            whole_file: true,
        },
        resolution: ConsolidationResolution::Resolved(content),
    }
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

    // Apply the daft-file plan resolved at validation time. The target
    // worktree is only ever written by an explicit Consolidate choice;
    // Discard stashes the refinements and never touches the target;
    // pristine/subsumed copies (Nothing) are simply deleted with the
    // worktree. (The old behavior — silently source-wins-merging the
    // removed worktree's files into the target — is exactly the data-loss
    // bug this replaces.)
    if !remote_only && let Some(ref wt_path) = branch.worktree_path {
        match &branch.daft_files {
            DaftFilePlan::Nothing => {}
            DaftFilePlan::Consolidate(files) => {
                if let Some(target_wt) = worktree_map.get(&ctx.default_branch) {
                    for (filename, content) in files {
                        match std::fs::write(target_wt.join(filename), content) {
                            Ok(()) => sink.on_warning(&format!(
                                "Consolidated {filename} refinements from '{}' into {}",
                                branch.name,
                                target_wt.display()
                            )),
                            Err(e) => result.errors.push(format!(
                                "Failed to consolidate {filename} into {}: {e}",
                                target_wt.display()
                            )),
                        }
                    }
                }
            }
            DaftFilePlan::Discard(files) => {
                for filename in files {
                    let file = wt_path.join(filename);
                    match visitor_seeds::stash_file(
                        &ctx.git_dir,
                        visitor_seeds::StashKind::Discarded,
                        &branch.name,
                        &file,
                    ) {
                        Some(dest) => sink.on_warning(&format!(
                            "Discarded {filename} refinements from '{}' — saved to {}",
                            branch.name,
                            dest.display()
                        )),
                        None => sink.on_warning(&format!(
                            "Discarded {filename} refinements from '{}' (stash copy failed; \
                             the file is gone with the worktree)",
                            branch.name
                        )),
                    }
                }
            }
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

    // The worktree is gone — its seed provenance rows are meaningless now
    // (a future re-checkout of the same branch re-seeds). Best-effort.
    if result.worktree_removed
        && let Some(seeds) = crate::hooks::visitor_seeds::SeedsContext::open(&ctx.git_dir)
    {
        seeds.delete_seeds_for_branch(&branch.name);
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
            daft_files: DaftFilePlan::Nothing,
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
            daft_files: DaftFilePlan::Nothing,
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
            daft_files: DaftFilePlan::Nothing,
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
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, &mut bridge).expect("keep_local_branch should succeed");

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
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, &mut bridge).unwrap();

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
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "merge".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };

        let mut output = TestOutput::new();
        // Use with_trust_db so the hook runs with explicit Allow trust.
        // Set trust for the canonical git_dir path (what get_git_common_dir() returns).
        let canonical_git_dir = tmp.path().join(".git").canonicalize().unwrap();
        let mut trust_db = crate::hooks::TrustDatabase::default();
        trust_db.set_trust_level(&canonical_git_dir, crate::hooks::TrustLevel::Allow);
        let executor = HookExecutor::with_trust_db(HooksConfig::default(), trust_db);
        let mut bridge = CommandBridge::new(&mut output, executor);
        let bd_result = execute(&params, &mut bridge).unwrap();
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

    // ── Daft-file provenance guard tests ───────────────────────────────────

    /// Test bridge with a scriptable consolidation answer. Never touches a
    /// terminal — unit tests must not route through CommandBridge's real
    /// prompt, which would block on a keypress when cargo test runs under a
    /// TTY.
    struct ScriptedBridge {
        choice: crate::core::ConsolidationChoice,
        side: crate::core::ConflictSide,
    }

    impl ScriptedBridge {
        fn aborting() -> Self {
            Self {
                choice: crate::core::ConsolidationChoice::Abort,
                side: crate::core::ConflictSide::Abort,
            }
        }
    }

    impl ProgressSink for ScriptedBridge {
        fn on_step(&mut self, _msg: &str) {}
        fn on_warning(&mut self, _msg: &str) {}
        fn on_debug(&mut self, _msg: &str) {}
    }

    impl crate::core::HookRunner for ScriptedBridge {
        fn run_hook(
            &mut self,
            _ctx: &crate::hooks::HookContext,
        ) -> anyhow::Result<crate::core::HookOutcome> {
            Ok(crate::core::HookOutcome {
                success: true,
                skipped: true,
                skip_reason: None,
            })
        }
    }

    impl crate::core::ConsolidationPrompter for ScriptedBridge {
        fn on_refined(
            &mut self,
            _req: &crate::core::ConsolidationRequest,
        ) -> crate::core::ConsolidationChoice {
            self.choice
        }

        fn on_conflicts(&mut self, _filename: &str, _keys: &[String]) -> crate::core::ConflictSide {
            self.side
        }
    }

    /// Regression test: the provenance guard refuses branch-delete when a
    /// daft.local.yml in the feature worktree has refinements the default
    /// branch worktree lacks (and no interactive consolidation happens).
    ///
    /// To isolate Check 6 (daft files) from Check 3 (uncommitted changes), we
    /// add daft.local.yml to .gitignore so git does not see it as dirty. This
    /// mirrors real usage: daft.local.yml is a personal overlay that should be
    /// gitignored in the repository.
    #[test]
    #[serial]
    fn divergence_guard_refuses_delete_when_local_yml_differs() {
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
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut bridge = ScriptedBridge::aborting();
        let result = execute(&params, &mut bridge).unwrap();

        assert!(
            !result.validation_errors.is_empty(),
            "should have a validation error when daft.local.yml diverges"
        );
        let message = &result.validation_errors[0].message;
        assert!(
            message.contains("refined daft files"),
            "error message must mention refined daft files, got: {message}"
        );
        assert!(
            message.contains("daft file merge"),
            "error message must point at the consolidation command, got: {message}"
        );
        assert!(
            message.contains("-D/--force"),
            "error message must name the caller's force flag, got: {message}"
        );
        // Feature worktree must NOT have been removed.
        assert!(
            feat_wt.exists(),
            "feature worktree must still exist after refusal"
        );
    }

    /// Regression test: --force discards refined daft files to the stash and
    /// NEVER writes them into the default-branch worktree (the old salvage
    /// behavior silently propagated them — issue #628).
    #[test]
    #[serial]
    fn divergence_guard_bypassed_with_force() {
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
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut bridge = ScriptedBridge::aborting();
        let result = execute(&params, &mut bridge).unwrap();

        assert!(
            result.validation_errors.is_empty(),
            "force should bypass the provenance guard, got: {:?}",
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

        // Force means DISCARD: the target worktree is never written...
        assert!(
            !tmp.path().join("daft.local.yml").exists(),
            "forced removal must not propagate the refined file into the \
             default-branch worktree"
        );
        // ...and the refinements land in the stash for recovery.
        let stash = tmp
            .path()
            .join(".git/.daft/discarded/feature/daft.local.yml");
        assert!(
            stash.is_file(),
            "discarded refinements must be stashed at {}",
            stash.display()
        );
        assert!(
            std::fs::read_to_string(&stash)
                .unwrap()
                .contains("echo personal"),
            "stash must hold the discarded content"
        );
    }

    /// Interactive consolidation: answering Consolidate merges the refined
    /// file into the default-branch worktree, then removes the worktree.
    #[test]
    #[serial]
    fn consolidation_choice_writes_target_then_removes() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);

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
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut bridge = ScriptedBridge {
            choice: crate::core::ConsolidationChoice::Consolidate,
            side: crate::core::ConflictSide::Abort,
        };
        let result = execute(&params, &mut bridge).unwrap();

        assert!(
            result.validation_errors.is_empty(),
            "consolidation answer must let the removal proceed, got: {:?}",
            result
                .validation_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert!(!feat_wt.exists(), "worktree must be removed");
        let consolidated = std::fs::read_to_string(tmp.path().join("daft.local.yml"))
            .expect("default-branch worktree must gain the consolidated file");
        assert!(
            consolidated.contains("echo personal"),
            "consolidated content must carry the refinement: {consolidated}"
        );
    }
}
