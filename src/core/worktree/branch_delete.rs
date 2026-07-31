//! Core logic for the `git-worktree-branch-delete` command.
//!
//! Deletes branches and their associated worktrees.

use crate::core::stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};
use crate::core::worktree::ports::{ForgeMergedWitness, NoopStageRunner};
use crate::core::worktree::push::{PushAction, push_with_hooks, resolve_delete_pre_push};
use crate::core::{
    ConflictSide, ConsolidationChoice, ConsolidationPrompter, ConsolidationRequest, HookRunner,
    ProgressSink, RefinedFileSummary,
};
use crate::executor::presenter::JobPresenter;
use crate::git::GitCommand;
use crate::hooks::visitor_seeds::{self, FileClass, SeedsContext};
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::get_default_branch_local;
use crate::settings::{PruneCdTarget, PushVerify};
use crate::{get_git_common_dir, get_project_root};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
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
    /// When the remote-branch delete runs the repo's pre-push hook
    /// (`daft.pushVerify`). A delete is statically ref-only, so `auto`
    /// skips the hook (#747); `always` re-arms it for ref-policy gates.
    pub push_verify: PushVerify,
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
    /// Skip the repo's pre-push hook on the remote-branch delete.
    no_verify: bool,
    /// When the remote-branch delete runs the repo's pre-push hook (#747).
    push_verify: PushVerify,
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
    /// Why Check 4 concluded this deletion loses nothing, surfaced as a
    /// timeline annotation. `None` when the check was skipped (--force,
    /// keep_local_branch, merge's own validation) — then nothing was proven
    /// and the rail must not claim otherwise.
    safe_because: Option<SafeBecause>,
    /// What to do with the worktree's untracked daft files before removal.
    daft_files: DaftFilePlan,
    /// A daft-created sandbox (#53): `name` is the directory name, there is
    /// no branch to delete (`worktree_only` is also set), hooks get the
    /// branchless context, and identity cleanup goes through the sandbox
    /// forget path.
    is_sandbox: bool,
    /// The sandbox's pinned commit, threaded through for the removal hooks'
    /// `DAFT_COMMIT`. `None` for branch targets.
    pinned_commit: Option<String>,
}

/// The proof Check 4 accepted that deleting this branch destroys no work.
///
/// Two independent sufficient conditions, not one and a fallback. "Merged
/// into the default branch" is the historical proof; "still whole on a remote
/// this run will not touch" (#783) is equally sufficient and far more common
/// day to day — work pushed, PR open, worktree no longer needed. Keeping both
/// as named variants is what lets the rail say *which* one held, rather than
/// silently dropping a refusal.
enum SafeBecause {
    /// Contained in the default branch. `via` names the PR/MR when the forge
    /// is what proved it (#737) — git found nothing in the branch's own
    /// history, so an unexplained deletion would look wrong.
    Merged {
        into: String,
        via: Option<crate::core::worktree::forge_ref::ForgeBranchRef>,
    },
    /// Identical to a remote branch that survives this run, verified on the
    /// wire rather than from the local tracking cache. `remote_ref` is the
    /// `<remote>/<branch>` the commits remain reachable at.
    FullyPushed { remote_ref: String },
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

/// One removal target after argument resolution: a branch, or a daft-created
/// sandbox (#53). Foreign detached worktrees resolve to neither — they keep
/// the historical refusal.
enum ResolvedTarget {
    Branch(String),
    Sandbox(SandboxTarget),
}

/// A sandbox removal target: identified by its directory name, with the
/// provenance the safety checks need.
struct SandboxTarget {
    dirname: String,
    path: PathBuf,
    /// The commit the sandbox was pinned at when created. `None` when the
    /// record predates the pin or was written without one — the moved-HEAD
    /// check then has nothing to compare and stays silent.
    pinned_commit: Option<String>,
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Execute the branch-delete operation.
///
/// `presenter` reports the pre-push hook run on remote-branch deletes
/// (#599); pass `None` to skip that reporting (the hook is still honored).
/// `witness` lets Check 4 accept a branch the forge merged but git cannot
/// place (#737); pass [`NoopForgeWitness`] to decide on git alone.
pub fn execute(
    params: &BranchDeleteParams,
    presenter: Option<&Arc<dyn JobPresenter>>,
    witness: &dyn ForgeMergedWitness,
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
        no_verify: params.no_verify,
        push_verify: params.push_verify,
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

    // Resolve arguments: each arg can be a branch name, a worktree path, or
    // a sandbox name. The identity records are what tell a daft-created
    // sandbox apart from a foreign detached worktree.
    let identities = crate::core::worktree::identity_store::read_identities(&ctx.git_dir);
    let resolved = resolve_branch_args(
        &params.branches,
        &worktree_entries,
        &ctx.project_root,
        &identities,
        &git,
        sink,
    )?;

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
        witness,
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

    // Commit the execution plan (#651): validation is done, every prompt has
    // fired, mutation is about to begin. Rows are ordered exactly as
    // `execute_deletions` will run them — regular branches first, the
    // current-worktree branch deferred to last.
    let (deferred, regular): (Vec<&ValidatedBranch>, Vec<&ValidatedBranch>) =
        validated.iter().partition(|b| b.is_current_worktree);
    let exec_order: Vec<&ValidatedBranch> =
        regular.iter().chain(deferred.iter()).copied().collect();
    let hook_rows = HookRowPlan::probe(&exec_order, &ctx.source_worktree, sink);
    sink.on_plan(build_plan(&exec_order, params, &hook_rows));

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

// ── Timeline plan (#651) ───────────────────────────────────────────────────

/// Which hook-phase rows the committed plan includes: a row is planned only
/// when the phase has hooks discoverable at plan time — the rail lists only
/// work that happens, and remove's hook config sources exist and are exact
/// before the plan commits. Pre-remove hooks are read from each branch's
/// own worktree; post-remove hooks from the source worktree (the tree the
/// executor reads once the target is gone). Runtime discovery stays
/// authoritative: hooks run regardless of planning, and a planned row can
/// still vanish (condition skips) or turn yellow (trust refusal).
struct HookRowPlan {
    /// Parallel to the plan's execution order.
    pre_remove: Vec<bool>,
    post_remove: bool,
}

impl HookRowPlan {
    fn probe(
        exec_order: &[&ValidatedBranch],
        source_worktree: &Path,
        runner: &impl HookRunner,
    ) -> Self {
        let pre_remove = exec_order
            .iter()
            .map(|b| {
                b.worktree_path
                    .as_deref()
                    .is_some_and(|wt| runner.hook_phase_has_work(HookType::PreRemove, wt))
            })
            .collect();
        let post_remove = exec_order.iter().any(|b| b.worktree_path.is_some())
            && runner.hook_phase_has_work(HookType::PostRemove, source_worktree);
        Self {
            pre_remove,
            post_remove,
        }
    }
}

/// Build the plan rows for the branches in execution order. Steps mirror the
/// conditionals of `delete_single_branch` exactly. Remote fate shows only
/// when remote deletion is in scope for the invocation: the `DeleteRemote`
/// step, or a dim `no remote branch` note when there is no upstream to
/// delete. A daft configured local-only never mentions remotes.
fn build_plan(
    exec_order: &[&ValidatedBranch],
    params: &BranchDeleteParams,
    hook_rows: &HookRowPlan,
) -> PlanCommit {
    let multi = exec_order.len() > 1;
    // Replace the seeded header with what validation actually settled on.
    // The seed already resolves a path shorthand to its branch (#813), so
    // this is a no-op for the plain single-target case; it earns its keep
    // when the *count* moves — a wildcard expanding to N sandboxes, or
    // validation dropping targets down to one.
    let header = if multi {
        format!("Removing {} branches", exec_order.len())
    } else {
        format!("Removing {}", exec_order[0].name)
    };
    let mut rows = Vec::new();

    for (i, branch) in exec_order.iter().enumerate() {
        let key = |id: StageId| StepKey::scoped(id, branch.name.clone());
        if multi {
            rows.push(Row::Group {
                label: branch.name.clone(),
            });
        }

        let has_worktree = branch.worktree_path.is_some();
        let deletes_remote = !params.keep_local_branch
            && !branch.worktree_only
            && (params.delete_remote || params.remote_only)
            && branch.remote_name.is_some()
            && branch.remote_branch_name.is_some();

        if has_worktree && hook_rows.pre_remove[i] {
            rows.push(Row::Step(StepSpec::new(key(StageId::PreRemoveHooks))));
        }
        if deletes_remote {
            let annotation = format!(
                "{}/{}",
                branch.remote_name.as_deref().unwrap_or_default(),
                branch.remote_branch_name.as_deref().unwrap_or_default()
            );
            rows.push(Row::Step(
                StepSpec::new(key(StageId::DeleteRemote)).with_annotation(annotation),
            ));
        }
        if let Some(ref wt) = branch.worktree_path {
            rows.push(Row::Step(
                StepSpec::new(key(StageId::RemoveWorktree)).with_annotation(display_path(wt)),
            ));
        }
        if !params.remote_only && !params.keep_local_branch && !branch.worktree_only {
            let mut spec = StepSpec::new(key(StageId::DeleteLocalBranch));
            if let Some(ref reason) = branch.safe_because {
                // The annotation is the only account of why deleting this is
                // safe, so it names the specific proof: the PR when the forge
                // is what found the merge (the branch's own history shows
                // nothing), or the remote the commits stay reachable at.
                spec = spec.with_annotation(match reason {
                    SafeBecause::Merged { into, via: Some(v) } => {
                        format!("was merged into {into} via {}", v.short())
                    }
                    SafeBecause::Merged { into, via: None } => format!("was merged into {into}"),
                    SafeBecause::FullyPushed { remote_ref } => {
                        format!("fully pushed to {remote_ref}")
                    }
                });
            }
            rows.push(Row::Step(spec));
        }
        if has_worktree && hook_rows.post_remove {
            rows.push(Row::Step(StepSpec::new(key(StageId::PostRemoveHooks))));
        }

        // Remote fate is a topic only while remote deletion is in scope for
        // this invocation. When configuration takes remotes out of scope
        // (`daft.branchDelete.remote` off — the default, and what
        // `daft config remote-sync` "local only" sets — or `--local`), the
        // rail never mentions them, mirroring the create rail's push row.
        let remote_in_scope = params.delete_remote || params.remote_only;
        if !params.keep_local_branch {
            if branch.worktree_only {
                let text = if branch.is_sandbox {
                    "sandbox \u{2014} no branch to delete"
                } else if remote_in_scope {
                    "branch and remote kept (default branch)"
                } else {
                    "branch kept (default branch)"
                };
                rows.push(Row::Note {
                    text: text.to_string(),
                });
            } else if remote_in_scope && !deletes_remote {
                // In scope but no DeleteRemote step: the branch has no
                // upstream, so there was nothing to delete.
                rows.push(Row::Note {
                    text: "no remote branch".to_string(),
                });
            }
        }
    }

    PlanCommit::new(rows).with_header(header)
}

/// Path annotation for a worktree row: relative to the current directory
/// when that is shorter to read, absolute otherwise.
///
/// The path *being* the current directory relativizes to the empty string,
/// which renderers drop as "no annotation" — so the row would lose the very
/// path it exists to show (`daft push` from the branch's own worktree is the
/// common case). Render the relative spelling for "here" instead.
pub(crate) fn display_path(path: &Path) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| pathdiff::diff_paths(path, &cwd))
        .filter(|rel| rel.components().count() <= path.components().count())
        .map(|rel| rel.display().to_string())
        .map(|rel| if rel.is_empty() { ".".to_string() } else { rel })
        .unwrap_or_else(|| path.display().to_string())
}

// ── Header seed ────────────────────────────────────────────────────────────

/// The rail header for a removal, resolved before the rail opens (#813).
///
/// The rail's header carries *identity* — what is being acted on — while the
/// annotation column beside each step carries *location*. A seed built from
/// raw args alone puts a path in the identity slot, so `daft remove .`
/// announces that it is removing `.`, which names nothing the user
/// recognizes. Resolving here means the first frame already reads
/// `Removing feat/thing`, and the annotation still reads `.` — the two
/// together say "the thing at `.` is feat/thing" without narrating it.
///
/// It also keeps the header agreeing with the text below it on the paths
/// that never commit a plan. A dirty worktree aborts with
/// `cannot delete 'feat/thing': has uncommitted changes` while the seed is
/// the only line still on screen; a header reading `.` (or a bare count)
/// disagrees with that error, and no later replacement can fix it because
/// [`PlanCommit::header`] only lands when a plan commits.
///
/// Multi-target keeps its count form: a count is true from raw args and
/// stays true, and the committed plan names each branch on its own group
/// row.
pub fn header_seed(params: &BranchDeleteParams) -> String {
    match params.branches.as_slice() {
        [only] => format!(
            "Removing {}",
            display_identity(only, params.use_gitoxide, params.is_quiet)
        ),
        rest => format!("Removing {} branches", rest.len()),
    }
}

/// The identity to render for one raw removal argument.
///
/// Display-only and best-effort by construction: it returns a `String` for
/// rendering, never a target, so it cannot change which entity gets removed
/// — [`resolve_branch_args`] alone decides that. Anything it fails to
/// resolve comes back as the user's own spelling, which is what the
/// validation error will echo too: replacing an unresolvable `../typo` with
/// a guess is worse than showing a path.
fn display_identity(arg: &str, use_gitoxide: bool, quiet: bool) -> String {
    let verbatim = || arg.to_string();
    let (Ok(project_root), Ok(git_dir)) = (get_project_root(), get_git_common_dir()) else {
        return verbatim();
    };
    let git = GitCommand::new(quiet).with_gitoxide(use_gitoxide);
    let Ok(entries) = parse_worktree_list(&git) else {
        return verbatim();
    };

    match resolve_single_arg(arg, &entries, &project_root) {
        ResolveResult::Branch(name) => name,
        // A daft sandbox has no branch, so its dirname is its identity. A
        // detached worktree daft has no record of keeps the user's spelling:
        // the removal refuses it either way, and the refusal quotes the path.
        ResolveResult::DetachedHead(path) => {
            let identities = crate::core::worktree::identity_store::read_identities(&git_dir);
            sandbox_target_by_path(&path, &identities)
                .map_or_else(verbatim, |target| target.dirname)
        }
        // A branch name, a sandbox dirname, or a path that matched nothing.
        // All three are already the best spelling available here.
        ResolveResult::PassThrough => verbatim(),
    }
}

// ── Argument resolution ────────────────────────────────────────────────────

/// Resolve each argument to a removal target.
///
/// Arguments can be:
///   - A branch name (passed through as-is if no worktree path matches)
///   - A worktree path (absolute or relative to cwd, including ".")
///   - A sandbox's directory name or path (#53), recognized through the
///     identity records — a foreign detached worktree matches none of these
///     and keeps the historical refusal.
fn resolve_branch_args(
    args: &[String],
    worktree_entries: &[WorktreeListEntry],
    project_root: &Path,
    identities: &HashMap<String, crate::store::models::WorktreeIdentityRow>,
    git: &GitCommand,
    sink: &mut dyn ProgressSink,
) -> Result<Vec<ResolvedTarget>> {
    let mut resolved = Vec::with_capacity(args.len());
    // Sandbox dirnames already targeted by an earlier arg or pattern — a
    // worktree can only be removed once, so overlaps collapse to one target
    // instead of a doomed second delete.
    let mut claimed: HashSet<String> = HashSet::new();

    for arg in args {
        // Wildcard tier: `*`/`?` are illegal in git refnames and absent from
        // sandbox dirnames, so a metachar can only mean a pattern — the
        // branch-precedence question the dirname tier wrestles with cannot
        // arise here.
        if is_wildcard_pattern(arg) {
            expand_sandbox_pattern(
                arg,
                identities,
                worktree_entries,
                &mut claimed,
                &mut resolved,
                sink,
            )?;
            continue;
        }
        match resolve_single_arg(arg, worktree_entries, project_root) {
            ResolveResult::Branch(name) => {
                sink.on_step(&format!("Resolved path '{}' to branch '{}'", arg, name));
                resolved.push(ResolvedTarget::Branch(name));
            }
            ResolveResult::PassThrough => {
                // Dirname tier: a name that is not a branch but is a recorded
                // sandbox targets the sandbox. Branch precedence is
                // load-bearing — a branch sharing a sandbox's spelling must
                // keep meaning the branch.
                let is_branch = git
                    .show_ref_exists(&format!("refs/heads/{arg}"))
                    .unwrap_or(false);
                match (!is_branch)
                    .then(|| sandbox_target_by_name(arg, identities, worktree_entries))
                    .flatten()
                {
                    Some(target) if !claimed.insert(target.dirname.clone()) => {
                        sink.on_step(&format!(
                            "Sandbox '{}' already targeted; skipping duplicate",
                            target.dirname
                        ));
                    }
                    Some(target) => {
                        sink.on_step(&format!("Resolved '{arg}' to sandbox worktree"));
                        resolved.push(ResolvedTarget::Sandbox(target));
                    }
                    None => resolved.push(ResolvedTarget::Branch(arg.clone())),
                }
            }
            ResolveResult::DetachedHead(path) => {
                match sandbox_target_by_path(&path, identities) {
                    Some(target) if !claimed.insert(target.dirname.clone()) => {
                        sink.on_step(&format!(
                            "Sandbox '{}' already targeted; skipping duplicate",
                            target.dirname
                        ));
                    }
                    Some(target) => {
                        sink.on_step(&format!(
                            "Resolved path '{arg}' to sandbox '{}'",
                            target.dirname
                        ));
                        resolved.push(ResolvedTarget::Sandbox(target));
                    }
                    // Not a daft sandbox: keep the historical protection for
                    // detached worktrees daft has no record of.
                    None => anyhow::bail!(
                        "worktree at '{}' has a detached HEAD; specify a branch name instead",
                        path.display()
                    ),
                }
            }
        }
    }

    Ok(resolved)
}

/// The sandbox target for a detached worktree at `path`, if the identity
/// records say daft created it as one.
fn sandbox_target_by_path(
    path: &Path,
    identities: &HashMap<String, crate::store::models::WorktreeIdentityRow>,
) -> Option<SandboxTarget> {
    let id = crate::core::worktree::identity_store::worktree_id_for(path)?;
    let row = identities.get(&id).filter(|row| row.kind.is_sandbox())?;
    Some(SandboxTarget {
        dirname: row.branch.clone(),
        path: path.to_path_buf(),
        pinned_commit: row.pinned_commit.clone(),
    })
}

/// The sandbox target recorded under `name`, if its worktree is still a live
/// detached entry.
fn sandbox_target_by_name(
    name: &str,
    identities: &HashMap<String, crate::store::models::WorktreeIdentityRow>,
    worktree_entries: &[WorktreeListEntry],
) -> Option<SandboxTarget> {
    let row = identities
        .values()
        .find(|row| row.kind.is_sandbox() && row.branch == name)?;
    let recorded = PathBuf::from(&row.worktree_path);
    let canonical = std::fs::canonicalize(&recorded).ok()?;
    let live = worktree_entries.iter().any(|e| {
        e.branch.is_none()
            && !e.is_bare
            && std::fs::canonicalize(&e.path).is_ok_and(|p| p == canonical)
    });
    live.then(|| SandboxTarget {
        dirname: row.branch.clone(),
        path: recorded,
        pinned_commit: row.pinned_commit.clone(),
    })
}

/// True when `arg` is a wildcard pattern over sandbox names. Git refuses
/// `*` and `?` in refnames and the sandbox naming charset excludes them, so
/// a metachar cannot belong to a branch or sandbox name.
fn is_wildcard_pattern(arg: &str) -> bool {
    arg.contains(['*', '?'])
}

/// Glob-style match over a whole name: `*` matches any run of characters
/// (including none), `?` matches exactly one, everything else is literal.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            backtrack = Some((pi, ti));
            pi += 1;
        } else if let Some((star, mark)) = backtrack {
            // The literal run after the last `*` failed — let the star
            // swallow one more character and retry.
            backtrack = Some((star, mark + 1));
            pi = star + 1;
            ti = mark + 1;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

/// Expand a wildcard argument over the recorded sandbox names into removal
/// targets.
///
/// Patterns deliberately never match branches: removing a branch also
/// deletes its remote, and fleet-scale branch cleanup is `daft prune`'s
/// job. A pattern matching no live sandbox fails the whole command — a glob
/// silently expanding to nothing would turn "remove these" into "remove
/// nothing".
fn expand_sandbox_pattern(
    pattern: &str,
    identities: &HashMap<String, crate::store::models::WorktreeIdentityRow>,
    worktree_entries: &[WorktreeListEntry],
    claimed: &mut HashSet<String>,
    resolved: &mut Vec<ResolvedTarget>,
    sink: &mut dyn ProgressSink,
) -> Result<()> {
    let mut names: Vec<&str> = identities
        .values()
        .filter(|row| row.kind.is_sandbox() && wildcard_match(pattern, &row.branch))
        .map(|row| row.branch.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();

    let mut live = 0usize;
    for name in names {
        // Stale records (worktree already gone) are not matches.
        let Some(target) = sandbox_target_by_name(name, identities, worktree_entries) else {
            continue;
        };
        live += 1;
        if !claimed.insert(target.dirname.clone()) {
            sink.on_step(&format!(
                "Sandbox '{name}' already targeted; skipping duplicate"
            ));
            continue;
        }
        sink.on_step(&format!("Pattern '{pattern}' matched sandbox '{name}'"));
        resolved.push(ResolvedTarget::Sandbox(target));
    }

    if live == 0 {
        let mut branches: Vec<&str> = worktree_entries
            .iter()
            .filter_map(|e| e.branch.as_deref())
            .filter(|b| wildcard_match(pattern, b))
            .collect();
        branches.sort_unstable();
        branches.dedup();
        if branches.is_empty() {
            anyhow::bail!(
                "pattern '{pattern}' matches no sandbox worktrees \
                 (wildcards match sandbox names, not paths or branches)"
            );
        }
        let noun = if branches.len() == 1 {
            "branch"
        } else {
            "branches"
        };
        let list = branches
            .iter()
            .map(|b| format!("'{b}'"))
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "pattern '{pattern}' matches no sandbox worktrees; it does match {noun} {list} — \
             wildcards never target branches, name them explicitly"
        );
    }
    Ok(())
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

/// Validate a sandbox removal target: it must be clean (unless forced) and
/// still sitting at its pinned commit (unless forced) — commits made on a
/// detached HEAD die with the worktree, so a moved HEAD refuses with the
/// promotion hint rather than silently unreaching someone's work.
fn validate_sandbox_target(
    ctx: &BranchDeleteContext,
    sandbox: &SandboxTarget,
    params: &BranchDeleteParams,
    current_wt_path: Option<&PathBuf>,
    sink: &mut dyn ProgressSink,
) -> Result<ValidatedBranch, ValidationError> {
    // Clean: the same protection branch worktrees get from Check 3.
    if !params.force {
        match ctx.git.has_uncommitted_changes_in(&sandbox.path) {
            Ok(true) => {
                return Err(ValidationError {
                    branch: sandbox.dirname.clone(),
                    message: "has uncommitted changes in worktree (use -D to force)".to_string(),
                });
            }
            Ok(false) => {}
            Err(e) => {
                return Err(ValidationError {
                    branch: sandbox.dirname.clone(),
                    message: format!(
                        "failed to check for uncommitted changes: {e} (use -D to force)"
                    ),
                });
            }
        }
    }

    // Pinned: a sandbox that moved off its creation commit holds work no
    // branch protects.
    if !params.force
        && let Some(pin) = sandbox.pinned_commit.as_deref()
        && let Some(head) = super::sandbox::worktree_head(&sandbox.path)
        && head != pin
    {
        return Err(ValidationError {
            branch: sandbox.dirname.clone(),
            message: format!(
                "has moved off its pinned commit ({} \u{2192} {}); commits made inside it \
                 may become unreachable after removal — promote them with `{}` from inside \
                 the sandbox first, or use {} to remove anyway",
                &pin[..pin.len().min(7)],
                &head[..head.len().min(7)],
                crate::daft_cmd("start <new-branch>"),
                params.force_flag_label
            ),
        });
    }

    // Path comparison only: `current_branch` is meaningless on a detached
    // HEAD, and a like-named branch elsewhere must not mark this row current.
    let is_current = current_wt_path.is_some_and(|current| {
        &sandbox.path == current
            || std::fs::canonicalize(&sandbox.path).ok() == std::fs::canonicalize(current).ok()
    });

    sink.on_step(&format!("Sandbox '{}' passed validation", sandbox.dirname));

    Ok(ValidatedBranch {
        name: sandbox.dirname.clone(),
        worktree_path: Some(sandbox.path.clone()),
        remote_name: None,
        remote_branch_name: None,
        is_current_worktree: is_current,
        // No branch to delete: reuse the worktree-only skips of the remote
        // and local-branch steps.
        worktree_only: true,
        safe_because: None,
        daft_files: DaftFilePlan::Nothing,
        is_sandbox: true,
        pinned_commit: sandbox.pinned_commit.clone(),
    })
}

#[cfg(test)]
mod sandbox_target_tests {
    use super::*;
    use crate::store::models::{WorktreeIdentityRow, WorktreeKind};

    fn sandbox_row(id: &str, dirname: &str, path: &Path) -> (String, WorktreeIdentityRow) {
        (
            id.to_string(),
            WorktreeIdentityRow {
                repo_hash: "repo".into(),
                worktree_id: id.into(),
                branch: dirname.into(),
                worktree_path: path.display().to_string(),
                updated_at: chrono::Utc::now(),
                kind: WorktreeKind::Canonical,
                source_spelling: Some(dirname.into()),
                pinned_commit: Some("a".repeat(40)),
            },
        )
    }

    fn detached_entry(path: &Path) -> WorktreeListEntry {
        WorktreeListEntry {
            path: path.to_path_buf(),
            branch: None,
            is_bare: false,
            is_detached: true,
            head: None,
        }
    }

    #[test]
    fn a_recorded_sandbox_with_a_live_detached_entry_resolves() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("v1");
        std::fs::create_dir_all(&wt).unwrap();
        let identities = HashMap::from([sandbox_row("wt-a", "v1", &wt)]);

        let target = sandbox_target_by_name("v1", &identities, &[detached_entry(&wt)])
            .expect("resolves to the sandbox");
        assert_eq!(target.dirname, "v1");
        assert_eq!(
            target.pinned_commit.as_deref(),
            Some("a".repeat(40).as_str())
        );
    }

    /// A record whose worktree is gone (or was never detached) must not
    /// resolve — the arg falls through to the branch path and its
    /// "branch not found" error.
    #[test]
    fn a_stale_or_attached_record_does_not_resolve() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("v1");
        std::fs::create_dir_all(&wt).unwrap();
        let identities = HashMap::from([sandbox_row("wt-a", "v1", &wt)]);

        // No live entry at all.
        assert!(sandbox_target_by_name("v1", &identities, &[]).is_none());
        // The entry at that path is attached to a branch.
        let attached = WorktreeListEntry {
            path: wt.clone(),
            branch: Some("v1".into()),
            is_bare: false,
            is_detached: false,
            head: None,
        };
        assert!(sandbox_target_by_name("v1", &identities, &[attached]).is_none());
    }

    /// A detached worktree daft has no record of is foreign: by path it must
    /// resolve to nothing so the historical refusal fires.
    #[test]
    fn a_foreign_detached_worktree_has_no_sandbox_target() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("foreign");
        std::fs::create_dir_all(&wt).unwrap();
        assert!(sandbox_target_by_path(&wt, &HashMap::new()).is_none());
    }

    #[test]
    fn wildcard_match_covers_star_question_literals_and_backtracking() {
        assert!(wildcard_match("main-fork*", "main-fork"));
        assert!(wildcard_match("main-fork*", "main-fork-2"));
        assert!(!wildcard_match("main-fork*", "main"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("*-fork-?", "main-fork-2"));
        assert!(!wildcard_match("*-fork-?", "main-fork-22"));
        assert!(wildcard_match("*a*b", "xaycb"));
        assert!(!wildcard_match("*a*b", "xayc"));
        assert!(wildcard_match("abc", "abc"));
        assert!(!wildcard_match("abc", "abd"));
        assert!(wildcard_match("", ""));
        assert!(!wildcard_match("", "x"));
        assert!(!wildcard_match("?", ""));
        assert!(wildcard_match("**", "x"));
    }

    fn resolved_names(resolved: &[ResolvedTarget]) -> Vec<&str> {
        resolved
            .iter()
            .map(|t| match t {
                ResolvedTarget::Branch(name) => name.as_str(),
                ResolvedTarget::Sandbox(s) => s.dirname.as_str(),
            })
            .collect()
    }

    #[test]
    fn a_pattern_expands_to_every_live_matching_sandbox() {
        let tmp = tempfile::tempdir().unwrap();
        let mut identities = HashMap::new();
        let mut entries = Vec::new();
        for name in ["main-fork", "main-fork-2", "v1.0"] {
            let wt = tmp.path().join(name);
            std::fs::create_dir_all(&wt).unwrap();
            let (id, mut row) = sandbox_row(&format!("wt-{name}"), name, &wt);
            if name.starts_with("main-fork") {
                row.kind = WorktreeKind::Fork;
            }
            identities.insert(id, row);
            entries.push(detached_entry(&wt));
        }

        let mut claimed = HashSet::new();
        let mut resolved = Vec::new();
        expand_sandbox_pattern(
            "main-fork*",
            &identities,
            &entries,
            &mut claimed,
            &mut resolved,
            &mut crate::core::NullSink,
        )
        .expect("pattern with matches expands");
        assert_eq!(resolved_names(&resolved), ["main-fork", "main-fork-2"]);
    }

    /// A pattern that matches nothing must abort the command, not quietly
    /// contribute zero targets.
    #[test]
    fn a_pattern_matching_no_sandbox_fails_closed() {
        let err = expand_sandbox_pattern(
            "nosuch*",
            &HashMap::new(),
            &[],
            &mut HashSet::new(),
            &mut Vec::new(),
            &mut crate::core::NullSink,
        )
        .expect_err("no matches must fail");
        assert!(err.to_string().contains("matches no sandbox worktrees"));
    }

    /// A record whose worktree is gone is not a match — a pattern over only
    /// stale records fails closed like any other zero-match pattern.
    #[test]
    fn a_pattern_over_only_stale_records_fails_closed() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("main-fork");
        std::fs::create_dir_all(&wt).unwrap();
        let identities = HashMap::from([sandbox_row("wt-a", "main-fork", &wt)]);

        // No live worktree entries: the record is stale.
        let err = expand_sandbox_pattern(
            "main-fork*",
            &identities,
            &[],
            &mut HashSet::new(),
            &mut Vec::new(),
            &mut crate::core::NullSink,
        )
        .expect_err("stale-only matches must fail");
        assert!(err.to_string().contains("matches no sandbox worktrees"));
    }

    /// Branches a pattern would textually match are named in the error so
    /// the refusal explains itself — but they are never targeted.
    #[test]
    fn a_pattern_matching_only_branches_names_them_in_the_error() {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("main-forky");
        std::fs::create_dir_all(&wt).unwrap();
        let attached = WorktreeListEntry {
            path: wt,
            branch: Some("main-forky".into()),
            is_bare: false,
            is_detached: false,
            head: None,
        };

        let mut resolved = Vec::new();
        let err = expand_sandbox_pattern(
            "main-fork*",
            &HashMap::new(),
            &[attached],
            &mut HashSet::new(),
            &mut resolved,
            &mut crate::core::NullSink,
        )
        .expect_err("branch-only matches must fail");
        let msg = err.to_string();
        assert!(msg.contains("branch 'main-forky'"));
        assert!(msg.contains("never target branches"));
        assert!(resolved.is_empty());
    }

    /// Overlapping patterns (or a pattern over an explicitly named sandbox)
    /// collapse to one target instead of a doomed second delete.
    #[test]
    fn a_pattern_skips_sandboxes_already_targeted() {
        let tmp = tempfile::tempdir().unwrap();
        let mut identities = HashMap::new();
        let mut entries = Vec::new();
        for name in ["main-fork", "main-fork-2"] {
            let wt = tmp.path().join(name);
            std::fs::create_dir_all(&wt).unwrap();
            let (id, row) = sandbox_row(&format!("wt-{name}"), name, &wt);
            identities.insert(id, row);
            entries.push(detached_entry(&wt));
        }

        let mut claimed = HashSet::from(["main-fork".to_string()]);
        let mut resolved = Vec::new();
        expand_sandbox_pattern(
            "main-fork*",
            &identities,
            &entries,
            &mut claimed,
            &mut resolved,
            &mut crate::core::NullSink,
        )
        .expect("live matches expand even when some are claimed");
        assert_eq!(resolved_names(&resolved), ["main-fork-2"]);
    }
}

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
    targets: &[ResolvedTarget],
    params: &BranchDeleteParams,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    witness: &dyn ForgeMergedWitness,
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
    // Remotes that failed to answer Check 4's wire probe in this run, mapped
    // to the refusal text they produced. A wildcard removal can target dozens
    // of branches on one remote; without this, an unreachable host bills the
    // full timeout once per branch to reach the identical verdict.
    let mut unreachable_remotes: HashMap<String, String> = HashMap::new();

    'branches: for target in targets {
        // Sandboxes run their own three checks (registered, clean, pinned);
        // everything branch-shaped continues into the 6-check body below.
        let branch = match target {
            ResolvedTarget::Branch(name) => name,
            ResolvedTarget::Sandbox(sandbox) => {
                sink.on_step(&format!("Validating sandbox '{}'...", sandbox.dirname));
                match validate_sandbox_target(ctx, sandbox, params, current_wt_path, sink) {
                    Ok(v) => validated.push(v),
                    Err(err) => errors.push(err),
                }
                continue;
            }
        };
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
                // Remote-only deletion skips the local merge checks entirely.
                safe_because: None,
                daft_files: DaftFilePlan::Nothing,
                is_sandbox: false,
                pinned_commit: None,
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
                    safe_because: None,
                    daft_files: DaftFilePlan::Nothing,
                    is_sandbox: false,
                    pinned_commit: None,
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

        // Determine remote tracking info for this branch. Resolved before
        // Check 4 rather than after it: the second proof Check 4 accepts is
        // built out of exactly this, and it is the same cheap config lookup
        // wherever it sits.
        let (remote_name, remote_branch_name) = resolve_remote_tracking(ctx, branch);

        // Check 4: nothing this deletion removes is lost (skip with --force,
        // keep_local_branch, or the merge cleanup's own validation).
        //
        // Two independent sufficient proofs, tried cheapest-first. Merge goes
        // first so a merged branch is never asked about on the wire: every
        // removal that passes today keeps costing exactly what it costs
        // today. Only a branch the merge check turns down reaches the #783
        // pushed proof, and that one bails on free local evidence before it
        // opens a connection — so the round trip is confined to the case it
        // exists to rescue (work pushed, PR open, worktree done).
        let mut safe_because = None;
        // Carries Check 4's local comparison into Check 5 so the two do not
        // ask git the same three questions twice. `None` when the checks are
        // skipped or the branch has no tracking config, in which case Check 5
        // falls back to computing it.
        let mut tracking: Option<TrackingComparison> = None;
        if !force && !keep_local_branch && !skip_merge_validation {
            let merge_check = is_branch_merged(ctx, branch, witness);
            if let Ok(ref verdict) = merge_check
                && verdict.is_merged()
            {
                let via = verdict.via();
                let how = via.map_or_else(String::new, |r| format!(" (via {})", r.short()));
                sink.on_step(&format!(
                    "Branch '{branch}' is merged into default branch{how}"
                ));
                safe_because = Some(SafeBecause::Merged {
                    into: ctx.default_branch.clone(),
                    via,
                });
            }

            // The pushed proof holds only while the remote copy outlives the
            // run. Deleting local *and* remote together destroys the commits
            // as surely as an unmerged local-only delete, so a run that takes
            // the remote with it gets no relaxation — mirroring the
            // `deletes_remote` predicate execution builds later.
            let remote_outlives_run = !params.delete_remote && !params.remote_only;
            // Why the pushed proof did not hold, phrased for the refusal.
            // Distinguished from a local fault, which is a different problem
            // with a different fix and must not be blamed on the network.
            let mut pushed_denial: Option<String> = None;
            let mut local_fault: Option<String> = None;
            if safe_because.is_none()
                && remote_outlives_run
                && let Some(ref remote) = remote_name
                && let Some(ref remote_branch) = remote_branch_name
            {
                match compare_local_to_tracking(ctx, branch, remote, remote_branch) {
                    Ok(comparison) => {
                        // Only a branch that already looks whole locally is
                        // worth a round trip — and once a remote has failed to
                        // answer in this run, every later branch on it would
                        // buy the same wait for the same answer.
                        if let TrackingComparison::Equal(ref sha) = comparison {
                            match unreachable_remotes.get(remote.as_str()) {
                                Some(known) => pushed_denial = Some(known.clone()),
                                None => match probe_remote_holds(ctx, remote, remote_branch, sha) {
                                    Ok(WireProof::Holds) => {
                                        let remote_ref = format!("{remote}/{remote_branch}");
                                        sink.on_step(&format!(
                                            "Branch '{branch}' is fully pushed to {remote_ref} \
                                             (remote branch preserved)"
                                        ));
                                        safe_because =
                                            Some(SafeBecause::FullyPushed { remote_ref });
                                    }
                                    Ok(WireProof::BranchGone) => {
                                        pushed_denial = Some(format!(
                                            "{remote} no longer has '{remote_branch}' — the local \
                                             {remote}/{remote_branch} is a stale cache"
                                        ));
                                    }
                                    // Not "stale, go fetch": the remote still
                                    // has the branch, just further along. A
                                    // fetch would only move the cached ref off
                                    // local and swap this for a vaguer refusal.
                                    Ok(WireProof::MovedOn) => {
                                        pushed_denial = Some(format!(
                                            "{remote}/{remote_branch} has moved on since it was \
                                             last fetched, so this commit could not be confirmed \
                                             on the remote"
                                        ));
                                    }
                                    Err(e) => {
                                        let why = format!(
                                            "could not reach {remote} to verify '{branch}' is \
                                             still pushed: {e}"
                                        );
                                        unreachable_remotes.insert(remote.to_string(), why.clone());
                                        pushed_denial = Some(why);
                                    }
                                },
                            }
                        }
                        if let TrackingComparison::Differs { local_ahead: true } = comparison {
                            pushed_denial = Some(format!(
                                "'{branch}' has commits that are not on {remote}/{remote_branch} \
                                 — push it and the removal needs no force"
                            ));
                        }
                        tracking = Some(comparison);
                    }
                    // A local fault, not a network one. Reporting it as an
                    // unreachable remote sends the user to check their VPN
                    // while the damaged ref goes unnoticed.
                    Err(e) => local_fault = Some(format!("failed to inspect '{branch}': {e}")),
                }
            }

            if safe_because.is_none() {
                // Neither proof held. Name what was actually learned: an
                // unreachable remote, a damaged local ref and a genuinely
                // unmerged, unpushed branch all refuse here, and telling the
                // user the wrong one sends them to fix the wrong thing.
                let message = match (&merge_check, local_fault, pushed_denial) {
                    (Err(e), _, _) => format!(
                        "failed to check merge status: {e} (use {} to force)",
                        params.force_flag_label
                    ),
                    (Ok(_), Some(why), _) => {
                        format!("{why} (use {} to force)", params.force_flag_label)
                    }
                    (Ok(_), None, Some(why)) => format!(
                        "not merged into '{}', and {why} (use {} to force)",
                        ctx.default_branch, params.force_flag_label
                    ),
                    (Ok(_), None, None) => format!(
                        "not merged into '{}' (use {} to force)",
                        ctx.default_branch, params.force_flag_label
                    ),
                };
                errors.push(ValidationError {
                    branch: branch.clone(),
                    message,
                });
                continue;
            }

            // The pushed proof carried the day but the merge probe had failed
            // on the way there. The deletion is still provably lossless, so
            // refusing would be wrong — but the repository is telling us
            // something (an unresolvable default branch, a damaged history)
            // that used to surface as a hard error and must not vanish now.
            if let Err(e) = merge_check {
                sink.on_warning(&format!(
                    "Could not check whether '{branch}' is merged into '{}': {e} — allowing the \
                     removal because the branch is fully pushed",
                    ctx.default_branch
                ));
            }
        }

        // Check 5: Local/remote in sync (skip with --force, keep_local_branch,
        // or the merge cleanup's own validation). Reuses Check 4's comparison
        // when it computed one: a confirmed pushed proof has already
        // established equality, so re-forking three git processes to rediscover
        // it is pure waste.
        if !force
            && !keep_local_branch
            && !skip_merge_validation
            && let Some(ref remote) = remote_name
            && let Some(ref remote_branch) = remote_branch_name
        {
            let sync = match tracking {
                // No tracking ref means nothing to be out of sync with.
                Some(TrackingComparison::NoTrackingRef) => Ok(true),
                Some(TrackingComparison::Equal(_)) => Ok(true),
                Some(TrackingComparison::Differs { .. }) => Ok(false),
                None => check_local_remote_sync(ctx, branch, remote, remote_branch),
            };
            match sync {
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
                    match plan_refined_files(
                        ctx,
                        branch,
                        wt,
                        target_wt,
                        &blocking,
                        seeds.as_ref(),
                        params,
                        sink,
                    ) {
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
            safe_because,
            daft_files,
            is_sandbox: false,
            pinned_commit: None,
        });
    }

    (validated, errors)
}

/// Build the consolidation/discard plan for a branch whose daft files are
/// refined (or provenance-less) and not subsumed by the target. Returns the
/// refusal message as `Err` when the user (or a non-interactive context)
/// aborts.
#[allow(clippy::too_many_arguments)]
fn plan_refined_files(
    ctx: &BranchDeleteContext,
    branch: &str,
    wt: &Path,
    target_wt: Option<&PathBuf>,
    blocking: &[FileClass],
    seeds: Option<&SeedsContext>,
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

    // Dry-run the consolidation per file (shared with the merge flow) so
    // the prompt can show exactly what would happen. Reuses the seed store
    // handle opened by validate_branches rather than opening a second one.
    let prepared: Vec<visitor_seeds::ConsolidationPreview> = blocking
        .iter()
        .map(|class| visitor_seeds::prepare_consolidation(seeds, branch, wt, target_wt, class))
        .collect();

    let request = ConsolidationRequest {
        branch: branch.to_string(),
        worktree_display: wt.display().to_string(),
        target_display: target_wt.display().to_string(),
        files: prepared
            .iter()
            .map(|p| RefinedFileSummary {
                filename: p.filename.clone(),
                adopt_keys: p.adopt_keys.clone(),
                conflict_keys: p.conflict_keys.clone(),
                whole_file: p.whole_file,
            })
            .collect(),
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
                    visitor_seeds::PreviewResolution::Resolved(content) => content,
                    visitor_seeds::PreviewResolution::NeedsSide {
                        target_priority,
                        source_priority,
                    } => match sink.on_conflicts(&prepared.filename, &prepared.conflict_keys) {
                        ConflictSide::Target => target_priority,
                        ConflictSide::Source => source_priority,
                        ConflictSide::Abort => {
                            return Err(refusal(&target_wt.display().to_string()));
                        }
                    },
                };
                resolved_files.push((prepared.filename.clone(), content));
            }
            Ok(DaftFilePlan::Consolidate(resolved_files))
        }
    }
}

// ── Merge checking ─────────────────────────────────────────────────────────

/// Check whether a branch has been merged into the default branch.
/// Delegates to the shared [`super::merged`] helpers (also used by prune's
/// gone-but-unmerged guard).
fn is_branch_merged(
    ctx: &BranchDeleteContext,
    branch: &str,
    witness: &dyn ForgeMergedWitness,
) -> Result<super::merged::MergedVerdict> {
    super::merged::is_branch_merged(
        ctx.git,
        branch,
        &ctx.default_branch,
        &ctx.remote_name,
        witness,
    )
}

/// How the local branch stands against its remote-tracking ref — the free,
/// purely local half of both Check 4's pushed proof and Check 5.
///
/// Computed once per branch and consumed by both. They used to ask git the
/// same three questions separately (`show_ref_exists` + two `rev_parse`, none
/// memoized and `rev_parse` with no gix arm, so all real forks), and a
/// confirmed pushed proof already settles Check 5's answer.
enum TrackingComparison {
    /// No `refs/remotes/<remote>/<branch>` at all — never pushed, or the
    /// tracking ref was pruned.
    NoTrackingRef,
    /// Local and the tracking ref name the same commit. Carries the shared
    /// SHA so the wire probe does not have to resolve it again.
    Equal(String),
    /// They name different commits. `local_ahead` distinguishes the common
    /// near-miss (committed, forgot to push) from a genuine divergence, so
    /// the refusal can name the lossless remedy.
    Differs { local_ahead: bool },
}

/// Compare `refs/heads/<branch>` against `refs/remotes/<remote>/<branch>`.
///
/// Deliberately *not* [`check_local_remote_sync`]'s boolean, which answers
/// `Ok(true)` when the tracking ref does not exist. That is the correct
/// answer to its question ("out of sync with what?") and a catastrophic one
/// to Check 4's: it would wave through a never-pushed branch whose commits
/// exist nowhere but the ref about to be deleted. Here the three states stay
/// distinct and each caller collapses them its own way.
///
/// Errors are all local faults (unreadable refs, a damaged object store) and
/// must not be reported as anything else.
fn compare_local_to_tracking(
    ctx: &BranchDeleteContext,
    branch: &str,
    remote: &str,
    remote_branch: &str,
) -> Result<TrackingComparison> {
    let remote_ref = format!("refs/remotes/{remote}/{remote_branch}");
    if !ctx
        .git
        .show_ref_exists(&remote_ref)
        .context("failed to check remote ref existence")?
    {
        return Ok(TrackingComparison::NoTrackingRef);
    }

    let local_sha = ctx
        .git
        .rev_parse(&format!("refs/heads/{branch}"))
        .context("failed to resolve local branch SHA")?;
    let cached_sha = ctx
        .git
        .rev_parse(&remote_ref)
        .context("failed to resolve remote tracking SHA")?;
    if local_sha == cached_sha {
        return Ok(TrackingComparison::Equal(local_sha));
    }

    // Only asked on the refusal path, and only to pick the wording: if the
    // tracking ref is an ancestor of local, the branch is simply unpushed and
    // `git push` makes the removal lossless. A failed probe just means the
    // hint is withheld.
    let local_ahead = ctx
        .git
        .merge_base_is_ancestor(&cached_sha, &local_sha)
        .unwrap_or(false);
    Ok(TrackingComparison::Differs { local_ahead })
}

/// What the remote itself said about a commit the tracking ref claims it has.
enum WireProof {
    /// The remote holds exactly this commit.
    Holds,
    /// The remote answered and has no such branch — a server-side delete the
    /// local cache outlived.
    BranchGone,
    /// The remote has the branch at a different commit.
    MovedOn,
}

/// Ask `remote` what it actually holds for `remote_branch`.
///
/// The tracking ref is only a cache: it goes on naming commits the remote
/// dropped in a server-side delete or a force-push, and a proof that a stale
/// ref can satisfy is not a proof. This is the one call on the path that
/// touches the network, so its `Err` means exactly one thing — the remote
/// could not be asked — and callers may say so.
fn probe_remote_holds(
    ctx: &BranchDeleteContext,
    remote: &str,
    remote_branch: &str,
    expected_sha: &str,
) -> Result<WireProof> {
    match ctx.git.ls_remote_branch_oid(remote, remote_branch)? {
        Some(sha) if sha == expected_sha => Ok(WireProof::Holds),
        Some(_) => Ok(WireProof::MovedOn),
        None => Ok(WireProof::BranchGone),
    }
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
    // Capture the identity key while the directory can still be probed:
    // records are keyed on the private-gitdir id, not the branch, and a
    // drifted record does not match the branch name we know here.
    let identity_id = branch
        .worktree_path
        .as_deref()
        .and_then(crate::core::worktree::identity_store::worktree_id_for);
    let stage_key = |id: StageId| StepKey::scoped(id, branch.name.clone());

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
            branch,
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
        sink.on_stage(&stage_key(StageId::DeleteRemote), StageEvent::Started);
        // Run from the branch's worktree when it still exists (Step 3 removes
        // it later) so the repo's pre-push hook fires there; otherwise any
        // directory inside the repo works for a remote delete.
        let push_cwd = branch
            .worktree_path
            .as_deref()
            .filter(|p| p.is_dir())
            .unwrap_or(&ctx.project_root);
        // A delete pushes no content, so under the default `pushVerify =
        // auto` the pre-push gate has nothing to validate and is skipped
        // (#747); `always` re-arms it. A failed delete still lands in
        // `result.errors` below regardless of the hook verdict — a skipped
        // gate must not soften a genuine transport or server-side failure.
        //
        // The plan is resolved per branch, not hoisted out of the loop: it
        // probes `push_cwd`, and a relative `core.hooksPath` resolves against
        // each worktree separately, so two branches in one invocation can
        // genuinely have different hooks. One `rev-parse` per branch is the
        // price of asking about the directory the hook would really run in.
        let hook_plan = resolve_delete_pre_push(ctx.git, push_cwd, ctx.push_verify, ctx.no_verify);
        if let Some(reason) = &hook_plan.skip_reason {
            sink.on_step(reason);
        }
        match push_with_hooks(
            ctx.git,
            PushAction::Delete {
                remote,
                branch: remote_branch,
            },
            push_cwd,
            hook_plan.verify,
            &NoopStageRunner,
            ctx.presenter,
            hook_plan.hook_present,
        )
        .and_then(crate::core::worktree::push::PushOutcome::into_result)
        {
            Ok(_) => {
                result.remote_deleted = true;
                sink.on_step(&format!(
                    "Remote branch {}/{} deleted",
                    remote, remote_branch
                ));
                sink.on_stage(
                    &stage_key(StageId::DeleteRemote),
                    StageEvent::Completed { annotation: None },
                );
            }
            Err(e) => {
                sink.on_stage(
                    &stage_key(StageId::DeleteRemote),
                    StageEvent::Failed {
                        detail: "failed (see below)".to_string(),
                    },
                );
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
            sink.on_stage(
                &stage_key(StageId::RemoveWorktree),
                StageEvent::Failed {
                    detail: "main working tree — cannot remove".to_string(),
                },
            );
            result.errors.push(format!(
                "Cannot remove '{}': this is the main working tree. \
                 Use `daft layout transform` to restructure, or delete other worktrees instead.",
                branch.name
            ));
        } else if wt_path.exists() {
            sink.on_step(&format!("Removing worktree at {}...", wt_path.display()));
            sink.on_stage(&stage_key(StageId::RemoveWorktree), StageEvent::Started);
            match remove_worktree_completing_orphans(ctx.git, wt_path, force, sink) {
                Ok(()) => {
                    result.worktree_removed = true;
                    sink.on_step(&format!("Removed worktree '{}'", branch.name));
                    sink.on_stage(
                        &stage_key(StageId::RemoveWorktree),
                        StageEvent::Completed { annotation: None },
                    );
                }
                Err(e) => {
                    sink.on_stage(
                        &stage_key(StageId::RemoveWorktree),
                        StageEvent::Failed {
                            detail: "failed (see below)".to_string(),
                        },
                    );
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
            sink.on_stage(&stage_key(StageId::RemoveWorktree), StageEvent::Started);
            match ctx.git.worktree_remove(wt_path, true) {
                Ok(()) => {
                    result.worktree_removed = true;
                    sink.on_step(&format!("Removed worktree '{}'", branch.name));
                    sink.on_stage(
                        &stage_key(StageId::RemoveWorktree),
                        StageEvent::Completed {
                            annotation: Some("orphaned record removed".to_string()),
                        },
                    );
                }
                Err(e) => {
                    sink.on_stage(
                        &stage_key(StageId::RemoveWorktree),
                        StageEvent::Failed {
                            detail: "failed (see below)".to_string(),
                        },
                    );
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
        sink.on_stage(&stage_key(StageId::DeleteLocalBranch), StageEvent::Started);
        match ctx.git.branch_delete(&branch.name, true) {
            Ok(()) => {
                result.branch_deleted = true;
                sink.on_step(&format!("Branch {} deleted", branch.name));
                sink.on_stage(
                    &stage_key(StageId::DeleteLocalBranch),
                    StageEvent::Completed { annotation: None },
                );
            }
            Err(e) => {
                sink.on_stage(
                    &stage_key(StageId::DeleteLocalBranch),
                    StageEvent::Failed {
                        detail: "failed (see below)".to_string(),
                    },
                );
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
            branch,
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
    if result.worktree_removed
        && let Some(store) =
            crate::core::worktree::identity_store::IdentityStore::open(&ctx.git_dir)
    {
        // Sandboxes go through the sandbox forget path: its by-name fallback
        // sweeps only sandbox rows, so a branch sharing the spelling keeps
        // its record (and vice versa on the branch path).
        if branch.is_sandbox {
            store.forget_sandbox(identity_id.as_deref(), &branch.name);
        } else {
            store.forget(identity_id.as_deref(), &branch.name);
        }
    }

    result
}

// ── Hook execution ─────────────────────────────────────────────────────────

/// Run a lifecycle hook (pre-remove or post-remove) for a worktree.
///
/// Sandbox targets get the branchless contract: `DAFT_BRANCH_NAME` is the
/// empty string and `DAFT_COMMIT` carries the pinned OID — the dirname is
/// not a branch and must not masquerade as one in hook environments.
fn run_removal_hook(
    hook_type: HookType,
    ctx: &BranchDeleteContext,
    worktree_path: &Path,
    branch: &ValidatedBranch,
    command_label: &str,
    sink: &mut (impl ProgressSink + HookRunner),
) {
    let branch_name = if branch.is_sandbox { "" } else { &branch.name };
    let mut hook_ctx = HookContext::new(
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
    if let Some(pin) = branch.pinned_commit.as_deref() {
        hook_ctx = hook_ctx.with_commit(pin);
    }

    if let Err(e) = sink.run_hook(&hook_ctx) {
        sink.on_warning(&format!(
            "{} hook failed for {}: {e}",
            match hook_type {
                HookType::PreRemove => "Pre-remove",
                HookType::PostRemove => "Post-remove",
                _ => "Hook",
            },
            branch.name
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

/// Remove a worktree via git, completing the delete by hand when git
/// unregisters it but fails to clear the directory.
///
/// `git worktree remove` clears the working tree and its admin entry as
/// separate steps: when something wins a file-create race mid-delete
/// (observed: a warmup build still writing while its fork was removed),
/// git reports "Directory not empty" — with the worktree already
/// unregistered. Left there, the directory is invisible to git and daft
/// alike, so a retry has nothing to act on. Once git has disowned the
/// path, finishing the delete ourselves is the only way to complete the
/// removal the target was already validated for. If the registration
/// state can't be read, or git still lists the worktree, the original
/// error stands untouched.
fn remove_worktree_completing_orphans(
    git: &GitCommand,
    wt_path: &Path,
    force: bool,
    sink: &mut dyn ProgressSink,
) -> Result<()> {
    let orig = match git.worktree_remove(wt_path, force) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    let target = std::fs::canonicalize(wt_path).unwrap_or_else(|_| wt_path.to_path_buf());
    let still_registered = parse_worktree_list(git)
        .map(|entries| {
            entries.iter().any(|e| {
                std::fs::canonicalize(&e.path).unwrap_or_else(|_| e.path.clone()) == target
            })
        })
        // Can't tell → assume registered and keep the original error.
        .unwrap_or(true);
    if still_registered || !wt_path.exists() {
        return Err(orig);
    }
    sink.on_step(&format!(
        "git unregistered the worktree but left '{}'; completing the delete",
        wt_path.display()
    ));
    match std::fs::remove_dir_all(wt_path) {
        Ok(()) => Ok(()),
        Err(e) => Err(orig.context(format!("the direct delete of the leftover failed too: {e}"))),
    }
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
    use crate::core::worktree::ports::NoopForgeWitness;

    /// Plan fixture where every hook phase has discoverable work — the
    /// pre-gating shape, for tests exercising other row conditionals.
    fn all_hook_rows(n: usize) -> HookRowPlan {
        HookRowPlan {
            pre_remove: vec![true; n],
            post_remove: true,
        }
    }

    #[test]
    fn plan_header_names_resolved_branches_not_raw_args() {
        let branch = |name: &str| ValidatedBranch {
            name: name.to_string(),
            worktree_path: None,
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
            worktree_only: false,
            safe_because: None,
            daft_files: DaftFilePlan::Nothing,
            is_sandbox: false,
            pinned_commit: None,
        };
        let params = BranchDeleteParams {
            branches: vec![".".to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };

        // The raw arg was a path shorthand; the header carries the branch.
        let a = branch("feat-x");
        let plan = build_plan(&[&a], &params, &all_hook_rows(1));
        assert_eq!(plan.header.as_deref(), Some("Removing feat-x"));

        let b = branch("feat-y");
        let plan = build_plan(&[&a, &b], &params, &all_hook_rows(2));
        assert_eq!(plan.header.as_deref(), Some("Removing 2 branches"));
    }

    /// #813: multi-target keeps the count form, and it is reachable without
    /// touching a repo — the seed only resolves when there is exactly one
    /// argument to resolve. The single-target spellings are covered where
    /// they actually render, in `tests/integration/test_branch_delete.sh`.
    #[test]
    fn header_seed_keeps_the_count_form_for_multiple_targets() {
        let params = |branches: Vec<String>| BranchDeleteParams {
            branches,
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };

        assert_eq!(
            header_seed(&params(vec![".".into(), "feat-y".into()])),
            "Removing 2 branches"
        );
        assert_eq!(
            header_seed(&params(vec!["a".into(), "b".into(), "c".into()])),
            "Removing 3 branches"
        );
    }

    #[test]
    fn remote_fate_note_planned_only_when_remote_deletion_in_scope() {
        let branch = |worktree_only: bool| ValidatedBranch {
            name: "feat-x".to_string(),
            worktree_path: None,
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
            worktree_only,
            safe_because: None,
            daft_files: DaftFilePlan::Nothing,
            is_sandbox: false,
            pinned_commit: None,
        };
        let params = |delete_remote: bool| BranchDeleteParams {
            branches: vec!["feat-x".to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let notes = |plan: &PlanCommit| -> Vec<String> {
            plan.rows
                .iter()
                .filter_map(|r| match r {
                    Row::Note { text } => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };

        // Remote deletion off (config default, remote-sync local-only, or
        // --local): the plan never mentions the remote.
        let plan = build_plan(&[&branch(false)], &params(false), &all_hook_rows(1));
        assert!(
            notes(&plan).is_empty(),
            "out-of-scope remote plans no note: {:?}",
            plan.rows
        );

        // Remote deletion on but the branch has no upstream: the dim note
        // records there was nothing to delete.
        let plan = build_plan(&[&branch(false)], &params(true), &all_hook_rows(1));
        assert_eq!(notes(&plan), vec!["no remote branch".to_string()]);

        // Default-branch worktree removal keeps its explanation, mentioning
        // the remote only while remote deletion is in scope.
        let plan = build_plan(&[&branch(true)], &params(false), &all_hook_rows(1));
        assert_eq!(
            notes(&plan),
            vec!["branch kept (default branch)".to_string()]
        );
        let plan = build_plan(&[&branch(true)], &params(true), &all_hook_rows(1));
        assert_eq!(
            notes(&plan),
            vec!["branch and remote kept (default branch)".to_string()]
        );
    }

    #[test]
    fn test_validated_branch_fields() {
        let vb = ValidatedBranch {
            name: "feature/test".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/project/feature/test")),
            remote_name: Some("origin".to_string()),
            remote_branch_name: Some("feature/test".to_string()),
            is_current_worktree: false,
            worktree_only: false,
            safe_because: None,
            daft_files: DaftFilePlan::Nothing,
            is_sandbox: false,
            pinned_commit: None,
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
            safe_because: None,
            daft_files: DaftFilePlan::Nothing,
            is_sandbox: false,
            pinned_commit: None,
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
            safe_because: None,
            daft_files: DaftFilePlan::Nothing,
            is_sandbox: false,
            pinned_commit: None,
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

    use crate::store::paths::IsolatedStateDir;
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

    /// Commit an empty change on whatever branch `dir` has checked out.
    fn commit_empty(dir: &std::path::Path, message: &str) {
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", message])
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    /// Wire `root` to a real bare repo at `bare` under `remote_name` and push
    /// `branch` to it, with upstream tracking set.
    ///
    /// The #783 proof is verified on the wire, so these tests need a remote
    /// that genuinely answers `ls-remote` — a hand-written tracking ref is
    /// exactly the stale cache the probe exists to catch. A local bare repo
    /// answers over the filesystem, so this stays offline and fast.
    fn setup_remote(
        root: &std::path::Path,
        bare: &std::path::Path,
        remote_name: &str,
        branch: &str,
    ) {
        ShellCommand::new("git")
            .args([
                "init",
                "--bare",
                "-q",
                "-b",
                "main",
                &bare.display().to_string(),
            ])
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        git_quiet(
            root,
            &["remote", "add", remote_name, &bare.display().to_string()],
        );
        assert!(
            git_quiet(root, &["push", "-q", "-u", remote_name, branch]).success(),
            "failed to push {branch} to the test remote"
        );
    }

    /// Params for a plain `daft remove <branch>`, no force.
    fn remove_params(branch: &str) -> BranchDeleteParams {
        BranchDeleteParams {
            branches: vec![branch.to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-f/--force".to_string(),
        }
    }

    /// Run validation-through-execution the way `daft remove` does.
    fn run_remove(params: &BranchDeleteParams) -> BranchDeleteResult {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        execute(params, None, &NoopForgeWitness, &mut bridge).unwrap()
    }

    /// An unmerged `feature` branch in its own worktree, pushed whole to a
    /// real bare `origin`. The #783 shape: nothing to lose, no force needed.
    fn pushed_unmerged_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);
        commit_empty(&feat_wt, "feature work");
        let bare = tmp.path().join("origin.git");
        setup_remote(tmp.path(), &bare, "origin", "feature");
        (tmp, feat_wt, bare)
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

    /// The half-removed state `remove_worktree_completing_orphans` exists
    /// for: git has unregistered the worktree (admin entry gone) but the
    /// directory remains — observed when a dying background build won a
    /// file-create race during `git worktree remove`. The helper must
    /// finish the delete instead of erroring against a directory git no
    /// longer owns.
    #[test]
    #[serial]
    fn an_unregistered_leftover_worktree_dir_is_deleted_directly() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let wt = tmp.path().join("fork");
        setup_worktree(tmp.path(), "fork-branch", &wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        // Simulate git's mid-delete unregister: admin entry gone, dir kept.
        std::fs::remove_dir_all(tmp.path().join(".git/worktrees/fork")).unwrap();
        assert!(wt.exists());

        let git = GitCommand::new(true);
        remove_worktree_completing_orphans(&git, &wt, false, &mut crate::core::NullSink)
            .expect("a disowned directory must still get deleted");
        assert!(!wt.exists(), "the leftover directory must be gone");
    }

    /// A still-registered worktree whose removal fails keeps the original
    /// git error — the direct delete fires only once git has disowned the
    /// path, never as a way around git's own refusals.
    #[test]
    #[serial]
    fn a_registered_worktree_failure_keeps_the_git_error() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let wt = tmp.path().join("dirty");
        setup_worktree(tmp.path(), "dirty-branch", &wt);
        std::env::set_current_dir(tmp.path()).unwrap();
        // Untracked file: a non-force `git worktree remove` refuses.
        std::fs::write(wt.join("junk.txt"), "x").unwrap();

        let git = GitCommand::new(true);
        let err = remove_worktree_completing_orphans(&git, &wt, false, &mut crate::core::NullSink)
            .expect_err("a registered dirty worktree must keep refusing");
        assert!(err.to_string().contains("Git worktree remove failed"));
        assert!(wt.exists(), "the worktree must be untouched");
    }

    #[test]
    #[serial]
    fn keep_local_branch_removes_worktree_only() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
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
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, None, &NoopForgeWitness, &mut bridge)
            .expect("keep_local_branch should succeed");

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

    /// #783's core: a stale `refs/remotes/origin/feature` must not stand in
    /// for the remote. Written first deliberately — it is the one test that
    /// passes trivially if the wire probe is never wired in, so it is the
    /// only proof the `ls-remote` round trip is really happening.
    #[test]
    #[serial]
    fn a_stale_tracking_ref_does_not_prove_the_branch_is_pushed() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, _feat_wt, bare) = pushed_unmerged_repo();

        // Server-side delete, behind the tracking ref's back: origin drops
        // the branch while the local cache goes on naming its commit.
        assert!(git_quiet(&bare, &["update-ref", "-d", "refs/heads/feature"]).success());
        assert!(
            tmp.path().join(".git/refs/remotes/origin/feature").exists()
                || git_quiet(
                    tmp.path(),
                    &["show-ref", "--verify", "refs/remotes/origin/feature"]
                )
                .success(),
            "test precondition: the tracking ref must survive the server-side delete"
        );

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert_eq!(
            result.validation_errors.len(),
            1,
            "a branch whose remote copy is gone must still be refused"
        );
        let message = &result.validation_errors[0].message;
        assert!(
            message.contains("stale"),
            "refusal must name the stale cache rather than blaming the merge \
             state alone; got: {message}"
        );
        assert!(result.deletions.is_empty(), "nothing may be deleted");
    }

    /// The trap the ticket leads with: `check_local_remote_sync` answers
    /// `Ok(true)` when there is no tracking ref at all. Reusing that boolean
    /// would let a never-pushed branch skip the merge check and destroy the
    /// only copy of its commits.
    #[test]
    #[serial]
    fn a_never_pushed_branch_is_still_refused() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);
        commit_empty(&feat_wt, "feature work");

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert_eq!(
            result.validation_errors.len(),
            1,
            "an unmerged branch that exists nowhere else must be refused"
        );
        assert!(
            result.validation_errors[0].message.contains("not merged"),
            "got: {}",
            result.validation_errors[0].message
        );
        assert!(result.deletions.is_empty());
    }

    /// Local ahead of the remote: the extra commits exist only here. The
    /// refusal must name the lossless remedy (push) and not only the
    /// destructive one, since this is the commonest near-miss.
    #[test]
    #[serial]
    fn a_branch_ahead_of_its_remote_is_still_refused() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, feat_wt, _bare) = pushed_unmerged_repo();
        commit_empty(&feat_wt, "unpushed work");

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert_eq!(
            result.validation_errors.len(),
            1,
            "commits that never left this machine must still be protected"
        );
        let message = &result.validation_errors[0].message;
        assert!(
            message.contains("push it"),
            "an unpushed-ahead branch must be told to push, not only to force; got: {message}"
        );
        assert!(result.deletions.is_empty());
    }

    /// A remote that moved ahead still has the commit; it is not "stale", and
    /// telling the user to `git fetch --prune` would only make the next run's
    /// message worse. Regression for the wording, not the verdict.
    #[test]
    #[serial]
    fn a_remote_that_moved_on_is_not_reported_as_stale() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, _feat_wt, bare) = pushed_unmerged_repo();

        // Advance origin/feature past the local tip without fetching, so the
        // tracking ref still equals local and the wire disagrees.
        let other = tmp.path().join("other");
        git_quiet(
            tmp.path(),
            &[
                "clone",
                "-q",
                &bare.display().to_string(),
                &other.display().to_string(),
            ],
        );
        git_quiet(&other, &["checkout", "-q", "feature"]);
        commit_empty(&other, "someone else's work");
        assert!(git_quiet(&other, &["push", "-q", "origin", "feature"]).success());

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert_eq!(result.validation_errors.len(), 1);
        let message = &result.validation_errors[0].message;
        assert!(
            message.contains("moved on"),
            "must say the remote moved on; got: {message}"
        );
        assert!(
            !message.contains("fetch --prune"),
            "must not advise a fetch that degrades the next run's message; got: {message}"
        );
        assert!(result.deletions.is_empty());
    }

    /// The path the clap help, all three man pages and the docs promise by
    /// name: an unreachable remote refuses rather than assumes. Fails closed.
    #[test]
    #[serial]
    fn an_unreachable_remote_refuses_rather_than_assuming() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, _feat_wt, _bare) = pushed_unmerged_repo();

        // Point origin at nothing. The tracking ref survives and still claims
        // the branch is pushed, so only the wire probe can catch it.
        let gone = tmp.path().join("gone.git");
        git_quiet(
            tmp.path(),
            &["remote", "set-url", "origin", &gone.display().to_string()],
        );

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert_eq!(
            result.validation_errors.len(),
            1,
            "an unverifiable remote must refuse, never assume"
        );
        let message = &result.validation_errors[0].message;
        assert!(
            message.contains("could not reach origin"),
            "refusal must name the unreachable remote; got: {message}"
        );
        assert!(result.deletions.is_empty());
    }

    /// A local ref fault must surface as an `Err` from the comparison, never
    /// be folded into the wire probe's error channel — the caller renders
    /// those two as different problems with different fixes, and only this
    /// separation makes that possible.
    ///
    /// Tested at the seam rather than end-to-end: manufacturing a repo broken
    /// in exactly this way, but still intact enough to reach Check 4, means
    /// corrupting git's object store in a way no user hits and no assertion
    /// pins down. The distinction that matters is right here.
    #[test]
    #[serial]
    fn a_local_ref_fault_is_an_error_not_a_verdict() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, _feat_wt, _bare) = pushed_unmerged_repo();
        std::env::set_current_dir(tmp.path()).unwrap();

        let git = GitCommand::new(true);
        let ctx = BranchDeleteContext {
            git: &git,
            project_root: tmp.path().to_path_buf(),
            git_dir: tmp.path().join(".git"),
            remote_name: "origin".to_string(),
            source_worktree: tmp.path().to_path_buf(),
            default_branch: "main".to_string(),
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            presenter: None,
        };

        // Healthy: the tracking ref and the local branch agree.
        assert!(
            matches!(
                compare_local_to_tracking(&ctx, "feature", "origin", "feature"),
                Ok(TrackingComparison::Equal(_))
            ),
            "precondition: the fixture must start out fully pushed"
        );

        // Drop the local branch ref while the tracking ref survives, so the
        // local resolve fails after the existence probe has already passed.
        git_quiet(tmp.path(), &["worktree", "remove", "--force", "feat"]);
        std::fs::remove_file(tmp.path().join(".git/refs/heads/feature")).unwrap();

        let outcome = compare_local_to_tracking(&ctx, "feature", "origin", "feature");
        assert!(
            outcome.is_err(),
            "an unresolvable local branch is a fault to report, not a \
             'not pushed' verdict to act on"
        );
    }

    /// The relaxation is premised on the remote copy outliving the run, so a
    /// run configured to delete the remote too gets none of it.
    #[test]
    #[serial]
    fn a_run_that_deletes_the_remote_gets_no_relaxation() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, _feat_wt, _bare) = pushed_unmerged_repo();

        std::env::set_current_dir(tmp.path()).unwrap();
        let mut params = remove_params("feature");
        params.delete_remote = true;
        let result = run_remove(&params);

        assert_eq!(
            result.validation_errors.len(),
            1,
            "deleting local and remote together destroys the commits, so the \
             merge check must still apply"
        );
        assert!(result.deletions.is_empty());
    }

    /// The reported case: clean worktree, unmerged, identical to a remote
    /// that survives. No force, no refusal.
    #[test]
    #[serial]
    fn a_fully_pushed_branch_is_removed_without_force() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let (tmp, _feat_wt, bare) = pushed_unmerged_repo();

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert!(
            result.validation_errors.is_empty(),
            "a branch whole on a surviving remote loses nothing: {:?}",
            result
                .validation_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.deletions.len(), 1);
        assert!(result.deletions[0].branch_deleted);
        assert!(result.deletions[0].worktree_removed);
        assert!(
            git_quiet(&bare, &["show-ref", "--verify", "refs/heads/feature"]).success(),
            "the remote branch must survive — it is the whole basis for allowing this"
        );
    }

    /// The relaxation must key off the branch's own tracking remote, not the
    /// repo's default one. Reaching for `ctx.remote_name` would refuse a
    /// branch that is perfectly well pushed to `upstream`.
    #[test]
    #[serial]
    fn a_branch_tracking_a_non_default_remote_is_removed_without_force() {
        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);
        commit_empty(&feat_wt, "feature work");
        let bare = tmp.path().join("upstream.git");
        setup_remote(tmp.path(), &bare, "upstream", "feature");

        std::env::set_current_dir(tmp.path()).unwrap();
        let result = run_remove(&remove_params("feature"));

        assert!(
            result.validation_errors.is_empty(),
            "pushed to 'upstream' is just as lossless as pushed to 'origin': {:?}",
            result
                .validation_errors
                .iter()
                .map(|e| &e.message)
                .collect::<Vec<_>>()
        );
        assert_eq!(result.deletions.len(), 1);
        assert!(result.deletions[0].branch_deleted);
    }

    #[test]
    #[serial]
    fn keep_local_branch_skips_merged_into_default_check() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
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
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut output = TestOutput::new();
        let executor = HookExecutor::new(HooksConfig::default()).unwrap();
        let mut bridge = CommandBridge::new(&mut output, executor);
        let result = execute(&params, None, &NoopForgeWitness, &mut bridge).unwrap();

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
    fn plan_commits_after_validation_in_execution_order() {
        use crate::core::RecordingStageSink;
        use crate::core::stage::{Row, StageEvent, StageId};

        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);

        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            force: true, // skip merged/sync checks (no real remote here)
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-f/--force".to_string(),
        };
        let mut sink = RecordingStageSink::default();
        let result = execute(&params, None, &NoopForgeWitness, &mut sink).unwrap();
        assert!(result.validation_errors.is_empty());

        // Exactly one plan, committed before any deletion executed.
        let plan = sink.plan.as_ref().expect("plan must be committed");
        let ids: Vec<StageId> = plan
            .steps()
            .map(|s| {
                assert_eq!(s.key.scope.as_deref(), Some("feature"), "rows are scoped");
                s.key.id
            })
            .collect();
        // Execution order: hooks bracket the removal; no DeleteRemote step
        // and no remote-fate note — remote deletion is off, so the plan
        // never mentions the remote. (The hook rows are present because
        // `RecordingStageSink` keeps speculative rows via the trait's
        // default probe; row gating is pinned separately below.)
        assert_eq!(
            ids,
            vec![
                StageId::PreRemoveHooks,
                StageId::RemoveWorktree,
                StageId::DeleteLocalBranch,
                StageId::PostRemoveHooks,
            ]
        );
        assert!(
            !plan.rows.iter().any(|r| matches!(r, Row::Note { .. })),
            "remote out of scope plans no note: {:?}",
            plan.rows
        );
        // Single-branch plans carry no group anchors.
        assert!(!plan.rows.iter().any(|r| matches!(r, Row::Group { .. })));

        // Events: worktree and branch both started and completed, in order.
        let completed: Vec<StageId> = sink
            .events
            .iter()
            .filter_map(|(k, e)| matches!(e, StageEvent::Completed { .. }).then_some(k.id))
            .collect();
        assert_eq!(
            completed,
            vec![StageId::RemoveWorktree, StageId::DeleteLocalBranch]
        );
        // Both removal hooks fired through the sink.
        assert_eq!(
            sink.hooks_run,
            vec![
                crate::hooks::HookType::PreRemove,
                crate::hooks::HookType::PostRemove
            ]
        );
    }

    #[test]
    fn hook_rows_planned_only_when_the_phase_has_work() {
        let branch = |name: &str| ValidatedBranch {
            name: name.to_string(),
            worktree_path: Some(PathBuf::from(format!("/tmp/{name}"))),
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
            worktree_only: false,
            safe_because: None,
            daft_files: DaftFilePlan::Nothing,
            is_sandbox: false,
            pinned_commit: None,
        };
        let params = BranchDeleteParams {
            branches: vec!["feat-x".to_string()],
            force: false,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let ids = |plan: &PlanCommit| -> Vec<StageId> { plan.steps().map(|s| s.key.id).collect() };

        // Nothing discoverable: neither hook row — the rail lists only
        // work that happens (these rows used to be planned speculatively
        // and vanish at resolution).
        let a = branch("feat-x");
        let none = HookRowPlan {
            pre_remove: vec![false],
            post_remove: false,
        };
        assert_eq!(
            ids(&build_plan(&[&a], &params, &none)),
            vec![StageId::RemoveWorktree, StageId::DeleteLocalBranch],
        );

        // Pre-remove discovery is per-branch (each worktree carries its own
        // config); post-remove is per-invocation (source worktree).
        let b = branch("feat-y");
        let mixed = HookRowPlan {
            pre_remove: vec![true, false],
            post_remove: false,
        };
        let plan = build_plan(&[&a, &b], &params, &mixed);
        let pre_scopes: Vec<_> = plan
            .steps()
            .filter(|s| s.key.id == StageId::PreRemoveHooks)
            .map(|s| s.key.scope.clone())
            .collect();
        assert_eq!(pre_scopes, vec![Some("feat-x".to_string())]);
        assert!(plan.steps().all(|s| s.key.id != StageId::PostRemoveHooks));
    }

    #[test]
    #[serial]
    fn plan_omits_hook_rows_but_execution_still_fires_hooks() {
        use crate::core::RecordingStageSink;
        use std::cell::RefCell;

        // A sink whose probe finds no work anywhere — the real bridges
        // delegate to `HookExecutor::hook_phase_has_work`; this pins the
        // core's wiring: which phases get probed, with which source paths,
        // and that gating the row never gates the run.
        struct GatedSink {
            inner: RecordingStageSink,
            probes: RefCell<Vec<(crate::hooks::HookType, PathBuf)>>,
        }
        impl ProgressSink for GatedSink {
            fn on_step(&mut self, msg: &str) {
                self.inner.on_step(msg);
            }
            fn on_warning(&mut self, msg: &str) {
                self.inner.on_warning(msg);
            }
            fn on_debug(&mut self, msg: &str) {
                self.inner.on_debug(msg);
            }
            fn on_plan(&mut self, plan: crate::core::stage::PlanCommit) {
                self.inner.on_plan(plan);
            }
            fn on_stage(&mut self, key: &StepKey, event: StageEvent) {
                self.inner.on_stage(key, event);
            }
        }
        impl crate::core::ConsolidationPrompter for GatedSink {}
        impl crate::core::HookRunner for GatedSink {
            fn hook_phase_has_work(
                &self,
                hook_type: crate::hooks::HookType,
                hook_source_worktree: &Path,
            ) -> bool {
                self.probes
                    .borrow_mut()
                    .push((hook_type, hook_source_worktree.to_path_buf()));
                false
            }
            fn run_hook(
                &mut self,
                ctx: &crate::hooks::HookContext,
            ) -> anyhow::Result<crate::core::HookOutcome> {
                self.inner.run_hook(ctx)
            }
        }

        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        let feat_wt = tmp.path().join("feat");
        setup_worktree(tmp.path(), "feature", &feat_wt);
        std::env::set_current_dir(tmp.path()).unwrap();

        let params = BranchDeleteParams {
            branches: vec!["feature".to_string()],
            force: true,
            use_gitoxide: false,
            is_quiet: true,
            remote_name: "origin".to_string(),
            delete_remote: false,
            remote_only: false,
            keep_local_branch: false,
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-f/--force".to_string(),
        };
        let mut sink = GatedSink {
            inner: RecordingStageSink::default(),
            probes: RefCell::new(Vec::new()),
        };
        let result = execute(&params, None, &NoopForgeWitness, &mut sink).unwrap();
        assert!(result.validation_errors.is_empty());

        // No discoverable hooks: the plan omits both hook rows...
        let plan = sink.inner.plan.as_ref().expect("plan must be committed");
        assert!(
            plan.steps().all(|s| !s.key.id.is_hook_phase()),
            "no hook rows without discoverable work: {:?}",
            plan.rows
        );
        // ...while execution still consults the executor for both phases.
        assert_eq!(
            sink.inner.hooks_run,
            vec![
                crate::hooks::HookType::PreRemove,
                crate::hooks::HookType::PostRemove
            ]
        );
        // Each phase was probed at its true config source: the branch's own
        // worktree for pre-remove, the source worktree for post-remove.
        let probes = sink.probes.borrow();
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].0, crate::hooks::HookType::PreRemove);
        assert!(
            probes[0].1.ends_with("feat"),
            "pre-remove probes the branch worktree: {}",
            probes[0].1.display()
        );
        assert_eq!(probes[1].0, crate::hooks::HookType::PostRemove);
    }

    #[test]
    #[serial]
    fn run_removal_hook_uses_command_label_from_params() {
        use crate::core::CommandBridge;
        use crate::hooks::{HookExecutor, HooksConfig};
        use crate::output::TestOutput;

        let _cwd = CwdGuard::new();
        let _state = IsolatedStateDir::new();
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
            push_verify: crate::settings::PushVerify::Auto,
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
        let bd_result = execute(&params, None, &NoopForgeWitness, &mut bridge).unwrap();
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
        let _state = IsolatedStateDir::new();
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
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut bridge = ScriptedBridge::aborting();
        let result = execute(&params, None, &NoopForgeWitness, &mut bridge).unwrap();

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
        let _state = IsolatedStateDir::new();
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
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut bridge = ScriptedBridge::aborting();
        let result = execute(&params, None, &NoopForgeWitness, &mut bridge).unwrap();

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
        let _state = IsolatedStateDir::new();
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
            no_verify: false,
            push_verify: crate::settings::PushVerify::Auto,
            prune_cd_target: crate::settings::PruneCdTarget::Root,
            command_label: "branch-delete".to_string(),
            skip_merge_validation: false,
            force_flag_label: "-D/--force".to_string(),
        };
        let mut bridge = ScriptedBridge {
            choice: crate::core::ConsolidationChoice::Consolidate,
            side: crate::core::ConflictSide::Abort,
        };
        let result = execute(&params, None, &NoopForgeWitness, &mut bridge).unwrap();

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
