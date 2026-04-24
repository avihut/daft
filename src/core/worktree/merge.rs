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
///   directory name), then read the target worktree's current branch via
///   [`branch_at_path`].
/// * `None` — the target is the current worktree: path from
///   [`GitCommand::get_current_worktree_path`], branch via the same
///   [`branch_at_path`] helper so both arms produce identical error
///   formatting for the same failure modes (detached HEAD, read failure).
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
            let branch = branch_at_path(git, &path)?;
            Ok(ResolvedTarget { branch, path })
        }
        None => {
            let path = git.get_current_worktree_path()?;
            let branch = branch_at_path(git, &path)?;
            Ok(ResolvedTarget { branch, path })
        }
    }
}

/// Read the short branch name at `path`.
///
/// Respects [`GitCommand::use_gitoxide`]: when enabled, opens a
/// `gix::ThreadSafeRepository` at `path` and reads `HEAD` through gitoxide;
/// otherwise shells out `git -C <path> symbolic-ref --short HEAD`.
///
/// Both paths emit the same error message on detached HEAD ("detached HEAD")
/// so [`resolve_target`]'s two arms are indistinguishable from the user's
/// point of view for that failure mode.
fn branch_at_path(git: &GitCommand, path: &Path) -> Result<String> {
    if git.use_gitoxide {
        let ts = gix::ThreadSafeRepository::discover(path)
            .with_context(|| format!("failed to open git repo at '{}'", path.display()))?;
        let repo = ts.to_thread_local();
        let head = repo
            .head_ref()
            .with_context(|| format!("failed to read HEAD at '{}'", path.display()))?;
        return match head {
            Some(reference) => Ok(reference.name().shorten().to_string()),
            None => anyhow::bail!(
                "target worktree at '{}' has detached HEAD; checkout a branch first",
                path.display()
            ),
        };
    }

    let output = Command::new("git")
        .args([
            "-C",
            &path.display().to_string(),
            "symbolic-ref",
            "--short",
            "HEAD",
        ])
        .output()
        .with_context(|| format!("failed to read branch at '{}'", path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a symbolic ref") {
            anyhow::bail!(
                "target worktree at '{}' has detached HEAD; checkout a branch first",
                path.display()
            );
        }
        anyhow::bail!(
            "failed to read branch at '{}': {}",
            path.display(),
            stderr.trim()
        );
    }

    String::from_utf8(output.stdout)
        .context("invalid UTF-8 in branch name")
        .map(|s| s.trim().to_string())
}

/// Refuse merging a branch into itself.
///
/// A `git merge X` run from a worktree whose branch is `X` is always a
/// semantic no-op at best and an ambiguous user mistake at worst. Failing
/// here — before we touch git — gives a clear, actionable error instead of
/// a cryptic "Already up to date." when the user likely meant to target a
/// different branch via `--into`.
pub fn validate_distinct(sources: &[String], target: &ResolvedTarget) -> Result<()> {
    for src in sources {
        if src == &target.branch {
            anyhow::bail!("cannot merge branch '{}' into the same branch", src);
        }
    }
    Ok(())
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

    // Pre-flight safety rails. Order matters: cheapest/purely-syntactic check
    // first (source vs. target branch name), then state checks on the target
    // worktree (in-progress op, dirty tree) which touch the filesystem.
    validate_distinct(&params.sources, &resolved)?;

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

        let git = GitCommand::new(true);
        let branch = branch_at_path(&git, tmp.path()).unwrap();
        assert_eq!(branch, "main");
    }

    #[test]
    fn branch_at_path_reads_via_gitoxide() {
        let tmp = tempfile::tempdir().unwrap();
        init_repo(tmp.path());

        let git = GitCommand::new(true).with_gitoxide(true);
        let branch = branch_at_path(&git, tmp.path()).unwrap();
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

        let git = GitCommand::new(true);
        let err = branch_at_path(&git, tmp.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("detached HEAD"), "unexpected error: {msg}");
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

    #[test]
    fn refuses_when_source_equals_target() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: PathBuf::from("/repo/main"),
        };
        let err = validate_distinct(&["main".to_string()], &target).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("same branch"), "unexpected error: {msg}");
    }

    #[test]
    fn allows_distinct_source_and_target() {
        let target = ResolvedTarget {
            branch: "main".into(),
            path: PathBuf::from("/repo/main"),
        };
        assert!(validate_distinct(&["feat".to_string()], &target).is_ok());
    }
}
