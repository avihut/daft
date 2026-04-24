//! Core logic for `daft merge`.
//!
//! This module owns the business logic for starting a merge in a target
//! worktree. In Slice 2 the scope is intentionally minimal: dispatch
//! `git merge <sources...>` in the target directory and report whether the
//! invocation conflicted. Richer outcome detection (already-up-to-date,
//! fast-forward vs. true merge, octopus announcements, target resolution)
//! lands in later slices.
//!
//! The established pattern for ad-hoc git invocations is
//! `std::process::Command::new("git").args(...).current_dir(...).status()`,
//! mirroring `src/core/worktree/rebase.rs`. `GitCommand` intentionally does
//! not expose a generic `run` helper, so we go through `std::process::Command`
//! directly.
//!
//! # Parameters
//!
//! [`StartParams`] captures the inputs to a merge start: the list of source
//! refs that will be merged into the target worktree's current branch.
//!
//! # Outcome
//!
//! [`StartOutcome`] reports the result: whether the merge was a no-op because
//! the target was already up to date, and whether the merge left conflicts.
//! Later slices will expand this to distinguish fast-forward, true merge, and
//! octopus cases.

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
    /// `git merge` reported conflicts and left the worktree mid-merge.
    pub conflicted: bool,
}

/// Run `git merge <sources...>` inside `target_worktree`.
///
/// Returns [`StartOutcome`] describing the result. In this Slice-2 form we
/// detect conflicts solely via git's exit status; `already_up_to_date` is
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
        conflicted: !status.success(),
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
        assert!(!outcome.conflicted);
    }

    #[test]
    fn module_exports_are_visible() {
        // Smoke test: the public types and function are reachable through the
        // module path the commands layer will use.
        fn _assert_signature(p: &Path, params: &StartParams) -> Result<StartOutcome> {
            execute_start(p, params)
        }
    }
}
