//! Core logic for the `git-worktree-checkout-branch` command.
//!
//! Creates a worktree with a new branch.

use crate::config::git::{COMMITS_AHEAD_THRESHOLD, DEFAULT_COMMIT_COUNT};
use crate::core::layout::{Layout, auto_gitignore_if_needed};
use crate::core::{HookOutcome, HookRunner, ProgressSink};
use crate::git::{GitCommand, PushIo, PushOptions};
use crate::hooks::{HookContext, HookType};
use crate::multi_remote::path::{
    build_template_context, calculate_worktree_path, resolve_remote_for_branch,
};
use crate::utils::*;
use anyhow::Result;
use std::path::{Path, PathBuf};

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
pub fn execute(
    params: &CheckoutBranchParams,
    git: &GitCommand,
    project_root: &Path,
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

    // Fetch latest changes
    if params.checkout_fetch {
        fetch_remote(git, &params.remote_name, sink);
    }

    // Determine the best checkout base (three-way branch selection)
    let checkout_base = select_checkout_base(git, &base_branch, &params.remote_name, sink)?;

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

    if let Err(e) = git.worktree_add_new_branch(
        &worktree_path,
        &params.new_branch_name,
        &checkout_base,
        no_track,
    ) {
        restore_stash_on_failure(stash_created, carry_source.as_deref(), git, sink);
        anyhow::bail!("Failed to create git worktree: {}", e);
    }

    if !worktree_path.exists() {
        anyhow::bail!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        );
    }

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
    let (stash_applied, stash_conflict) = apply_stash(stash_created, git, sink);

    // Push and set upstream
    let (push_set, push_skipped) = push_if_enabled(params, git, &worktree_path, sink);

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
    crate::core::shared::link_shared_files_on_create(&worktree_path, &git_dir, project_root);

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
fn fetch_remote(git: &GitCommand, remote_name: &str, sink: &mut impl ProgressSink) {
    sink.on_step(&format!(
        "Fetching latest changes from remote '{remote_name}'..."
    ));
    if let Err(e) = git.fetch(remote_name, false) {
        sink.on_warning(&format!("Failed to fetch from remote '{remote_name}': {e}"));
    }

    sink.on_step("Setting up remote tracking branches...");
    if let Err(e) = git.fetch_refspec(
        remote_name,
        &format!("+refs/heads/*:refs/remotes/{remote_name}/*"),
    ) {
        sink.on_warning(&format!("Failed to set up remote tracking branches: {e}"));
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
/// in the branch being pushed.
fn push_if_enabled(
    params: &CheckoutBranchParams,
    git: &GitCommand,
    worktree_path: &Path,
    sink: &mut impl ProgressSink,
) -> (bool, bool) {
    if !params.checkout_push {
        sink.on_step("Skipping push (disabled in config)");
        return (false, true);
    }

    sink.on_step(&format!(
        "Pushing and setting upstream to '{}/{}'...",
        params.remote_name, params.new_branch_name
    ));

    let result = git
        .push_set_upstream_from(
            &params.remote_name,
            &params.new_branch_name,
            worktree_path,
            &PushOptions::default(),
        )
        .and_then(PushIo::into_result);

    if let Err(e) = result {
        sink.on_warning(&format!(
            "Could not push '{}' to '{}': {}. The worktree is ready locally. Push manually with: git push -u {} {}",
            params.new_branch_name, params.remote_name, e,
            params.remote_name, params.new_branch_name
        ));
        (false, false)
    } else {
        sink.on_step(&format!(
            "Push to '{}' and upstream tracking set successfully",
            params.remote_name
        ));
        (true, false)
    }
}
