//! Core logic for the `git-worktree-checkout-branch` command.
//!
//! Creates a worktree with a new branch.

use crate::config::git::{COMMITS_AHEAD_THRESHOLD, DEFAULT_COMMIT_COUNT};
use crate::core::layout::{Layout, auto_gitignore_if_needed};
use crate::core::settings::PushVerify;
use crate::core::stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};
use crate::core::worktree::ports::NoopStageRunner;
use crate::core::worktree::push::{
    HookVerdict, PrePushDecision, PushAction, PushPayload, push_with_hooks, resolve_pre_push,
};
use crate::core::{HookOutcome, HookRunner, ProgressSink};
use crate::executor::presenter::JobPresenter;
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType};
use crate::multi_remote::path::{
    build_template_context, calculate_worktree_path, resolve_remote_for_branch,
};
use crate::utils::*;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Input parameters for the checkout-branch operation.
pub struct CheckoutBranchParams {
    /// Name for the new branch.
    pub new_branch_name: String,
    /// Branch to use as the base (None = current branch).
    pub base_branch_name: Option<String>,
    /// Apply uncommitted changes to the new worktree.
    pub carry: bool,
    /// Do not carry uncommitted changes.
    pub no_carry: bool,
    /// Remote for worktree organization (multi-remote mode).
    pub remote: Option<String>,
    /// Remote name (from settings).
    pub remote_name: String,
    /// Whether multi-remote mode is enabled.
    pub multi_remote_enabled: bool,
    /// Default remote name for multi-remote.
    pub multi_remote_default: String,
    /// Whether carry is enabled by default (from settings).
    pub checkout_branch_carry: bool,
    /// Whether to push and set upstream (from settings).
    pub checkout_push: bool,
    /// Skip the repo's pre-push hook on the upstream push (`--no-verify`).
    pub no_verify: bool,
    /// When the upstream push runs the repo's pre-push hook (from settings).
    pub push_verify: PushVerify,
    /// Whether to fetch from remote before creating the worktree.
    pub checkout_fetch: bool,
    /// Optional layout for computing the worktree path.
    /// When `Some`, uses `layout.worktree_path()` instead of `calculate_worktree_path()`.
    pub layout: Option<Layout>,
    /// Explicit path override for worktree placement (`--at` flag).
    /// When `Some`, takes priority over both `layout` and the default path computation.
    pub at_path: Option<PathBuf>,
}

/// Result of a checkout-branch operation.
pub struct CheckoutBranchResult {
    pub new_branch_name: String,
    pub base_branch: String,
    pub worktree_path: PathBuf,
    pub cd_target: PathBuf,
    pub stash_applied: bool,
    pub stash_conflict: bool,
    pub push_set: bool,
    pub push_skipped: bool,
    pub git_dir: PathBuf,
    pub post_hook_outcome: HookOutcome,
}

/// Execute the checkout-branch operation.
///
/// `presenter` reports the pre-push hook run on the automatic upstream push
/// (#599); pass `None` to skip that reporting (the hook is still honored).
pub fn execute(
    params: &CheckoutBranchParams,
    git: &GitCommand,
    project_root: &Path,
    presenter: Option<&Arc<dyn JobPresenter>>,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<CheckoutBranchResult> {
    validate_branch_name(&params.new_branch_name)?;

    let base_branch = resolve_base_branch(params, git, sink)?;

    let git_dir = crate::core::repo::get_git_common_dir()?;
    let source_worktree =
        resolve_source_worktree(git, &git_dir, &params.remote_name, Some(&base_branch))?;

    let worktree_path = if let Some(ref at) = params.at_path {
        at.clone()
    } else if let Some(ref layout) = params.layout {
        // For wrapped non-bare layouts (e.g., contained-classic), the project
        // root from get_project_root() is the clone subdirectory (repo/main/),
        // but the template expects the wrapper directory (repo/).
        let effective_root = if layout.needs_wrapper() {
            project_root
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| project_root.to_path_buf())
        } else {
            project_root.to_path_buf()
        };
        let ctx = build_template_context(&effective_root, &params.new_branch_name);
        layout.worktree_path(&ctx)?
    } else {
        let remote_for_path = resolve_remote_for_branch(
            git,
            &params.new_branch_name,
            params.remote.as_deref(),
            &params.multi_remote_default,
        )?;
        calculate_worktree_path(
            project_root,
            &params.new_branch_name,
            &remote_for_path,
            params.multi_remote_enabled,
        )
    };

    // Commit the execution plan (#651): the requested base and the worktree
    // path are resolved, and everything left to do is planned work — the
    // remote fetch included, so the rail appears before the network
    // round-trips instead of hiding them behind a spinner. The plan lists
    // only steps that will actually be attempted — a push or fetch known to
    // be off (config or --local) plans no row. The header carries the
    // requested base; when the three-way selection lands on a different ref
    // (`origin/<base>` was fresher), the branch row picks it up via `Note`.
    let should_carry = params.carry || (!params.no_carry && params.checkout_branch_carry);
    let mut plan_rows = Vec::new();
    if params.checkout_fetch {
        plan_rows.push(Row::Step(
            StepSpec::new(StepKey::new(StageId::Fetch)).with_annotation(params.remote_name.clone()),
        ));
        plan_rows.push(Row::Step(StepSpec::new(StepKey::new(StageId::Tracking))));
    }
    plan_rows.extend([
        Row::Step(StepSpec::new(StepKey::new(StageId::PreCreateHooks))),
        Row::Step(StepSpec::new(StepKey::new(StageId::CreateBranch))),
        Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut))),
        Row::Step(
            StepSpec::new(StepKey::new(StageId::CreateWorktree))
                .with_annotation(super::branch_delete::display_path(&worktree_path)),
        ),
    ]);
    if should_carry {
        plan_rows.push(Row::Step(StepSpec::new(StepKey::new(StageId::Carry))));
    }
    if params.checkout_push {
        plan_rows.push(Row::Step(
            StepSpec::new(StepKey::new(StageId::Push)).with_annotation(format!(
                "\u{2192} {}/{}",
                params.remote_name, params.new_branch_name
            )),
        ));
    }
    // Shared files declared in the source worktree's config get a section:
    // a dim anchor plus one row per file. The probe reads the same config
    // that propagation carries into the new worktree, so plan and execution
    // agree except when the target branch's tracked daft.yml diverges —
    // rows that turn out to be no-ops vanish silently either way.
    let planned_shared =
        crate::core::shared::read_shared_paths(&source_worktree).unwrap_or_default();
    crate::core::shared::push_shared_section(&mut plan_rows, &planned_shared);
    plan_rows.push(Row::Step(StepSpec::new(StepKey::new(
        StageId::PostCreateHooks,
    ))));
    sink.on_plan(
        PlanCommit::new(plan_rows).with_header_annotation(format!("\u{2190} {base_branch}")),
    );

    // Fetch latest changes (planned above; failures warn and continue)
    if params.checkout_fetch {
        fetch_remote(git, &params.remote_name, sink);
    }

    // Determine the best checkout base (three-way branch selection). The
    // header already names the requested base; record the resolved ref on
    // the branch row only when the selection differs.
    let checkout_base = select_checkout_base(git, &base_branch, &params.remote_name, sink)?;
    if checkout_base != base_branch {
        sink.on_stage(
            &StepKey::new(StageId::CreateBranch),
            StageEvent::Note(format!("\u{2190} {checkout_base}")),
        );
    }

    // Stash uncommitted changes if carry is enabled
    let (stash_created, carry_source) = stash_if_carry(params, git, &base_branch, sink)?;

    // Run pre-create hook
    let hook_ctx = HookContext::new(
        HookType::PreCreate,
        "checkout",
        project_root,
        &git_dir,
        &params.remote_name,
        &source_worktree,
        &worktree_path,
        &params.new_branch_name,
    )
    .with_new_branch(true)
    .with_base_branch(&base_branch);

    let hook_outcome = sink.run_hook(&hook_ctx)?;
    if !hook_outcome.success && !hook_outcome.skipped {
        anyhow::bail!("Pre-create hook failed");
    }

    sink.on_step(&format!(
        "Creating worktree at '{}' with new branch '{}' from '{}'",
        worktree_path.display(),
        params.new_branch_name,
        checkout_base
    ));

    // When push is disabled, pass --no-track to prevent git's
    // branch.autoSetupMerge from auto-configuring upstream tracking
    // (the checkout base may be a remote-tracking ref like origin/master).
    let no_track = !params.checkout_push;

    // `git worktree add -b` creates the branch, checks it out, and creates
    // the worktree in one call; the three plan rows resolve around it as a
    // cosmetic split of the same operation.
    sink.on_stage(&StepKey::new(StageId::CreateBranch), StageEvent::Started);
    if let Err(e) = git.worktree_add_new_branch(
        &worktree_path,
        &params.new_branch_name,
        &checkout_base,
        no_track,
    ) {
        sink.on_stage(
            &StepKey::new(StageId::CreateBranch),
            StageEvent::Failed {
                detail: "failed (see below)".to_string(),
            },
        );
        restore_stash_on_failure(stash_created, carry_source.as_deref(), git, sink);
        anyhow::bail!("Failed to create git worktree: {}", e);
    }
    // Remember what this worktree is for (see checkout.rs). Best-effort.
    if let Some(store) = crate::core::worktree::identity_store::IdentityStore::open(&git_dir) {
        store.record(&worktree_path, &params.new_branch_name);
    }
    sink.on_stage(
        &StepKey::new(StageId::CreateBranch),
        StageEvent::Completed { annotation: None },
    );
    sink.on_stage(&StepKey::new(StageId::CheckOut), StageEvent::Started);
    sink.on_stage(
        &StepKey::new(StageId::CheckOut),
        StageEvent::Completed { annotation: None },
    );
    sink.on_stage(&StepKey::new(StageId::CreateWorktree), StageEvent::Started);

    if !worktree_path.exists() {
        sink.on_stage(
            &StepKey::new(StageId::CreateWorktree),
            StageEvent::Failed {
                detail: "directory was not created".to_string(),
            },
        );
        anyhow::bail!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        );
    }
    sink.on_stage(
        &StepKey::new(StageId::CreateWorktree),
        StageEvent::Completed { annotation: None },
    );

    // Auto-add worktree parent directory to .gitignore for in-repo layouts
    if let Err(e) = auto_gitignore_if_needed(project_root, &worktree_path, params.layout.as_ref()) {
        sink.on_warning(&format!("Could not update .gitignore: {e}"));
    }

    sink.on_step(&format!(
        "Changing directory to worktree: {}",
        worktree_path.display()
    ));
    change_directory(&worktree_path)?;

    // Apply stashed changes
    if stash_created {
        sink.on_stage(&StepKey::new(StageId::Carry), StageEvent::Started);
    }
    let (stash_applied, stash_conflict) = apply_stash(stash_created, git, sink);
    super::resolve_carry_row(should_carry, stash_created, stash_applied, sink);

    // Push and set upstream. A pre-push gate refusal is deferred, not an
    // immediate bail: the worktree must still be fully initialized
    // (propagation, shared links, post-create hooks) before the command
    // fails, so the user is left with a usable worktree.
    let (push_set, push_skipped, push_gate_error) =
        push_if_enabled(params, git, &worktree_path, presenter, sink);

    // Propagate in-scope untracked daft files from source worktree to the new
    // worktree, so that user post-create hooks can read them.
    //
    // Propagation entry points audit (Task 4.3):
    //   - checkout_branch (this site): creates a worktree with a NEW branch from an
    //     existing source worktree — propagates here.
    //   - checkout (checkout.rs execute): creates a worktree for an EXISTING branch
    //     from an existing source worktree — also propagates (same pattern).
    //   - clone (clone.rs): starts from a remote URL with no source worktree — no
    //     propagation needed (fresh repo with no visitor-config context to carry).
    //   - init (init.rs): creates a brand-new empty repo — no source worktree, no
    //     propagation needed.
    //   - checkout's early-return paths (existing worktree for branch / existing dir
    //     on disk): navigate to an already-materialized worktree — no new worktree
    //     is created, no propagation step.
    match crate::hooks::visitor_propagation::propagate(&source_worktree, &worktree_path) {
        Ok(result) => {
            for filename in &result.files_propagated {
                crate::log_debug!("propagated {} to new worktree", filename);
            }
            // Record what was just written as the new worktree's seed: the
            // provenance base for pristine/refined classification and
            // three-way consolidation. Best-effort by design.
            if !result.files_propagated.is_empty()
                && let Some(seeds) = crate::hooks::visitor_seeds::SeedsContext::open(&git_dir)
            {
                seeds.record_seeds(
                    &params.new_branch_name,
                    &worktree_path,
                    &result.files_propagated,
                );
            }
        }
        Err(e) => {
            sink.on_warning(&format!("visitor-config propagation failed: {}", e));
        }
    }

    // Link shared files AFTER propagation and BEFORE post-create hooks.
    // Order is load-bearing: a *visitor* daft.yml (untracked) reaches the new
    // worktree only via the propagation step above, so reading `shared:` before
    // propagation finds no config and silently links nothing. (A tracked daft.yml
    // arrives via the git checkout regardless of order, which is why this bug was
    // invisible until visitor configs existed — do not move this back above
    // propagation.) Linking before hooks lets hooks depend on .env etc.
    let link_result =
        crate::core::shared::link_shared_files_on_create(&worktree_path, &git_dir, project_root);
    crate::core::shared::report_link_results(&link_result, &planned_shared, sink);

    // Run post-create hook
    let post_hook_ctx = HookContext::new(
        HookType::PostCreate,
        "checkout",
        project_root,
        &git_dir,
        &params.remote_name,
        &source_worktree,
        &worktree_path,
        &params.new_branch_name,
    )
    .with_new_branch(true)
    .with_base_branch(&base_branch);

    let post_hook_outcome = sink.run_hook(&post_hook_ctx)?;

    // The worktree is fully set up — now surface a deferred pre-push gate
    // refusal as the command's failure (#599 acceptance: non-zero exit).
    if let Some(message) = push_gate_error {
        anyhow::bail!(message);
    }

    Ok(CheckoutBranchResult {
        new_branch_name: params.new_branch_name.clone(),
        base_branch: checkout_base,
        worktree_path,
        cd_target: get_current_directory()?,
        stash_applied,
        stash_conflict,
        push_set,
        push_skipped,
        git_dir,
        post_hook_outcome,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Resolve the base branch (explicit or current).
fn resolve_base_branch(
    params: &CheckoutBranchParams,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> Result<String> {
    match &params.base_branch_name {
        Some(branch) => {
            sink.on_step(&format!(
                "Using explicitly provided base branch: '{branch}'"
            ));
            Ok(branch.clone())
        }
        None => {
            sink.on_step("Base branch not specified, using current branch...");
            let current = git.symbolic_ref_short_head()?;
            sink.on_step(&format!("Using current branch as base: '{current}'"));
            Ok(current)
        }
    }
}

/// Resolve the worktree to use as the `source_worktree` — the visitor-config
/// propagation source and the hook context's source path.
///
/// Normally this is the worktree the command was run in (its toplevel, which
/// also normalizes a subdirectory cwd to the worktree root). But `daft start` /
/// `daft go` are legitimately run from a contained-layout's **bare container
/// root**, which is not a worktree and holds no `daft.yml`. Using it as the
/// propagation source means a *visitor* (untracked) `daft.yml` never reaches the
/// new worktree — no hooks, no shared files. (Tracked configs are unaffected:
/// they arrive via the git checkout regardless of cwd.) When cwd is not a
/// worktree, fall back to a worktree that holds the user's config: the
/// `preferred_branch`'s worktree (the base branch), then the default branch's.
/// Falls back to cwd when none is found, so propagation simply no-ops as before.
///
/// The structural "where am I" decision is delegated to
/// [`crate::core::repo::resolve_worktree_position`] (the shared primitive that
/// `daft install`/`daft doctor` also use, so the two resolvers can't drift).
/// This adds the checkout-specific bias on top: prefer the `preferred_branch`'s
/// worktree, then the default branch's (via the network-capable
/// `get_default_branch_local`), then any worktree the local probe already found.
pub(crate) fn resolve_source_worktree(
    git: &GitCommand,
    git_dir: &Path,
    remote_name: &str,
    preferred_branch: Option<&str>,
) -> Result<PathBuf> {
    use crate::core::repo::WorktreePosition;

    match crate::core::repo::resolve_worktree_position(&get_current_directory()?) {
        // Inside a worktree → its toplevel (also normalizes a subdir cwd).
        WorktreePosition::InWorktree { root } => Ok(root),

        // Bare container root: bias toward the worktree that carries the user's
        // visitor config before falling back to whatever the probe found.
        WorktreePosition::ContainerRoot { representative } => {
            // Prefer the base branch's worktree (the propagation source).
            if let Some(branch) = preferred_branch
                && let Ok(Some(wt)) = git.find_worktree_for_branch(branch)
            {
                return Ok(wt);
            }

            // Then the default branch's worktree.
            if let Ok(default_branch) =
                crate::core::remote::get_default_branch_local(git_dir, remote_name, false)
                && let Ok(Some(wt)) = git.find_worktree_for_branch(&default_branch)
            {
                return Ok(wt);
            }

            // Then any worktree the local probe already resolved.
            if let Some(wt) = representative {
                return Ok(wt);
            }

            // Nothing resolvable — preserve prior behavior (propagation no-ops).
            get_current_directory()
        }

        // Not in a repo — preserve prior behavior (propagation no-ops).
        WorktreePosition::NotInRepo => get_current_directory(),
    }
}

/// Fetch latest changes from the remote.
///
/// Both fetches are planned rail rows (`Fetch`, then `Tracking`). Failures
/// are non-fatal by design — the command continues on local refs — so a
/// failed fetch resolves its row as a yellow attention skip and the full
/// error lands above the rail as a warning.
fn fetch_remote(git: &GitCommand, remote_name: &str, sink: &mut impl ProgressSink) {
    const FETCH_FAILED: &str = super::FETCH_FAILED_REASON;

    sink.on_stage(&StepKey::new(StageId::Fetch), StageEvent::Started);
    sink.on_step(&format!(
        "Fetching latest changes from remote '{remote_name}'..."
    ));
    match git.fetch(remote_name, false) {
        Ok(()) => sink.on_stage(
            &StepKey::new(StageId::Fetch),
            StageEvent::Completed { annotation: None },
        ),
        Err(e) => {
            sink.on_stage(
                &StepKey::new(StageId::Fetch),
                StageEvent::SkippedAttention {
                    reason: FETCH_FAILED.to_string(),
                },
            );
            sink.on_warning(&format!("Failed to fetch from remote '{remote_name}': {e}"));
        }
    }

    sink.on_stage(&StepKey::new(StageId::Tracking), StageEvent::Started);
    sink.on_step("Setting up remote tracking branches...");
    match git.fetch_refspec(
        remote_name,
        &format!("+refs/heads/*:refs/remotes/{remote_name}/*"),
    ) {
        Ok(()) => sink.on_stage(
            &StepKey::new(StageId::Tracking),
            StageEvent::Completed { annotation: None },
        ),
        Err(e) => {
            sink.on_stage(
                &StepKey::new(StageId::Tracking),
                StageEvent::SkippedAttention {
                    reason: FETCH_FAILED.to_string(),
                },
            );
            sink.on_warning(&format!("Failed to set up remote tracking branches: {e}"));
        }
    }
}

/// Three-way branch selection algorithm for optimal worktree base branch.
fn select_checkout_base(
    git: &GitCommand,
    base_branch: &str,
    remote_name: &str,
    sink: &mut impl ProgressSink,
) -> Result<String> {
    let local_ref = format!("refs/heads/{base_branch}");
    let remote_ref = format!("refs/remotes/{remote_name}/{base_branch}");

    let local_exists = git.show_ref_exists(&local_ref)?;
    let remote_exists = git.show_ref_exists(&remote_ref)?;

    if remote_exists && local_exists {
        let local_ahead = git
            .rev_list_count(&format!("{remote_name}/{base_branch}..{base_branch}"))
            .unwrap_or(DEFAULT_COMMIT_COUNT)
            > COMMITS_AHEAD_THRESHOLD;

        if local_ahead {
            sink.on_step(&format!(
                "Using local branch '{base_branch}' as base (has local commits)"
            ));
            Ok(base_branch.to_string())
        } else {
            sink.on_step(&format!(
                "Using remote branch '{remote_name}/{base_branch}' as base (has latest changes)"
            ));
            Ok(format!("{remote_name}/{base_branch}"))
        }
    } else if local_exists {
        sink.on_step(&format!("Using local branch '{base_branch}' as base"));
        Ok(base_branch.to_string())
    } else if remote_exists {
        sink.on_step(&format!(
            "Local branch '{base_branch}' not found, using remote branch '{remote_name}/{base_branch}'"
        ));
        Ok(format!("{remote_name}/{base_branch}"))
    } else {
        sink.on_step(&format!(
            "Neither local nor remote branch found for '{base_branch}', using as-is"
        ));
        Ok(base_branch.to_string())
    }
}

/// Stash uncommitted changes if carry behavior is enabled.
///
/// Returns (stash_created, carry_source_path).
fn stash_if_carry(
    params: &CheckoutBranchParams,
    git: &GitCommand,
    base_branch: &str,
    sink: &mut impl ProgressSink,
) -> Result<(bool, Option<PathBuf>)> {
    let should_carry = if params.carry {
        true
    } else if params.no_carry {
        false
    } else {
        params.checkout_branch_carry
    };

    if !should_carry {
        sink.on_step("Skipping carry (--no-carry flag set or carry disabled in config)");
        return Ok((false, None));
    }

    // Determine the carry source worktree
    let carry_source = if params.base_branch_name.is_some() {
        // Explicit base branch: find its worktree
        match git.find_worktree_for_branch(base_branch) {
            Ok(Some(path)) => {
                sink.on_step(&format!(
                    "Found worktree for base branch '{}' at '{}'",
                    base_branch,
                    path.display()
                ));
                Some(path)
            }
            Ok(None) => {
                sink.on_step(&format!(
                    "No worktree found for base branch '{}', skipping carry",
                    base_branch
                ));
                return Ok((false, None));
            }
            Err(e) => {
                sink.on_warning(&format!(
                    "Could not look up worktree for base branch '{}': {e}",
                    base_branch
                ));
                return Ok((false, None));
            }
        }
    } else {
        // No explicit base branch: carry from current worktree
        let in_worktree = git.rev_parse_is_inside_work_tree().unwrap_or(false);
        if in_worktree {
            Some(get_current_directory()?)
        } else {
            sink.on_step("Skipping carry (not inside a worktree)");
            return Ok((false, None));
        }
    };

    let carry_path = carry_source.as_ref().unwrap();
    // Never stash a worktree paused mid-operation. `git stash push` on a
    // half-done rebase can even succeed — reverting a resolved conflict back
    // to the upstream content — after which `git rebase --continue` sees an
    // empty patch and silently drops the commit from the branch. The lookup
    // above can resolve such a worktree precisely because it recovers
    // mid-operation identity (op_state), and the current-worktree arm can be
    // sitting in one too.
    if let Some(op) = crate::git::op_state::probe_op_state(carry_path) {
        sink.on_step(&format!(
            "Skipping carry: worktree at '{}' is {}",
            carry_path.display(),
            op.kind.label()
        ));
        return Ok((false, None));
    }
    change_directory(carry_path)?;

    match git.has_uncommitted_changes() {
        Ok(true) => {
            sink.on_step(&format!(
                "Stashing uncommitted changes from '{}'...",
                carry_path.display()
            ));
            if let Err(e) = git.stash_push_with_untracked("daft: carry changes to new worktree") {
                anyhow::bail!("Failed to stash uncommitted changes: {e}");
            }
            Ok((true, carry_source))
        }
        Ok(false) => {
            sink.on_step("No uncommitted changes to carry");
            Ok((false, carry_source))
        }
        Err(e) => {
            sink.on_warning(&format!("Could not check for uncommitted changes: {e}"));
            Ok((false, carry_source))
        }
    }
}

/// Restore stashed changes when worktree creation fails.
fn restore_stash_on_failure(
    stash_created: bool,
    carry_source: Option<&Path>,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) {
    if stash_created {
        if let Some(carry_path) = carry_source {
            change_directory(carry_path).ok();
        }
        sink.on_step("Restoring stashed changes due to worktree creation failure...");
        if let Err(pop_err) = git.stash_pop() {
            sink.on_warning(&format!(
                "Your changes are still in the stash. Run 'git stash pop' to restore them. Error: {pop_err}"
            ));
        }
    }
}

/// Apply stashed changes to the new worktree.
fn apply_stash(
    stash_created: bool,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> (bool, bool) {
    if stash_created {
        sink.on_step("Applying stashed changes to new worktree...");
        if let Err(e) = git.stash_pop() {
            sink.on_warning(&format!(
                "Stash could not be applied cleanly. Resolve conflicts and run 'git stash pop'. Error: {e}"
            ));
            (false, true)
        } else {
            sink.on_step("Changes successfully applied to new worktree");
            (true, false)
        }
    } else {
        (false, false)
    }
}

/// Push and set upstream tracking if the setting is enabled.
///
/// Runs the push from the new worktree so the repo's `pre-push` hook fires
/// in the branch being pushed. Whether git dispatches the hook at all is
/// resolved per `daft.checkout.pushVerify` (#679): under `auto`, a ref-only
/// push — one introducing no commits absent from the target remote — has
/// nothing for a content gate to validate and suppresses the hook.
/// Returns `(push_set, push_skipped, gate_error)`. A push failure with the
/// repo's pre-push hook in effect escalates via `gate_error` (the caller
/// fails the command after finishing worktree setup, #599); hook-less,
/// bypassed, or hook-skipped failures keep the legacy warn-and-continue
/// behavior (a gate that never ran cannot have refused the push).
fn push_if_enabled(
    params: &CheckoutBranchParams,
    git: &GitCommand,
    worktree_path: &Path,
    presenter: Option<&Arc<dyn JobPresenter>>,
    sink: &mut impl ProgressSink,
) -> (bool, bool, Option<String>) {
    if !params.checkout_push {
        // A known-off push plans no row (#651), so there is nothing to resolve.
        sink.on_step("Skipping push (disabled in config)");
        return (false, true, None);
    }
    let push_key = StepKey::new(StageId::Push);

    sink.on_step(&format!(
        "Pushing and setting upstream to '{}/{}'...",
        params.remote_name, params.new_branch_name
    ));
    sink.on_stage(&push_key, StageEvent::Started);

    // Probe lazily: hook existence only when `auto` can act on it, and the
    // unpushed-commit count only when a hook is actually present (with no
    // hook, verify and skip are behaviorally identical). The probe uses the
    // fully-qualified branch ref — a same-named tag would shadow the short
    // name in rev-list's resolution.
    let (hook_present, unpushed_count) = match params.push_verify {
        // `--no-verify` skips silently regardless of hook presence, so don't probe.
        _ if params.no_verify => (None, None),
        PushVerify::Auto => {
            let present = git.pre_push_hook_exists(worktree_path);
            let count = if present {
                match git.count_commits_not_on_remote(
                    &format!("refs/heads/{}", params.new_branch_name),
                    &params.remote_name,
                    worktree_path,
                ) {
                    Ok(count) => Some(count),
                    Err(e) => {
                        crate::log_debug!("pre-push ref-only probe failed: {e}");
                        None
                    }
                }
            } else {
                None
            };
            (Some(present), count)
        }
        // `never` announces the skip only when a hook actually exists, so probe
        // its presence (a cheap stat). `always` verifies unconditionally and
        // lets push_with_hooks resolve presence for its own verdict.
        PushVerify::Never => (Some(git.pre_push_hook_exists(worktree_path)), None),
        PushVerify::Always => (None, None),
    };

    let verify = match resolve_pre_push(
        params.push_verify,
        params.no_verify,
        hook_present.unwrap_or(false),
        PushPayload::Commits { unpushed_count },
    ) {
        PrePushDecision::Verify => true,
        PrePushDecision::Skip(reason) => {
            if let Some(reason) = reason {
                sink.on_step(reason);
            }
            false
        }
    };

    // When the hook renders through the presenter its `MultiProgress` owns the
    // terminal, so pause the outer "Creating worktree..." spinner across the
    // render (the same contract CommandBridge::run_hook uses for post-create
    // hooks). A ref-only push under `auto` skips the hook (#679), so the
    // spinner keeps running and stays the only progress the user sees.
    let renders_hook = verify
        && presenter.is_some()
        && hook_present.unwrap_or_else(|| git.pre_push_hook_exists(worktree_path));
    if renders_hook {
        sink.pause_spinner();
    }
    let result = push_with_hooks(
        git,
        PushAction::SetUpstream {
            remote: &params.remote_name,
            branch: &params.new_branch_name,
            force_with_lease: false,
        },
        worktree_path,
        verify,
        &NoopStageRunner,
        presenter,
        hook_present,
    );
    if renders_hook {
        sink.resume_spinner();
    }

    let failure = match result {
        Ok(outcome) => match outcome.failure {
            None => {
                sink.on_step(&format!(
                    "Push to '{}' and upstream tracking set successfully",
                    params.remote_name
                ));
                sink.on_stage(&push_key, StageEvent::Completed { annotation: None });
                return (true, false, None);
            }
            Some(msg) => {
                let gated = matches!(outcome.hook, HookVerdict::Rejected | HookVerdict::Passed);
                if gated {
                    // The rail detail must not blame the hook for a push it
                    // let through, nor the remote for one git refused
                    // locally — `HookVerdict::rail_detail` owns that line
                    // for every call site.
                    sink.on_stage(
                        &push_key,
                        StageEvent::Failed {
                            detail: outcome.hook.rail_detail().to_string(),
                        },
                    );
                    let hint = if outcome.hook.no_verify_might_help() {
                        " (or re-run with --no-verify to bypass the hook)"
                    } else {
                        ""
                    };
                    return (
                        false,
                        false,
                        Some(format!(
                            "Could not push '{}' to '{}': {} ({}). \
                             The worktree was created and is ready at '{}'. \
                             Push manually with: git push -u {} {}{}",
                            params.new_branch_name,
                            params.remote_name,
                            msg,
                            outcome.hook.failure_cause(),
                            worktree_path.display(),
                            params.remote_name,
                            params.new_branch_name,
                            hint,
                        )),
                    );
                }
                // #679: the hook was skipped (ref-only auto-skip or --no-verify)
                // but the push itself failed for a non-hook reason — a server-side
                // rejection, transport, or auth error. A gate that never ran
                // cannot have refused the push, yet the branch genuinely never
                // reached the remote, so escalate to a hard error (like a real
                // hook rejection) rather than warn-and-continue. This keeps
                // `daft start <b> && …` from proceeding on a branch that is not
                // on the remote. Hook-less repos (verdict NoHook) keep the legacy
                // warn-and-continue behavior.
                if outcome.hook == HookVerdict::Bypassed {
                    // Resolve the Push row this branch Started — returning
                    // without an event would leave it to render as a dim
                    // "(not run)" under a hard push error.
                    sink.on_stage(
                        &push_key,
                        StageEvent::Failed {
                            detail: "failed (see below)".to_string(),
                        },
                    );
                    return (
                        false,
                        false,
                        Some(format!(
                            "Could not push '{}' to '{}': {}. \
                             The worktree was created and is ready at '{}'. \
                             Push manually with: git push -u {} {}",
                            params.new_branch_name,
                            params.remote_name,
                            msg,
                            worktree_path.display(),
                            params.remote_name,
                            params.new_branch_name,
                        )),
                    );
                }
                msg
            }
        },
        Err(e) => format!("{e}"),
    };

    sink.on_stage(
        &push_key,
        StageEvent::Failed {
            detail: "failed (see below)".to_string(),
        },
    );
    sink.on_warning(&format!(
        "Could not push '{}' to '{}': {}. The worktree is ready locally. Push manually with: git push -u {} {}",
        params.new_branch_name, params.remote_name, failure,
        params.remote_name, params.new_branch_name
    ));
    (false, false, None)
}

#[cfg(test)]
mod tests {
    use super::{CheckoutBranchParams, push_if_enabled};
    use crate::core::ProgressSink;
    use crate::core::settings::PushVerify;
    use crate::core::stage::{StageEvent, StageId, StepKey};
    use crate::core::worktree::push::HookVerdict;
    use crate::executor::presenter::{JobPresenter, NullPresenter};
    use crate::git::GitCommand;
    use crate::utils::git_command_at;
    use std::path::{Path, PathBuf};
    use std::process::Stdio;
    use std::sync::Arc;

    // The pure `resolve_pre_push` decision tests live with the fn in
    // `core::worktree::push` since it moved there for the delete sites (#747).

    // --- integration: spinner coordination around the pre-push render (#679) ---

    /// Records the spinner-control calls `push_if_enabled` makes so a test can
    /// assert the pre-push render is (or is not) bracketed by pause/resume.
    #[derive(Default)]
    struct RecordingSink {
        events: Vec<&'static str>,
    }

    impl ProgressSink for RecordingSink {
        fn on_step(&mut self, _msg: &str) {
            self.events.push("step");
        }
        fn on_warning(&mut self, _msg: &str) {
            self.events.push("warning");
        }
        fn on_debug(&mut self, _msg: &str) {}
        fn pause_spinner(&mut self) {
            self.events.push("pause");
        }
        fn resume_spinner(&mut self) {
            self.events.push("resume");
        }
    }

    fn git_in(dir: &Path, args: &[&str]) {
        // commit.gpgsign=false: a global signing config would route fixture
        // commits through the real gpg agent, which flakes under parallel
        // test load (same rationale as git::mod's seeding tests).
        let status = git_command_at(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed to spawn git");
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    /// Bare remote + a one-commit `main` clone pushed to origin, with `feat`
    /// created at the already-pushed tip (so its upstream push is ref-only) and
    /// a passing `pre-push` hook installed (so the probe sees a hook and the
    /// push still succeeds).
    fn repo_with_hook_and_remote() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let remote = dir.path().join("remote.git");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&remote).unwrap();
        git_in(&remote, &["init", "--bare"]);
        std::fs::create_dir_all(&work).unwrap();
        git_in(&work, &["init", "-b", "main"]);
        git_in(
            &work,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        std::fs::write(work.join("a.txt"), "a").unwrap();
        git_in(&work, &["add", "."]);
        git_in(&work, &["commit", "-m", "init"]);
        git_in(&work, &["push", "-u", "origin", "main"]);
        git_in(&work, &["branch", "feat"]);

        let hook = work.join(".git").join("hooks").join("pre-push");
        std::fs::write(&hook, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (dir, work)
    }

    fn params_for(branch: &str, push_verify: PushVerify) -> CheckoutBranchParams {
        CheckoutBranchParams {
            new_branch_name: branch.to_string(),
            base_branch_name: None,
            carry: false,
            no_carry: false,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            checkout_branch_carry: false,
            checkout_push: true,
            no_verify: false,
            push_verify,
            checkout_fetch: false,
            layout: None,
            at_path: None,
        }
    }

    #[test]
    fn render_is_bracketed_by_spinner_pause_and_resume() {
        // `always` forces the hook to run on the (ref-only) upstream push, so it
        // renders through the presenter — the outer spinner must be paused for
        // the duration and resumed after, mirroring CommandBridge::run_hook.
        let (_dir, work) = repo_with_hook_and_remote();
        let git = GitCommand::new(false);
        let params = params_for("feat", PushVerify::Always);
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let mut sink = RecordingSink::default();

        let (push_set, _skipped, gate) =
            push_if_enabled(&params, &git, &work, Some(&presenter), &mut sink);

        assert!(
            push_set,
            "ref-only push with a passing hook should succeed: {gate:?}"
        );
        let pause = sink.events.iter().position(|e| *e == "pause");
        let resume = sink.events.iter().position(|e| *e == "resume");
        assert!(
            pause.is_some() && resume.is_some(),
            "the render must be bracketed by pause/resume: {:?}",
            sink.events
        );
        assert!(
            pause < resume,
            "pause must precede resume: {:?}",
            sink.events
        );
    }

    #[test]
    fn auto_ref_only_skip_leaves_the_spinner_running() {
        // The #679 case: a ref-only push under `auto` skips the hook, so the
        // spinner is never paused — it stays visible for the whole checkout
        // instead of the terminal going silent.
        let (_dir, work) = repo_with_hook_and_remote();
        let git = GitCommand::new(false);
        let params = params_for("feat", PushVerify::Auto);
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let mut sink = RecordingSink::default();

        let (push_set, _skipped, gate) =
            push_if_enabled(&params, &git, &work, Some(&presenter), &mut sink);

        assert!(push_set, "the ref-only push should still succeed: {gate:?}");
        assert!(
            !sink.events.contains(&"pause"),
            "a skipped hook must not pause the spinner: {:?}",
            sink.events
        );
    }

    // --- the Push row resolves on every failure shape (#688 review) ---

    fn push_failure_detail(sink: &crate::core::RecordingStageSink) -> Option<String> {
        let push_key = StepKey::new(StageId::Push);
        sink.events.iter().find_map(|(k, e)| match e {
            StageEvent::Failed { detail } if *k == push_key => Some(detail.clone()),
            _ => None,
        })
    }

    #[test]
    fn bypassed_push_failure_resolves_the_push_row() {
        // The Bypassed branch returned without resolving the Push row it
        // Started, so the receipt rendered the failed push as a dim
        // "(not run)" directly above a hard "Could not push" error.
        let (_dir, work) = repo_with_hook_and_remote();
        git_in(
            &work,
            &[
                "remote",
                "set-url",
                "origin",
                "/nonexistent/daft-remote.git",
            ],
        );
        let git = GitCommand::new(false);
        let mut params = params_for("feat", PushVerify::Auto);
        params.no_verify = true; // hook present + bypassed
        let mut sink = crate::core::RecordingStageSink::default();

        let (push_set, skipped, gate) = push_if_enabled(&params, &git, &work, None, &mut sink);

        assert!(!push_set && !skipped, "the push must hard-fail: {gate:?}");
        assert_eq!(
            push_failure_detail(&sink).as_deref(),
            Some("failed (see below)"),
            "the Started push row must resolve Failed: {:?}",
            sink.events
        );
    }

    #[test]
    fn passed_hook_remote_rejection_does_not_blame_the_gate() {
        // `Passed` means the pre-push hook let the push through and the
        // push was rejected downstream — the rail detail must not read
        // "pre-push gate refused" (push.rs's failure_cause tests guard the
        // same attribution line for the error text).
        let (dir, work) = repo_with_hook_and_remote();
        let remote_hook = dir
            .path()
            .join("remote.git")
            .join("hooks")
            .join("pre-receive");
        std::fs::write(&remote_hook, "#!/bin/sh\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&remote_hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let git = GitCommand::new(false);
        let params = params_for("feat", PushVerify::Always);
        let mut sink = crate::core::RecordingStageSink::default();

        let (push_set, _skipped, gate) = push_if_enabled(&params, &git, &work, None, &mut sink);

        assert!(!push_set, "the remote must reject the push: {gate:?}");
        assert_eq!(
            push_failure_detail(&sink).as_deref(),
            Some(HookVerdict::Passed.rail_detail()),
            "a passed hook must not be blamed for the rejection: {:?}",
            sink.events
        );
    }
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    use crate::core::RecordingStageSink;
    use crate::core::stage::{Row, StageEvent, StageId};
    use serial_test::serial;
    use std::process::Stdio;

    /// Run git through `utils::git_command_at`, which scrubs the full set
    /// of `GIT_*` discovery vars (a hand-rolled remove of GIT_DIR /
    /// GIT_WORK_TREE misses the rest — the Test Hygiene rule exists for
    /// exactly this). Local test identity only, never global config.
    fn git(dir: &Path, args: &[&str]) {
        crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
    }

    struct CwdGuard {
        original: PathBuf,
    }
    impl CwdGuard {
        fn new() -> Self {
            Self {
                original: std::env::current_dir().expect("cwd readable"),
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

    /// The plan commits with the locked row set, the header carries the
    /// requested base, and events narrate the cosmetic
    /// branch/checkout/worktree split plus the expected push skip (#651).
    #[test]
    #[serial]
    fn plan_commits_with_locked_row_set_and_push_skip() {
        // `execute` records the worktree's identity — without this, the
        // write lands in the developer's real state dir (#697).
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        git(tmp.path(), &["commit", "--allow-empty", "-q", "-m", "init"]);
        std::env::set_current_dir(tmp.path()).unwrap();

        let worktree_path = tmp.path().join("feat-x");
        let params = CheckoutBranchParams {
            new_branch_name: "feat-x".to_string(),
            base_branch_name: Some("main".to_string()),
            carry: false,
            no_carry: true,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            checkout_branch_carry: false,
            checkout_push: false,
            no_verify: false,
            push_verify: PushVerify::Auto,
            checkout_fetch: false,
            layout: None,
            at_path: Some(worktree_path.clone()),
        };

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let result = execute(&params, &git_cmd, tmp.path(), None, &mut sink)
            .expect("checkout-branch succeeds");
        assert_eq!(result.new_branch_name, "feat-x");
        assert!(worktree_path.exists());

        let plan = sink.plan.as_ref().expect("plan committed");
        assert_eq!(
            plan.header_annotation.as_deref(),
            Some("\u{2190} main"),
            "header carries the resolved base"
        );
        let ids: Vec<StageId> = plan.steps().map(|s| s.key.id).collect();
        assert_eq!(
            ids,
            vec![
                StageId::PreCreateHooks,
                StageId::CreateBranch,
                StageId::CheckOut,
                StageId::CreateWorktree,
                StageId::PostCreateHooks,
            ],
            "carry off => no Carry row; push off => no Push row"
        );
        assert!(!plan.rows.iter().any(|r| matches!(r, Row::Group { .. })));

        // A push known to be off is not planned, so no Push event may fire.
        assert!(
            sink.events.iter().all(|(k, _)| k.id != StageId::Push),
            "events: {:?}",
            sink.events
        );
        // The atomic worktree-add narrates all three creation steps, in order.
        let completed: Vec<StageId> = sink
            .events
            .iter()
            .filter_map(|(k, e)| matches!(e, StageEvent::Completed { .. }).then_some(k.id))
            .collect();
        assert_eq!(
            completed,
            vec![
                StageId::CreateBranch,
                StageId::CheckOut,
                StageId::CreateWorktree
            ]
        );
        // Hooks fired through the sink (pre + post).
        assert_eq!(
            sink.hooks_run,
            vec![
                crate::hooks::HookType::PreCreate,
                crate::hooks::HookType::PostCreate
            ]
        );
    }

    /// With `daft.checkout.fetch` on, the fetch is planned work: the rail
    /// opens with `Fetch` + `Tracking` rows (no pre-rail spinner), the header
    /// names the requested base, and the resolved base reaches the branch row
    /// via `Note` when the three-way selection picks a different ref. Fetch
    /// failures are non-fatal: the rows resolve as attention skips and the
    /// worktree is still created.
    #[test]
    #[serial]
    fn fetch_rows_planned_first_and_resolved_base_noted_on_branch_row() {
        // `execute` records the worktree's identity — without this, the
        // write lands in the developer's real state dir (#697).
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        git(tmp.path(), &["commit", "--allow-empty", "-q", "-m", "init"]);
        // Fake a remote-tracking ref at HEAD: the three-way selection sees
        // local and remote in sync and picks `origin/main`. No `origin`
        // remote is configured, so both fetches fail (non-fatally).
        git(
            tmp.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        std::env::set_current_dir(tmp.path()).unwrap();

        let worktree_path = tmp.path().join("feat-x");
        let params = CheckoutBranchParams {
            new_branch_name: "feat-x".to_string(),
            base_branch_name: Some("main".to_string()),
            carry: false,
            no_carry: true,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            checkout_branch_carry: false,
            checkout_push: false,
            no_verify: false,
            push_verify: PushVerify::Auto,
            checkout_fetch: true,
            layout: None,
            at_path: Some(worktree_path.clone()),
        };

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let result = execute(&params, &git_cmd, tmp.path(), None, &mut sink)
            .expect("fetch failure is non-fatal");
        assert!(worktree_path.exists());
        // The result reports the resolved base — the selection picked the
        // remote-tracking ref.
        assert_eq!(result.base_branch, "origin/main");

        let plan = sink.plan.as_ref().expect("plan committed");
        assert_eq!(
            plan.header_annotation.as_deref(),
            Some("\u{2190} main"),
            "header names the requested base, known before the fetch"
        );
        let specs: Vec<_> = plan.steps().collect();
        assert_eq!(specs[0].key.id, StageId::Fetch);
        assert_eq!(
            specs[0].annotation.as_deref(),
            Some("origin"),
            "fetch row names the remote"
        );
        assert_eq!(specs[1].key.id, StageId::Tracking);
        assert_eq!(specs[2].key.id, StageId::PreCreateHooks);

        // Both fetch rows started and resolved as attention skips.
        for id in [StageId::Fetch, StageId::Tracking] {
            let events: Vec<_> = sink
                .events
                .iter()
                .filter(|(k, _)| k.id == id)
                .map(|(_, e)| e.clone())
                .collect();
            assert_eq!(events[0], StageEvent::Started, "{id:?}");
            assert!(
                matches!(
                    &events[1],
                    StageEvent::SkippedAttention { reason }
                        if reason.contains("continuing with local refs")
                ),
                "{id:?} resolves as attention skip, got {events:?}"
            );
        }

        // The selection picked the remote ref; the branch row records it.
        assert!(
            sink.events
                .iter()
                .any(|(k, e)| k.id == StageId::CreateBranch
                    && *e == StageEvent::Note("\u{2190} origin/main".to_string())),
            "resolved base noted on the branch row: {:?}",
            sink.events
        );
    }

    /// A `shared:` declaration in the source worktree plans the shared-files
    /// section (anchor + one row per path); execution completes the row of a
    /// collected file and silently vanishes the row of a declared-but-never-
    /// collected one (#651).
    #[test]
    #[serial]
    fn plan_shared_section_rows_resolve_by_outcome() {
        // `execute` records the worktree's identity — without this, the
        // write lands in the developer's real state dir (#697).
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(
            tmp.path().join("daft.yml"),
            "shared:\n  - .env\n  - .envrc\n",
        )
        .unwrap();
        git(tmp.path(), &["add", "daft.yml"]);
        git(tmp.path(), &["commit", "-q", "-m", "init"]);
        // Collect `.env` into shared storage; leave `.envrc` declared only.
        let storage = crate::core::shared::shared_storage_dir(&tmp.path().join(".git"));
        std::fs::create_dir_all(&storage).unwrap();
        std::fs::write(storage.join(".env"), "SECRET=1\n").unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let worktree_path = tmp.path().join("feat-x");
        let params = CheckoutBranchParams {
            new_branch_name: "feat-x".to_string(),
            base_branch_name: Some("main".to_string()),
            carry: false,
            no_carry: true,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            checkout_branch_carry: false,
            checkout_push: false,
            no_verify: false,
            push_verify: PushVerify::Auto,
            checkout_fetch: false,
            layout: None,
            at_path: Some(worktree_path.clone()),
        };

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        execute(&params, &git_cmd, tmp.path(), None, &mut sink).expect("checkout-branch succeeds");

        let plan = sink.plan.as_ref().expect("plan committed");
        assert!(
            plan.rows
                .iter()
                .any(|r| matches!(r, Row::Group { label } if label == "shared files")),
            "shared anchor planned"
        );
        let shared_labels: Vec<_> = plan
            .steps()
            .filter(|s| s.key.id == StageId::SharedFile)
            .map(|s| (s.key.scope.clone(), s.label.clone()))
            .collect();
        assert_eq!(
            shared_labels,
            vec![
                (Some(".env".into()), Some(".env".into())),
                (Some(".envrc".into()), Some(".envrc".into())),
            ],
            "one row per declared path, path as fixed label"
        );

        let shared_events: Vec<_> = sink
            .events
            .iter()
            .filter(|(k, _)| k.id == StageId::SharedFile)
            .map(|(k, e)| (k.scope.clone().unwrap(), e.clone()))
            .collect();
        assert_eq!(shared_events.len(), 2);
        assert_eq!(
            shared_events[0],
            (".env".into(), StageEvent::Completed { annotation: None }),
            "collected file completes its row"
        );
        // Declared but never collected: the receipt must say the file is
        // missing, not drop the row.
        let (path, event) = &shared_events[1];
        assert_eq!(path, ".envrc");
        assert!(
            matches!(
                event,
                StageEvent::SkippedAttention { reason } if reason.contains("missing from shared storage")
            ),
            "declared-only file resolves as missing, got {event:?}"
        );
        assert!(
            worktree_path.join(".env").is_symlink(),
            "the link really happened"
        );
    }

    /// A worktree paused mid-operation must never be a carry source: `git
    /// stash push` on a half-done rebase can *succeed* — emptying a resolved
    /// conflict back to the upstream content — after which `git rebase
    /// --continue` sees an empty patch and silently drops the commit from
    /// the branch. The base worktree here is detached, as it is for a
    /// rebase's whole duration, so it is resolved through the
    /// recovered-identity tier — exactly the lookup that makes it findable
    /// as a carry source at all.
    #[test]
    #[serial]
    fn carry_skips_a_base_worktree_paused_mid_rebase() {
        // `execute` records the worktree's identity — without this, the
        // write lands in the developer's real state dir (#697).
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(tmp.path().join("f.txt"), "base\n").unwrap();
        git(tmp.path(), &["add", "f.txt"]);
        git(tmp.path(), &["commit", "-q", "-m", "init"]);
        git(tmp.path(), &["branch", "feat"]);
        let feat_wt = tmp.path().join("feat-wt");
        git(
            tmp.path(),
            &["worktree", "add", "-q", feat_wt.to_str().unwrap(), "feat"],
        );
        // Mid-rebase shape: HEAD detached (git's posture for the whole
        // operation) plus the state files the stat-only probe reads.
        git(&feat_wt, &["checkout", "-q", "--detach"]);
        let private = crate::git::op_state::resolve_worktree_git_dir(&feat_wt).unwrap();
        std::fs::create_dir_all(private.join("rebase-merge")).unwrap();
        std::fs::write(private.join("rebase-merge/head-name"), "refs/heads/feat\n").unwrap();
        // The "conflict resolved, staged, awaiting --continue" content that
        // an unguarded carry would stash away.
        std::fs::write(feat_wt.join("f.txt"), "resolved\n").unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let new_wt = tmp.path().join("new-wt");
        let params = CheckoutBranchParams {
            new_branch_name: "feat-next".to_string(),
            base_branch_name: Some("feat".to_string()),
            carry: true,
            no_carry: false,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            checkout_branch_carry: false,
            checkout_push: false,
            no_verify: false,
            push_verify: PushVerify::Auto,
            checkout_fetch: false,
            layout: None,
            at_path: Some(new_wt.clone()),
        };
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        execute(&params, &git_cmd, tmp.path(), None, &mut sink)
            .expect("checkout-branch succeeds with carry skipped");

        assert!(new_wt.exists(), "the new worktree is still created");
        assert_eq!(
            std::fs::read_to_string(feat_wt.join("f.txt")).unwrap(),
            "resolved\n",
            "the paused worktree's resolved content must not be stashed away"
        );
        let stashes = crate::utils::git_command_at(&feat_wt)
            .args(["stash", "list"])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&stashes.stdout).trim(),
            "",
            "no stash was created"
        );
        assert!(
            private.join("rebase-merge").is_dir(),
            "the rebase state is untouched"
        );
    }
}
