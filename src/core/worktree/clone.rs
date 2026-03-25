//! Core logic for the `git-worktree-clone` command.
//!
//! Clones a repository into a worktree-based directory structure.

use crate::core::layout::Layout;
use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::hooks::TrustDatabase;
use crate::remote::{get_default_branch_remote, get_remote_branches, is_remote_empty};
use crate::resolve_initial_branch;
use crate::utils::*;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Input parameters for the bare clone phase.
///
/// Contains everything needed to clone a repository as bare. Layout is NOT
/// included — it's decided after the bare clone, once daft.yml can be read.
pub struct BareCloneParams {
    pub repository_url: String,
    pub branch: Option<String>,
    pub no_checkout: bool,
    pub all_branches: bool,
    pub remote: Option<String>,
    pub remote_name: String,
    pub multi_remote_enabled: bool,
    pub multi_remote_default: String,
    pub checkout_upstream: bool,
    pub use_gitoxide: bool,
}

/// Result of the bare clone phase.
///
/// Contains all the information needed by subsequent phases to set up
/// worktrees (bare layout) or convert to a regular repo (non-bare layout).
pub struct BareCloneResult {
    pub repo_name: String,
    pub parent_dir: PathBuf,
    pub git_dir: PathBuf,
    pub default_branch: String,
    pub target_branch: String,
    pub branch_exists: bool,
    pub is_empty: bool,
    pub remote_name: String,
    pub repository_url: String,
}

/// Result of a clone operation.
pub struct CloneResult {
    pub repo_name: String,
    pub target_branch: String,
    pub default_branch: String,
    pub parent_dir: PathBuf,
    pub git_dir: PathBuf,
    pub remote_name: String,
    pub repository_url: String,
    /// Where to cd into after the operation (None for bare/no-checkout).
    pub cd_target: Option<PathBuf>,
    /// The worktree that was created (for hook context).
    pub worktree_dir: Option<PathBuf>,
    /// True if no worktree was created because the branch doesn't exist.
    pub branch_not_found: bool,
    /// True if the repository was empty (no commits).
    pub is_empty: bool,
    /// True if in no-checkout mode.
    pub no_checkout: bool,
}

/// Store the resolved layout in repos.json.
/// Store the layout for a freshly cloned repo, resetting any prior registry
/// entry. A re-clone means the old config is stale — start fresh.
fn store_layout(git_dir: &Path, layout: &Layout, progress: &mut dyn ProgressSink) {
    match TrustDatabase::load() {
        Ok(mut db) => {
            // Reset prior entries: a re-clone into the same path means the old
            // config is stale and should not carry over.
            db.reset_repo(git_dir);

            db.set_layout(git_dir, layout.name.clone());
            if let Err(e) = db.save() {
                progress.on_warning(&format!("Could not save layout to repos.json: {e}"));
            }
        }
        Err(e) => {
            progress.on_warning(&format!("Could not load repos.json to save layout: {e}"));
        }
    }
}

/// Phase 1: Clone a repository as bare into `<repo>/.git`.
///
/// Every clone starts here regardless of the final layout. After this
/// phase the caller reads daft.yml and resolves the layout, then calls
/// either `setup_bare_worktrees()` or `unbare_and_checkout()`.
///
/// On return the process cwd is `parent_dir` (the repo directory).
pub fn clone_bare_phase(
    params: &BareCloneParams,
    progress: &mut dyn ProgressSink,
) -> Result<BareCloneResult> {
    let repo_name = crate::extract_repo_name(&params.repository_url)?;
    progress.on_step(&format!("Repository name detected: '{repo_name}'"));

    let (default_branch, target_branch, branch_exists, is_empty) =
        detect_branches(params, progress)?;

    let parent_dir = PathBuf::from(&repo_name);

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    progress.on_step("Creating repository directory...");
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    progress.on_step(&format!(
        "Cloning bare repository into './{}'...",
        git_dir.display()
    ));

    if let Err(e) = git.clone_bare(&params.repository_url, &git_dir) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git clone failed"));
    }

    let git_dir = git_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize git directory: {}",
            git_dir.display()
        )
    })?;

    progress.on_step(&format!(
        "Changing directory to './{}'",
        parent_dir.display()
    ));
    change_directory(&parent_dir)?;

    // Set up fetch refspec for bare repo
    progress.on_step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&params.remote_name) {
        progress.on_warning(&format!("Could not set fetch refspec: {e}"));
    }

    // Set multi-remote config if --remote was provided
    if params.remote.is_some() {
        progress.on_step("Enabling multi-remote mode for this repository...");
        crate::multi_remote::config::set_multi_remote_enabled(&git, true)?;
        let remote_for_path = params
            .remote
            .clone()
            .unwrap_or_else(|| params.multi_remote_default.clone());
        crate::multi_remote::config::set_multi_remote_default(&git, &remote_for_path)?;
    }

    Ok(BareCloneResult {
        repo_name,
        parent_dir,
        git_dir,
        default_branch,
        target_branch,
        branch_exists,
        is_empty,
        remote_name: params.remote_name.clone(),
        repository_url: params.repository_url.clone(),
    })
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Detect the default branch, target branch, and whether the repo is empty.
fn detect_branches(
    params: &BareCloneParams,
    progress: &mut dyn ProgressSink,
) -> Result<(String, String, bool, bool)> {
    match get_default_branch_remote(&params.repository_url, params.use_gitoxide) {
        Ok(default_branch) => {
            progress.on_step(&format!("Default branch detected: '{default_branch}'"));

            let (target_branch, branch_exists) = if let Some(ref specified) = params.branch {
                progress.on_step(&format!(
                    "Checking if branch '{specified}' exists on remote..."
                ));
                let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);
                let exists = git
                    .ls_remote_branch_exists(&params.repository_url, specified)
                    .unwrap_or(false);
                if exists {
                    progress.on_step(&format!("Branch '{specified}' found on remote"));
                } else {
                    progress.on_warning(&format!("Branch '{specified}' does not exist on remote"));
                }
                (specified.clone(), exists)
            } else {
                (default_branch.clone(), true)
            };

            Ok((default_branch, target_branch, branch_exists, false))
        }
        Err(e) => {
            if is_remote_empty(&params.repository_url, params.use_gitoxide).unwrap_or(false) {
                let local_default = resolve_initial_branch(&params.branch);
                progress.on_step(&format!(
                    "Empty repository detected, using branch: '{local_default}'"
                ));
                Ok((local_default.clone(), local_default, false, true))
            } else {
                Err(e.context("Failed to determine default branch"))
            }
        }
    }
}

fn create_single_worktree(
    git: &GitCommand,
    branch: &str,
    worktree_path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step(&format!(
        "Creating initial worktree for branch '{}' at '{}'...",
        branch,
        worktree_path.display()
    ));
    ensure_parent_dir(worktree_path)?;
    git.worktree_add(worktree_path, branch)
        .context("Failed to create initial worktree")?;
    Ok(())
}

fn create_orphan_worktree(
    git: &GitCommand,
    branch: &str,
    worktree_path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step(&format!(
        "Creating initial worktree for empty repository at '{}'...",
        worktree_path.display()
    ));
    ensure_parent_dir(worktree_path)?;
    git.worktree_add_orphan(worktree_path, branch)
        .context("Failed to create initial worktree for empty repository")?;
    Ok(())
}

fn create_all_worktrees(
    git: &GitCommand,
    remote_name: &str,
    use_multi_remote: bool,
    remote_for_path: &str,
    use_gitoxide: bool,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    progress.on_step("Fetching all remote branches...");
    git.fetch(remote_name, false)?;

    let remote_branches =
        get_remote_branches(remote_name, use_gitoxide).context("Failed to get remote branches")?;

    if remote_branches.is_empty() {
        anyhow::bail!("No remote branches found");
    }

    for branch in &remote_branches {
        let worktree_path = if use_multi_remote {
            PathBuf::from(remote_for_path).join(branch)
        } else {
            PathBuf::from(branch)
        };

        progress.on_step(&format!(
            "Creating worktree for branch '{}' at '{}'...",
            branch,
            worktree_path.display()
        ));

        if let Some(parent) = worktree_path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    progress.on_warning(&format!("creating directory '{}': {e}", parent.display()));
                    continue;
                }
            }
        }

        if let Err(e) = git.worktree_add(&worktree_path, branch) {
            progress.on_warning(&format!("creating worktree for branch '{branch}': {e}"));
            continue;
        }
    }

    Ok(())
}

/// Ensure parent directory exists (for multi-remote mode).
fn ensure_parent_dir(worktree_path: &Path) -> Result<()> {
    if let Some(parent) = worktree_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }
    }
    Ok(())
}

/// Phase 4a: Set up worktrees for a bare layout.
///
/// Creates worktrees via `git worktree add`, sets up tracking, and returns
/// the final `CloneResult`. Assumes cwd is `bare_result.parent_dir`.
pub fn setup_bare_worktrees(
    bare_result: &BareCloneResult,
    params: &BareCloneParams,
    layout: &crate::core::layout::Layout,
    progress: &mut dyn ProgressSink,
) -> Result<CloneResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);
    let use_multi_remote = params.remote.is_some() || params.multi_remote_enabled;
    let remote_for_path = params
        .remote
        .clone()
        .unwrap_or_else(|| params.multi_remote_default.clone());

    // Store layout in repos.json
    store_layout(&bare_result.git_dir, layout, progress);

    let should_create_worktree =
        !params.no_checkout && (bare_result.branch_exists || bare_result.is_empty);

    if should_create_worktree {
        let relative_worktree_path = if use_multi_remote {
            PathBuf::from(&remote_for_path).join(&bare_result.target_branch)
        } else {
            PathBuf::from(&bare_result.target_branch)
        };

        if params.all_branches {
            if bare_result.is_empty {
                anyhow::bail!(
                    "Cannot use --all-branches with an empty repository (no branches exist)"
                );
            }
            create_all_worktrees(
                &git,
                &bare_result.remote_name,
                use_multi_remote,
                &remote_for_path,
                params.use_gitoxide,
                progress,
            )?;
        } else if bare_result.is_empty {
            create_orphan_worktree(
                &git,
                &bare_result.target_branch,
                &relative_worktree_path,
                progress,
            )?;
        } else {
            create_single_worktree(
                &git,
                &bare_result.target_branch,
                &relative_worktree_path,
                progress,
            )?;
        }

        progress.on_step(&format!(
            "Changing directory to worktree: './{}'",
            relative_worktree_path.display()
        ));

        if let Err(e) = change_directory(&relative_worktree_path) {
            change_directory(bare_result.parent_dir.parent().unwrap_or(Path::new("."))).ok();
            return Err(e);
        }

        if !bare_result.is_empty {
            setup_tracking(
                &git,
                &bare_result.remote_name,
                &bare_result.target_branch,
                params.checkout_upstream,
                progress,
            );
        }

        let current_dir = get_current_directory()?;

        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir.clone()),
            worktree_dir: Some(current_dir),
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else if !params.no_checkout && !bare_result.branch_exists {
        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: None,
            branch_not_found: true,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else {
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: None,
            worktree_dir: None,
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: true,
        })
    }
}

/// Phase 4b: Convert a fresh bare clone to a regular (non-bare) repo.
///
/// For a fresh bare clone into `<repo>/.git`, the structure is already
/// correct for a regular repo. Just set `core.bare=false` and check out.
pub fn unbare_and_checkout(
    bare_result: &BareCloneResult,
    params: &BareCloneParams,
    layout: &crate::core::layout::Layout,
    progress: &mut dyn ProgressSink,
) -> Result<CloneResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    // Store layout in repos.json
    store_layout(&bare_result.git_dir, layout, progress);

    progress.on_step("Converting to non-bare repository...");
    git.config_set("core.bare", "false")
        .context("Failed to set core.bare to false")?;

    if !params.no_checkout && (bare_result.branch_exists || bare_result.is_empty) {
        if !bare_result.is_empty {
            progress.on_step("Checking out working tree...");
            git.checkout(&bare_result.target_branch)
                .context("Failed to check out working tree")?;

            // Fetch and set up tracking
            setup_tracking(
                &git,
                &bare_result.remote_name,
                &bare_result.target_branch,
                params.checkout_upstream,
                progress,
            );
        }

        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir.clone()),
            worktree_dir: Some(current_dir),
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else if !params.no_checkout && !bare_result.branch_exists {
        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: None,
            branch_not_found: true,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else {
        // --no-checkout: bare→non-bare conversion done, no checkout
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: None,
            worktree_dir: None,
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: true,
        })
    }
}

/// Phase 4c: Convert a fresh bare clone to a wrapped non-bare layout.
///
/// For `contained-classic`, the directory structure after clone_bare_phase is:
///   `<repo>/.git`  (bare)
///
/// This function moves `.git` into a subdirectory named after the default
/// branch, un-bares it, and checks out the working tree:
///   `<repo>/<default_branch>/.git`  (regular clone)
///
/// Additional worktrees are added as siblings of the default branch directory.
pub fn setup_wrapped_nonbare(
    bare_result: &BareCloneResult,
    params: &BareCloneParams,
    layout: &crate::core::layout::Layout,
    progress: &mut dyn ProgressSink,
) -> Result<CloneResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    // After clone_bare_phase, CWD is already inside parent_dir.
    // Use the branch name directly (relative to CWD) to avoid double nesting.
    let branch_dir = PathBuf::from(&bare_result.target_branch);
    let new_git_dir = branch_dir.join(".git");

    progress.on_step(&format!(
        "Moving repository into '{}/{}'...",
        bare_result.repo_name, bare_result.target_branch
    ));

    // Create the default branch subdirectory
    std::fs::create_dir_all(&branch_dir).context("Failed to create default branch subdirectory")?;

    // Move .git from the wrapper root into the branch subdirectory
    std::fs::rename(&bare_result.git_dir, &new_git_dir)
        .context("Failed to move .git into branch subdirectory")?;

    // CD into the branch directory (the new repo root)
    change_directory(&branch_dir)?;

    // Now that we're inside the branch dir, canonicalize the git_dir
    let canonical_git_dir = PathBuf::from(".git")
        .canonicalize()
        .context("Failed to canonicalize new git directory")?;

    // Un-bare and check out
    progress.on_step("Converting to non-bare repository...");
    git.config_set("core.bare", "false")
        .context("Failed to set core.bare to false")?;

    // Store layout in repos.json using the canonicalized git_dir path
    store_layout(&canonical_git_dir, layout, progress);

    if !params.no_checkout && (bare_result.branch_exists || bare_result.is_empty) {
        if !bare_result.is_empty {
            progress.on_step("Checking out working tree...");
            git.checkout(&bare_result.target_branch)
                .context("Failed to check out working tree")?;

            setup_tracking(
                &git,
                &bare_result.remote_name,
                &bare_result.target_branch,
                params.checkout_upstream,
                progress,
            );
        }

        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: canonical_git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir.clone()),
            worktree_dir: Some(current_dir),
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else if !params.no_checkout && !bare_result.branch_exists {
        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: canonical_git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: None,
            branch_not_found: true,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else {
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: canonical_git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: None,
            worktree_dir: None,
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: true,
        })
    }
}

/// Create a satellite worktree for an additional branch.
///
/// Used by multi-branch clone to create worktrees beyond the base.
/// The caller must ensure CWD is `parent_dir` (the repo directory).
pub fn create_satellite_worktree(
    branch: &str,
    worktree_path: &Path,
    remote_name: &str,
    checkout_upstream: bool,
    use_gitoxide: bool,
    progress: &mut dyn ProgressSink,
) -> Result<PathBuf> {
    progress.on_step(&format!("Creating worktree for '{}'", branch));

    let git = GitCommand::new(false).with_gitoxide(use_gitoxide);

    ensure_parent_dir(worktree_path)?;
    git.worktree_add(worktree_path, branch)
        .with_context(|| format!("Failed to create worktree for branch '{branch}'"))?;

    // Set up upstream tracking
    if checkout_upstream {
        if let Err(e) = git.set_upstream(remote_name, branch) {
            progress.on_warning(&format!("Could not set upstream for '{}': {e}", branch));
        }
    }

    Ok(worktree_path.to_path_buf())
}

/// Set up remote tracking refs and upstream after worktree creation.
fn setup_tracking(
    git: &GitCommand,
    remote_name: &str,
    target_branch: &str,
    checkout_upstream: bool,
    progress: &mut dyn ProgressSink,
) {
    progress.on_step(&format!(
        "Fetching from '{remote_name}' to set up remote tracking..."
    ));
    if let Err(e) = git.fetch(remote_name, false) {
        progress.on_warning(&format!("Could not fetch from remote: {e}"));
    }

    if let Err(e) = git.remote_set_head_auto(remote_name) {
        progress.on_warning(&format!("Could not set remote HEAD: {e}"));
    }

    if checkout_upstream {
        progress.on_step(&format!(
            "Setting upstream to '{remote_name}/{target_branch}'..."
        ));
        if let Err(e) = git.set_upstream(remote_name, target_branch) {
            progress.on_warning(&format!(
                "Could not set upstream tracking: {e}. You may need to set it manually."
            ));
        }
    } else {
        progress.on_step("Skipping upstream setup (disabled in config)");
    }
}
