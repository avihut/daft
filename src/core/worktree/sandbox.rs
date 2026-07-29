//! Core logic for anonymous (detached-HEAD) sandbox worktrees (#53).
//!
//! A sandbox is a worktree pinned at a commit with no branch attached: no
//! tracking, no push, a lifetime exactly as long as its directory. Two
//! entrances create them, along the verb taxonomy — go visits, start mints:
//!
//! - [`execute_visit`] — `daft go <commit-ish>`. Idempotent: the first visit
//!   materializes the *canonical* sandbox for a commit, every later visit
//!   navigates back to it (by directory name for stable spellings, by pinned
//!   commit for derived ones like `HEAD~2`, so every spelling of one commit
//!   converges on one sandbox).
//! - [`execute_create`] — `daft start --fork`. Always fresh: no idempotence,
//!   the caller has already claimed a directory, and the resulting *fork* is
//!   never matched by go's resolution — it is reachable only by name.
//!
//! Deliberately a lean sibling of [`super::checkout::execute`] rather than a
//! parameterization of it: a sandbox has no fetch, no upstream, no forge and
//! no branch, which is most of what `CheckoutParams` exists to carry. What it
//! shares — hooks, carry, shared-file links, visitor propagation, the cd
//! contract — it shares by calling the same helpers in the same order.

use crate::core::layout::{Layout, auto_gitignore_if_needed};
use crate::core::stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};
use crate::core::{HookOutcome, HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType};
use crate::multi_remote::path::{build_template_context, calculate_worktree_path};
use crate::store::models::{WorktreeIdentityRow, WorktreeKind};
use crate::utils::*;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// Hex length used for directory names derived from a commit (`HEAD~2`, raw
/// SHAs — spellings that would lie tomorrow or are unwieldy as names). Longer
/// than git's usual display abbreviation on purpose: a directory name never
/// re-resolves, so it only has to dodge sibling-directory collisions.
const DERIVED_DIRNAME_HEX: usize = 12;

/// Input parameters shared by both sandbox entrances.
pub struct SandboxParams {
    /// What the user typed (`v1.18.0`, `origin/master`, `HEAD~2`; `HEAD` for
    /// a bare `--fork`). Recorded as provenance, shown in announcements.
    pub spelling: String,
    /// The spelling resolved to a full commit OID, by the command layer.
    pub commit: String,
    /// Final directory name. The worktree's identity everywhere (list,
    /// `daft go <name>`, `daft remove <name>`).
    pub dirname: String,
    /// `Canonical` for a visit, `Fork` for a mint. Decides whether go's
    /// resolution may ever match the worktree again.
    pub kind: WorktreeKind,
    /// The fork claim loop already created the (empty) directory; skip the
    /// exists handling.
    pub path_preclaimed: bool,
    /// Carry flags, as on checkout: explicit request, explicit refusal, and
    /// the `daft.checkout.carry` default.
    pub carry: bool,
    pub no_carry: bool,
    pub checkout_carry: bool,
    /// Remote override for worktree organization (multi-remote mode).
    pub remote: Option<String>,
    /// Remote name from settings (hook context only — nothing is fetched).
    pub remote_name: String,
    pub multi_remote_enabled: bool,
    pub multi_remote_default: String,
    /// Layout for computing the worktree path, as on checkout.
    pub layout: Option<Layout>,
    /// Explicit placement override (`--at`).
    pub at_path: Option<PathBuf>,
}

/// Result of a sandbox visit or creation.
pub struct SandboxResult {
    pub dirname: String,
    pub worktree_path: PathBuf,
    /// Full OID the sandbox is pinned at.
    pub commit: String,
    /// True when an existing sandbox was found and navigated into.
    pub already_existed: bool,
    /// Directory to cd into after the operation.
    pub cd_target: PathBuf,
    pub git_dir: PathBuf,
    pub stash_applied: bool,
    pub stash_conflict: bool,
    pub post_hook_outcome: HookOutcome,
}

/// Short display form of a full OID.
pub fn short_oid(oid: &str) -> &str {
    &oid[..oid.len().min(7)]
}

/// `HEAD` of the worktree at `path`, as a full OID. `None` when unreadable —
/// callers degrade (report the requested commit, skip a guard) rather than
/// erroring on a broken worktree.
pub(crate) fn worktree_head(path: &Path) -> Option<String> {
    let out = crate::utils::git_command_at(path)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!oid.is_empty()).then_some(oid)
}

/// Directory name for a spelling that cannot name itself: a hex prefix of
/// the resolved commit.
pub fn derived_dirname(commit: &str) -> String {
    commit[..commit.len().min(DERIVED_DIRNAME_HEX)].to_string()
}

/// The directory name a spelling earns, or `None` when the name must be
/// derived from the resolved commit instead.
///
/// A spelling is *nameable* when it reads as a stable human name: only
/// `[A-Za-z0-9._/-]`, no `..`, no leading `-` or `.` or `/`, not bare hex
/// long enough to be a full object id, and not a `HEAD`-family pseudo-ref.
/// Slashes flatten to `-` (`origin/master` → `origin-master`), matching how
/// branch worktree directories are named. Everything else — `HEAD~2`,
/// `@{-1}`, `v1^{}`, a 40-hex SHA — describes a *position*, not a name: its
/// spelling names a different commit tomorrow (or is unwieldy), so the
/// directory takes [`derived_dirname`] of the commit it resolved to.
///
/// Not [`crate::core::layout::template`]'s `sanitize` (which only maps
/// slashes) nor `temp_worktree`'s slug (`/` → `--`): commit-ish spellings
/// need the reject-set, and a rejected spelling must fall through to the
/// commit-derived name rather than being force-sanitized into something that
/// lies.
pub fn sandbox_dirname(spelling: &str) -> Option<String> {
    if spelling.is_empty()
        || spelling.starts_with('-')
        || spelling.starts_with('.')
        || spelling.starts_with('/')
        || spelling.contains("..")
    {
        return None;
    }
    if !spelling
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return None;
    }
    // Pseudo-refs describe positions, never names.
    if spelling == "HEAD" || spelling.ends_with("_HEAD") {
        return None;
    }
    // Bare hex at object-id length is a position too.
    if spelling.len() >= 40 && spelling.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(spelling.replace('/', "-"))
}

/// Visit the canonical sandbox for `params.commit`: navigate to it if it
/// exists, materialize it otherwise. The `daft go <commit-ish>` engine.
pub fn execute_visit(
    params: &SandboxParams,
    git: &GitCommand,
    project_root: &Path,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<SandboxResult> {
    let git_dir = crate::core::repo::get_git_common_dir()?;

    let entries = detached_entries(git)?;
    let records = super::identity_store::read_identities(&git_dir);

    // Idempotence A — the spelling's own directory name. A detached worktree
    // already sitting at that name is the destination, unless it is a fork:
    // forks are private, and resolution must never route a visitor into one.
    // (No record at all — a foreign detached worktree parked at this name —
    // navigates too: the alternative is a confusing "directory exists" error
    // for something that is exactly the worktree the user asked for.)
    if let Some(dir) = sandbox_dirname(&params.spelling)
        && let Some(entry) = entries
            .iter()
            .find(|e| e.path.file_name().is_some_and(|f| f == dir.as_str()))
    {
        let kind = record_for(&records, &entry.path).map(|r| r.kind);
        if kind != Some(WorktreeKind::Fork) {
            return navigate(
                &dir,
                &entry.path.clone(),
                &params.commit,
                &params.spelling,
                &git_dir,
                sink,
            );
        }
    }

    // Idempotence B — the commit. Whatever the spelling (`HEAD~2` today, the
    // tag yesterday, the raw SHA in a script), one commit has at most one
    // canonical sandbox. Forks are excluded by construction (`Canonical`
    // match only); a record whose worktree is gone is dropped so the next
    // visit rebuilds cleanly.
    let mut canonical: Vec<&WorktreeIdentityRow> = records
        .values()
        .filter(|r| r.kind == WorktreeKind::Canonical)
        .filter(|r| r.pinned_commit.as_deref() == Some(params.commit.as_str()))
        .collect();
    canonical.sort_by_key(|r| std::cmp::Reverse(r.updated_at));
    for row in canonical {
        let live = entries.iter().find(|e| {
            super::identity_store::worktree_id_for(&e.path).as_deref() == Some(&row.worktree_id)
        });
        match live {
            Some(entry) => {
                return navigate(
                    &row.branch,
                    &entry.path.clone(),
                    &params.commit,
                    &params.spelling,
                    &git_dir,
                    sink,
                );
            }
            None => {
                // The worktree is gone but the record survived (removed
                // outside daft). Best-effort cleanup, then keep looking.
                if let Some(store) = super::identity_store::IdentityStore::open(&git_dir) {
                    store.forget_id(&row.worktree_id);
                }
            }
        }
    }

    create_sandbox(params, git, project_root, &git_dir, sink)
}

/// Mint a fresh sandbox at `params.commit`, unconditionally. The
/// `daft start --fork` engine: the caller owns naming and has already
/// claimed the directory.
pub fn execute_create(
    params: &SandboxParams,
    git: &GitCommand,
    project_root: &Path,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<SandboxResult> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    create_sandbox(params, git, project_root, &git_dir, sink)
}

/// The shared creation pipeline: path, plan, carry, hooks, `git worktree add
/// --detach`, provenance, links, propagation. Mirrors the order of
/// [`super::checkout::execute`]'s creation arm.
fn create_sandbox(
    params: &SandboxParams,
    git: &GitCommand,
    project_root: &Path,
    git_dir: &Path,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<SandboxResult> {
    let source_worktree =
        super::checkout_branch::resolve_source_worktree(git, git_dir, &params.remote_name, None)?;

    let worktree_path = if let Some(ref at) = params.at_path {
        at.clone()
    } else if let Some(ref layout) = params.layout {
        // Wrapped non-bare layouts: the template expects the wrapper
        // directory, one level above the clone (see checkout::execute).
        let effective_root = if layout.needs_wrapper() {
            project_root
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| project_root.to_path_buf())
        } else {
            project_root.to_path_buf()
        };
        let ctx = build_template_context(&effective_root, &params.dirname);
        layout.worktree_path(&ctx)?
    } else {
        let remote_for_path = params
            .remote
            .clone()
            .unwrap_or_else(|| params.multi_remote_default.clone());
        calculate_worktree_path(
            project_root,
            &params.dirname,
            &remote_for_path,
            params.multi_remote_enabled,
        )
    };

    sink.on_step(&format!(
        "Path: {}, Commit: {}, Project Root: {}",
        worktree_path.display(),
        short_oid(&params.commit),
        project_root.display()
    ));

    if !params.path_preclaimed && worktree_path.exists() {
        // A worktree already lives here — enter it rather than erroring on
        // the deterministic path (the identity store may have been
        // unavailable when it was created, so the lookups above can miss).
        if worktree_path.join(".git").is_file() {
            sink.on_step(&format!(
                "Sandbox directory '{}' already exists, switching to it",
                worktree_path.display()
            ));
            return navigate(
                &params.dirname,
                &worktree_path,
                &params.commit,
                &params.spelling,
                git_dir,
                sink,
            );
        }
        // An empty directory is claimable (a crashed earlier attempt);
        // anything else is in the way.
        let occupied = worktree_path.is_file()
            || std::fs::read_dir(&worktree_path).map(|mut d| d.next().is_some())?;
        if occupied {
            anyhow::bail!(
                "directory '{}' already exists and is not a worktree",
                worktree_path.display()
            );
        }
    }

    // Commit the execution plan: the path is resolved and everything left is
    // planned work.
    let should_carry = params.carry || (!params.no_carry && params.checkout_carry);
    let annotation = if params.commit.starts_with(&params.spelling) {
        format!("detached @ {}", short_oid(&params.commit))
    } else {
        format!(
            "\u{2190} {} @ {}",
            params.spelling,
            short_oid(&params.commit)
        )
    };
    let mut plan_rows = vec![
        Row::Step(StepSpec::new(StepKey::new(StageId::PreCreateHooks))),
        Row::Step(StepSpec::new(StepKey::new(StageId::CheckOut)).with_annotation(annotation)),
        Row::Step(
            StepSpec::new(StepKey::new(StageId::CreateWorktree))
                .with_annotation(super::branch_delete::display_path(&worktree_path)),
        ),
    ];
    if should_carry {
        plan_rows.push(Row::Step(StepSpec::new(StepKey::new(StageId::Carry))));
    }
    let planned_shared =
        crate::core::shared::read_shared_paths(&source_worktree).unwrap_or_default();
    // Cache paths declared with `copy:` get their own section (#387). A
    // sandbox is minted from a real source worktree just like a branch
    // worktree is, so `daft go <tag>` and `daft start --fork` have exactly
    // as much to gain from a warm cache — leaving them out would make forks
    // silently the slow path. Read ONCE, here, and reused verbatim at
    // execution below so plan and receipt cannot disagree. Revisits never
    // reach this code: both idempotence lookups and the on-disk fallback
    // return through `navigate` above, before the plan commits.
    let copy_config = crate::core::copy_paths::read_copy_config(&source_worktree);
    let planned_copy = copy_config
        .as_ref()
        .map(|c| c.paths.clone())
        .unwrap_or_default();
    // A sandbox has no branch, so the pinned commit is the whole anchor — and
    // it is the sharpest one any journey has: another worktree already sitting
    // at this exact OID holds precisely the tree being minted. `--fork -n N`
    // benefits most, since every fork after the first has a sibling at the
    // identical commit to copy from.
    let copy_source = copy_config.as_ref().map(|_| {
        crate::core::copy_source::resolve(
            git,
            &crate::core::copy_source::CopyAnchor {
                branch: None,
                commit: Some(params.commit.clone()),
            },
            &source_worktree,
            &worktree_path,
            &planned_copy,
        )
    });
    // Caches first, daft-managed links on top — see the execution ordering
    // below, which the plan face must mirror row for row.
    if let Some(source) = &copy_source {
        crate::core::copy_paths::push_copy_section(&mut plan_rows, &planned_copy, source);
    }
    crate::core::shared::push_shared_section(&mut plan_rows, &planned_shared);
    plan_rows.push(Row::Step(StepSpec::new(StepKey::new(
        StageId::PostCreateHooks,
    ))));
    sink.on_plan(PlanCommit::new(plan_rows));

    let stash_created = stash_if_carry(should_carry, git, sink)?;

    // Pre-create hook, with the branchless context: DAFT_BRANCH_NAME is
    // empty and the pinned commit carries the identity.
    let hook_ctx = HookContext::new(
        HookType::PreCreate,
        "checkout",
        project_root,
        git_dir,
        &params.remote_name,
        &source_worktree,
        &worktree_path,
        "",
    )
    .with_new_branch(false)
    .with_commit(&params.commit);

    let hook_outcome = sink.run_hook(&hook_ctx)?;
    if !hook_outcome.success && !hook_outcome.skipped {
        restore_stash_on_failure(stash_created, git, sink);
        return Err(anyhow::anyhow!("Pre-create hook failed"));
    }

    sink.on_stage(&StepKey::new(StageId::CheckOut), StageEvent::Started);
    if let Err(e) = git.worktree_add_detached(&worktree_path, &params.commit) {
        sink.on_stage(
            &StepKey::new(StageId::CheckOut),
            StageEvent::Failed {
                detail: "failed (see below)".to_string(),
            },
        );
        restore_stash_on_failure(stash_created, git, sink);
        return Err(anyhow::anyhow!("Failed to create git worktree: {e}"));
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
        anyhow::bail!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        );
    }
    // Remember what this sandbox is, while we still know: the dirname is its
    // identity, the spelling and commit its provenance. This is what names
    // the row in `daft list`, gives revisits their return trip, and lets
    // removal warn when HEAD later moves off the pin. Best-effort.
    if let Some(store) = super::identity_store::IdentityStore::open(git_dir) {
        store.record_sandbox(
            &worktree_path,
            &params.dirname,
            params.kind,
            &params.spelling,
            &params.commit,
        );
    }
    sink.on_stage(
        &StepKey::new(StageId::CreateWorktree),
        StageEvent::Completed { annotation: None },
    );

    if let Err(e) = auto_gitignore_if_needed(project_root, &worktree_path, params.layout.as_ref()) {
        sink.on_warning(&format!("Could not update .gitignore: {e}"));
    }

    sink.on_step(&format!(
        "Sandbox created at '{}' pinned at {}",
        worktree_path.display(),
        short_oid(&params.commit)
    ));
    // Absolutize and lexically clean before the chdir — a relative `--at`
    // would otherwise be re-resolved against the new worktree by everything
    // below (shared linking, the copy stage, DAFT_WORKTREE_PATH). See
    // `super::normalize_worktree_path`.
    let worktree_path = super::normalize_worktree_path(worktree_path)?;
    change_directory(&worktree_path)?;

    if stash_created {
        sink.on_stage(&StepKey::new(StageId::Carry), StageEvent::Started);
    }
    let (stash_applied, stash_conflict) = apply_stash(stash_created, git, sink);
    super::resolve_carry_row(should_carry, stash_created, stash_applied, sink);

    // Visitor-config propagation, keyed by the sandbox's dirname where a
    // branch name would go. See checkout_branch.rs for the canonical audit
    // comment covering all worktree-creating entry points.
    match crate::hooks::visitor_propagation::propagate(&source_worktree, &worktree_path) {
        Ok(result) => {
            for filename in &result.files_propagated {
                crate::log_debug!("propagated {} to new worktree", filename);
            }
            if !result.files_propagated.is_empty()
                && let Some(seeds) = crate::hooks::visitor_seeds::SeedsContext::open(git_dir)
            {
                seeds.record_seeds(&params.dirname, &worktree_path, &result.files_propagated);
            }
        }
        Err(e) => {
            sink.on_warning(&format!("visitor-config propagation failed: {e}"));
        }
    }

    // Caches, then shared links, both after propagation and before the hooks —
    // all three orderings load-bearing, all three explained in
    // `checkout_branch::execute`, which this mirrors step for step.
    // `force = false`: an entry already present at the destination is left
    // alone; only `daft warm --force` clobbers.
    // Both are Some together or neither is: the source is resolved exactly
    // when a declaration exists, so that no repository without `copy:` pays
    // for a worktree listing on the creation path.
    if let (Some(config), Some(source)) = (&copy_config, &copy_source) {
        let copy_result = crate::core::copy_paths::copy_entries(
            &source.path,
            &worktree_path,
            config,
            false,
            sink,
        );
        crate::core::copy_paths::report_copy_results(&copy_result, &planned_copy, sink);
    }

    let link_result =
        crate::core::shared::link_shared_files_on_create(&worktree_path, git_dir, project_root);
    crate::core::shared::report_link_results(&link_result, &planned_shared, sink);

    let post_hook_ctx = HookContext::new(
        HookType::PostCreate,
        "checkout",
        project_root,
        git_dir,
        &params.remote_name,
        &source_worktree,
        &worktree_path,
        "",
    )
    .with_new_branch(false)
    .with_commit(&params.commit);

    let post_hook_outcome = sink.run_hook(&post_hook_ctx)?;

    Ok(SandboxResult {
        dirname: params.dirname.clone(),
        worktree_path,
        commit: params.commit.clone(),
        already_existed: false,
        cd_target: get_current_directory()?,
        git_dir: git_dir.to_path_buf(),
        stash_applied,
        stash_conflict,
        post_hook_outcome,
    })
}

/// Enter an existing sandbox: the navigation arm shared by both idempotence
/// lookups and the on-disk fallback.
///
/// Deliberately does *not* observe into the identity store: `observe_all`
/// fills in missing records as branch identities, which would mislabel a
/// foreign detached worktree the moment someone visited it.
fn navigate(
    dirname: &str,
    worktree_path: &Path,
    commit: &str,
    spelling: &str,
    git_dir: &Path,
    sink: &mut impl ProgressSink,
) -> Result<SandboxResult> {
    sink.on_step(&format!(
        "Sandbox '{dirname}' already exists at '{}', switching to it",
        worktree_path.display()
    ));
    // Report the worktree's real position, not the caller's freshly-resolved
    // commit: the two diverge when the spelling is a moving ref
    // (`origin/master` after a fetch) or when commits were made inside the
    // sandbox. Landing the shell at X while announcing Y is the lie to avoid;
    // the sandbox itself deliberately keeps its position (never mutated on
    // revisit — it may hold uncommitted or committed work).
    let head = worktree_head(worktree_path);
    let actual = head.as_deref().unwrap_or(commit);
    if actual != commit {
        sink.on_warning(&format!(
            "Sandbox '{dirname}' sits at {}, but '{spelling}' now resolves to {} — \
             it keeps its position; remove the sandbox and revisit to take the new one",
            short_oid(actual),
            short_oid(commit)
        ));
    }
    change_directory(worktree_path)?;
    Ok(SandboxResult {
        dirname: dirname.to_string(),
        worktree_path: worktree_path.to_path_buf(),
        commit: actual.to_string(),
        already_existed: true,
        cd_target: get_current_directory()?,
        git_dir: git_dir.to_path_buf(),
        stash_applied: false,
        stash_conflict: false,
        post_hook_outcome: HookOutcome {
            success: true,
            skipped: true,
            skip_reason: None,
        },
    })
}

/// Detached, non-bare porcelain entries — the only worktrees a sandbox
/// lookup may ever match.
fn detached_entries(git: &GitCommand) -> Result<Vec<super::porcelain::WorktreeListEntry>> {
    let porcelain = git.worktree_list_porcelain()?;
    Ok(super::porcelain::parse_worktree_list_porcelain(&porcelain)
        .into_iter()
        .filter(|e| !e.is_bare && e.is_detached)
        .collect())
}

/// The identity record for the worktree at `path`, if any.
fn record_for<'a>(
    records: &'a std::collections::HashMap<String, WorktreeIdentityRow>,
    path: &Path,
) -> Option<&'a WorktreeIdentityRow> {
    let id = super::identity_store::worktree_id_for(path)?;
    records.get(&id)
}

/// Stash uncommitted changes when carry is on. Same contract as checkout's,
/// minus the `CheckoutParams` coupling.
fn stash_if_carry(
    should_carry: bool,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> Result<bool> {
    let in_worktree = git.rev_parse_is_inside_work_tree().unwrap_or(false);
    if !(should_carry && in_worktree) {
        return Ok(false);
    }
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
}

/// Restore stashed changes when sandbox creation fails.
fn restore_stash_on_failure(stash_created: bool, git: &GitCommand, sink: &mut impl ProgressSink) {
    if stash_created {
        sink.on_step("Restoring stashed changes due to worktree creation failure...");
        if let Err(pop_err) = git.stash_pop() {
            sink.on_warning(&format!(
                "Your changes are still in the stash. Run 'git stash pop' to restore them. \
                 Error: {pop_err}"
            ));
        }
    }
}

/// Apply stashed changes inside the new sandbox.
fn apply_stash(
    stash_created: bool,
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> (bool, bool) {
    if !stash_created {
        return (false, false);
    }
    sink.on_step("Applying stashed changes to worktree...");
    if let Err(e) = git.stash_pop() {
        sink.on_warning(&format!(
            "Stash could not be applied cleanly. Resolve conflicts and run 'git stash pop'. \
             Error: {e}"
        ));
        (false, true)
    } else {
        sink.on_step("Changes successfully applied to worktree");
        (true, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NullBridge;
    use serial_test::serial;

    // ── sandbox_dirname (pure) ──────────────────────────────────────────────

    #[test]
    fn stable_spellings_name_themselves() {
        assert_eq!(sandbox_dirname("v1.18.0").as_deref(), Some("v1.18.0"));
        assert_eq!(
            sandbox_dirname("origin/master").as_deref(),
            Some("origin-master")
        );
        assert_eq!(sandbox_dirname("rc_2").as_deref(), Some("rc_2"));
        // Containing HEAD is not being a HEAD pseudo-ref.
        assert_eq!(
            sandbox_dirname("release-HEAD-x").as_deref(),
            Some("release-HEAD-x")
        );
        // Hex below object-id length is a legitimate tag-like name.
        assert_eq!(sandbox_dirname("abc123d").as_deref(), Some("abc123d"));
    }

    #[test]
    fn derived_spellings_yield_none() {
        // Position expressions: their spelling names a different commit
        // tomorrow.
        assert_eq!(sandbox_dirname("HEAD"), None);
        assert_eq!(sandbox_dirname("HEAD~2"), None);
        assert_eq!(sandbox_dirname("FETCH_HEAD"), None);
        assert_eq!(sandbox_dirname("ORIG_HEAD"), None);
        assert_eq!(sandbox_dirname("@{-1}"), None);
        assert_eq!(sandbox_dirname("master^0"), None);
        assert_eq!(sandbox_dirname("v1^{}"), None);
        // Full object ids are positions too.
        assert_eq!(sandbox_dirname(&"a".repeat(40)), None);
        assert_eq!(sandbox_dirname(&"0123456789abcdef".repeat(4)), None);
    }

    #[test]
    fn hostile_spellings_yield_none() {
        assert_eq!(sandbox_dirname(""), None);
        assert_eq!(sandbox_dirname("-rf"), None);
        assert_eq!(sandbox_dirname(".hidden"), None);
        assert_eq!(sandbox_dirname("/etc/passwd"), None);
        assert_eq!(sandbox_dirname("a/../b"), None);
        assert_eq!(sandbox_dirname("with space"), None);
        assert_eq!(sandbox_dirname("semi;colon"), None);
    }

    #[test]
    fn derived_dirname_is_a_hex_prefix() {
        let oid = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(derived_dirname(oid), "0123456789ab");
        assert_eq!(short_oid(oid), "0123456");
    }

    // ── execute_visit / execute_create (real git) ───────────────────────────

    fn git(dir: &Path, args: &[&str]) {
        // Ambient user config must not steer the fixture: a global
        // `tag.gpgSign`/`commit.gpgsign` would turn `git tag v1` into an
        // annotated signing prompt ("fatal: no tag message?").
        let out = crate::utils::git_command_at(dir)
            .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgSign=false"])
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Restore the test process's cwd on drop — `execute_*` chdirs into the
    /// sandbox it creates.
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
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    /// A repo with two commits and tag `v1` on the first. Returns
    /// (tempdir, repo path, v1's full OID).
    fn repo_with_tag() -> (tempfile::TempDir, PathBuf, String) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "--allow-empty", "-q", "-m", "one"]);
        git(&repo, &["tag", "v1"]);
        git(&repo, &["commit", "--allow-empty", "-q", "-m", "two"]);
        let oid = git_out(&repo, &["rev-parse", "v1^{commit}"]);
        (tmp, repo, oid)
    }

    fn params(spelling: &str, commit: &str, kind: WorktreeKind, at: PathBuf) -> SandboxParams {
        SandboxParams {
            spelling: spelling.to_string(),
            commit: commit.to_string(),
            dirname: at.file_name().unwrap().to_string_lossy().into_owned(),
            kind,
            path_preclaimed: false,
            carry: false,
            no_carry: true,
            checkout_carry: false,
            remote: None,
            remote_name: "origin".to_string(),
            multi_remote_enabled: false,
            multi_remote_default: "origin".to_string(),
            layout: None,
            at_path: Some(at),
        }
    }

    /// The `copy:` stage (#387) is inert on the sandbox journey too — the
    /// third of three verbatim call sites, and the one reached by
    /// `daft go <tag>` and `daft start --fork`. Each site carries its own
    /// guard because a regression that edits only ONE of them has to be
    /// caught in the file it edits; without this, a stray empty anchor here
    /// would put a dim "copied paths" heading on every fork anyone ever
    /// mints and nothing would object.
    ///
    /// Pins absence and wiring only, NOT the section's plan position: with no
    /// `copy:` declared, `push_copy_section` is a no-op and the call could sit
    /// anywhere in `create_sandbox` with this test still green. The positional
    /// test needs a declaring fixture, which needs the engine. Siblings live
    /// in checkout.rs and checkout_branch.rs.
    #[test]
    #[serial]
    fn no_copy_section_when_the_source_declares_none() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        // A config that exists and parses, but says nothing about `copy:`.
        // The `shared:` key is the vacuity guard: it makes the plan under
        // test one that really did read a config and really did plan a
        // neighbouring section, so the absences below are about `copy:` and
        // not about a plan that came out empty for an unrelated reason.
        std::fs::write(repo.join("daft.yml"), "shared:\n  - .env\n").unwrap();
        std::env::set_current_dir(&repo).unwrap();

        let wt = tmp.path().join("v1");
        let mut sink = crate::core::RecordingStageSink::default();
        execute_visit(
            &params("v1", &oid, WorktreeKind::Canonical, wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut sink,
        )
        .expect("visit materializes");

        let plan = sink.plan.as_ref().expect("plan committed");
        assert!(
            !plan
                .rows
                .iter()
                .any(|r| matches!(r, Row::Group { label } if label == "copied paths")),
            "no `copy:` key, no anchor: {:?}",
            plan.rows
        );
        assert_eq!(
            plan.steps()
                .filter(|s| s.key.id == StageId::CopyPath)
                .count(),
            0,
            "no `copy:` key, no rows"
        );
        assert!(
            !sink.events.iter().any(|(k, _)| k.id == StageId::CopyPath),
            "no `copy:` key, no events: {:?}",
            sink.events
        );
        assert_eq!(
            plan.steps()
                .filter(|s| s.key.id == StageId::SharedFile)
                .count(),
            1,
            "the neighbouring shared section is present: {:?}",
            plan.rows
        );
        // The plan still ends with the post-create row — the shape the copy
        // section splices into. (A plan-shape guard, not a splice-point
        // guard: see the doc comment.)
        assert!(
            matches!(
                plan.rows.last(),
                Some(Row::Step(spec)) if spec.key.id == StageId::PostCreateHooks
            ),
            "post-create still closes the plan: {:?}",
            plan.rows
        );
    }

    /// The `copy:` section's POSITION in the sandbox plan (#387) — the same
    /// splice the two checkout journeys make, on the path `daft go <tag>` and
    /// `daft start --fork` take. See the checkout.rs twin for why the
    /// declares-nothing sibling above cannot make this claim.
    #[test]
    #[serial]
    fn copy_section_sits_before_shared_and_both_before_post_create() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::fs::write(
            repo.join("daft.yml"),
            "shared:\n  - .env\ncopy:\n  - cache\n  - node_modules\n",
        )
        .unwrap();
        std::env::set_current_dir(&repo).unwrap();

        let wt = tmp.path().join("v1");
        let mut sink = crate::core::RecordingStageSink::default();
        execute_visit(
            &params("v1", &oid, WorktreeKind::Canonical, wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut sink,
        )
        .expect("visit materializes");

        let plan = sink.plan.as_ref().expect("plan committed");
        assert_eq!(
            crate::core::worktree::plan_shape_from_copied(&plan.rows),
            [
                // A sandbox names no branch and the tag sits at no worktree's
                // tip, so both specific rungs miss and the floor — where the
                // command was run — carries the section.
                "group:copied paths from 'main'",
                "step:CopyPath",
                "step:CopyPath",
                "endgroup",
                // Caches first, daft-managed links on top: shared linking
                // creates the parents it needs, and an empty scaffold made the
                // copy report `already present` and never run.
                "group:shared files",
                "step:SharedFile",
                "endgroup",
                "step:PostCreateHooks",
            ],
            "caches land before the shared links, and both before post-create: {:?}",
            plan.rows
        );
        let scopes: Vec<_> = plan
            .steps()
            .filter(|s| s.key.id == StageId::CopyPath)
            .map(|s| s.key.scope.clone())
            .collect();
        assert_eq!(
            scopes,
            vec![Some("cache".to_string()), Some("node_modules".to_string())],
        );
    }

    #[test]
    #[serial]
    fn a_visit_materializes_a_detached_worktree_and_records_provenance() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let wt = tmp.path().join("v1");
        let result = execute_visit(
            &params("v1", &oid, WorktreeKind::Canonical, wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut NullBridge,
        )
        .expect("visit materializes");

        assert!(!result.already_existed);
        assert!(wt.join(".git").is_file(), "a linked worktree was created");
        assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), oid, "pinned at v1");
        assert_eq!(
            git_out(&wt, &["symbolic-ref", "-q", "HEAD"]),
            "",
            "HEAD is detached"
        );

        let records = super::super::identity_store::read_identities(&repo.join(".git"));
        let row = records
            .values()
            .find(|r| r.branch == "v1")
            .expect("recorded");
        assert_eq!(row.kind, WorktreeKind::Canonical);
        assert_eq!(row.source_spelling.as_deref(), Some("v1"));
        assert_eq!(row.pinned_commit.as_deref(), Some(oid.as_str()));
    }

    #[test]
    #[serial]
    fn revisiting_the_same_spelling_navigates_instead_of_creating() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let wt = tmp.path().join("v1");
        let p = params("v1", &oid, WorktreeKind::Canonical, wt.clone());
        execute_visit(&p, &GitCommand::new(true), &repo, &mut NullBridge).unwrap();
        std::env::set_current_dir(&repo).unwrap();

        let second = execute_visit(&p, &GitCommand::new(true), &repo, &mut NullBridge)
            .expect("revisit succeeds");
        assert!(second.already_existed, "the second visit navigates");
        assert_eq!(second.worktree_path.file_name().unwrap(), "v1");
    }

    /// Different spellings of one commit converge on one canonical sandbox:
    /// the tag created it, the raw SHA finds it by pinned commit.
    #[test]
    #[serial]
    fn a_derived_spelling_of_the_same_commit_converges_by_pin() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let wt = tmp.path().join("v1");
        execute_visit(
            &params("v1", &oid, WorktreeKind::Canonical, wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut NullBridge,
        )
        .unwrap();
        std::env::set_current_dir(&repo).unwrap();

        // Visit again by the full OID: dirname would be derived, but the
        // commit-keyed lookup lands in the tag's sandbox.
        let by_sha = params(
            &oid.clone(),
            &oid,
            WorktreeKind::Canonical,
            tmp.path().join(derived_dirname(&oid)),
        );
        let result = execute_visit(&by_sha, &GitCommand::new(true), &repo, &mut NullBridge)
            .expect("the sha visit resolves");
        assert!(result.already_existed);
        assert_eq!(
            result.worktree_path.file_name().unwrap(),
            "v1",
            "one commit, one canonical sandbox"
        );
    }

    /// Forks are private: a visit at the same commit must never route into
    /// one, even when it is the only worktree pinned there.
    #[test]
    #[serial]
    fn a_visit_never_enters_a_fork_pinned_at_the_same_commit() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let fork_wt = tmp.path().join("main-fork");
        execute_create(
            &params("HEAD", &oid, WorktreeKind::Fork, fork_wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut NullBridge,
        )
        .expect("fork creation succeeds");
        std::env::set_current_dir(&repo).unwrap();

        let visit_wt = tmp.path().join(derived_dirname(&oid));
        let result = execute_visit(
            &params(
                &oid.clone(),
                &oid,
                WorktreeKind::Canonical,
                visit_wt.clone(),
            ),
            &GitCommand::new(true),
            &repo,
            &mut NullBridge,
        )
        .expect("the visit creates its own sandbox");
        assert!(!result.already_existed, "the fork was not matched");
        assert_ne!(result.worktree_path, fork_wt);
    }

    /// A revisit reports the sandbox's real position, not the freshly
    /// resolved commit: when the spelling moves (a fetch advancing
    /// `origin/master`, a re-tagged release), the shell lands in the
    /// existing worktree — the result must say where that is, and the
    /// divergence must be called out.
    #[test]
    #[serial]
    fn a_revisit_after_the_spelling_moved_reports_the_sandbox_position() {
        struct WarningSink {
            warnings: Vec<String>,
        }
        impl ProgressSink for WarningSink {
            fn on_step(&mut self, _msg: &str) {}
            fn on_warning(&mut self, msg: &str) {
                self.warnings.push(msg.to_string());
            }
            fn on_debug(&mut self, _msg: &str) {}
        }
        impl HookRunner for WarningSink {
            fn run_hook(&mut self, _ctx: &HookContext) -> Result<HookOutcome> {
                Ok(HookOutcome {
                    success: true,
                    skipped: true,
                    skip_reason: None,
                })
            }
        }

        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let wt = tmp.path().join("v1");
        execute_visit(
            &params("v1", &oid, WorktreeKind::Canonical, wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut NullBridge,
        )
        .expect("first visit materializes");
        std::env::set_current_dir(&repo).unwrap();

        // The spelling moves: model a fetch/re-tag by resolving it to main's
        // tip on the second visit.
        let moved = git_out(&repo, &["rev-parse", "HEAD"]);
        assert_ne!(moved, oid);

        let mut sink = WarningSink {
            warnings: Vec::new(),
        };
        let result = execute_visit(
            &params("v1", &moved, WorktreeKind::Canonical, wt.clone()),
            &GitCommand::new(true),
            &repo,
            &mut sink,
        )
        .expect("revisit navigates");

        assert!(result.already_existed);
        assert_eq!(result.commit, oid, "reports the worktree's real position");
        assert!(
            sink.warnings
                .iter()
                .any(|w| w.contains(short_oid(&oid)) && w.contains(short_oid(&moved))),
            "the divergence is called out: {:?}",
            sink.warnings
        );
    }

    /// The hook context on the sandbox path is branchless: an empty branch
    /// name plus the pinned commit.
    #[test]
    #[serial]
    fn sandbox_hooks_get_the_branchless_context() {
        struct RecordingRunner {
            contexts: Vec<(String, Option<String>)>,
        }
        impl ProgressSink for RecordingRunner {
            fn on_step(&mut self, _msg: &str) {}
            fn on_warning(&mut self, _msg: &str) {}
            fn on_debug(&mut self, _msg: &str) {}
        }
        impl HookRunner for RecordingRunner {
            fn run_hook(&mut self, ctx: &HookContext) -> Result<HookOutcome> {
                self.contexts
                    .push((ctx.branch_name.clone(), ctx.commit.clone()));
                Ok(HookOutcome {
                    success: true,
                    skipped: true,
                    skip_reason: None,
                })
            }
        }

        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let mut sink = RecordingRunner { contexts: vec![] };
        execute_visit(
            &params("v1", &oid, WorktreeKind::Canonical, tmp.path().join("v1")),
            &GitCommand::new(true),
            &repo,
            &mut sink,
        )
        .unwrap();

        assert_eq!(sink.contexts.len(), 2, "pre- and post-create hooks fired");
        for (branch, commit) in &sink.contexts {
            assert_eq!(branch, "", "sandbox hooks see no branch");
            assert_eq!(commit.as_deref(), Some(oid.as_str()));
        }
    }

    /// `execute_create` never reuses: two creations at one commit are two
    /// worktrees.
    #[test]
    #[serial]
    fn create_is_never_idempotent() {
        let _state = crate::store::paths::IsolatedStateDir::new();
        let _cwd = CwdGuard::new();
        let (tmp, repo, oid) = repo_with_tag();
        std::env::set_current_dir(&repo).unwrap();

        let first = tmp.path().join("main-fork");
        let second = tmp.path().join("main-fork-2");
        for wt in [&first, &second] {
            std::env::set_current_dir(&repo).unwrap();
            let result = execute_create(
                &params("HEAD", &oid, WorktreeKind::Fork, wt.clone()),
                &GitCommand::new(true),
                &repo,
                &mut NullBridge,
            )
            .expect("fork creation succeeds");
            assert!(!result.already_existed);
        }
        assert!(first.join(".git").is_file());
        assert!(second.join(".git").is_file());
    }
}
