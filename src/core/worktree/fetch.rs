//! Core logic for the `git-worktree-fetch` / `daft update` command.
//!
//! Updates worktree branches by pulling from their remote tracking branches,
//! or syncs a worktree to a different remote branch via refspec syntax.

use crate::core::ProgressSink;
use crate::git::GitCommand;
use crate::utils::*;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A parsed refspec describing which remote branch to pull into which worktree.
#[derive(Debug, Clone)]
pub struct UpdateRefSpec {
    /// Remote branch to fetch from.
    pub source: String,
    /// Local worktree/branch to update.
    pub destination: String,
}

impl UpdateRefSpec {
    /// Returns true if source and destination are the same branch (same-branch mode).
    pub fn is_same_branch(&self) -> bool {
        self.source == self.destination
    }
}

/// Parse a target string as a refspec.
///
/// - `"master"` → `UpdateRefSpec { source: "master", destination: "master" }`
/// - `"master:test"` → `UpdateRefSpec { source: "master", destination: "test" }`
pub fn parse_refspec(target: &str) -> UpdateRefSpec {
    if let Some((source, destination)) = target.split_once(':') {
        UpdateRefSpec {
            source: source.to_string(),
            destination: destination.to_string(),
        }
    } else {
        UpdateRefSpec {
            source: target.to_string(),
            destination: target.to_string(),
        }
    }
}

/// Input parameters for the update operation.
pub struct FetchParams {
    /// Target worktrees by directory name, branch name, or refspec (source:destination).
    pub targets: Vec<String>,
    /// Update all worktrees.
    pub all: bool,
    /// Update even if worktree has uncommitted changes.
    pub force: bool,
    /// Show what would be done without making changes.
    pub dry_run: bool,
    /// Use git pull --rebase (same-branch mode only).
    pub rebase: bool,
    /// Use git pull --autostash (same-branch mode only).
    pub autostash: bool,
    /// Only fast-forward (default behavior, same-branch mode only).
    pub ff_only: bool,
    /// Allow merge commits (disables --ff-only, same-branch mode only).
    pub no_ff_only: bool,
    /// Additional arguments to pass to git pull (same-branch mode only).
    pub pull_args: Vec<String>,
    /// Whether to run in quiet mode (suppresses git pull output).
    pub quiet: bool,
    /// Remote name to use for fetch/pull operations.
    pub remote_name: String,
}

/// Result of a fetch operation for a single worktree.
#[derive(Debug, Default)]
pub struct WorktreeFetchResult {
    pub worktree_name: String,
    pub success: bool,
    pub message: String,
    pub skipped: bool,
    /// True when the pull succeeded but there were no new changes.
    pub up_to_date: bool,
    /// Captured git pull stdout (diff stats, fast-forward info). None when up-to-date or on error.
    pub pull_output: Option<String>,
}

/// Aggregated result of fetching all worktrees.
pub struct FetchResult {
    /// Per-worktree results.
    pub results: Vec<WorktreeFetchResult>,
    /// The remote name that was fetched.
    pub remote_name: String,
    /// The remote URL (if available).
    pub remote_url: Option<String>,
    /// The pull arguments used (for same-branch mode).
    pub pull_args: Vec<String>,
}

impl FetchResult {
    pub fn updated_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.success && !r.skipped && !r.up_to_date)
            .count()
    }

    pub fn up_to_date_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| r.success && !r.skipped && r.up_to_date)
            .count()
    }

    pub fn skipped_count(&self) -> usize {
        self.results.iter().filter(|r| r.skipped).count()
    }

    pub fn failed_count(&self) -> usize {
        self.results
            .iter()
            .filter(|r| !r.success && !r.skipped)
            .count()
    }
}

/// Execute the update operation.
pub fn execute(
    params: &FetchParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<FetchResult> {
    let remote_name = &params.remote_name;
    let original_dir = get_current_directory()?;

    // Determine refspecs and their resolved worktree paths
    let refspecs = determine_refspecs(params, git, project_root, progress)?;

    if refspecs.is_empty() {
        return Ok(FetchResult {
            results: Vec::new(),
            remote_name: remote_name.to_string(),
            remote_url: git.remote_get_url(remote_name).ok(),
            pull_args: Vec::new(),
        });
    }

    // Build pull arguments (used for same-branch mode)
    let pull_args = build_pull_args(params);

    progress.on_step(&format!("Pull arguments: {}", pull_args.join(" ")));

    // Process each target
    let mut results: Vec<WorktreeFetchResult> = Vec::new();

    for (refspec, target_path) in &refspecs {
        let worktree_name = target_path
            .strip_prefix(project_root)
            .ok()
            .and_then(|p| p.to_str())
            .unwrap_or("unknown")
            .to_string();

        let result = process_worktree(
            git,
            target_path,
            &worktree_name,
            &pull_args,
            params,
            refspec,
            progress,
        );
        results.push(result);
    }

    // Return to original directory
    change_directory(&original_dir)?;

    Ok(FetchResult {
        results,
        remote_name: remote_name.to_string(),
        remote_url: git.remote_get_url(remote_name).ok(),
        pull_args,
    })
}

/// Determine which worktrees to update based on arguments, returning refspecs with resolved paths.
fn determine_refspecs(
    params: &FetchParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<Vec<(UpdateRefSpec, PathBuf)>> {
    if params.all {
        // --all: self-referencing refspec for every worktree
        let worktrees = get_all_worktrees_with_branches(git)?;
        Ok(worktrees
            .into_iter()
            .map(|(path, branch)| {
                let refspec = UpdateRefSpec {
                    source: branch.clone(),
                    destination: branch,
                };
                (refspec, path)
            })
            .collect())
    } else if params.targets.is_empty() {
        // No args: self-referencing refspec for current worktree
        let current = git.get_current_worktree_path()?;
        let branch = git.symbolic_ref_short_head()?;
        let refspec = UpdateRefSpec {
            source: branch.clone(),
            destination: branch,
        };
        Ok(vec![(refspec, current)])
    } else {
        // Explicit targets: parse each as a refspec
        let mut resolved: Vec<(UpdateRefSpec, PathBuf)> = Vec::new();
        let mut errors: Vec<String> = Vec::new();

        for target in &params.targets {
            let refspec = parse_refspec(target);
            // Resolve the destination branch to a worktree path
            match git.resolve_worktree_path(&refspec.destination, project_root) {
                Ok(path) => resolved.push((refspec, path)),
                Err(e) => errors.push(format!("'{}': {}", refspec.destination, e)),
            }
        }

        if !errors.is_empty() {
            for error in &errors {
                progress.on_warning(&format!("Failed to resolve target {error}"));
            }
            anyhow::bail!("Failed to resolve {} target(s)", errors.len());
        }

        Ok(resolved)
    }
}

/// Get all non-bare worktrees with their branch names from git worktree list.
pub fn get_all_worktrees_with_branches(git: &GitCommand) -> Result<Vec<(PathBuf, String)>> {
    let porcelain_output = git.worktree_list_porcelain()?;
    let mut worktrees: Vec<(PathBuf, String)> = Vec::new();
    let mut current_worktree: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;

    for line in porcelain_output.lines() {
        if let Some(worktree_path) = line.strip_prefix("worktree ") {
            if let Some(path) = current_worktree.take() {
                if !is_bare {
                    if let Some(branch) = current_branch.take() {
                        worktrees.push((path, branch));
                    }
                }
            }
            current_worktree = Some(PathBuf::from(worktree_path));
            current_branch = None;
            is_bare = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            // branch refs/heads/main -> main
            current_branch = Some(
                branch_ref
                    .strip_prefix("refs/heads/")
                    .unwrap_or(branch_ref)
                    .to_string(),
            );
        } else if line == "bare" {
            is_bare = true;
        }
    }

    if let Some(path) = current_worktree {
        if !is_bare {
            if let Some(branch) = current_branch {
                worktrees.push((path, branch));
            }
        }
    }

    Ok(worktrees)
}

/// Build pull arguments from params and settings (used for same-branch mode).
fn build_pull_args(params: &FetchParams) -> Vec<String> {
    let mut pull_args: Vec<String> = Vec::new();

    if params.rebase {
        pull_args.push("--rebase".to_string());
    }

    if !params.no_ff_only && !params.rebase {
        pull_args.push("--ff-only".to_string());
    }

    if params.autostash {
        pull_args.push("--autostash".to_string());
    }

    for arg in &params.pull_args {
        pull_args.push(arg.clone());
    }

    pull_args
}

/// Process a single worktree, choosing between same-branch and cross-branch mode.
fn process_worktree(
    git: &GitCommand,
    target_path: &Path,
    worktree_name: &str,
    pull_args: &[String],
    params: &FetchParams,
    refspec: &UpdateRefSpec,
    progress: &mut dyn ProgressSink,
) -> WorktreeFetchResult {
    progress.on_step(&format!("Processing '{worktree_name}'..."));

    // Change to worktree directory
    if let Err(e) = change_directory(target_path) {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            message: format!("Failed to change to directory: {e}"),
            ..Default::default()
        };
    }

    // Check for uncommitted changes (both modes)
    match git.has_uncommitted_changes_in(target_path) {
        Ok(has_changes) => {
            if has_changes && !params.force {
                progress.on_warning(&format!(
                    "Skipping '{worktree_name}': has uncommitted changes (use --force to update anyway)"
                ));
                return WorktreeFetchResult {
                    worktree_name: worktree_name.to_string(),
                    success: true,
                    message: "Skipped: uncommitted changes".to_string(),
                    skipped: true,
                    ..Default::default()
                };
            }
        }
        Err(e) => {
            return WorktreeFetchResult {
                worktree_name: worktree_name.to_string(),
                message: format!("Failed to check status: {e}"),
                ..Default::default()
            };
        }
    }

    if refspec.is_same_branch() {
        process_same_branch(git, worktree_name, pull_args, params, progress)
    } else {
        process_cross_branch(
            git,
            worktree_name,
            refspec,
            &params.remote_name,
            params,
            progress,
        )
    }
}

/// Same-branch mode: uses `git pull` with configured arguments.
fn process_same_branch(
    git: &GitCommand,
    worktree_name: &str,
    pull_args: &[String],
    params: &FetchParams,
    progress: &mut dyn ProgressSink,
) -> WorktreeFetchResult {
    // Check if branch has an upstream
    if check_has_upstream(git).is_err() {
        progress.on_warning(&format!(
            "Skipping '{worktree_name}': no tracking branch configured"
        ));
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: "Skipped: no tracking branch".to_string(),
            skipped: true,
            ..Default::default()
        };
    }

    // Dry run mode
    if params.dry_run {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: format!("Dry run: would pull with: git pull {}", pull_args.join(" ")),
            skipped: true,
            ..Default::default()
        };
    }

    // Run git pull (always capture output for structured rendering)
    let pull_args_refs: Vec<&str> = pull_args.iter().map(|s| s.as_str()).collect();

    match git.pull(&pull_args_refs) {
        Ok(output) => {
            let trimmed = output.trim();
            let up_to_date =
                trimmed.contains("Already up to date") || trimmed.contains("is up to date");
            let pull_output = if up_to_date || trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            WorktreeFetchResult {
                worktree_name: worktree_name.to_string(),
                success: true,
                message: if up_to_date {
                    "Already up to date".to_string()
                } else {
                    "Updated successfully".to_string()
                },
                skipped: false,
                up_to_date,
                pull_output,
            }
        }
        Err(e) => WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            message: format!("Failed: {e}"),
            ..Default::default()
        },
    }
}

/// Cross-branch mode: uses `git fetch` + `git reset --hard` for deterministic sync.
fn process_cross_branch(
    git: &GitCommand,
    worktree_name: &str,
    refspec: &UpdateRefSpec,
    remote_name: &str,
    params: &FetchParams,
    progress: &mut dyn ProgressSink,
) -> WorktreeFetchResult {
    let remote_ref = format!("{}/{}", remote_name, refspec.source);

    // Dry run mode
    if params.dry_run {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            success: true,
            message: format!(
                "Dry run: would fetch {}/{} and reset --hard to {}",
                remote_name, refspec.source, remote_ref
            ),
            skipped: true,
            ..Default::default()
        };
    }

    progress.on_step(&format!(
        "Cross-branch update: {} -> {} (via {})",
        refspec.source, refspec.destination, remote_ref
    ));

    // git fetch <remote> <source_branch>
    if let Err(e) = git.fetch_refspec(remote_name, &refspec.source) {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            message: format!("Failed to fetch {}/{}: {e}", remote_name, refspec.source),
            ..Default::default()
        };
    }

    // git reset --hard <remote>/<source_branch>
    if let Err(e) = git.reset_hard(&remote_ref) {
        return WorktreeFetchResult {
            worktree_name: worktree_name.to_string(),
            message: format!("Failed to reset to {remote_ref}: {e}"),
            ..Default::default()
        };
    }

    WorktreeFetchResult {
        worktree_name: worktree_name.to_string(),
        success: true,
        message: format!("Updated to {remote_ref}"),
        ..Default::default()
    }
}

/// Check if the current branch has an upstream tracking branch.
fn check_has_upstream(git: &GitCommand) -> Result<()> {
    let branch = git.symbolic_ref_short_head()?;
    let remote_key = format!("branch.{}.remote", branch);
    if git.config_get(&remote_key)?.is_none() {
        anyhow::bail!("No upstream configured for branch '{}'", branch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_refspec_same_branch() {
        let refspec = parse_refspec("master");
        assert_eq!(refspec.source, "master");
        assert_eq!(refspec.destination, "master");
        assert!(refspec.is_same_branch());
    }

    #[test]
    fn test_parse_refspec_cross_branch() {
        let refspec = parse_refspec("master:test");
        assert_eq!(refspec.source, "master");
        assert_eq!(refspec.destination, "test");
        assert!(!refspec.is_same_branch());
    }

    #[test]
    fn test_parse_refspec_with_slashes() {
        let refspec = parse_refspec("feature/auth:develop");
        assert_eq!(refspec.source, "feature/auth");
        assert_eq!(refspec.destination, "develop");
        assert!(!refspec.is_same_branch());
    }

    #[test]
    fn test_parse_refspec_self_referencing_with_slash() {
        let refspec = parse_refspec("feature/auth");
        assert_eq!(refspec.source, "feature/auth");
        assert_eq!(refspec.destination, "feature/auth");
        assert!(refspec.is_same_branch());
    }
}
