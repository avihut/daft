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

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Inputs to a merge-start operation.
#[derive(Debug, Clone)]
pub struct StartParams {
    /// One or more source refs to merge into the target worktree's branch.
    pub sources: Vec<String>,
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

/// Execute a merge in `target_worktree`.
///
/// **Caller invariant:** `target_worktree` must be the root of a git worktree.
/// This function does not validate the path — it delegates to `git merge`, which
/// will surface any error itself. The command layer enforces the invariant via
/// `is_git_repository()` before calling this function. Slice 3 introduces
/// `--into` target resolution that preserves the invariant by construction.
///
/// Returns [`StartOutcome`] describing the result. In this Slice-2 form we
/// detect failure solely via git's exit status; `already_up_to_date` is
/// always reported as `false` here and will be upgraded in later slices.
pub fn execute_start(target_worktree: &Path, params: &StartParams) -> Result<StartOutcome> {
    let mut argv: Vec<String> = vec!["merge".to_string()];
    argv.extend(params.sources.iter().cloned());

    let status = Command::new("git")
        .args(&argv)
        .current_dir(target_worktree)
        .status()
        .with_context(|| {
            format!(
                "failed to invoke `git merge` in '{}'",
                target_worktree.display()
            )
        })?;

    Ok(StartOutcome {
        already_up_to_date: false,
        failed: !status.success(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_params_holds_sources() {
        let params = StartParams {
            sources: vec!["feature/x".to_string(), "feature/y".to_string()],
        };
        assert_eq!(params.sources.len(), 2);
        assert_eq!(params.sources[0], "feature/x");
    }

    #[test]
    fn start_outcome_default_is_clean() {
        let outcome = StartOutcome::default();
        assert!(!outcome.already_up_to_date);
        assert!(!outcome.failed);
    }
}
