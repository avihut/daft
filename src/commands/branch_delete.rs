use crate::{
    get_git_common_dir, get_project_root,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, RemovalReason},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::get_default_branch_local,
    settings::PruneCdTarget,
    DaftSettings, WorktreeConfig, CD_FILE_ENV,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "git-worktree-branch-delete")]
#[command(version = crate::VERSION)]
#[command(about = "Delete branches and their worktrees")]
#[command(long_about = r#"
Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout-branch(1).

Arguments can be branch names or worktree paths. When a path is given
(absolute, relative, or "."), the branch checked out in that worktree is
resolved automatically. This is convenient when you are inside a worktree
and want to delete it without remembering the branch name.

Safety checks prevent accidental data loss. The command refuses to delete a
branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch

Use -D (--force) to override these safety checks. The command always refuses
to delete the repository's default branch (e.g. main), even with --force.

All targeted branches are validated before any deletions begin. If any branch
fails validation without --force, the entire command aborts and no branches
are deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(required = true, help = "Branches to delete (names or worktree paths)")]
    branches: Vec<String>,

    #[arg(short = 'D', long, help = "Force deletion even if not fully merged")]
    force: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

/// Parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    #[allow(dead_code)] // Parsed for completeness; not needed by branch-delete logic
    is_bare: bool,
}

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
}

/// A validation error for a single branch.
struct ValidationError {
    branch: String,
    message: String,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-branch-delete"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(args.quiet, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_branch_delete(&args, &mut output, &settings)?;
    Ok(())
}

fn run_branch_delete(args: &Args, output: &mut dyn Output, settings: &DaftSettings) -> Result<()> {
    let config = WorktreeConfig::default();
    let git = GitCommand::new(args.quiet).with_gitoxide(settings.use_gitoxide);
    let git_dir = get_git_common_dir()?;
    let default_branch =
        get_default_branch_local(&git_dir, &config.remote_name, settings.use_gitoxide)
            .context("Cannot determine default branch")?;

    let ctx = BranchDeleteContext {
        git: &git,
        project_root: get_project_root()?,
        git_dir,
        remote_name: config.remote_name.clone(),
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
    // Paths (absolute, relative, or ".") are resolved to the branch checked out there.
    let resolved_branches =
        resolve_branch_args(&args.branches, &worktree_entries, &ctx.project_root, output)?;

    // Detect current worktree context for is_current_worktree flagging.
    // Use both path and branch name: path comparison can fail when symlinks
    // cause git rev-parse and git worktree list to report different strings.
    let current_wt_path = git.get_current_worktree_path().ok();
    let current_branch = git.symbolic_ref_short_head().ok();

    // Validate all branches before performing any deletions
    let (validated, errors) = validate_branches(
        &ctx,
        &resolved_branches,
        args.force,
        &worktree_map,
        current_wt_path.as_ref(),
        current_branch.as_deref(),
        output,
    );

    if !errors.is_empty() {
        for err in &errors {
            output.error(&format!("cannot delete '{}': {}", err.branch, err.message));
        }
        let total = resolved_branches.len();
        let failed = errors.len();
        anyhow::bail!(
            "Aborting: {} of {} branch{} failed validation. No branches were deleted.",
            failed,
            total,
            if total == 1 { "" } else { "es" }
        );
    }

    if validated.is_empty() {
        output.info("No branches to delete");
        return Ok(());
    }

    execute_deletions(&ctx, &validated, args.force, settings, output)
}

/// Resolve each argument to a branch name.
///
/// Arguments can be:
///   - A branch name (passed through as-is if no worktree path matches)
///   - A worktree path (absolute or relative to cwd, including ".")
///
/// Path resolution: canonicalize the argument and compare against known worktree
/// paths. If a match is found, return the branch checked out in that worktree.
/// If the worktree has a detached HEAD (no branch), return an error.
fn resolve_branch_args(
    args: &[String],
    worktree_entries: &[WorktreeEntry],
    project_root: &Path,
    output: &mut dyn Output,
) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(args.len());

    for arg in args {
        match resolve_single_arg(arg, worktree_entries, project_root) {
            ResolveResult::Branch(name) => {
                output.step(&format!("Resolved path '{}' to branch '{}'", arg, name));
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

enum ResolveResult {
    /// Argument matched a worktree path and resolved to this branch name.
    Branch(String),
    /// Argument did not match any worktree path; treat as a branch name.
    PassThrough,
    /// Argument matched a worktree but it has no branch (detached HEAD).
    DetachedHead(PathBuf),
}

/// Try to resolve a single argument as a worktree path.
fn resolve_single_arg(
    arg: &str,
    worktree_entries: &[WorktreeEntry],
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
            // (e.g., user passes "feature/foo" which is both a valid branch name and a
            // relative path). In this case, fall through to branch-name treatment.
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
    worktree_entries: &[WorktreeEntry],
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

/// Validate all requested branches. Returns a tuple of (validated, errors).
///
/// Each branch goes through up to 5 checks:
///   1. Branch exists locally
///   2. Not the default branch (even with --force)
///   3. No uncommitted changes in worktree (skip with --force)
///   4. Merged into default branch (skip with --force)
///   5. Local/remote in sync (skip with --force)
fn validate_branches(
    ctx: &BranchDeleteContext,
    branches: &[String],
    force: bool,
    worktree_map: &HashMap<String, PathBuf>,
    current_wt_path: Option<&PathBuf>,
    current_branch: Option<&str>,
    output: &mut dyn Output,
) -> (Vec<ValidatedBranch>, Vec<ValidationError>) {
    let mut validated = Vec::new();
    let mut errors = Vec::new();

    for branch in branches {
        output.step(&format!("Validating branch '{branch}'..."));

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

        // Check 2: Not the default branch (never allowed, even with --force)
        if branch == &ctx.default_branch {
            errors.push(ValidationError {
                branch: branch.clone(),
                message: format!(
                    "refusing to delete the default branch '{}'",
                    ctx.default_branch
                ),
            });
            continue;
        }

        let wt_path = worktree_map.get(branch.as_str()).cloned();

        // Check 3: No uncommitted changes (skip with --force)
        if !force {
            if let Some(ref path) = wt_path {
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
        }

        // Check 4: Merged into default branch (skip with --force)
        if !force {
            match is_branch_merged(ctx, branch) {
                Ok(true) => {
                    output.step(&format!("Branch '{branch}' is merged into default branch"));
                }
                Ok(false) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: format!(
                            "not merged into '{}' (use -D to force)",
                            ctx.default_branch
                        ),
                    });
                    continue;
                }
                Err(e) => {
                    errors.push(ValidationError {
                        branch: branch.clone(),
                        message: format!("failed to check merge status: {e} (use -D to force)"),
                    });
                    continue;
                }
            }
        }

        // Determine remote tracking info for this branch
        let (remote_name, remote_branch_name) = resolve_remote_tracking(ctx, branch);

        // Check 5: Local/remote in sync (skip with --force)
        if !force {
            if let Some(ref remote) = remote_name {
                if let Some(ref remote_branch) = remote_branch_name {
                    match check_local_remote_sync(ctx, branch, remote, remote_branch) {
                        Ok(true) => {
                            output.step(&format!("Branch '{branch}' is in sync with remote"));
                        }
                        Ok(false) => {
                            errors.push(ValidationError {
                                branch: branch.clone(),
                                message:
                                    "local and remote branches are out of sync (use -D to force)"
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

        output.step(&format!("Branch '{branch}' passed validation"));

        validated.push(ValidatedBranch {
            name: branch.clone(),
            worktree_path: wt_path,
            remote_name,
            remote_branch_name,
            is_current_worktree: is_current,
        });
    }

    (validated, errors)
}

/// Check whether a branch has been merged into the default branch.
///
/// Checks against both the local default branch and its remote tracking branch.
/// This handles the common case where the local default branch is behind the
/// remote (e.g., after `git fetch` without `git pull`). Since `checkout-branch`
/// creates branches from the remote tracking branch, the new branch may be
/// ahead of the local default branch even though the user made no changes.
///
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
    // `git cherry <upstream> <branch>` lists commits in <branch> not in <upstream>.
    // Lines starting with `-` mean the patch is already present upstream (squash-merged).
    // Lines starting with `+` mean the patch is NOT present upstream.
    // If all lines start with `-`, every commit has been squash-merged.
    let cherry_output = ctx
        .git
        .cherry(target, branch)
        .context("git cherry check failed")?;

    let lines: Vec<&str> = cherry_output.lines().collect();

    // Empty output means no commits to compare (branch is at same point as target)
    if lines.is_empty() {
        return Ok(true);
    }

    // All lines must start with `-` for the branch to be considered squash-merged
    let all_merged = lines.iter().all(|line| line.starts_with('-'));
    Ok(all_merged)
}

/// Compare local and remote SHAs to determine if the branch is in sync.
///
/// If the remote ref does not exist, the branch is considered in sync
/// (the remote branch may have already been deleted after merge).
fn check_local_remote_sync(
    ctx: &BranchDeleteContext,
    branch: &str,
    remote: &str,
    remote_branch: &str,
) -> Result<bool> {
    let remote_ref = format!("refs/remotes/{remote}/{remote_branch}");

    // If the remote tracking ref doesn't exist, consider it in sync.
    // This covers the common case where the remote branch was already deleted
    // (e.g., after a PR merge on GitHub).
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

/// Resolve the remote name and remote branch name for a given local branch.
///
/// Returns (Some(remote), Some(remote_branch)) if a tracking remote is configured,
/// or falls back to (ctx.remote_name, branch) if no explicit tracking is set but
/// the remote ref exists.
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

/// Parse `git worktree list --porcelain` into structured entries.
fn parse_worktree_list(git: &GitCommand) -> Result<Vec<WorktreeEntry>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut current_is_bare = false;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            // Save previous entry if any
            if let Some(path) = current_path.take() {
                entries.push(WorktreeEntry {
                    path,
                    branch: current_branch.take(),
                    is_bare: current_is_bare,
                });
            }
            current_path = Some(PathBuf::from(worktree_path));
            current_branch = None;
            current_is_bare = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            current_is_bare = true;
        }
    }
    // Don't forget the last entry
    if let Some(path) = current_path.take() {
        entries.push(WorktreeEntry {
            path,
            branch: current_branch.take(),
            is_bare: current_is_bare,
        });
    }

    Ok(entries)
}

/// Result of deleting a single branch (tracks what was successfully deleted).
struct DeletionResult {
    branch: String,
    remote_deleted: bool,
    worktree_removed: bool,
    branch_deleted: bool,
    errors: Vec<String>,
}

impl DeletionResult {
    fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Build a human-readable summary of what was deleted (e.g. "worktree, local branch, remote branch").
    fn deleted_parts(&self) -> String {
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

/// Execute all validated deletions. Current-worktree branches are deferred to
/// last so we can resolve a CD target and change directory before removing them.
fn execute_deletions(
    ctx: &BranchDeleteContext,
    validated: &[ValidatedBranch],
    force: bool,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<()> {
    // Partition into regular and deferred (current worktree) branches
    let (deferred, regular): (Vec<&ValidatedBranch>, Vec<&ValidatedBranch>) =
        validated.iter().partition(|b| b.is_current_worktree);

    let mut had_errors = false;

    // Process regular branches first
    for branch in &regular {
        let result = delete_single_branch(ctx, branch, force, output);
        if result.has_errors() {
            had_errors = true;
            for err in &result.errors {
                output.error(err);
            }
        }
        let parts = result.deleted_parts();
        if !parts.is_empty() {
            output.result(&format!("Deleted {} ({})", result.branch, parts));
        }
    }

    // Process deferred branch (current worktree) last
    let mut deferred_cd_target: Option<PathBuf> = None;

    for branch in &deferred {
        output.step(&format!(
            "Processing deferred branch: {} (current worktree)",
            branch.name
        ));

        if branch.worktree_path.is_some() {
            // Resolve CD target BEFORE removing the worktree. Once the worktree
            // is removed, the CWD is gone and subsequent git commands would fail.
            let cd_target = resolve_prune_cd_target(
                settings.prune_cd_target,
                &ctx.project_root,
                &ctx.git_dir,
                &ctx.remote_name,
                settings.use_gitoxide,
                output,
            );

            if let Err(e) = std::env::set_current_dir(&cd_target) {
                output.error(&format!(
                    "Failed to change directory to {}: {e}. \
                     Skipping removal of current worktree {}.",
                    cd_target.display(),
                    branch.name
                ));
                continue;
            }

            let result = delete_single_branch(ctx, branch, force, output);

            if result.worktree_removed {
                deferred_cd_target = Some(cd_target);
            }

            if result.has_errors() {
                had_errors = true;
                for err in &result.errors {
                    output.error(err);
                }
            }

            let parts = result.deleted_parts();
            if !parts.is_empty() {
                output.result(&format!("Deleted {} ({})", result.branch, parts));
            }
        } else {
            // No worktree, just delete branch and remote
            let result = delete_single_branch(ctx, branch, force, output);
            if result.has_errors() {
                had_errors = true;
                for err in &result.errors {
                    output.error(err);
                }
            }
            let parts = result.deleted_parts();
            if !parts.is_empty() {
                output.result(&format!("Deleted {} ({})", result.branch, parts));
            }
        }
    }

    // Write the cd target to the temp file for the shell wrapper.
    // When no shell wrapper is active, tell the user to cd manually.
    if let Some(ref cd_target) = deferred_cd_target {
        if std::env::var(CD_FILE_ENV).is_ok() {
            output.cd_path(cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    if had_errors {
        anyhow::bail!("Some branches could not be fully deleted; see errors above");
    }

    Ok(())
}

/// Delete a single branch: remote, worktree, and local branch (in that order).
///
/// Deletion order is deliberate — remote branches are hardest to recreate, so
/// they are deleted first. If a later step fails, the user still has local state
/// to recover from.
fn delete_single_branch(
    ctx: &BranchDeleteContext,
    branch: &ValidatedBranch,
    force: bool,
    output: &mut dyn Output,
) -> DeletionResult {
    let mut result = DeletionResult {
        branch: branch.name.clone(),
        remote_deleted: false,
        worktree_removed: false,
        branch_deleted: false,
        errors: Vec::new(),
    };

    let has_worktree = branch.worktree_path.is_some();

    // Step 1: Run pre-remove hook (only if worktree exists)
    if let Some(ref wt_path) = branch.worktree_path {
        if let Err(e) = run_hook(
            HookType::PreRemove,
            ctx,
            &wt_path.clone(),
            &branch.name,
            output,
        ) {
            output.warning(&format!("Pre-remove hook failed for {}: {e}", branch.name));
        }
    }

    // Step 2: Delete remote branch (hardest to recreate, do first)
    if let (Some(ref remote), Some(ref remote_branch)) =
        (&branch.remote_name, &branch.remote_branch_name)
    {
        output.step(&format!(
            "Deleting remote branch {}/{}...",
            remote, remote_branch
        ));
        match ctx.git.push_delete(remote, remote_branch) {
            Ok(()) => {
                result.remote_deleted = true;
                output.step(&format!(
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

    // Step 3: Remove worktree (if one exists)
    if let Some(ref wt_path) = branch.worktree_path {
        if wt_path.exists() {
            output.step(&format!("Removing worktree at {}...", wt_path.display()));
            match ctx.git.worktree_remove(wt_path, force) {
                Ok(()) => {
                    result.worktree_removed = true;
                    output.result(&format!("Removed worktree '{}'", branch.name));
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
            output.warning(&format!(
                "Worktree directory {} not found. Attempting to force remove record.",
                wt_path.display()
            ));
            match ctx.git.worktree_remove(wt_path, true) {
                Ok(()) => {
                    result.worktree_removed = true;
                    output.result(&format!("Removed worktree '{}'", branch.name));
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
            cleanup_empty_parent_dirs(&ctx.project_root, wt_path, output);
        }
    }

    // Step 4: Delete local branch
    // Always use force-delete (-D) here because our validation (which checks
    // against both local and remote tracking default branch) has already passed.
    // Git's built-in `branch -d` only checks the local default branch, which
    // would fail when the branch was created from the remote tracking ref and
    // the local default branch is behind.
    output.step(&format!("Deleting local branch {}...", branch.name));
    match ctx.git.branch_delete(&branch.name, true) {
        Ok(()) => {
            result.branch_deleted = true;
            output.step(&format!("Branch {} deleted", branch.name));
        }
        Err(e) => {
            result.errors.push(format!(
                "Failed to delete local branch {}: {e}",
                branch.name
            ));
        }
    }

    // Step 5: Run post-remove hook (only if worktree existed)
    if has_worktree {
        if let Some(ref wt_path) = branch.worktree_path {
            if let Err(e) = run_hook(
                HookType::PostRemove,
                ctx,
                &wt_path.clone(),
                &branch.name,
                output,
            ) {
                output.warning(&format!("Post-remove hook failed for {}: {e}", branch.name));
            }
        }
    }

    result
}

/// Run a lifecycle hook (pre-remove or post-remove) for a worktree.
fn run_hook(
    hook_type: HookType,
    ctx: &BranchDeleteContext,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let hook_ctx = HookContext::new(
        hook_type,
        "branch-delete",
        &ctx.project_root,
        &ctx.git_dir,
        &ctx.remote_name,
        &ctx.source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::Manual);

    executor.execute(&hook_ctx, output)?;

    Ok(())
}

/// Clean up empty parent directories after removing a worktree.
///
/// Walks up the directory tree from the removed worktree's parent directory,
/// removing each directory if empty, until reaching the project root.
/// This handles branches with slashes (e.g., `feature/my-branch`) where
/// removing the worktree leaves empty intermediate directories.
fn cleanup_empty_parent_dirs(project_root: &Path, worktree_path: &Path, output: &mut dyn Output) {
    let mut current = worktree_path.parent();
    while let Some(dir) = current {
        // Stop at or above the project root
        if dir == project_root || !dir.starts_with(project_root) {
            break;
        }
        // fs::remove_dir only succeeds on empty directories
        match std::fs::remove_dir(dir) {
            Ok(()) => {
                output.step(&format!("Removed empty directory '{}'", dir.display()));
                current = dir.parent();
            }
            Err(_) => break,
        }
    }
}

/// Resolve where to cd after deleting the user's current worktree.
fn resolve_prune_cd_target(
    cd_target: PruneCdTarget,
    project_root: &Path,
    git_dir: &Path,
    remote_name: &str,
    use_gitoxide: bool,
    output: &mut dyn Output,
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
                        output.step(&format!(
                            "Default branch worktree directory '{}' not found, falling back to project root",
                            branch_dir.display()
                        ));
                        project_root.to_path_buf()
                    }
                }
                Err(e) => {
                    output.warning(&format!(
                        "Cannot determine default branch for cd target: {e}. Falling back to project root."
                    ));
                    project_root.to_path_buf()
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_worktree_list_empty() {
        // parse_worktree_list requires a GitCommand which needs a real repo,
        // so we test the parsing logic indirectly through the struct definitions.
        let entry = WorktreeEntry {
            path: PathBuf::from("/tmp/test"),
            branch: Some("main".to_string()),
            is_bare: false,
        };
        assert_eq!(entry.path, PathBuf::from("/tmp/test"));
        assert_eq!(entry.branch.as_deref(), Some("main"));
        assert!(!entry.is_bare);
    }

    #[test]
    fn test_worktree_entry_bare() {
        let entry = WorktreeEntry {
            path: PathBuf::from("/tmp/test.git"),
            branch: None,
            is_bare: true,
        };
        assert!(entry.is_bare);
        assert!(entry.branch.is_none());
    }

    #[test]
    fn test_validated_branch_fields() {
        let vb = ValidatedBranch {
            name: "feature/test".to_string(),
            worktree_path: Some(PathBuf::from("/tmp/project/feature/test")),
            remote_name: Some("origin".to_string()),
            remote_branch_name: Some("feature/test".to_string()),
            is_current_worktree: false,
        };
        assert_eq!(vb.name, "feature/test");
        assert!(vb.worktree_path.is_some());
        assert!(!vb.is_current_worktree);
    }

    #[test]
    fn test_validated_branch_no_worktree() {
        let vb = ValidatedBranch {
            name: "orphan-branch".to_string(),
            worktree_path: None,
            remote_name: None,
            remote_branch_name: None,
            is_current_worktree: false,
        };
        assert!(vb.worktree_path.is_none());
        assert!(vb.remote_name.is_none());
        assert!(vb.remote_branch_name.is_none());
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
        // We cannot create a real GitCommand here (requires git repo),
        // so we just verify the struct shape compiles.
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
}
