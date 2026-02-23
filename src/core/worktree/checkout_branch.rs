//! Core logic for the `git-worktree-checkout-branch` command.
//!
//! Creates a worktree with a new branch.

use crate::config::git::{COMMITS_AHEAD_THRESHOLD, DEFAULT_COMMIT_COUNT};
use crate::core::{HookOutcome, HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType};
use crate::multi_remote::path::{calculate_worktree_path, resolve_remote_for_branch};
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
    let source_worktree = get_current_directory()?;

    let remote_for_path = resolve_remote_for_branch(
        git,
        &params.new_branch_name,
        params.remote.as_deref(),
        &params.multi_remote_default,
    )?;

    let worktree_path = calculate_worktree_path(
        project_root,
        &params.new_branch_name,
        &remote_for_path,
        params.multi_remote_enabled,
    );

    // Fetch latest changes
    fetch_remote(git, &params.remote_name, sink);

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

    if let Err(e) =
        git.worktree_add_new_branch(&worktree_path, &params.new_branch_name, &checkout_base)
    {
        restore_stash_on_failure(stash_created, carry_source.as_deref(), git, sink);
        anyhow::bail!("Failed to create git worktree: {}", e);
    }

    if !worktree_path.exists() {
        anyhow::bail!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        );
    }

    sink.on_step(&format!(
        "Changing directory to worktree: {}",
        worktree_path.display()
    ));
    change_directory(&worktree_path)?;

    // Apply stashed changes
    let (stash_applied, stash_conflict) = apply_stash(stash_created, git, sink);

    // Push and set upstream
    let (push_set, push_skipped) = push_if_enabled(params, git, sink);

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
fn push_if_enabled(
    params: &CheckoutBranchParams,
    git: &GitCommand,
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

    if let Err(e) = git.push_set_upstream(&params.remote_name, &params.new_branch_name) {
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
