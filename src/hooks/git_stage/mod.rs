//! Git-stage hooks — daft as the repository's git hooks manager.
//!
//! Where the rest of [`crate::hooks`] gates daft's own worktree lifecycle,
//! this module gates git's: `pre-commit`, `commit-msg`, `pre-push` and the
//! rest of the stages git dispatches out of the hooks directory. Definitions
//! share the same `daft.yml` `hooks:` block and the same job-orchestration
//! semantics, so a stage is "just another hook" everywhere downstream of
//! config resolution.
//!
//! Submodules split along the two directions the feature runs in: outward
//! (writing shims into the hooks directory so git will call daft) and inward
//! (dispatching a stage when git does).

pub mod gitdir;

/// Environment variable naming the git stage daft is currently executing.
///
/// Set for the duration of a stage run — on the dispatching process, on every
/// job, and around [`StageRunner::run_stage`] — and honoured **same-stage
/// only**: a shim, a `__hook` entry, or a `manages_stage` probe stands down
/// when the value equals its own stage and proceeds otherwise.
///
/// Scoping it to one stage rather than to "any daft-spawned git" is
/// deliberate, and it is what drop-in fidelity costs. A `pre-commit` job that
/// commits, or a `pre-push` job that checks out a branch, must fire the
/// repo's `commit-msg` and `post-checkout` definitions exactly as an
/// incumbent manager would; a blanket guard would silently make daft-run
/// stages weaker than the same config run by the tool it replaced. The narrow
/// guard still closes the only loop that matters — a stage re-entering
/// itself.
///
/// [`StageRunner::run_stage`]: crate::core::worktree::ports::StageRunner::run_stage
pub const STAGE_GUARD_VAR: &str = "DAFT_STAGE_GUARD";

/// Whether a run of `stage` is already in progress in this process tree.
///
/// The single question every guard site asks. `stage` is the git hook
/// filename (`pre-commit`, `pre-push`, …), matching what
/// [`guard_value_for`] writes.
pub fn guard_blocks(stage: &str) -> bool {
    std::env::var(STAGE_GUARD_VAR).is_ok_and(|active| active == stage)
}

/// The `(name, value)` pair marking `stage` as in progress, for callers that
/// set it on a child process rather than on themselves.
///
/// Production code must not reach for `std::env::set_var` to do this — it is
/// an `unsafe fn` in edition 2024, and this crate forbids `unsafe` outside
/// tests. Pass the pair to `Command::env` instead.
pub fn guard_value_for(stage: &str) -> (&'static str, String) {
    (STAGE_GUARD_VAR, stage.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Set/clear the guard around a closure. `env::set_var` is `unsafe fn` in
    /// edition 2024; tests may wrap it, production code may not.
    fn with_guard<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let prior = std::env::var(STAGE_GUARD_VAR).ok();
        unsafe {
            match value {
                Some(v) => std::env::set_var(STAGE_GUARD_VAR, v),
                None => std::env::remove_var(STAGE_GUARD_VAR),
            }
        }
        let out = f();
        unsafe {
            match prior {
                Some(v) => std::env::set_var(STAGE_GUARD_VAR, v),
                None => std::env::remove_var(STAGE_GUARD_VAR),
            }
        }
        out
    }

    #[test]
    #[serial]
    fn unset_guard_blocks_nothing() {
        with_guard(None, || {
            assert!(!guard_blocks("pre-commit"));
            assert!(!guard_blocks("pre-push"));
        });
    }

    #[test]
    #[serial]
    fn guard_blocks_only_its_own_stage() {
        with_guard(Some("pre-commit"), || {
            assert!(guard_blocks("pre-commit"));
            // The whole point: a pre-commit job that commits still gets its
            // commit-msg hook, exactly as it would under any other manager.
            assert!(!guard_blocks("commit-msg"));
            assert!(!guard_blocks("post-commit"));
        });
    }

    #[test]
    fn guard_value_names_the_stage() {
        let (name, value) = guard_value_for("pre-push");
        assert_eq!(name, STAGE_GUARD_VAR);
        assert_eq!(value, "pre-push");
    }
}
