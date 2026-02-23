//! Core logic for the `git-worktree-checkout` command.
//!
//! Creates a worktree for an existing branch.

use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::{HookContext, HookType};
use crate::multi_remote::path::{calculate_worktree_path, resolve_remote_for_branch};
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
}

/// Execute the checkout operation.
pub fn execute(
    params: &CheckoutParams,
    git: &GitCommand,
    project_root: &Path,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<CheckoutResult, CheckoutError> {
    validate_branch_name(&params.branch_name)?;

    let git_dir = resolve_git_dir(git)?;
    let source_worktree = get_current_directory()?;

    let remote_for_path = resolve_remote_for_branch(
        git,
        &params.branch_name,
        params.remote.as_deref(),
        &params.multi_remote_default,
    )?;

    let worktree_path = calculate_worktree_path(
        project_root,
        &params.branch_name,
        &remote_for_path,
        params.multi_remote_enabled,
    );

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
        });
    }

    // Fetch latest changes from remote
    let fetch_failed = !fetch_branch(git, &params.remote_name, &params.branch_name, sink);

    // Check if local and/or remote branch exists
    let (local_exists, remote_exists) =
        check_branch_existence(git, &params.branch_name, &params.remote_name)?;

    if !local_exists && !remote_exists {
        return Err(CheckoutError::BranchNotFound {
            branch: params.branch_name.clone(),
            remote: params.remote_name.clone(),
            fetch_failed,
        });
    }

    let use_local_branch = if local_exists {
        sink.on_step(&format!(
            "Local branch '{}' found, using it for worktree creation",
            params.branch_name
        ));
        true
    } else {
        sink.on_step(&format!(
            "Local branch '{}' not found, will create from remote '{}/{}'",
            params.branch_name, params.remote_name, params.branch_name
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

    // Create worktree
    let worktree_result = if use_local_branch {
        git.worktree_add(&worktree_path, &params.branch_name)
    } else {
        let remote_ref = format!("{}/{}", params.remote_name, params.branch_name);
        git.worktree_add_new_branch(&worktree_path, &params.branch_name, &remote_ref)
    };

    if let Err(e) = worktree_result {
        restore_stash_on_failure(stash_created, git, sink);
        return Err(anyhow::anyhow!("Failed to create git worktree: {}", e).into());
    }

    if !worktree_path.exists() {
        return Err(anyhow::anyhow!(
            "Worktree directory was not created at '{}'",
            worktree_path.display()
        )
        .into());
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
    let (stash_applied, stash_conflict) = apply_stash(stash_created, git, sink);

    // Set upstream tracking
    let (upstream_set, upstream_skipped) = set_upstream_if_enabled(params, git, sink)?;

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

    sink.run_hook(&post_hook_ctx)?;

    Ok(CheckoutResult {
        branch_name: params.branch_name.clone(),
        worktree_path,
        already_existed: false,
        cd_target: get_current_directory()?,
        stash_applied,
        stash_conflict,
        upstream_set,
        upstream_skipped,
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Resolve the git common directory as an absolute path.
fn resolve_git_dir(git: &GitCommand) -> Result<PathBuf> {
    let git_dir_str = git.rev_parse_git_common_dir()?;
    let git_dir = PathBuf::from(&git_dir_str);
    if git_dir.is_absolute() {
        Ok(git_dir)
    } else {
        Ok(get_current_directory()?.join(git_dir))
    }
}

/// Check if a worktree already exists for the given branch name.
fn find_existing_worktree_for_branch(
    git: &GitCommand,
    branch_name: &str,
) -> Result<Option<PathBuf>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut current_path: Option<PathBuf> = None;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(worktree_path));
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if let Some(branch) = branch_ref.strip_prefix("refs/heads/") {
                if branch == branch_name {
                    return Ok(current_path.take());
                }
            }
            current_path = None;
        } else if line.is_empty() {
            current_path = None;
        }
    }

    Ok(None)
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

    general_ok || specific_ok
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
    git: &GitCommand,
    sink: &mut impl ProgressSink,
) -> Result<(bool, bool)> {
    if !params.checkout_upstream {
        sink.on_step("Skipping upstream setup (disabled in config)");
        return Ok((false, true));
    }

    let remote_branch_ref = format!("refs/remotes/{}/{}", params.remote_name, params.branch_name);
    sink.on_step(&format!(
        "Checking for remote branch '{}/{}'...",
        params.remote_name, params.branch_name
    ));

    if !git.show_ref_exists(&remote_branch_ref)? {
        sink.on_step(&format!(
            "Remote branch '{}/{}' not found, skipping upstream setup",
            params.remote_name, params.branch_name
        ));
        return Ok((false, true));
    }

    sink.on_step(&format!(
        "Setting upstream to '{}/{}'...",
        params.remote_name, params.branch_name
    ));

    if let Err(e) = git.set_upstream(&params.remote_name, &params.branch_name) {
        sink.on_warning(&format!(
            "Failed to set upstream tracking: {}. Worktree created, but upstream may need manual configuration.",
            e
        ));
        Ok((false, false))
    } else {
        sink.on_step(&format!(
            "Upstream tracking set to '{}/{}'",
            params.remote_name, params.branch_name
        ));
        Ok((true, false))
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
            if let Some(branch) = trimmed.strip_prefix(&format!("{remote_name}/")) {
                if seen.insert(branch.to_string()) {
                    names.push(branch.to_string());
                }
            }
        }
    }

    names
}
