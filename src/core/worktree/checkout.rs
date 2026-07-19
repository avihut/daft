//! Core logic for the `git-worktree-checkout` command.
//!
//! Creates a worktree for an existing branch.

use crate::core::layout::{Layout, auto_gitignore_if_needed};
use crate::core::stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};
use crate::core::{HookOutcome, HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType};
use crate::multi_remote::path::{
    build_template_context, calculate_worktree_path, resolve_remote_for_branch,
};
use crate::utils::*;
use anyhow::Result;
use std::fmt;
use std::path::{Path, PathBuf};

/// Errors specific to the checkout operation.
#[derive(Debug)]
pub enum CheckoutError {
    /// The requested branch was not found locally or on the remote.
    BranchNotFound {
        branch: String,
        remote: String,
        fetch_failed: bool,
    },
    /// Any other error during checkout.
    Other(anyhow::Error),
}

impl fmt::Display for CheckoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BranchNotFound { branch, remote, .. } => {
                write!(
                    f,
                    "Branch '{branch}' does not exist locally or on remote '{remote}'"
                )
            }
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CheckoutError {}

impl From<anyhow::Error> for CheckoutError {
    fn from(err: anyhow::Error) -> Self {
        Self::Other(err)
    }
}

/// Input parameters for the checkout operation.
pub struct CheckoutParams {
    /// Name of the branch to check out.
    pub branch_name: String,
    /// Apply uncommitted changes from the current worktree.
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
    pub checkout_carry: bool,
    /// Whether to set upstream tracking (from settings).
    pub checkout_upstream: bool,
    /// Whether to fetch from remote before creating the worktree.
    pub checkout_fetch: bool,
    /// Optional layout for computing the worktree path.
    /// When `Some`, uses `layout.worktree_path()` instead of `calculate_worktree_path()`.
    pub layout: Option<Layout>,
    /// Explicit path override for worktree placement (`--at` flag).
    /// When `Some`, takes priority over both `layout` and the default path computation.
    pub at_path: Option<PathBuf>,
    /// The caller morphs a missing branch into branch creation (`daft go
    /// --start` / `daft.go.autoStart`), and the morph must leave no rail
    /// behind — so with the fetch on, the fetch runs under the planning
    /// face and the plan commits only once the branch is known to exist,
    /// leading with the already-done fetch row. Without this, the morph
    /// rendered go's `Failed` receipt and then start's rail — two rails on
    /// an exit-0 invocation.
    pub defer_plan_until_branch_known: bool,
    /// Set when the checkout target was a forge PR/MR reference (`pr:123`,
    /// `mr:45`, or a PR/MR URL). The command layer resolved it via `gh`/`glab`
    /// and rewrote `branch_name` to the source branch; this carries the extra
    /// facts core needs (the fork fetch + tracking config, rail annotations).
    /// The command layer also forces `checkout_fetch = true` for forge targets.
    pub forge: Option<ForgeCheckout>,
}

/// A resolved forge PR/MR checkout threaded from the command layer into
/// [`execute`]. `branch_name` is already the PR/MR's source branch; this
/// carries what core needs beyond an ordinary checkout.
#[derive(Debug, Clone)]
pub struct ForgeCheckout {
    /// Base remote the PR/MR ref lives on and is fetched from.
    pub remote: String,
    /// Fork (cross-repo) PR/MR only: the fetch + tracking-config details.
    /// `None` for a same-repo PR/MR, whose source branch is a real branch on
    /// the base repo — reached through the normal remote-branch path with the
    /// fetch forced on.
    pub fork: Option<ForgeForkRefs>,
    /// Same-repo PR/MR only: the head-ref details consulted when the source
    /// branch turns out to be gone from the base repo (deleted after a merge
    /// or close). The head ref still holds the commits, so the checkout falls
    /// back to the fork mechanics — fetch it into its tracking ref, create
    /// the branch from there — instead of failing. `None` for forks, whose
    /// `fork` refs are the primary mechanism.
    pub head_fallback: Option<ForgeForkRefs>,
    /// Compact identity for rail rows and the result line: `PR #123` / `MR !45`.
    pub display: String,
    /// PR/MR title, shown as the resolve row's annotation.
    pub title: String,
    /// Advisory for a non-open PR/MR (closed/merged), surfaced via
    /// `sink.on_warning` right after the plan commits.
    pub state_note: Option<String>,
    /// `gh`/`glab` resolution wall time, for the pre-completed `ResolveRef` row.
    pub resolve_elapsed: std::time::Duration,
}

/// Fork-specific refs for a cross-repo PR/MR checkout.
#[derive(Debug, Clone)]
pub struct ForgeForkRefs {
    /// Stable head ref on the base repo, written to `branch.<name>.merge` so
    /// `git pull` updates from the PR/MR head (`refs/pull/123/head` /
    /// `refs/merge-requests/45/head`).
    pub head_ref: String,
    /// Local remote-tracking ref the head is fetched into and the new branch
    /// is created from (`refs/remotes/<remote>/pr/123`). Fetching into a
    /// tracking ref — not a local branch — keeps `git worktree add -b` the
    /// sole branch creator, so a pre-create-hook or worktree-add failure can't
    /// orphan a half-made branch.
    pub local_ref: String,
}

/// Outcome of a fetch that ran before the plan committed (the morph-armed
/// `defer_plan_until_branch_known` path).
struct Prefetch {
    elapsed: std::time::Duration,
    failed: bool,
}

/// Result of a checkout operation.
pub struct CheckoutResult {
    pub branch_name: String,
    pub worktree_path: PathBuf,
    /// True if an existing worktree was found and we just switched to it.
    pub already_existed: bool,
    /// Directory to cd into after the operation.
    pub cd_target: PathBuf,
    pub stash_applied: bool,
    pub stash_conflict: bool,
    pub upstream_set: bool,
    pub upstream_skipped: bool,
    pub git_dir: PathBuf,
    pub post_hook_outcome: HookOutcome,
}

/// Execute the checkout operation.
pub fn execute(
    params: &CheckoutParams,
    git: &GitCommand,
    project_root: &Path,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<CheckoutResult, CheckoutError> {
    validate_branch_name(&params.branch_name)?;

    let git_dir = crate::core::repo::get_git_common_dir()?;
    // The target branch's worktree doesn't exist yet here, so there is no
    // `preferred_branch` to bias toward — fall back to the default branch's
    // worktree when cwd isn't a worktree (see `resolve_source_worktree`).
    let source_worktree = crate::core::worktree::checkout_branch::resolve_source_worktree(
        git,
        &git_dir,
        &params.remote_name,
        None,
    )?;

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
        let ctx = build_template_context(&effective_root, &params.branch_name);
        layout.worktree_path(&ctx)?
    } else {
        let remote_for_path = resolve_remote_for_branch(
            git,
            &params.branch_name,
            params.remote.as_deref(),
            &params.multi_remote_default,
        )?;
        calculate_worktree_path(
            project_root,
            &params.branch_name,
            &remote_for_path,
            params.multi_remote_enabled,
        )
    };

    sink.on_step(&format!(
        "Path: {}, Branch: {}, Project Root: {}",
        worktree_path.display(),
        params.branch_name,
        project_root.display()
    ));

    // Check if worktree already exists for this branch
    if let Some(existing_path) = find_existing_worktree_for_branch(git, &params.branch_name)? {
        sink.on_step(&format!(
            "Branch '{}' already has a worktree at '{}'",
            params.branch_name,
            existing_path.display()
        ));
        sink.on_step("Changing to existing worktree...");
        // Observed attached to this branch — refresh the record opportunistically,
        // which is also how worktrees daft did not create acquire one.
        if let Some(store) = crate::core::worktree::identity_store::IdentityStore::open(&git_dir) {
            store.record(&existing_path, &params.branch_name);
        }
        change_directory(&existing_path)?;

        return Ok(CheckoutResult {
            branch_name: params.branch_name.clone(),
            worktree_path: existing_path,
            already_existed: true,
            cd_target: get_current_directory()?,
            stash_applied: false,
            stash_conflict: false,
            upstream_set: false,
            upstream_skipped: true,
            git_dir,
            post_hook_outcome: HookOutcome {
                success: true,
                skipped: true,
                skip_reason: None,
            },
        });
    }

    // Fallback: check if the worktree directory already exists on disk.
    // This handles cases where the branch association is missing from
    // `git worktree list` (e.g., detached HEAD from an interrupted rebase).
    if worktree_path.exists() && worktree_path.join(".git").is_file() {
        sink.on_step(&format!(
            "Worktree directory '{}' already exists, switching to it",
            worktree_path.display()
        ));
        sink.on_warning(
            "Worktree may be in detached HEAD state (e.g., from an interrupted rebase). \
             Run 'git status' to check, and 'git rebase --abort' or 'git checkout <branch>' to recover.",
        );
        change_directory(&worktree_path)?;

        return Ok(CheckoutResult {
            branch_name: params.branch_name.clone(),
            worktree_path,
            already_existed: true,
            cd_target: get_current_directory()?,
            stash_applied: false,
            stash_conflict: false,
            upstream_set: false,
            upstream_skipped: true,
            git_dir,
            post_hook_outcome: HookOutcome {
                success: true,
                skipped: true,
                skip_reason: None,
            },
        });
    }

    // Forge PR/MR facts (resolved by the command layer). `fork` is Some only
    // for a cross-repo PR/MR, whose head lives at a base-repo ref rather than a
    // normal remote branch. It is rebound when a same-repo checkout falls back
    // to the head ref because the source branch is gone (deleted after merge).
    let forge = params.forge.as_ref();
    let mut fork = forge.and_then(|f| f.fork.as_ref());
    // Every remote-branch fact (fetch, existence probe, creation ref,
    // upstream) follows the PR/MR's base remote for a forge checkout — the
    // resolved PR names it, and it may differ from the settings default
    // (`daft.remote`, usually `origin`).
    let source_remote: &str = forge.map_or(&params.remote_name, |f| f.remote.as_str());

    // The CheckOut row's provenance annotation, from the existence probe.
    let annotation_for = |local_exists: bool, remote_exists: bool| {
        // A fork PR/MR branch is created from the PR head ref, not a normal
        // remote branch — `← origin/<branch>` would name a ref that doesn't
        // exist, so show the PR/MR identity instead.
        if let Some(fc) = forge
            && fc.fork.is_some()
            && !local_exists
        {
            return format!("\u{2190} {}", fc.display);
        }
        if !local_exists {
            format!("\u{2190} {}/{}", source_remote, params.branch_name)
        } else if remote_exists && params.checkout_upstream {
            format!("tracking {}/{}", source_remote, params.branch_name)
        } else {
            "local only".to_string()
        }
    };

    // Without a fetch every branch fact is local: probe now, so an unknown
    // branch errors before any plan commits (no rail renders for resolve
    // errors). With `daft.checkout.fetch` on the probe must follow the
    // network — it moves below the plan commit, and the fetch becomes
    // planned work instead of hiding behind the planning face. The
    // exception is a morph-armed caller (`defer_plan_until_branch_known`):
    // the fetch runs under the face — its stage events land on no row —
    // and joins the committed plan as a pre-completed row, so a missing
    // branch still errors before any plan commits and the morph's own rail
    // is the only rail.
    let mut prefetch: Option<Prefetch> = None;
    let pre_plan_probe = if fork.is_some() {
        // Fork PR/MR: the source branch isn't a normal ref on the base repo —
        // it is materialized below from the PR head ref. A pre-existing local
        // branch means re-checkout (the command layer's preflight verified it
        // tracks this ref); otherwise the branch is created from the fetched
        // ref. Either way the ref is known to exist (resolve succeeded), so
        // there is no "branch not found" outcome, and the standard
        // remote-branch probe (which looks for refs/remotes/<remote>/<branch>)
        // doesn't apply.
        let local_exists = git
            .show_ref_exists(&format!("refs/heads/{}", params.branch_name))
            .map_err(CheckoutError::Other)?;
        Some((local_exists, false))
    } else if params.checkout_fetch {
        if params.defer_plan_until_branch_known {
            let fetch_started = std::time::Instant::now();
            let failed = !fetch_branch(git, &params.remote_name, &params.branch_name, sink);
            prefetch = Some(Prefetch {
                elapsed: fetch_started.elapsed(),
                failed,
            });
            let (local_exists, remote_exists) =
                check_branch_existence(git, &params.branch_name, &params.remote_name)?;
            if !local_exists && !remote_exists {
                return Err(CheckoutError::BranchNotFound {
                    branch: params.branch_name.clone(),
                    remote: params.remote_name.clone(),
                    fetch_failed: failed,
                });
            }
            Some((local_exists, remote_exists))
        } else {
            None
        }
    } else {
        let (local_exists, remote_exists) =
            check_branch_existence(git, &params.branch_name, &params.remote_name)?;
        if !local_exists && !remote_exists {
            return Err(CheckoutError::BranchNotFound {
                branch: params.branch_name.clone(),
                remote: params.remote_name.clone(),
                fetch_failed: false,
            });
        }
        Some((local_exists, remote_exists))
    };

    // Commit the execution plan (#651): the worktree path is resolved and
    // everything left is planned work. Earlier returns (existing worktree,
    // fetch-off branch not found) never reach this point, so no rail
    // renders for them.
    let should_carry = params.carry || (!params.no_carry && params.checkout_carry);
    let mut plan_rows = Vec::new();
    if let Some(fc) = forge {
        // Resolution already happened, under the planning face (it determines
        // the plan). This pre-completed row is its receipt, leading the plan
        // like clone's bare phase — its label is the PR/MR identity, its
        // annotation the title.
        plan_rows.push(Row::Step(
            StepSpec::new(StepKey::new(StageId::ResolveRef))
                .with_label(fc.display.clone())
                .with_annotation(fc.title.clone())
                .pre_completed(fc.resolve_elapsed),
        ));
    }
    if params.checkout_fetch {
        let mut fetch_spec =
            StepSpec::new(StepKey::new(StageId::Fetch)).with_annotation(source_remote.to_string());
        // A deferred fetch that succeeded leads the plan as a receipt row,
        // like clone's bare phase; a failed one keeps the normal row and
        // resolves yellow right after the commit (below).
        if let Some(pf) = &prefetch
            && !pf.failed
        {
            fetch_spec = fetch_spec.pre_completed(pf.elapsed);
        }
        plan_rows.push(Row::Step(fetch_spec));
    }
    let checkout_spec = match pre_plan_probe {
        // Probe already ran (fetch off, or a deferred fetch): the
        // provenance is resolved.
        Some((local, remote)) => StepSpec::new(StepKey::new(StageId::CheckOut))
            .with_annotation(annotation_for(local, remote)),
        // Fetch on: resolved post-fetch, noted onto the pending row.
        None => StepSpec::new(StepKey::new(StageId::CheckOut)),
    };
    plan_rows.extend([
        Row::Step(StepSpec::new(StepKey::new(StageId::PreCreateHooks))),
        Row::Step(checkout_spec),
        Row::Step(
            StepSpec::new(StepKey::new(StageId::CreateWorktree))
                .with_annotation(super::branch_delete::display_path(&worktree_path)),
        ),
    ]);
    if should_carry {
        plan_rows.push(Row::Step(StepSpec::new(StepKey::new(StageId::Carry))));
    }
    // Shared files declared in the source worktree's config get a section
    // (see checkout_branch.rs for the probe-vs-execution contract).
    let planned_shared =
        crate::core::shared::read_shared_paths(&source_worktree).unwrap_or_default();
    crate::core::shared::push_shared_section(&mut plan_rows, &planned_shared);
    plan_rows.push(Row::Step(StepSpec::new(StepKey::new(
        StageId::PostCreateHooks,
    ))));
    sink.on_plan(PlanCommit::new(plan_rows));

    // A closed/merged PR/MR is still worth inspecting — note it, then proceed.
    if let Some(note) = forge.and_then(|f| f.state_note.as_deref()) {
        sink.on_warning(note);
    }

    // Fork PR/MR: materialize the head ref into its local tracking ref now.
    // Unlike the best-effort branch fetch this is required — the source lives
    // nowhere else — so a failure aborts the rail rather than continuing on
    // local refs. (Runs before the post-fetch probe below reads `local_exists`;
    // it targets refs/remotes/<remote>/pr/N, so the refs/heads/<branch> probe
    // result is unchanged by it.)
    if let (Some(fc), Some(fk)) = (forge, fork)
        && !fetch_forge_fork_ref(git, &fc.remote, fk, sink)
    {
        return Err(CheckoutError::Other(anyhow::anyhow!(
            "Could not fetch {}'s head ({}) from '{}'",
            fc.display,
            fk.head_ref,
            fc.remote
        )));
    }

    // Fetch (planned above; a failure warns, turns the row yellow, and the
    // checkout continues on local refs), then the post-fetch probe.
    let local_exists = match pre_plan_probe {
        Some((local_exists, _)) => {
            // A deferred fetch that failed planned the normal row — resolve
            // it yellow now, exactly like the planned-fetch path below.
            if prefetch.as_ref().is_some_and(|pf| pf.failed) {
                sink.on_stage(
                    &StepKey::new(StageId::Fetch),
                    StageEvent::SkippedAttention {
                        reason: super::FETCH_FAILED_REASON.to_string(),
                    },
                );
            }
            local_exists
        }
        None => {
            let fetch_failed = !fetch_branch(git, source_remote, &params.branch_name, sink);
            let (local_exists, remote_exists) =
                check_branch_existence(git, &params.branch_name, source_remote)?;
            if !local_exists && !remote_exists {
                // The plan is committed: the command layer aborts the rail
                // into a Failed receipt, and errors print below it.
                let Some(fc) = forge else {
                    return Err(CheckoutError::BranchNotFound {
                        branch: params.branch_name.clone(),
                        remote: params.remote_name.clone(),
                        fetch_failed,
                    });
                };
                // Same-repo PR/MR whose source branch is gone from the base
                // repo — deleted after a merge/close, or vanished between
                // resolution and fetch. The head ref still holds the commits,
                // so fall back to the fork mechanics: fetch it into its
                // tracking ref and create the branch from there. Errors are
                // Other, not BranchNotFound, so the command layer's
                // auto-start / catalog morph can't reinterpret the
                // (rewritten) source-branch name.
                let Some(fb) = fc.head_fallback.as_ref() else {
                    return Err(CheckoutError::Other(anyhow::anyhow!(
                        "{}'s source branch '{}' was not found on '{}' after fetching \
                         (it may have been deleted since)",
                        fc.display,
                        params.branch_name,
                        source_remote,
                    )));
                };
                sink.on_step(&format!(
                    "Source branch '{}' not found on '{}'; falling back to {} ({})",
                    params.branch_name, source_remote, fc.display, fb.head_ref
                ));
                // `+` force-updates the tracking ref; the Fetch row already
                // resolved above, so this fetch reports through the CheckOut
                // row's provenance note instead.
                let refspec = format!("+{}:{}", fb.head_ref, fb.local_ref);
                if let Err(e) = git.fetch_refspec(&fc.remote, &refspec) {
                    return Err(CheckoutError::Other(anyhow::anyhow!(
                        "{}'s source branch '{}' is gone from '{}' (deleted after \
                         merge/close), and its head ref {} could not be fetched: {e}",
                        fc.display,
                        params.branch_name,
                        source_remote,
                        fb.head_ref,
                    )));
                }
                sink.on_stage(
                    &StepKey::new(StageId::CheckOut),
                    StageEvent::Note(format!("\u{2190} {}", fc.display)),
                );
                fork = Some(fb);
                false
            } else {
                sink.on_stage(
                    &StepKey::new(StageId::CheckOut),
                    StageEvent::Note(annotation_for(local_exists, remote_exists)),
                );
                local_exists
            }
        }
    };

    let use_local_branch = if local_exists {
        sink.on_step(&format!(
            "Local branch '{}' found, using it for worktree creation",
            params.branch_name
        ));
        true
    } else {
        sink.on_step(&format!(
            "Local branch '{}' not found, will create from remote '{}/{}'",
            params.branch_name, source_remote, params.branch_name
        ));
        false
    };

    // Stash uncommitted changes if carry is enabled
    let stash_created = stash_if_carry(params, git, sink)?;

    // Run pre-create hook
    let hook_ctx = HookContext::new(
        HookType::PreCreate,
        "checkout",
        project_root,
        &git_dir,
        &params.remote_name,
        &source_worktree,
        &worktree_path,
        &params.branch_name,
    )
    .with_new_branch(false);

    let hook_outcome = sink.run_hook(&hook_ctx)?;
    if !hook_outcome.success && !hook_outcome.skipped {
        return Err(anyhow::anyhow!("Pre-create hook failed").into());
    }

    // Create worktree. `git worktree add` materializes the branch checkout
    // and the worktree in one call; the two plan rows resolve around it as a
    // cosmetic split of the same operation.
    sink.on_stage(&StepKey::new(StageId::CheckOut), StageEvent::Started);
    let worktree_result = if use_local_branch {
        git.worktree_add(&worktree_path, &params.branch_name)
    } else if let Some(fk) = fork {
        // Fork: create the branch from the fetched PR/MR head ref, with
        // no-track — this `git worktree add -b` is the sole branch creator (so
        // git owns its cleanup), and the branch tracks the PR head ref via the
        // config written in `set_forge_fork_tracking` below, not a normal
        // remote branch.
        git.worktree_add_new_branch(&worktree_path, &params.branch_name, &fk.local_ref, true)
    } else {
        let remote_ref = format!("{}/{}", source_remote, params.branch_name);
        git.worktree_add_new_branch(&worktree_path, &params.branch_name, &remote_ref, false)
    };

    if let Err(e) = worktree_result {
        sink.on_stage(
            &StepKey::new(StageId::CheckOut),
            StageEvent::Failed {
                detail: "failed (see below)".to_string(),
            },
        );
        restore_stash_on_failure(stash_created, git, sink);
        return Err(anyhow::anyhow!("Failed to create git worktree: {}", e).into());
    }
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
        return Err(anyhow::anyhow!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        )
        .into());
    }
    // Remember what this worktree is for, while we still know. Git records
    // nothing that survives a later detached checkout, so this is the only
    // moment the association is available for free. Best-effort.
    if let Some(store) = crate::core::worktree::identity_store::IdentityStore::open(&git_dir) {
        store.record(&worktree_path, &params.branch_name);
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
        "Worktree created at '{}' checking out branch '{}'",
        worktree_path.display(),
        params.branch_name
    ));

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

    // Set tracking: a fork PR/MR branch pulls from the PR head ref (branch
    // config written directly, since it has no refs/remotes/<remote>/<branch>
    // upstream); everything else takes the normal upstream path.
    let (upstream_set, upstream_skipped) = match fork {
        Some(fk) => set_forge_fork_tracking(
            git,
            &params.branch_name,
            &forge.expect("fork implies forge").remote,
            &fk.head_ref,
            sink,
        ),
        None => set_upstream_if_enabled(params, source_remote, git, sink)?,
    };

    // Propagate in-scope untracked daft files from source worktree to the new
    // worktree, so that user post-create hooks can read them.
    // Propagation entry point: this site creates a new worktree from an
    // existing source worktree. See checkout_branch.rs for the canonical audit
    // comment covering all worktree-creating entry points.
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
                    &params.branch_name,
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
        &params.branch_name,
    )
    .with_new_branch(false);

    let post_hook_outcome = sink.run_hook(&post_hook_ctx)?;

    Ok(CheckoutResult {
        branch_name: params.branch_name.clone(),
        worktree_path,
        already_existed: false,
        cd_target: get_current_directory()?,
        stash_applied,
        stash_conflict,
        upstream_set,
        upstream_skipped,
        git_dir,
        post_hook_outcome,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Check if a worktree already exists for the given branch name.
fn find_existing_worktree_for_branch(
    git: &GitCommand,
    branch_name: &str,
) -> Result<Option<PathBuf>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    Ok(
        crate::core::worktree::porcelain::parse_worktree_list_porcelain(&porcelain_output)
            .into_iter()
            .find(|e| e.branch.as_deref() == Some(branch_name))
            .map(|e| e.path),
    )
}

/// Fetch latest changes for a branch from the remote.
///
/// Returns `true` if at least the general fetch succeeded, `false` if both
/// fetches failed.
fn fetch_branch(
    git: &GitCommand,
    remote_name: &str,
    branch_name: &str,
    sink: &mut impl ProgressSink,
) -> bool {
    // The fetch is a planned rail row (mirrors checkout_branch's
    // `fetch_remote`): it resolves green on success and yellow — with the
    // continuing-anyway fact — when both fetches failed.
    sink.on_stage(&StepKey::new(StageId::Fetch), StageEvent::Started);
    sink.on_step(&format!(
        "Fetching latest changes from remote '{remote_name}'..."
    ));
    let general_ok = match git.fetch(remote_name, false) {
        Ok(()) => true,
        Err(e) => {
            sink.on_warning(&format!("Failed to fetch from remote '{remote_name}': {e}"));
            false
        }
    };

    sink.on_step(&format!(
        "Fetching specific branch '{branch_name}' from remote '{remote_name}'..."
    ));
    let specific_ok = match git.fetch_refspec(remote_name, &format!("{branch_name}:{branch_name}"))
    {
        Ok(()) => true,
        Err(e) => {
            sink.on_warning(&format!("Failed to fetch specific branch: {e}"));
            false
        }
    };

    if general_ok || specific_ok {
        sink.on_stage(
            &StepKey::new(StageId::Fetch),
            StageEvent::Completed { annotation: None },
        );
        true
    } else {
        sink.on_stage(
            &StepKey::new(StageId::Fetch),
            StageEvent::SkippedAttention {
                reason: super::FETCH_FAILED_REASON.to_string(),
            },
        );
        false
    }
}

/// Fetch a fork PR/MR's head ref into its local remote-tracking ref
/// (`refs/remotes/<remote>/pr/<n>`), the start point for the new branch.
///
/// Unlike [`fetch_branch`], this is **required** — the source branch lives on
/// the fork and is reachable only through the base repo's PR head ref — so the
/// Fetch row resolves red (not yellow) on failure and the caller aborts.
/// Returns `true` on success.
fn fetch_forge_fork_ref(
    git: &GitCommand,
    remote: &str,
    fork: &ForgeForkRefs,
    sink: &mut impl ProgressSink,
) -> bool {
    sink.on_stage(&StepKey::new(StageId::Fetch), StageEvent::Started);
    sink.on_step(&format!(
        "Fetching {} from remote '{remote}'...",
        fork.head_ref
    ));
    // `+` force-updates the tracking ref; `--` isn't needed (the refspec never
    // starts with `-`). fetch_refspec scrubs GIT_* so the fetch targets this
    // repo even inside a post-checkout hook.
    let refspec = format!("+{}:{}", fork.head_ref, fork.local_ref);
    match git.fetch_refspec(remote, &refspec) {
        Ok(()) => {
            sink.on_stage(
                &StepKey::new(StageId::Fetch),
                StageEvent::Completed { annotation: None },
            );
            true
        }
        Err(e) => {
            sink.on_stage(
                &StepKey::new(StageId::Fetch),
                StageEvent::Failed {
                    detail: "failed (see below)".to_string(),
                },
            );
            sink.on_warning(&format!("Failed to fetch {}: {e}", fork.head_ref));
            false
        }
    }
}

/// Check whether local and remote branch refs exist.
fn check_branch_existence(
    git: &GitCommand,
    branch_name: &str,
    remote_name: &str,
) -> Result<(bool, bool)> {
    let local_ref = format!("refs/heads/{branch_name}");
    let remote_ref = format!("refs/remotes/{remote_name}/{branch_name}");
    let local_exists = git.show_ref_exists(&local_ref)?;
    let remote_exists = git.show_ref_exists(&remote_ref)?;
    Ok((local_exists, remote_exists))
}

/// Stash uncommitted changes if carry behavior is enabled.
fn stash_if_carry(
    params: &CheckoutParams,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> Result<bool> {
    let should_carry = if params.carry {
        true
    } else if params.no_carry {
        false
    } else {
        params.checkout_carry
    };

    let in_worktree = git.rev_parse_is_inside_work_tree().unwrap_or(false);

    if should_carry && in_worktree {
        match git.has_uncommitted_changes() {
            Ok(true) => {
                sink.on_step("Stashing uncommitted changes...");
                if let Err(e) = git.stash_push_with_untracked("daft: carry changes to worktree") {
                    anyhow::bail!("Failed to stash uncommitted changes: {e}");
                }
                Ok(true)
            }
            Ok(false) => {
                sink.on_step("No uncommitted changes to carry");
                Ok(false)
            }
            Err(e) => {
                sink.on_warning(&format!("Could not check for uncommitted changes: {e}"));
                Ok(false)
            }
        }
    } else {
        Ok(false)
    }
}

/// Restore stashed changes when worktree creation fails.
fn restore_stash_on_failure(stash_created: bool, git: &GitCommand, sink: &mut impl ProgressSink) {
    if stash_created {
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
        sink.on_step("Applying stashed changes to worktree...");
        if let Err(e) = git.stash_pop() {
            sink.on_warning(&format!(
                "Stash could not be applied cleanly. Resolve conflicts and run 'git stash pop'. Error: {e}"
            ));
            (false, true)
        } else {
            sink.on_step("Changes successfully applied to worktree");
            (true, false)
        }
    } else {
        (false, false)
    }
}

/// Set upstream tracking if the setting is enabled.
fn set_upstream_if_enabled(
    params: &CheckoutParams,
    source_remote: &str,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> Result<(bool, bool)> {
    if !params.checkout_upstream {
        sink.on_step("Skipping upstream setup (disabled in config)");
        return Ok((false, true));
    }

    let remote_branch_ref = format!("refs/remotes/{}/{}", source_remote, params.branch_name);
    sink.on_step(&format!(
        "Checking for remote branch '{}/{}'...",
        source_remote, params.branch_name
    ));

    if !git.show_ref_exists(&remote_branch_ref)? {
        sink.on_step(&format!(
            "Remote branch '{}/{}' not found, skipping upstream setup",
            source_remote, params.branch_name
        ));
        return Ok((false, true));
    }

    sink.on_step(&format!(
        "Setting upstream to '{}/{}'...",
        source_remote, params.branch_name
    ));

    if let Err(e) = git.set_upstream(source_remote, &params.branch_name) {
        sink.on_warning(&format!(
            "Failed to set upstream tracking: {}. Worktree created, but upstream may need manual configuration.",
            e
        ));
        Ok((false, false))
    } else {
        sink.on_step(&format!(
            "Upstream tracking set to '{}/{}'",
            source_remote, params.branch_name
        ));
        Ok((true, false))
    }
}

/// Configure a fork PR/MR branch to pull from the PR/MR head ref.
///
/// The standard [`set_upstream_if_enabled`] can't express this: it needs a
/// `refs/remotes/<remote>/<branch>` upstream, which a fork branch doesn't have.
/// This writes `branch.<name>.remote` + `branch.<name>.merge` directly (via
/// [`GitCommand::set_branch_tracking`]) so `git pull` on the branch updates
/// from the PR/MR head. Always applied for a fork checkout regardless of
/// `daft.checkout.upstream` — the merge ref is the branch's only source, not an
/// optional convenience. Returns `(upstream_set, upstream_skipped)` to match
/// `set_upstream_if_enabled`.
fn set_forge_fork_tracking(
    git: &GitCommand,
    branch: &str,
    remote: &str,
    merge_ref: &str,
    sink: &mut impl ProgressSink,
) -> (bool, bool) {
    match git.set_branch_tracking(branch, remote, merge_ref) {
        Ok(()) => {
            sink.on_step(&format!("Configured '{branch}' to pull from {merge_ref}"));
            (true, false)
        }
        Err(e) => {
            sink.on_warning(&format!(
                "Failed to configure PR/MR tracking for '{branch}': {e}. \
                 Worktree created, but `git pull` may need manual setup."
            ));
            (false, false)
        }
    }
}

/// Collect all local and remote branch names for suggestion purposes.
pub fn collect_branch_names(git: &GitCommand, remote_name: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut names = Vec::new();

    // Local branches
    if let Ok(output) = git.for_each_ref("%(refname:short)", "refs/heads/") {
        for line in output.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_string()) {
                names.push(trimmed.to_string());
            }
        }
    }

    // Remote branches (strip remote prefix)
    let remote_refs = format!("refs/remotes/{remote_name}/");
    if let Ok(output) = git.for_each_ref("%(refname:short)", &remote_refs) {
        for line in output.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.ends_with("/HEAD") {
                continue;
            }
            // Strip the remote prefix to get just the branch name
            if let Some(branch) = trimmed.strip_prefix(&format!("{remote_name}/"))
                && seen.insert(branch.to_string())
            {
                names.push(branch.to_string());
            }
        }
    }

    names
}

#[cfg(test)]
mod timeline_tests {
    use super::*;
    use crate::core::RecordingStageSink;
    use crate::core::stage::{StageEvent, StageId, StepKey};
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

    fn params(branch: &str, at: PathBuf, fetch: bool) -> CheckoutParams {
        CheckoutParams {
            branch_name: branch.to_string(),
            carry: false,
            no_carry: true,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            checkout_carry: false,
            checkout_upstream: true,
            checkout_fetch: fetch,
            layout: None,
            at_path: Some(at),
            defer_plan_until_branch_known: false,
            forge: None,
        }
    }

    /// Fetch off: every branch fact is local, so the probe precedes the
    /// plan — no Fetch row, and the CheckOut row carries its resolved
    /// provenance from the moment the plan commits (#651).
    #[test]
    #[serial]
    fn fetch_off_plans_no_fetch_row_and_resolves_the_annotation_up_front() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        git(tmp.path(), &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(tmp.path(), &["branch", "feat-x"]);
        std::env::set_current_dir(tmp.path()).unwrap();

        let worktree_path = tmp.path().join("feat-x-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let result = execute(
            &params("feat-x", worktree_path.clone(), false),
            &git_cmd,
            tmp.path(),
            &mut sink,
        )
        .expect("checkout succeeds");
        assert!(!result.already_existed);
        assert!(worktree_path.exists());

        let plan = sink.plan.as_ref().expect("plan committed");
        let ids: Vec<StageId> = plan.steps().map(|s| s.key.id).collect();
        assert_eq!(
            ids,
            vec![
                StageId::PreCreateHooks,
                StageId::CheckOut,
                StageId::CreateWorktree,
                StageId::PostCreateHooks,
            ],
            "fetch off => no Fetch row"
        );
        let checkout_annotation = plan
            .steps()
            .find(|s| s.key.id == StageId::CheckOut)
            .and_then(|s| s.annotation.as_deref());
        assert_eq!(
            checkout_annotation,
            Some("local only"),
            "no remote ref => local-only provenance, resolved at plan time"
        );
        assert!(
            sink.events.iter().all(|(k, _)| k.id != StageId::Fetch),
            "no Fetch events without a planned row: {:?}",
            sink.events
        );
    }

    /// Fetch off + unknown branch: the resolve-phase error fires before any
    /// plan commits, so no rail ever renders for it.
    #[test]
    #[serial]
    fn fetch_off_unknown_branch_errors_before_any_plan() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        git(tmp.path(), &["commit", "--allow-empty", "-q", "-m", "init"]);
        std::env::set_current_dir(tmp.path()).unwrap();

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let Err(err) = execute(
            &params("no-such-branch", tmp.path().join("wt"), false),
            &git_cmd,
            tmp.path(),
            &mut sink,
        ) else {
            panic!("unknown branch must fail");
        };
        assert!(matches!(err, CheckoutError::BranchNotFound { .. }));
        assert!(sink.plan.is_none(), "no plan for a resolve-phase error");
    }

    /// Fetch on: the plan commits before the network — a Fetch row leads it,
    /// the CheckOut row starts without provenance, and the post-fetch probe
    /// notes the resolved annotation onto the pending row (#651).
    #[test]
    #[serial]
    fn fetch_on_plans_the_fetch_row_and_notes_the_annotation() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(&origin, &["branch", "feat-y"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("feat-y-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        execute(
            &params("feat-y", worktree_path.clone(), true),
            &git_cmd,
            &work,
            &mut sink,
        )
        .expect("checkout succeeds");
        assert!(worktree_path.exists());

        let plan = sink.plan.as_ref().expect("plan committed");
        let ids: Vec<StageId> = plan.steps().map(|s| s.key.id).collect();
        assert_eq!(
            ids,
            vec![
                StageId::Fetch,
                StageId::PreCreateHooks,
                StageId::CheckOut,
                StageId::CreateWorktree,
                StageId::PostCreateHooks,
            ],
            "fetch on => the Fetch row leads the plan"
        );
        let fetch_annotation = plan
            .steps()
            .find(|s| s.key.id == StageId::Fetch)
            .and_then(|s| s.annotation.as_deref());
        assert_eq!(fetch_annotation, Some("origin"));
        let checkout_annotation = plan
            .steps()
            .find(|s| s.key.id == StageId::CheckOut)
            .and_then(|s| s.annotation.as_deref());
        assert_eq!(
            checkout_annotation, None,
            "provenance is unknown until the fetch lands"
        );

        // The fetch row resolves, then the probe notes the provenance onto
        // the pending CheckOut row.
        let fetch_key = StepKey::new(StageId::Fetch);
        assert!(
            sink.events
                .contains(&(fetch_key.clone(), StageEvent::Started))
        );
        assert!(
            sink.events
                .contains(&(fetch_key, StageEvent::Completed { annotation: None }))
        );
        // The specific-branch fetch materialized the local ref, and the
        // remote-tracking ref exists from the clone: tracking provenance.
        assert!(
            sink.events.contains(&(
                StepKey::new(StageId::CheckOut),
                StageEvent::Note("tracking origin/feat-y".to_string())
            )),
            "events: {:?}",
            sink.events
        );
    }

    /// Fetch on + unknown branch: the plan is already committed when the
    /// post-fetch probe fails — the command layer aborts the rail into a
    /// Failed receipt (the accepted #651 semantic for fetch-on go).
    #[test]
    #[serial]
    fn fetch_on_unknown_branch_errors_after_the_committed_plan() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let Err(err) = execute(
            &params("no-such-branch", tmp.path().join("wt"), true),
            &git_cmd,
            &work,
            &mut sink,
        ) else {
            panic!("unknown branch must fail");
        };
        assert!(matches!(err, CheckoutError::BranchNotFound { .. }));
        assert!(
            sink.plan.is_some(),
            "fetch on commits the plan before the probe can fail"
        );
    }

    /// Morph-armed (`go --start` / autoStart) + fetch on + unknown branch:
    /// the fetch runs under the planning face and no plan ever commits —
    /// the face dissolves tracelessly and the morph's own rail is the only
    /// rail (two rails + a Failed receipt on an exit-0 run otherwise).
    #[test]
    #[serial]
    fn deferred_unknown_branch_commits_no_plan() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let mut p = params("no-such-branch", tmp.path().join("wt"), true);
        p.defer_plan_until_branch_known = true;
        let Err(err) = execute(&p, &git_cmd, &work, &mut sink) else {
            panic!("unknown branch must fail");
        };
        assert!(matches!(err, CheckoutError::BranchNotFound { .. }));
        assert!(
            sink.plan.is_none(),
            "a morph-armed miss must leave no plan behind"
        );
    }

    /// Morph-armed + fetch on + branch exists: the plan commits with the
    /// already-done fetch leading it as a pre-completed row, and the
    /// CheckOut row carries its provenance from the moment the plan
    /// commits (no post-fetch `Note` needed).
    #[test]
    #[serial]
    fn deferred_fetch_leads_the_plan_pre_completed() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(&origin, &["branch", "feat-z"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("feat-z-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let mut p = params("feat-z", worktree_path.clone(), true);
        p.defer_plan_until_branch_known = true;
        execute(&p, &git_cmd, &work, &mut sink).expect("checkout succeeds");
        assert!(worktree_path.exists());

        let plan = sink.plan.as_ref().expect("plan committed");
        let fetch_spec = plan
            .steps()
            .find(|s| s.key.id == StageId::Fetch)
            .expect("the fetch row still leads the plan");
        assert!(
            fetch_spec.pre_completed.is_some(),
            "the deferred fetch joins the plan as a receipt row"
        );
        let checkout_annotation = plan
            .steps()
            .find(|s| s.key.id == StageId::CheckOut)
            .and_then(|s| s.annotation.as_deref());
        assert_eq!(
            checkout_annotation,
            Some("tracking origin/feat-z"),
            "provenance is resolved at plan time"
        );
        assert!(
            !sink
                .events
                .iter()
                .any(|(k, e)| k.id == StageId::CheckOut && matches!(e, StageEvent::Note(_))),
            "no post-fetch Note — the plan already carried the provenance: {:?}",
            sink.events
        );
    }

    // ── Forge PR/MR checkout ─────────────────────────────────────────────

    /// Read a single git config/plumbing value from `dir`.
    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Build a `refs/pull/<n>/head` on `origin` that points at a commit which
    /// is not on any branch — the shape of a fork PR (the head is reachable
    /// from the base repo only through the pull ref).
    fn seed_pull_ref(origin: &Path, number: u32) {
        git(origin, &["checkout", "-q", "-b", "tmp-pr-src"]);
        git(
            origin,
            &["commit", "--allow-empty", "-q", "-m", "pr head commit"],
        );
        git(
            origin,
            &["update-ref", &format!("refs/pull/{number}/head"), "HEAD"],
        );
        git(origin, &["checkout", "-q", "main"]);
        git(origin, &["branch", "-qD", "tmp-pr-src"]);
    }

    fn forge_params(branch: &str, at: PathBuf, forge: ForgeCheckout) -> CheckoutParams {
        // The command layer forces the fetch on for every forge target.
        let mut p = params(branch, at, true);
        p.forge = Some(forge);
        p
    }

    /// Same-repo PR/MR: the source branch is a real branch on the base repo,
    /// so it rides the normal remote-branch path (upstream set to
    /// `origin/<branch>`). The forge layer only adds the pre-completed
    /// `ResolveRef` receipt at the head of the plan.
    #[test]
    #[serial]
    fn same_repo_forge_leads_plan_with_resolve_receipt() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(&origin, &["branch", "feat-sr"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("feat-sr-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let forge = ForgeCheckout {
            remote: "origin".to_string(),
            fork: None,
            head_fallback: None,
            display: "PR #7".to_string(),
            title: "Same-repo work".to_string(),
            state_note: None,
            resolve_elapsed: std::time::Duration::from_millis(3),
        };
        execute(
            &forge_params("feat-sr", worktree_path.clone(), forge),
            &git_cmd,
            &work,
            &mut sink,
        )
        .expect("same-repo PR checkout succeeds");
        assert!(worktree_path.exists());

        let plan = sink.plan.as_ref().expect("plan committed");
        let ids: Vec<StageId> = plan.steps().map(|s| s.key.id).collect();
        assert_eq!(
            ids,
            vec![
                StageId::ResolveRef,
                StageId::Fetch,
                StageId::PreCreateHooks,
                StageId::CheckOut,
                StageId::CreateWorktree,
                StageId::PostCreateHooks,
            ],
            "the resolve receipt leads the plan, then the normal fetch-on rows"
        );
        let resolve = plan
            .steps()
            .find(|s| s.key.id == StageId::ResolveRef)
            .expect("resolve row present");
        assert_eq!(resolve.label.as_deref(), Some("PR #7"));
        assert_eq!(resolve.annotation.as_deref(), Some("Same-repo work"));
        assert!(
            resolve.pre_completed.is_some(),
            "resolution already happened — the row is a receipt"
        );

        // Normal upstream: same-repo source branch tracks origin/<branch>.
        assert_eq!(
            git_out(&worktree_path, &["config", "--get", "branch.feat-sr.merge"]),
            "refs/heads/feat-sr"
        );
    }

    /// Fork PR/MR: the head lives at a base-repo pull ref, not a normal remote
    /// branch. It's fetched into `refs/remotes/<remote>/pr/<n>`, the new branch
    /// is created from there, and its tracking config points at the pull ref so
    /// `git pull` updates from the PR.
    #[test]
    #[serial]
    fn fork_forge_creates_branch_from_pull_ref_and_configures_tracking() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        seed_pull_ref(&origin, 1);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("contrib-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let forge = ForgeCheckout {
            remote: "origin".to_string(),
            fork: Some(ForgeForkRefs {
                head_ref: "refs/pull/1/head".to_string(),
                local_ref: "refs/remotes/origin/pr/1".to_string(),
            }),
            head_fallback: None,
            display: "PR #1".to_string(),
            title: "Fork contribution".to_string(),
            state_note: None,
            resolve_elapsed: std::time::Duration::from_millis(5),
        };
        execute(
            &forge_params("contributor-branch", worktree_path.clone(), forge),
            &git_cmd,
            &work,
            &mut sink,
        )
        .expect("fork PR checkout succeeds");
        assert!(worktree_path.exists());

        // The branch was created from the fetched pull ref, and pulls from it.
        assert_eq!(
            git_out(
                &worktree_path,
                &["config", "--get", "branch.contributor-branch.remote"]
            ),
            "origin"
        );
        assert_eq!(
            git_out(
                &worktree_path,
                &["config", "--get", "branch.contributor-branch.merge"]
            ),
            "refs/pull/1/head",
            "git pull updates the branch from the PR head"
        );
        // The pull ref was fetched into its local tracking ref.
        assert!(
            !git_out(
                &work,
                &["rev-parse", "--verify", "-q", "refs/remotes/origin/pr/1"]
            )
            .is_empty(),
            "the fork fetch materialized the tracking ref"
        );

        let plan = sink.plan.as_ref().expect("plan committed");
        let checkout_annotation = plan
            .steps()
            .find(|s| s.key.id == StageId::CheckOut)
            .and_then(|s| s.annotation.as_deref());
        assert_eq!(
            checkout_annotation,
            Some("\u{2190} PR #1"),
            "the CheckOut row names the PR, not a nonexistent origin/<branch>"
        );
    }

    /// A closed/merged PR/MR still checks out — the state note surfaces as a
    /// warning after the plan commits, and the checkout proceeds.
    #[test]
    #[serial]
    fn forge_state_note_warns_but_proceeds() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(&origin, &["branch", "feat-merged"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("merged-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let forge = ForgeCheckout {
            remote: "origin".to_string(),
            fork: None,
            head_fallback: None,
            display: "PR #9".to_string(),
            title: "Old work".to_string(),
            state_note: Some("PR #9 is merged".to_string()),
            resolve_elapsed: std::time::Duration::from_millis(1),
        };
        execute(
            &forge_params("feat-merged", worktree_path.clone(), forge),
            &git_cmd,
            &work,
            &mut sink,
        )
        .expect("merged PR still checks out");
        assert!(worktree_path.exists());
        assert!(
            sink.warnings.iter().any(|w| w == "PR #9 is merged"),
            "the state note surfaced as a warning: {:?}",
            sink.warnings
        );
    }

    /// A required fork fetch that can't resolve the pull ref aborts with a
    /// PR-specific error (not `BranchNotFound`, so the command layer's morph
    /// can't fire), after the plan has committed.
    #[test]
    #[serial]
    fn fork_forge_missing_pull_ref_aborts_after_plan() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let forge = ForgeCheckout {
            remote: "origin".to_string(),
            fork: Some(ForgeForkRefs {
                head_ref: "refs/pull/999/head".to_string(),
                local_ref: "refs/remotes/origin/pr/999".to_string(),
            }),
            head_fallback: None,
            display: "PR #999".to_string(),
            title: "Ghost".to_string(),
            state_note: None,
            resolve_elapsed: std::time::Duration::from_millis(1),
        };
        let Err(err) = execute(
            &forge_params("ghost", tmp.path().join("ghost-wt"), forge),
            &git_cmd,
            &work,
            &mut sink,
        ) else {
            panic!("a missing pull ref must fail");
        };
        assert!(
            matches!(err, CheckoutError::Other(_)),
            "forge failures are Other, never BranchNotFound (no morph)"
        );
        assert!(
            sink.plan.is_some(),
            "the plan commits before the required fork fetch runs"
        );
    }

    /// Same-repo PR/MR whose base remote is not the settings default: every
    /// remote-branch fact (fetch, existence probe, creation ref, upstream)
    /// must follow the PR's base remote, not `daft.remote`.
    #[test]
    #[serial]
    fn same_repo_forge_uses_the_base_remote_not_the_settings_default() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        // The PR's base repo is a second remote; only it has the source branch.
        let upstream = tmp.path().join("upstream");
        std::fs::create_dir_all(&upstream).unwrap();
        git(&upstream, &["init", "-q", "-b", "main"]);
        git(
            &upstream,
            &["commit", "--allow-empty", "-q", "-m", "base init"],
        );
        git(&upstream, &["branch", "feat-up"]);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        git(
            &work,
            &["remote", "add", "upstream", upstream.to_str().unwrap()],
        );
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("feat-up-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let forge = ForgeCheckout {
            remote: "upstream".to_string(),
            fork: None,
            head_fallback: Some(ForgeForkRefs {
                head_ref: "refs/pull/4/head".to_string(),
                local_ref: "refs/remotes/upstream/pr/4".to_string(),
            }),
            display: "PR #4".to_string(),
            title: "Upstream work".to_string(),
            state_note: None,
            resolve_elapsed: std::time::Duration::from_millis(2),
        };
        // params() sets remote_name = "origin" — the mismatch under test.
        execute(
            &forge_params("feat-up", worktree_path.clone(), forge),
            &git_cmd,
            &work,
            &mut sink,
        )
        .expect("the checkout fetches the branch from the PR's base remote");
        assert!(worktree_path.exists());
        assert_eq!(
            git_out(
                &worktree_path,
                &["config", "--get", "branch.feat-up.remote"]
            ),
            "upstream",
            "upstream tracking follows the base remote"
        );
    }

    /// Same-repo PR/MR whose source branch was deleted from the base repo
    /// (the merged-and-cleaned-up shape): the checkout falls back to the PR
    /// head ref — fork mechanics — instead of failing.
    #[test]
    #[serial]
    fn same_repo_forge_falls_back_to_the_head_ref_when_the_branch_is_gone() {
        let _cwd = CwdGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git(&origin, &["init", "-q", "-b", "main"]);
        git(&origin, &["commit", "--allow-empty", "-q", "-m", "init"]);
        // The PR head survives only as the pull ref — the branch is gone.
        seed_pull_ref(&origin, 6);
        git(tmp.path(), &["clone", "-q", "origin", "work"]);
        let work = tmp.path().join("work");
        std::env::set_current_dir(&work).unwrap();

        let worktree_path = tmp.path().join("gone-wt");
        let git_cmd = GitCommand::new(true);
        let mut sink = RecordingStageSink::default();
        let forge = ForgeCheckout {
            remote: "origin".to_string(),
            fork: None,
            head_fallback: Some(ForgeForkRefs {
                head_ref: "refs/pull/6/head".to_string(),
                local_ref: "refs/remotes/origin/pr/6".to_string(),
            }),
            display: "PR #6".to_string(),
            title: "Merged and cleaned up".to_string(),
            state_note: Some("PR #6 is merged".to_string()),
            resolve_elapsed: std::time::Duration::from_millis(2),
        };
        execute(
            &forge_params("feat-gone", worktree_path.clone(), forge),
            &git_cmd,
            &work,
            &mut sink,
        )
        .expect("a deleted source branch falls back to the PR head ref");
        assert!(worktree_path.exists());

        // The branch was created from the fetched head ref, and pulls from it.
        assert_eq!(
            git_out(
                &worktree_path,
                &["config", "--get", "branch.feat-gone.merge"]
            ),
            "refs/pull/6/head",
            "tracking falls back to the PR head ref"
        );
        assert_eq!(
            git_out(
                &worktree_path,
                &["config", "--get", "branch.feat-gone.remote"]
            ),
            "origin"
        );
        // The provenance note names the PR, not a nonexistent origin/<branch>.
        assert!(
            sink.events.iter().any(|(k, e)| k.id == StageId::CheckOut
                && matches!(e, StageEvent::Note(n) if n == "\u{2190} PR #6")),
            "the CheckOut row notes the head-ref provenance: {:?}",
            sink.events
        );
    }
}
