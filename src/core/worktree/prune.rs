//! Core logic for the `git-worktree-prune` command.
//!
//! Removes worktrees and branches whose remote branch was deleted — where
//! "deleted" means git or daft can evidence that the branch was on the remote
//! and no longer is. A branch nothing attests to is left alone.

use crate::core::worktree::provenance;
use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType, RemovalReason};
use crate::remote::get_default_branch_local;
use crate::settings::PruneCdTarget;
use crate::{get_git_common_dir, get_project_root};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Input parameters for the prune operation.
pub struct PruneParams {
    /// Force removal of worktrees with uncommitted changes.
    pub force: bool,
    /// Whether output is in quiet mode.
    pub is_quiet: bool,
    /// Remote name (from settings).
    pub remote_name: String,
    /// Where to cd after pruning the current worktree.
    pub prune_cd_target: PruneCdTarget,
    /// Optional two-stage cancel flag (sync's sequential path). When set,
    /// the phase's `git fetch --prune` is torn down on Ctrl+C instead of
    /// blocking to completion; `daft prune` and the DAG paths pass `None`.
    pub cancel: Option<std::sync::Arc<crate::git::cancel::CancelFlag>>,
    /// Settles branches the git probes cannot place, by asking the forge
    /// whether their PR merged (#737).
    ///
    /// Shared across the whole run — every DAG worker and the post-TUI
    /// deferred pass hold the same one — so its listing is fetched at most
    /// once and every branch is judged against identical data. A second
    /// witness here would mean a second fetch, and a no-op one in the
    /// deferred pass could reverse a verdict the table already showed.
    pub merged_witness: std::sync::Arc<dyn crate::core::worktree::ports::ForgeMergedWitness>,
}

/// Detail of a single pruned branch.
pub struct PrunedBranchDetail {
    pub branch_name: String,
    pub worktree_removed: bool,
    pub branch_deleted: bool,
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
    /// Per-branch detail of what was removed.
    pub pruned_branches: Vec<PrunedBranchDetail>,
    /// Branches kept because their worktrees hold refined untracked daft
    /// files — consolidate with `daft file merge` or re-run with --force.
    pub skipped_refined: Vec<String>,
    /// Branches kept because the remote is gone but the local branch is not
    /// merged into the default branch.
    pub skipped_unmerged: Vec<String>,
}

/// A worktree entry from `git worktree list --porcelain`.
///
/// Alias for the shared [`crate::core::worktree::porcelain::WorktreeListEntry`],
/// kept under this name so prune's callers (and `remove_repo`'s re-export) need
/// no churn.
pub use crate::core::worktree::porcelain::WorktreeListEntry as WorktreeEntry;

/// Bundles common state used throughout the prune operation.
pub struct PruneContext<'a> {
    pub git: &'a GitCommand,
    pub project_root: PathBuf,
    pub git_dir: PathBuf,
    pub remote_name: String,
    pub source_worktree: PathBuf,
    /// Default branch (merge target for the unmerged guard and daft-file
    /// classification). `None` when it cannot be resolved — the affected
    /// guards then degrade protectively (skip with a warning) rather than
    /// failing the prune.
    pub default_branch: Option<String>,
}

/// Result of pruning a single branch.
pub struct SingleBranchPruneResult {
    pub detail: PrunedBranchDetail,
    pub branches_deleted: u32,
    pub worktrees_removed: u32,
    pub deferred: bool,
    /// True when pruning was skipped because the worktree has uncommitted changes.
    pub skipped_dirty: bool,
    /// True when pruning was skipped because the worktree has refined
    /// untracked daft files not represented in the default branch's worktree.
    pub skipped_refined: bool,
    /// True when pruning was skipped because the remote branch is gone but
    /// the local branch is not merged into the default branch.
    pub skipped_unmerged: bool,
}

/// Result of removing a single worktree + deleting its branch.
struct SinglePruneResult {
    worktree_removed: bool,
    branch_deleted: bool,
    skipped_dirty: bool,
    skipped_refined: bool,
}

/// Outcome of attempting to remove a worktree.
enum RemoveOutcome {
    /// Worktree was successfully removed.
    Removed,
    /// Skipped because worktree has uncommitted changes.
    SkippedDirty,
    /// Skipped because the worktree has refined untracked daft files.
    SkippedRefined,
    /// Skipped due to an error (not dirty-related).
    Failed,
}

/// Execute the prune operation.
pub fn execute(
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<PruneResult> {
    let mut git = GitCommand::new(params.is_quiet);
    if let Some(cancel) = &params.cancel {
        git = git.with_cancel(std::sync::Arc::clone(cancel));
    }
    let git_dir = get_git_common_dir()?;
    // Reclaim leftovers from a reaper that died (#200): a failed background
    // delete must be delayed, never permanent. Cheap when the trash is empty.
    crate::core::worktree::trash::sweep(&git_dir);
    let default_branch = get_default_branch_local(&git_dir, &params.remote_name).ok();
    let ctx = PruneContext {
        git: &git,
        project_root: get_project_root()?,
        git_dir,
        remote_name: params.remote_name.clone(),
        source_worktree: std::env::current_dir()?,
        default_branch,
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
    // Skip detached HEAD worktrees (sandboxes) — they have no branch and must
    // never be pruned based on remote-tracking branch state.
    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if entry.is_detached {
            sink.on_step(&format!(
                "Skipping detached HEAD sandbox at {} during prune",
                entry.path.display()
            ));
            continue;
        }
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    // Identify gone branches
    let gone_branches = identify_gone_branches(
        &git,
        &worktree_map,
        &ctx.remote_name,
        ctx.default_branch.as_deref(),
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
            pruned_branches: Vec::new(),
            skipped_refined: Vec::new(),
            skipped_unmerged: Vec::new(),
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
    let mut pruned_branches: Vec<PrunedBranchDetail> = Vec::new();
    let mut skipped_refined: Vec<String> = Vec::new();
    let mut skipped_unmerged: Vec<String> = Vec::new();

    for branch_name in &gone_branches {
        let result = prune_single_branch(
            &ctx,
            branch_name,
            &worktree_map,
            is_bare_layout,
            &current_wt_path,
            &current_branch,
            params,
            sink,
        )?;

        branches_deleted += result.branches_deleted;
        worktrees_removed += result.worktrees_removed;
        if result.skipped_refined {
            skipped_refined.push(branch_name.clone());
        }
        if result.skipped_unmerged {
            skipped_unmerged.push(branch_name.clone());
        }

        if result.deferred {
            deferred_branch = Some(branch_name.clone());
        } else if result.detail.branch_deleted || result.detail.worktree_removed {
            pruned_branches.push(result.detail);
        }
    }

    // Process deferred branch (user's current worktree) last
    let prev_branches = branches_deleted;
    let prev_worktrees = worktrees_removed;
    let cd_target = process_deferred_branch(
        &ctx,
        &deferred_branch,
        &worktree_map,
        params,
        sink,
        &mut branches_deleted,
        &mut worktrees_removed,
    );
    if let Some(ref branch_name) = deferred_branch {
        let was_branch_deleted = branches_deleted > prev_branches;
        let was_worktree_removed = worktrees_removed > prev_worktrees;
        if was_branch_deleted || was_worktree_removed {
            pruned_branches.push(PrunedBranchDetail {
                branch_name: branch_name.clone(),
                worktree_removed: was_worktree_removed,
                branch_deleted: was_branch_deleted,
            });
        }
    }

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
        pruned_branches,
        skipped_refined,
        skipped_unmerged,
    })
}

// ── Per-branch prune (public for DAG workers) ─────────────────────────────

/// Prune a single branch. Called by the DAG executor for parallel pruning.
///
/// Returns the result of pruning the branch, including per-branch detail and
/// counters. When the branch corresponds to the user's current worktree, the
/// result has `deferred = true` and no work is done — the caller must handle
/// that branch separately (see `process_deferred_branch`).
#[allow(clippy::too_many_arguments)]
pub fn prune_single_branch(
    ctx: &PruneContext,
    branch_name: &str,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    is_bare_layout: bool,
    current_wt_path: &Option<PathBuf>,
    current_branch: &Option<String>,
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<SingleBranchPruneResult> {
    sink.on_step(&format!("Processing branch: {branch_name}"));

    let mut branches_deleted: u32 = 0;
    let mut worktrees_removed: u32 = 0;
    let mut deferred = false;
    let mut skipped_dirty = false;
    let mut skipped_refined = false;

    // Gone-but-unmerged guard: a remote branch disappearing does not mean
    // the work was merged — abandoned branches lose their remotes too.
    // Verify (ancestor or squash) before destroying local state; --force
    // overrides. The default branch itself is exempt (trivially merged);
    // an unresolvable default branch or a failed check skips protectively.
    if !params.force {
        let skip_reason = match ctx.default_branch.as_deref() {
            Some(default_branch) if default_branch == branch_name => None,
            Some(default_branch) => match crate::core::worktree::merged::is_branch_merged(
                ctx.git,
                branch_name,
                default_branch,
                &ctx.remote_name,
                params.merged_witness.as_ref(),
            ) {
                Ok(verdict) if verdict.is_merged() => {
                    // Name the PR when the forge is what proved it: nothing in
                    // the branch's own history shows the merge, so an
                    // unexplained deletion would look wrong.
                    if let Some(via) = verdict.via() {
                        sink.on_step(&format!(
                            "Branch '{branch_name}' was merged via {}",
                            via.short()
                        ));
                    }
                    None
                }
                Ok(_) => Some(format!(
                    "Skipping {branch_name}: remote branch is gone but the local branch \
                     is not merged into {default_branch} (use --force to delete anyway)"
                )),
                Err(e) => Some(format!(
                    "Skipping {branch_name}: could not verify merge status ({e}); \
                     use --force to delete anyway"
                )),
            },
            None => Some(format!(
                "Skipping {branch_name}: cannot determine the default branch to verify \
                 merge status; use --force to delete anyway"
            )),
        };
        if let Some(reason) = skip_reason {
            sink.on_warning(&reason);
            return Ok(SingleBranchPruneResult {
                detail: PrunedBranchDetail {
                    branch_name: branch_name.to_string(),
                    worktree_removed: false,
                    branch_deleted: false,
                },
                branches_deleted: 0,
                worktrees_removed: 0,
                deferred: false,
                skipped_dirty: false,
                skipped_refined: false,
                skipped_unmerged: true,
            });
        }
    }

    let wt_info = worktree_map.get(branch_name).cloned();

    match wt_info {
        Some((ref wt_path, true)) if !is_bare_layout => {
            // Branch is checked out in the main worktree of a regular repo
            process_main_worktree_branch(
                ctx,
                wt_path,
                branch_name,
                current_branch,
                params,
                sink,
                &mut branches_deleted,
                &mut worktrees_removed,
                &mut skipped_dirty,
                &mut skipped_refined,
            )?;
        }
        Some((ref wt_path, _)) if !is_bare_layout => {
            // Linked worktree in a non-bare repo
            let mut deferred_branch: Option<String> = None;
            process_linked_worktree_branch(
                ctx,
                wt_path,
                branch_name,
                current_wt_path,
                params.force,
                sink,
                &mut branches_deleted,
                &mut worktrees_removed,
                &mut deferred_branch,
                &mut skipped_dirty,
                &mut skipped_refined,
            );
            if deferred_branch.is_some() {
                deferred = true;
            }
        }
        Some((ref wt_path, is_main)) => {
            // Bare layout
            let mut deferred_branch: Option<String> = None;
            process_bare_layout_branch(
                ctx,
                wt_path,
                branch_name,
                is_main,
                current_wt_path,
                params.force,
                sink,
                &mut branches_deleted,
                &mut worktrees_removed,
                &mut deferred_branch,
                &mut skipped_dirty,
                &mut skipped_refined,
            );
            if deferred_branch.is_some() {
                deferred = true;
            }
        }
        None => {
            // No worktree for this branch
            sink.on_step(&format!("No associated worktree found for {branch_name}"));
            if delete_branch(ctx.git, branch_name, sink) {
                branches_deleted += 1;
                sink.on_step(&format!(" * [pruned] {}/{branch_name}", ctx.remote_name));
            }
        }
    }

    let detail = PrunedBranchDetail {
        branch_name: branch_name.to_string(),
        worktree_removed: worktrees_removed > 0,
        branch_deleted: branches_deleted > 0,
    };

    Ok(SingleBranchPruneResult {
        detail,
        branches_deleted,
        worktrees_removed,
        deferred,
        skipped_dirty,
        skipped_refined,
        skipped_unmerged: false,
    })
}

/// Handle a deferred branch (the user's current worktree) after the TUI finishes.
///
/// In parallel/TUI mode, `prune_single_branch` defers the current worktree
/// (returns `deferred: true` without removing it). This function performs the
/// actual removal after all other tasks complete.
///
/// Returns the cd target path if the worktree was successfully removed.
#[allow(clippy::too_many_arguments)]
pub fn handle_deferred_prune(
    ctx: &PruneContext,
    branch_name: &str,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Option<PathBuf> {
    let deferred_branch = Some(branch_name.to_string());
    let mut branches_deleted = 0u32;
    let mut worktrees_removed = 0u32;
    process_deferred_branch(
        ctx,
        &deferred_branch,
        worktree_map,
        params,
        sink,
        &mut branches_deleted,
        &mut worktrees_removed,
    )
}

// ── Branch identification ──────────────────────────────────────────────────

/// Identify local branches whose remote branch has been deleted.
///
/// Two arms, and nothing else is a candidate:
///
/// 1. **Git's own evidence** — `git branch -vv` reports `: gone]`, which needs
///    a configured upstream whose remote-tracking ref is missing. Only a branch
///    that was published (or explicitly tracked) ever gets one. Cross-checked
///    against `branch.<name>.remote`, because that report is read as a
///    substring of a line that ends in the commit subject.
/// 2. **Daft's own record** — daft pushed the branch to this remote
///    ([`provenance`]) and the remote no longer has it.
///
/// A branch with neither is *unknown*, and unknown is not a prune candidate.
/// That is the #858 fix: this used to flag any branch that had a worktree and
/// was absent from the remote, which is the state of every branch `daft start`
/// just created — and a zero-commit branch is trivially "merged", so the
/// gone-but-unmerged guard downstream waved it through. Absence is not
/// evidence: git destroys the remote-tracking ref *and* its reflog when the
/// branch disappears upstream, so a fresh branch and a deleted one look
/// identical from here. See ARCHITECTURE.md "Bridge git's evidence gaps with
/// daft's own records".
///
/// **Freshness:** arm 2 answers "is it still on the remote" from
/// `refs/remotes/<remote>/*`, so it is really "as of the most recent
/// `fetch --prune`". Callers that must be current fetch first — `execute`
/// below, and the two TUI orchestrators via `run_fetch_phase`. The one caller
/// that deliberately runs *before* the fetch (`commands::prune`'s pre-fetch
/// seeding, which populates the table from what is already known) accepts a
/// stale subset by design. The payoff of reading refs instead of asking the
/// remote is that identification is entirely local — no per-branch `ls-remote`,
/// nothing to cancel, no network.
///
/// **Refspec coverage:** reading `refs/remotes/<remote>/*` also means "still on
/// the remote" is only as wide as `remote.<name>.fetch`. In a repository whose
/// refspec is narrowed — a single-branch clone, a hand-edited refspec — a
/// branch that is alive upstream has no tracking ref here, so both arms read it
/// as gone. Arm 1 has always had this dependency (git computes `: gone]` the
/// same way); arm 2 now shares it. The gone-but-unmerged guard downstream is
/// what keeps the consequence to reclaiming merged work.
pub fn identify_gone_branches(
    git: &GitCommand,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    remote_name: &str,
    default_branch: Option<&str>,
    sink: &mut dyn ProgressSink,
) -> Result<Vec<String>> {
    sink.on_step("Identifying local branches whose upstream branch is gone...");
    let mut gone_branches = Vec::new();

    // The default branch is never a prune candidate: pruning reclaims merged
    // feature branches whose upstream is gone, not the repo's home branch. A
    // gone upstream on the default branch (e.g. the remote default was renamed)
    // must never delete it — and the gone-but-unmerged guard deliberately
    // proceeds on the default branch (trivially merged into itself), so the
    // exclusion has to happen here, at identification. Match by identity; when
    // the default cannot be resolved, fall back to the conventional names so
    // master/main stay protected.
    let is_default_branch = |name: &str| match default_branch {
        Some(default) => name == default,
        None => name == "master" || name == "main",
    };

    // What each branch actually tracks, straight out of config. Both arms
    // below need it, and it is one subprocess for the whole repository.
    let tracking = parse_tracking_config(&git.branch_tracking_entries()?);

    // Method 1: the `git branch -vv` rendering, read for `: gone]` lines.
    gone_branches.extend(
        gone_branches_from_verbose_listing(&git.branch_list_verbose()?, &tracking)
            .into_iter()
            .filter(|name| !is_default_branch(name))
            .map(str::to_string),
    );

    // Method 2: branches daft published to this remote that the remote no
    // longer has. The record is what separates them from branches that were
    // never there — see this function's docs.
    sink.on_step("Checking daft-published branches for a deleted remote branch...");
    // Best-effort, like the backfill below it: an unreadable record means
    // *unknown*, and unknown is not a prune candidate. Hard-failing here would
    // take the whole prune (and sync's phase 2) down over a record whose
    // absence only ever makes this arm more conservative — and the cwd this
    // read resolves can legitimately be gone, since removing the directory the
    // user is standing in is a thing prune does.
    let mut provenance = match provenance::Provenance::load(git) {
        Ok(provenance) => provenance,
        Err(e) => {
            sink.on_debug(&format!(
                "No publication records available ({e}); only branches git itself reports gone are in scope"
            ));
            provenance::Provenance::default()
        }
    };
    let remaining = remote_branches_after_fetch(git, remote_name)?;
    // Before deciding anything, write down what this run can already see: a
    // branch whose tracking ref is present is on the remote right now, whoever
    // put it there. Without this the record would only ever describe daft's own
    // pushes, and branches published by everything else in the repo would never
    // become reclaimable (#858).
    backfill_observed_publications(
        git,
        &tracking,
        remote_name,
        &remaining,
        &mut provenance,
        sink,
    );
    let ref_output = git.for_each_ref("%(refname:short)", "refs/heads")?;

    for line in ref_output.lines() {
        let branch_name = line.trim();
        if branch_name.is_empty() || is_default_branch(branch_name) {
            continue;
        }

        if !worktree_map.contains_key(branch_name)
            || gone_branches.contains(&branch_name.to_string())
        {
            continue;
        }

        if !provenance.published_on(branch_name, remote_name) {
            // Not a candidate, and say which kind of not-a-candidate: a branch
            // daft created and never published is the normal state, not a
            // warning, so this stays at debug level (#858).
            if !remaining.contains(branch_name) {
                sink.on_debug(&format!(
                    "Not pruning '{branch_name}': {}",
                    unpublished_reason(&provenance, branch_name)
                ));
            }
            continue;
        }

        if !remaining.contains(branch_name) {
            gone_branches.push(branch_name.to_string());
            sink.on_debug(&format!(
                "Found published branch whose remote branch is gone: {branch_name}"
            ));
        }
    }

    Ok(gone_branches)
}

/// Branch names the remote still has, per this run's freshly pruned
/// remote-tracking refs.
///
/// Local by design: `git fetch --prune` has just reconciled these against the
/// wire, so asking `ls-remote` again would be a second round trip per branch
/// for an answer already on disk. (`refs/remotes/<remote>/HEAD` shortens to
/// `<remote>/HEAD` — git's own `for-each-ref` would render a bare `<remote>` —
/// and neither survives the prefix strip as a real branch name.)
fn remote_branches_after_fetch(git: &GitCommand, remote_name: &str) -> Result<HashSet<String>> {
    let refs = git.for_each_ref("%(refname:short)", &format!("refs/remotes/{remote_name}"))?;
    Ok(parse_remote_branches(&refs, remote_name))
}

/// Strip `<remote>/` off each shortened ref, dropping the remote's HEAD.
fn parse_remote_branches(refs: &str, remote_name: &str) -> HashSet<String> {
    let prefix = format!("{remote_name}/");
    refs.lines()
        .filter_map(|line| line.trim().strip_prefix(&prefix))
        .filter(|name| *name != "HEAD")
        .map(str::to_string)
        .collect()
}

/// What a branch's configuration says it tracks.
struct TrackedUpstream {
    /// `branch.<name>.remote` — a named remote, or a bare URL.
    remote: String,
    /// `branch.<name>.merge` exactly as configured: `refs/heads/<branch>` for
    /// an ordinary tracking branch, but `refs/pull/<n>/head` and friends are
    /// legal too, which is why this is kept raw rather than shortened here.
    merge: String,
}

/// Branches a `git branch -vv`-shaped listing reports as tracking a gone
/// upstream, cross-checked against tracking configuration.
///
/// The `: gone]` test is a substring match over the whole line, and git's
/// own rendering ends the line with the commit subject, printed with no
/// delimiter after the tracking bracket — so a subject like
/// `fix: gone] handling` matches on a branch that has no upstream at all
/// (verified against real git). Requiring tracking configuration is what
/// makes the match trustworthy, and it excludes nothing real — the bracket
/// is only ever rendered when an upstream *is* configured, so every genuine
/// `: gone]` line has an entry in `tracking`. Without this a commit message
/// could talk a never-published branch into prune's scope, which is #858's
/// failure class arriving by another door. daft's own rendering
/// (`oxide::branch_list_verbose`) appends no subject, so the guard cannot
/// fire there today; it stays because the parser's contract is the line
/// shape, not the renderer behind it.
fn gone_branches_from_verbose_listing<'a>(
    listing: &'a str,
    tracking: &HashMap<String, TrackedUpstream>,
) -> Vec<&'a str> {
    listing
        .lines()
        .filter(|line| line.contains(": gone]"))
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.first() {
                Some(&"*") | Some(&"+") => parts.get(1).copied(),
                _ => parts.first().copied(),
            }
        })
        .filter(|name| !name.is_empty() && tracking.contains_key(*name))
        .collect()
}

/// Parse `git config --get-regexp` output into one entry per branch that has a
/// tracking remote configured.
///
/// Keyed on `branch.<name>.remote`: a branch with a remote but no merge ref is
/// still a branch someone pointed at a remote, and callers that need the
/// upstream ref check `merge` themselves.
///
/// The branch name is recovered by stripping the known suffix rather than by
/// splitting on dots — `branch.release.2.x.remote` is a branch called
/// `release.2.x`, and a dot-splitting parser would file it under `release`.
/// Same hazard, same handling as [`provenance::Provenance::parse`].
fn parse_tracking_config(entries: &str) -> HashMap<String, TrackedUpstream> {
    let mut remotes: HashMap<String, String> = HashMap::new();
    let mut merges: HashMap<String, String> = HashMap::new();

    for line in entries.lines() {
        let Some((key, value)) = line.trim_end().split_once(' ') else {
            continue;
        };
        let Some(rest) = key.strip_prefix("branch.") else {
            continue;
        };
        if let Some(branch) = rest.strip_suffix(".remote") {
            remotes.insert(branch.to_string(), value.trim().to_string());
        } else if let Some(branch) = rest.strip_suffix(".merge") {
            merges.insert(branch.to_string(), value.trim().to_string());
        }
    }

    remotes
        .into_iter()
        .map(|(branch, remote)| {
            let merge = merges.remove(&branch).unwrap_or_default();
            (branch, TrackedUpstream { remote, merge })
        })
        .collect()
}

/// Branches whose configuration says they track `<remote>/<same name>`, and
/// whose remote-tracking ref is present right now.
///
/// Both halves are load-bearing and neither is an inference. Config is a
/// deliberate statement that this branch belongs to that ref — a local branch
/// and a same-named remote branch can be entirely unrelated, and that
/// coincidence is what would let a teammate's `feat/x` authorize deleting
/// yours. `remaining` is this run's freshly pruned `refs/remotes/<remote>/*`,
/// so membership *is* the "still on the remote" test rather than a proxy for
/// one.
///
/// The name still has to match the upstream, because a matching name is the
/// question prune asks later — a branch tracking `<remote>/other` says nothing
/// about whether a remote branch of *its own* name exists.
///
/// Sorted so the records are written, and reported, in a stable order.
fn observed_on_remote<'a>(
    tracking: &'a HashMap<String, TrackedUpstream>,
    remote_name: &str,
    remaining: &HashSet<String>,
) -> Vec<&'a str> {
    let mut observed: Vec<&str> = tracking
        .iter()
        .filter(|(name, upstream)| {
            upstream.remote == remote_name
                && upstream.merge == format!("refs/heads/{name}")
                && remaining.contains(name.as_str())
        })
        .map(|(name, _)| name.as_str())
        .collect();
    observed.sort_unstable();
    observed
}

/// Record the publications this run can see for itself (#858).
///
/// daft is not the only thing that pushes in a repo. A teammate's `git push
/// -u`, a script, a plain `git worktree add` — all produce branches daft never
/// handled, and a record that only ever described daft's own pushes would
/// leave them permanently out of prune's reach. A branch configured to track
/// `<remote>/<same name>` whose ref is present is one this run has observed on
/// the remote, and an observation is exactly what the record is allowed to
/// hold.
///
/// Best-effort, like the push seam: a failed write costs a future prune, never
/// safety. Already-recorded branches are skipped so an ordinary sync does not
/// rewrite config for every tracking branch it has.
fn backfill_observed_publications(
    git: &GitCommand,
    tracking: &HashMap<String, TrackedUpstream>,
    remote_name: &str,
    remaining: &HashSet<String>,
    provenance: &mut provenance::Provenance,
    sink: &mut dyn ProgressSink,
) {
    // Repo-scoped write: `branch.*` lives in the shared config, and prune's
    // context is the repository it is standing in (fleet mode chdirs per repo),
    // which is the same directory `Provenance::load` reads through.
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return,
    };

    for branch in observed_on_remote(tracking, remote_name, remaining) {
        if provenance.published_on(branch, remote_name) {
            continue;
        }
        if provenance::mark_published(git, &cwd, remote_name, branch).is_ok() {
            provenance.record_published(branch, remote_name);
            sink.on_debug(&format!(
                "Recorded '{branch}' as published to '{remote_name}': its tracking ref is present"
            ));
        }
    }
}

/// Why a branch missing from the remote is nevertheless out of prune's scope.
fn unpublished_reason(provenance: &provenance::Provenance, branch: &str) -> String {
    if provenance.is_local_only(branch) {
        // Runtime output renders the executable the way the user invoked it,
        // so this reads `git daft remove` under `git worktree-prune`.
        format!(
            "created by `{}` and never published — `{}` to discard",
            crate::daft_cmd("start"),
            crate::daft_cmd(&format!("remove {branch}"))
        )
    } else {
        "no record of it ever being published to this remote".to_string()
    }
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
    skipped_dirty: &mut bool,
    skipped_refined: &mut bool,
) -> Result<()> {
    sink.on_step(&format!(
        "Branch {branch_name} is checked out in the main worktree"
    ));

    let is_current = current_branch.as_deref() == Some(branch_name);
    let mut wt_removed = false;

    if is_current {
        match get_default_branch_local(&ctx.git_dir, &ctx.remote_name) {
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
        let outcome = remove_worktree(ctx, wt_path, branch_name, params.force, sink);
        if !matches!(outcome, RemoveOutcome::Removed) {
            if matches!(outcome, RemoveOutcome::SkippedDirty) {
                *skipped_dirty = true;
            }
            if matches!(outcome, RemoveOutcome::SkippedRefined) {
                *skipped_refined = true;
            }
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
    skipped_dirty: &mut bool,
    skipped_refined: &mut bool,
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
    if result.skipped_dirty {
        *skipped_dirty = true;
    }
    if result.skipped_refined {
        *skipped_refined = true;
    }
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
    skipped_dirty: &mut bool,
    skipped_refined: &mut bool,
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
    if result.skipped_dirty {
        *skipped_dirty = true;
    }
    if result.skipped_refined {
        *skipped_refined = true;
    }
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

/// Remove a worktree (with hooks and dirty checks).
fn remove_worktree(
    ctx: &PruneContext,
    wt_path: &Path,
    branch_name: &str,
    force: bool,
    sink: &mut (impl ProgressSink + HookRunner),
) -> RemoveOutcome {
    // Daft-file provenance guard. Classify the worktree's untracked daft
    // files against their seeds: pristine or already-subsumed copies pass
    // silently — including the stale-but-untouched copy a moved-on default
    // branch used to false-positively skip (issue #628). Refined copies are
    // real user data; prune is a batch command and never prompts, so without
    // --force the worktree is kept (with a pointer at `daft file merge`),
    // and with --force the refinements are stashed under .daft/discarded/
    // before removal. The default-branch worktree is NEVER written by prune.
    // Open the seed store once: classification below consults it, and the
    // post-removal cleanup at the end reuses the same handle. Best-effort.
    let seeds = crate::hooks::visitor_seeds::SeedsContext::open(&ctx.git_dir);

    if wt_path.exists() {
        let target_wt: Option<PathBuf> = ctx
            .default_branch
            .as_deref()
            .map(|default_branch| ctx.project_root.join(default_branch))
            .filter(|p| p.is_dir());
        let classes = crate::hooks::visitor_seeds::classify_in_scope_files(
            seeds.as_ref(),
            branch_name,
            wt_path,
            target_wt.as_deref(),
        );
        let blocking = crate::hooks::visitor_seeds::blocking_files(&classes);
        if !blocking.is_empty() {
            if !force {
                let files = blocking
                    .iter()
                    .map(|c| c.filename.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                sink.on_warning(&format!(
                    "Keeping {branch_name}: {files} has refinements not in the default \
                     branch's worktree; consolidate with `daft file merge` or pass \
                     --force to discard"
                ));
                return RemoveOutcome::SkippedRefined;
            }
            for class in blocking {
                let file = wt_path.join(&class.filename);
                match crate::hooks::visitor_seeds::stash_file(
                    &ctx.git_dir,
                    crate::hooks::visitor_seeds::StashKind::Discarded,
                    branch_name,
                    &file,
                ) {
                    Some(dest) => sink.on_warning(&format!(
                        "Discarded {} refinements from '{branch_name}' — saved to {}",
                        class.filename,
                        dest.display()
                    )),
                    None => sink.on_warning(&format!(
                        "Discarded {} refinements from '{branch_name}' (stash copy failed; \
                         the file is gone with the worktree)",
                        class.filename
                    )),
                }
            }
        }
    }

    // Check for uncommitted changes
    if wt_path.exists() && !force {
        match ctx.git.has_uncommitted_changes_in(wt_path) {
            Ok(true) => {
                sink.on_warning(&format!(
                    "Skipping {branch_name}: worktree has uncommitted changes or untracked files (use --force to override)"
                ));
                return RemoveOutcome::SkippedDirty;
            }
            Ok(false) => {}
            Err(e) => {
                sink.on_warning(&format!(
                    "Skipping {branch_name}: failed to check for uncommitted changes: {e} (use --force to override)"
                ));
                return RemoveOutcome::Failed;
            }
        }
    }

    // Cancel any running background jobs for this worktree (best-effort).
    cancel_background_jobs_for_worktree(branch_name, sink);

    // Pre-remove hook
    run_removal_hook(HookType::PreRemove, ctx, wt_path, branch_name, sink);

    // Capture the identity key while the directory can still be probed:
    // records are keyed on the private-gitdir id, not the branch, and a
    // drifted record does not match the branch name we know here.
    let identity_id = crate::core::worktree::identity_store::worktree_id_for(wt_path);

    if wt_path.exists() {
        sink.on_step("Removing worktree...");
        // Rename aside rather than walking the tree (#200). Declining is not a
        // failure — it means the fast path did not apply, and the ordinary
        // removal below runs unchanged, including git's refusal of a dirty
        // worktree.
        use crate::core::worktree::trash::Disposition;
        let disposition =
            crate::core::worktree::trash::dispose(ctx.git, &ctx.git_dir, wt_path, force);
        if disposition == Disposition::Declined
            && let Err(e) = ctx.git.worktree_remove(wt_path, force)
        {
            sink.on_warning(&format!(
                "Failed to remove worktree {}: {e}. Skipping deletion of branch {branch_name}.",
                wt_path.display()
            ));
            return RemoveOutcome::Failed;
        }
        // `prune` clears many worktrees at once, so the gap between "removed"
        // and "the space is back" is the widest here of anywhere. Saying so
        // costs a clause and stops `df` from contradicting us.
        match disposition {
            Disposition::Deferred => sink.on_step(&format!(
                "Removed worktree '{branch_name}' (reclaiming space in background)"
            )),
            Disposition::Reclaimed | Disposition::Declined => {
                sink.on_step(&format!("Removed worktree '{branch_name}'"));
            }
        }
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
            return RemoveOutcome::Failed;
        }
        sink.on_step(&format!("Removed worktree '{branch_name}'"));
    }

    // Post-remove hook
    run_removal_hook(HookType::PostRemove, ctx, wt_path, branch_name, sink);

    // Clean up empty parent directories
    cleanup_empty_parent_dirs(&ctx.project_root, wt_path, sink);

    // The worktree is gone — drop its seed provenance rows. Best-effort,
    // reusing the handle opened at the top of the function.
    if let Some(seeds) = seeds.as_ref() {
        seeds.delete_seeds_for_branch(branch_name);
    }
    // ...and its recorded identity, for the same reason.
    if let Some(store) = crate::core::worktree::identity_store::IdentityStore::open(&ctx.git_dir) {
        store.forget(identity_id.as_deref(), branch_name);
    }

    RemoveOutcome::Removed
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

    let outcome = remove_worktree(ctx, wt_path, branch_name, force, sink);
    if !matches!(outcome, RemoveOutcome::Removed) {
        return SinglePruneResult {
            worktree_removed: false,
            branch_deleted: false,
            skipped_dirty: matches!(outcome, RemoveOutcome::SkippedDirty),
            skipped_refined: matches!(outcome, RemoveOutcome::SkippedRefined),
        };
    }

    let branch_deleted = delete_branch(ctx.git, branch_name, sink);

    SinglePruneResult {
        worktree_removed: true,
        branch_deleted,
        skipped_dirty: false,
        skipped_refined: false,
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

// ── Background job cancellation ────────────────────────────────────────────

/// Returns true when `job` belongs to the worktree identified by `branch_slug`.
///
/// `JobInfo.worktree` carries the branch slug (e.g. "feat/x") set from
/// `ctx.branch_name` at job-launch time, NOT the filesystem path. Comparing
/// against a path here would never match.
/// How long the polite (SIGTERM) phase of a worktree-removal cancel waits
/// before escalating to SIGKILL, and how long the SIGKILL phase waits
/// before giving up. Killing must finish before the caller starts deleting
/// the directory under the job — a build that outlives the cancel keeps
/// writing files mid-delete and `git worktree remove` dies on
/// "Directory not empty" (the field failure that motivated the wait).
const CANCEL_TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
const CANCEL_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(2);
const CANCEL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

/// The next move for the cancel wait-loop, decided from elapsed time alone.
/// Pure so the escalation ladder is unit-testable without a coordinator.
#[derive(Debug, PartialEq, Eq)]
enum CancelWaitAction {
    /// Keep polling — inside the SIGTERM grace window.
    Wait,
    /// SIGTERM grace expired: send the SIGKILL escalation.
    Escalate,
    /// Both grace windows expired: warn and let the caller proceed.
    GiveUp,
}

fn next_cancel_wait_action(elapsed: std::time::Duration, escalated: bool) -> CancelWaitAction {
    if elapsed < CANCEL_TERM_GRACE {
        CancelWaitAction::Wait
    } else if !escalated {
        CancelWaitAction::Escalate
    } else if elapsed < CANCEL_TERM_GRACE + CANCEL_KILL_GRACE {
        CancelWaitAction::Wait
    } else {
        CancelWaitAction::GiveUp
    }
}

/// Cancel any running background jobs for a specific worktree and wait for
/// them to actually die.
///
/// Best-effort: if no coordinator is running or unreachable, errors are
/// silently dropped so the worktree-removal flow proceeds. Implemented as
/// a single `CancelMatching { worktree: ... }` IPC call so the coordinator
/// filters its SQLite-recorded active jobs in one round trip rather than the
/// previous list-then-cancel-per-name dance.
///
/// The wait matters as much as the signal: the caller is about to delete
/// the worktree directory, and a job that is merely *signalled* keeps
/// writing until it exits — `git worktree remove` then races the dying
/// build and fails with "Directory not empty". So when the cancel matched
/// anything, this polls the coordinator until those jobs leave `Running`,
/// escalating SIGTERM → SIGKILL after a grace period and giving up (with a
/// warning) only after the kill grace also expires.
pub(crate) fn cancel_background_jobs_for_worktree(branch_slug: &str, sink: &mut dyn ProgressSink) {
    use crate::coordinator::client::CoordinatorClient;
    use crate::coordinator::log_store::JobStatus;

    let repo_hash = match crate::core::repo_identity::compute_repo_id() {
        Ok(id) => id,
        Err(_) => return,
    };

    let mut client = match CoordinatorClient::connect(&repo_hash) {
        Ok(Some(c)) => c,
        _ => return,
    };

    let names = match client.cancel_matching(None, Some(branch_slug), None, None, None, false) {
        Ok(names) if names.is_empty() => return,
        Ok(names) => names,
        Err(e) => {
            sink.on_warning(&format!(
                "Failed to cancel background jobs for '{branch_slug}': {e}"
            ));
            return;
        }
    };
    for name in &names {
        sink.on_step(&format!("Stopped background job '{name}'"));
    }

    // Wait for the signalled jobs to leave `Running` before returning to
    // the deletion path. Poll failures end the wait rather than block the
    // removal — the cancel itself stays best-effort.
    let start = std::time::Instant::now();
    let mut escalated = false;
    loop {
        let still_running = match client.list_jobs() {
            Ok(jobs) => jobs.into_iter().any(|j| {
                j.worktree == branch_slug
                    && matches!(j.status, JobStatus::Running)
                    && names.contains(&j.name)
            }),
            Err(_) => return,
        };
        if !still_running {
            return;
        }
        match next_cancel_wait_action(start.elapsed(), escalated) {
            CancelWaitAction::Wait => std::thread::sleep(CANCEL_POLL_INTERVAL),
            CancelWaitAction::Escalate => {
                escalated = true;
                let _ = client.cancel_matching(None, Some(branch_slug), None, None, None, true);
                sink.on_step(&format!(
                    "Background jobs for '{branch_slug}' outlived the polite cancel; killing"
                ));
            }
            CancelWaitAction::GiveUp => {
                sink.on_warning(&format!(
                    "Background jobs for '{branch_slug}' are still running; the worktree \
                     removal may fail — inspect them with `{}`",
                    crate::daft_cmd("hooks jobs")
                ));
                return;
            }
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Parse `git worktree list --porcelain` into structured entries.
///
/// Thin I/O wrapper: runs the git query, then delegates the string parse to the
/// shared [`crate::core::worktree::porcelain::parse_worktree_list_porcelain`].
/// Bare entries are retained (prune skips them itself where relevant).
pub fn parse_worktree_list(git: &GitCommand) -> Result<Vec<WorktreeEntry>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    Ok(crate::core::worktree::porcelain::parse_worktree_list_porcelain(&porcelain_output))
}

/// Resolve where to cd after pruning the user's current worktree.
fn resolve_prune_cd_target(
    cd_target: PruneCdTarget,
    project_root: &Path,
    git_dir: &Path,
    remote_name: &str,
    sink: &mut dyn ProgressSink,
) -> PathBuf {
    match cd_target {
        PruneCdTarget::Root => project_root.to_path_buf(),
        PruneCdTarget::DefaultBranch => match get_default_branch_local(git_dir, remote_name) {
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
        },
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

// Worktree-scoped cancel is now expressed as a single
// `CancelMatching { worktree: branch_slug, .. }` IPC call. The matching
// logic — that we filter on the JobRow's `worktree` field (branch slug),
// not the filesystem path — lives in
// `coordinator::process::filter_matching_jobs` and is covered by tests
// in that module (`filter_matching_jobs_combines_predicates_and`).

#[cfg(test)]
mod remote_branch_tests {
    use super::{gone_branches_from_verbose_listing, observed_on_remote, parse_tracking_config};
    use std::collections::HashSet;

    // ── `branch -vv` gone-upstream reading ─────────────────────────────

    /// A commit subject cannot pass for a gone upstream (#858 by another
    /// door). Git prints `[upstream: gone]` and the subject with no delimiter
    /// between them, so a subject containing `: gone]` on a branch with no
    /// upstream at all is byte-identical to the real thing — only tracking
    /// config separates them, and a never-published branch has none.
    #[test]
    fn subject_lookalike_without_tracking_is_not_gone() {
        let tracking = parse_tracking_config(
            "branch.feat/gone.remote origin\nbranch.feat/gone.merge refs/heads/feat/gone\n",
        );
        let listing = concat!(
            "  feat/fresh abc1234 fix: gone] handling in the parser\n",
            "* feat/gone  def5678 [origin/feat/gone: gone] real one\n",
            "+ main       0123abc [origin/main] seed\n",
        );
        assert_eq!(
            gone_branches_from_verbose_listing(listing, &tracking),
            vec!["feat/gone"],
            "only the branch whose tracking config exists is gone"
        );
    }

    /// The marker column (`*` current, `+` checked out elsewhere) is skipped
    /// to find the name, and a name with no `: gone]` is never reported even
    /// when tracking exists.
    #[test]
    fn verbose_listing_markers_and_plain_lines() {
        let tracking = parse_tracking_config(concat!(
            "branch.a.remote origin\nbranch.a.merge refs/heads/a\n",
            "branch.b.remote origin\nbranch.b.merge refs/heads/b\n",
            "branch.c.remote origin\nbranch.c.merge refs/heads/c\n",
        ));
        let listing = concat!(
            "* a 1111111 [origin/a: gone] x\n",
            "+ b 2222222 [origin/b: gone] y\n",
            "  c 3333333 [origin/c] still there\n",
        );
        assert_eq!(
            gone_branches_from_verbose_listing(listing, &tracking),
            vec!["a", "b"]
        );
    }

    // ── Backfill selection (#858) ───────────────────────────────────────

    /// `git config --local --get-regexp` output for a repo with one branch of
    /// each interesting shape.
    const TRACKING: &str = concat!(
        "branch.feat/x.remote origin\n",
        "branch.feat/x.merge refs/heads/feat/x\n",
        "branch.main.remote origin\n",
        "branch.main.merge refs/heads/main\n",
        // Upstream set to a *different* name: `git branch --set-upstream-to`.
        "branch.aliased.remote origin\n",
        "branch.aliased.merge refs/heads/main\n",
        // A second remote.
        "branch.forked.remote upstream\n",
        "branch.forked.merge refs/heads/forked\n",
        // Tracked, but the remote branch is gone.
        "branch.feat/gone.remote origin\n",
        "branch.feat/gone.merge refs/heads/feat/gone\n",
    );

    fn remaining(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| (*n).to_string()).collect()
    }

    #[test]
    fn only_same_name_tracking_with_a_live_ref_is_observed() {
        let tracking = parse_tracking_config(TRACKING);
        let present = remaining(&["feat/x", "main", "forked"]);

        let observed = observed_on_remote(&tracking, "origin", &present);

        assert_eq!(observed, vec!["feat/x", "main"]);
        assert!(
            !observed.contains(&"aliased"),
            "tracking origin/main says nothing about a remote branch named `aliased` -              which is the question prune asks later"
        );
        assert!(
            !observed.contains(&"forked"),
            "another remote is not this remote"
        );
        assert!(
            !observed.contains(&"feat/gone"),
            "configured to track it is not the same as it being there now"
        );
    }

    #[test]
    fn another_remote_is_observed_when_it_is_the_one_being_pruned() {
        let tracking = parse_tracking_config(TRACKING);

        let observed = observed_on_remote(&tracking, "upstream", &remaining(&["forked"]));

        assert_eq!(observed, vec!["forked"]);
    }

    #[test]
    fn a_branch_named_gone_is_still_observed() {
        // Nothing here parses a status word, so a branch may be called
        // anything at all.
        let tracking =
            parse_tracking_config("branch.gone.remote origin\nbranch.gone.merge refs/heads/gone\n");

        let observed = observed_on_remote(&tracking, "origin", &remaining(&["gone"]));

        assert_eq!(observed, vec!["gone"]);
    }

    /// A commit subject cannot reach this decision at all any more.
    ///
    /// The predecessor read `git branch -vv`, where `[%(upstream:short)]` and
    /// `%(subject)` are printed with no delimiter — a branch with no upstream
    /// whose subject began `[origin/<its own name>]` was byte-identical to one
    /// that really tracked it, and got stamped as published. Config is the
    /// authoritative statement instead, and a subject has no way into it.
    /// End-to-end coverage lives in
    /// `tests/manual/scenarios/prune/ignores-tracking-lookalike-subject.yml`,
    /// which is the only place the whole path is observable.
    #[test]
    fn a_branch_with_no_tracking_config_is_never_observed() {
        let tracking = parse_tracking_config("");
        assert!(tracking.is_empty());

        let observed = observed_on_remote(&tracking, "origin", &remaining(&["feat/x"]));

        assert!(
            observed.is_empty(),
            "a remote branch of the same name is a coincidence until config says otherwise"
        );
    }

    #[test]
    fn a_dotted_branch_name_survives_the_key_split() {
        // `branch.release.2.x.remote` is a branch called `release.2.x`; a
        // dot-splitting parser files it under `release` and the record lands
        // on the wrong branch.
        let tracking = parse_tracking_config(
            "branch.release.2.x.remote origin\nbranch.release.2.x.merge refs/heads/release.2.x\n",
        );

        let observed = observed_on_remote(&tracking, "origin", &remaining(&["release.2.x"]));

        assert_eq!(observed, vec!["release.2.x"]);
    }

    #[test]
    fn a_remote_without_a_merge_ref_is_tracked_but_never_observed() {
        // Method 1 gates on presence here, so a half-configured branch must
        // still appear; the backfill needs the merge ref and must not.
        let tracking = parse_tracking_config("branch.half.remote origin\n");

        assert!(tracking.contains_key("half"));
        assert!(
            observed_on_remote(&tracking, "origin", &remaining(&["half"])).is_empty(),
            "no upstream ref configured means nothing states what it tracks"
        );
    }

    #[test]
    fn a_pull_request_upstream_is_tracked_but_never_observed() {
        // `branch.pr-7.merge = refs/pull/7/head` is a real shape (forge PR
        // checkout). It must keep gating Method 1 - git will report `: gone]`
        // for it - while staying out of the backfill, which is about branches.
        let tracking = parse_tracking_config(
            "branch.pr-7.remote origin\nbranch.pr-7.merge refs/pull/7/head\n",
        );

        assert!(tracking.contains_key("pr-7"));
        assert!(observed_on_remote(&tracking, "origin", &remaining(&["pr-7"])).is_empty());
    }

    use super::parse_remote_branches;

    #[test]
    fn strips_the_remote_prefix_and_drops_head() {
        // gitoxide shortens `refs/remotes/origin/HEAD` to `origin/HEAD`; git's
        // own `for-each-ref` renders a bare `origin`. Neither may survive as a
        // branch name, or a branch would be matched against the remote's
        // symbolic HEAD instead of itself.
        let refs = "origin\norigin/HEAD\norigin/master\norigin/feat/x\n";

        let branches = parse_remote_branches(refs, "origin");

        assert!(branches.contains("master"));
        assert!(
            branches.contains("feat/x"),
            "slashes must survive the strip"
        );
        assert!(!branches.contains("HEAD"));
        assert!(!branches.contains("origin"));
        assert_eq!(branches.len(), 2);
    }

    #[test]
    fn another_remotes_refs_do_not_leak_in() {
        let refs = "upstream/master\norigin/master\n";

        let branches = parse_remote_branches(refs, "origin");

        assert_eq!(branches.len(), 1);
        assert!(branches.contains("master"));
    }
}

#[cfg(test)]
mod cancel_wait_tests {
    use super::*;
    use std::time::Duration;

    /// The escalation ladder that keeps `git worktree remove` from racing
    /// a signalled-but-alive background build: polite wait → SIGKILL →
    /// bounded give-up (the removal must never hang on an unkillable job).
    #[test]
    fn cancel_wait_ladder_escalates_then_gives_up() {
        assert_eq!(
            next_cancel_wait_action(Duration::from_secs(1), false),
            CancelWaitAction::Wait
        );
        assert_eq!(
            next_cancel_wait_action(CANCEL_TERM_GRACE, false),
            CancelWaitAction::Escalate
        );
        assert_eq!(
            next_cancel_wait_action(CANCEL_TERM_GRACE + Duration::from_millis(1), true),
            CancelWaitAction::Wait
        );
        assert_eq!(
            next_cancel_wait_action(CANCEL_TERM_GRACE + CANCEL_KILL_GRACE, true),
            CancelWaitAction::GiveUp
        );
        // An early `escalated` flag with a short clock still just waits —
        // the ladder is driven by elapsed time, never by call order.
        assert_eq!(
            next_cancel_wait_action(Duration::ZERO, true),
            CancelWaitAction::Wait
        );
    }
}
