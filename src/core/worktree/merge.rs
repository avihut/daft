//! Core logic for `daft merge`.
//!
//! This module owns the business logic for starting a merge in a target
//! worktree. In Slice 2 the scope is intentionally minimal: dispatch
//! `git merge <sources...>` in the target directory and report whether the
//! invocation conflicted. Richer outcome detection (already-up-to-date,
//! fast-forward vs. true merge, octopus announcements, target resolution)
//! lands in later slices.
//!
//! The direct use of `std::process::Command::new("git")` here is a Slice-2
//! shortcut. A later slice will replace it with a `GitCommand::merge_in`
//! helper (analogous to `GitCommand::rebase_in` in `src/git/remote.rs`) once
//! we need to capture stdout/stderr to detect signals like
//! "Already up to date." and distinguish true merge conflicts from other
//! failure modes.
//!
//! # Parameters
//!
//! [`StartParams`] captures the inputs to a merge start: the list of source
//! refs that will be merged into the target worktree's current branch.
//!
//! # Outcome
//!
//! [`StartOutcome`] reports the result: whether the merge was a no-op because
//! the target was already up to date, and whether the `git merge` invocation
//! exited non-zero. Later slices will expand this to distinguish fast-forward,
//! true merge, and octopus cases, and to separate real conflicts from other
//! failure modes.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Inputs to a merge-start operation.
#[derive(Debug, Clone)]
pub struct StartParams {
    /// One or more source refs to merge into the target worktree's branch.
    pub sources: Vec<String>,
    /// Optional target worktree/branch. `None` → current worktree's branch.
    pub target: Option<String>,
}

/// Target of a merge after resolution.
///
/// Slice 3 always produces a [`ResolvedTarget`] with a concrete worktree path.
/// A later slice (no-worktree target — ref-only merges) will likely change
/// `path` to `Option<PathBuf>` to represent a target branch that has no
/// checked-out worktree. Keeping the struct dedicated (rather than returning
/// a bare `(String, PathBuf)` tuple) makes that future change a local edit.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Branch being merged into — used for display and comparisons.
    pub branch: String,
    /// Path to the target worktree on disk.
    pub path: PathBuf,
}

/// Resolve the merge target.
///
/// * `Some(t)` — delegate to [`GitCommand::resolve_worktree_path`] (which
///   matches `t` as a relative path, then a branch name, then a worktree
///   directory name), then read the target worktree's current branch by
///   shelling out `git -C <path> symbolic-ref --short HEAD`.
/// * `None` — the target is the current worktree: path from
///   [`GitCommand::get_current_worktree_path`], branch from
///   [`GitCommand::symbolic_ref_short_head`] (both operate on CWD).
///
/// No dedicated "branch-at-path" helper exists in `src/git/` today, so the
/// explicit target branch is resolved via `std::process::Command` — the same
/// shell-out style used in [`execute_start`]. If a later slice introduces a
/// `GitCommand::branch_at(&Path)` helper, this function should switch to it.
///
/// Fails loudly on detached HEAD. Merging into a detached HEAD would fail
/// downstream anyway; the explicit error here surfaces the problem earlier.
pub fn resolve_target(
    target: Option<&str>,
    git: &GitCommand,
    project_root: &Path,
) -> Result<ResolvedTarget> {
    match target {
        Some(t) => {
            let path = git.resolve_worktree_path(t, project_root)?;
            let branch = branch_at_path(&path)?;
            Ok(ResolvedTarget { branch, path })
        }
        None => {
            let path = git.get_current_worktree_path()?;
            let branch = git.symbolic_ref_short_head()?;
            Ok(ResolvedTarget { branch, path })
        }
    }
}

/// Read the short branch name at `path` by shelling out
/// `git -C <path> symbolic-ref --short HEAD`.
///
/// Returns an error on detached HEAD or any other git failure.
fn branch_at_path(path: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .with_context(|| {
            format!(
                "failed to invoke `git symbolic-ref` in '{}'",
                path.display()
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "failed to read current branch at '{}': {}",
            path.display(),
            stderr.trim()
        );
    }

    let branch = String::from_utf8(output.stdout)
        .with_context(|| format!("non-UTF-8 branch name at '{}'", path.display()))?
        .trim()
        .to_string();

    if branch.is_empty() {
        anyhow::bail!("no branch detected at '{}'", path.display());
    }

    Ok(branch)
}

/// Result of a merge-start operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartOutcome {
    /// The target branch was already up to date with the sources; nothing to do.
    pub already_up_to_date: bool,
    /// True if `git merge` exited non-zero for any reason (conflict, unknown ref, bad state).
    /// Slice 5+ will refine this into `conflicted` vs other failure modes via stderr parsing
    /// or `.git/MERGE_HEAD` inspection.
    pub failed: bool,
}

/// Execute a merge against the resolved target.
///
/// Resolves the target (explicit `--into` value or current worktree) through
/// [`resolve_target`], then dispatches `git merge <sources...>` with the
/// target worktree as CWD so git updates the correct worktree's index and
/// working tree.
///
/// Returns [`StartOutcome`] describing the result. In this Slice-3 form we
/// still detect failure solely via git's exit status; `already_up_to_date` is
/// always reported as `false` here and will be upgraded in later slices.
///
/// # Signature stability
///
/// Taking `git: &GitCommand` and `project_root: &Path` lets later slices add
/// flag passthrough (Slice 6, via `StartParams`) and ref-only targets
/// (Slice 9, via changes to [`ResolvedTarget`]) without another signature
/// churn.
pub fn execute_start(
    params: &StartParams,
    git: &GitCommand,
    project_root: &Path,
) -> Result<StartOutcome> {
    let resolved = resolve_target(params.target.as_deref(), git, project_root)?;

    let mut argv: Vec<String> = vec!["merge".to_string()];
    argv.extend(params.sources.iter().cloned());

    let status = Command::new("git")
        .args(&argv)
        .current_dir(&resolved.path)
        .status()
        .with_context(|| {
            format!(
                "failed to invoke `git merge` in '{}'",
                resolved.path.display()
            )
        })?;

    Ok(StartOutcome {
        already_up_to_date: false,
        failed: !status.success(),
    })
}

#[cfg(test)]
mod tests {
    //! Test coverage notes for [`resolve_target`]:
    //!
    //! * `branch_at_path` is covered directly against a real `git init`ed
    //!   temp directory.
    //! * The happy-path for `resolve_target(Some(...), ...)` and
    //!   `resolve_target(None, ...)` requires a multi-worktree fixture
    //!   (either setting CWD to exercise the `None` branch, or using
    //!   `git worktree add` to exercise `Some(...)`). Both are expensive
    //!   to stand up here and fragile in parallel tests because
    //!   `get_current_worktree_path` and `symbolic_ref_short_head` read
    //!   the process CWD. End-to-end coverage lives in the YAML scenario
    //!   `tests/manual/scenarios/merge/cross-worktree.yml`.
    //! * The error-path `resolve_target(Some("bogus"), ...)` bubbles up
    //!   from `GitCommand::resolve_worktree_path`, which is exercised by
    //!   its own tests and by the `carry` scenarios.
    use super::*;
    use std::process::Command as ShellCommand;

    fn init_repo(path: &Path) {
        ShellCommand::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(path)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .status()
            .unwrap();
        // Identity via env avoids any global config dependency.
        ShellCommand::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", "init"])
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .status()
            .unwrap();
    }

    #[test]
    fn branch_at_path_reads_current_branch() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let branch = branch_at_path(tmp.path()).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn branch_at_path_fails_on_detached_head() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());
        // Detach HEAD at the current commit.
        ShellCommand::new("git")
            .args(["checkout", "--detach", "-q"])
            .current_dir(tmp.path())
            .status()
            .unwrap();

        let err = branch_at_path(tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("failed to read current branch"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn start_params_holds_sources() {
        let params = StartParams {
            sources: vec!["feature/x".to_string(), "feature/y".to_string()],
            target: None,
        };
        assert_eq!(params.sources.len(), 2);
        assert_eq!(params.sources[0], "feature/x");
        assert!(params.target.is_none());
    }

    #[test]
    fn start_params_holds_target() {
        let params = StartParams {
            sources: vec!["feature/x".to_string()],
            target: Some("main".to_string()),
        };
        assert_eq!(params.target.as_deref(), Some("main"));
    }

    #[test]
    fn start_outcome_default_is_clean() {
        let outcome = StartOutcome::default();
        assert!(!outcome.already_up_to_date);
        assert!(!outcome.failed);
    }
}
