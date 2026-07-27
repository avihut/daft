//! Core worktree operations.
//!
//! Each submodule contains the business logic for a daft command, separated
//! from argument parsing and output rendering. Functions accept structured
//! params, a `GitCommand`, and a `ProgressSink`, and return structured results.
//!
//! **Note:** `flow_adopt` and `flow_eject` are deprecated compatibility
//! wrappers. The canonical bare/non-bare conversion logic lives in
//! [`crate::core::layout::transform`]. New code should call that module
//! directly.

pub mod branch_delete;
pub mod branch_source;
pub mod carry;
pub(crate) mod cell_cache;
pub mod checkout;
pub mod checkout_branch;
pub mod clone;
pub mod exec;
pub mod fetch;
pub mod flow_adopt;
pub mod flow_eject;
pub mod forge_ref;
pub mod fork_names;
pub mod identity;
pub mod identity_store;
pub mod info_field;
pub mod init;
pub mod list;
pub mod list_stream;
pub mod merge;
pub mod merge_set_default;
pub mod merged;
pub mod porcelain;
pub mod ports;
pub mod pr_rows;
pub mod previous;
pub mod prune;
pub mod push;
pub mod rebase;
pub mod remove_repo;
pub mod rename;
pub mod sandbox;
pub mod sync_dag;
pub mod temp_worktree;
pub mod warm;

/// Configuration for worktree operations.
#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    pub remote_name: String,
    pub quiet: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            remote_name: "origin".to_string(),
            quiet: false,
        }
    }
}

/// The planned Fetch row's yellow resolution when every fetch attempt
/// failed — shared by the two creation cores so the reason reads
/// identically wherever the row appears.
pub(crate) const FETCH_FAILED_REASON: &str = "failed \u{2014} continuing with local refs";

/// A `worktree-post-create` hook that failed under the default abort fail
/// mode (#765).
///
/// This is a *typed* error rather than a bare `anyhow!` on purpose. It carries
/// two distinct meanings for its two kinds of caller:
///
///   - **Single-repo cores** propagate it with `?`. Its `Display` is the
///     user-facing recovery message, and the `?` returns before the command's
///     `-x`/`--exec` tail runs — fail-closed, so a forgotten check can never
///     re-open the "`-x` ran after a failed hook" hole this change closed.
///   - **The `--with-related` fan-out** `downcast`s it to tell "worktree
///     created, hook failed" apart from "creation failed": siblings still get
///     the branch (as before #765), each repo's outcome is reported
///     accurately, and the command still exits non-zero.
///
/// The worktree already exists and is deliberately kept on disk (a
/// partially-populated `.env`/`node_modules` may be worth inspecting or
/// repairing).
#[derive(Debug)]
pub(crate) struct PostCreateHookFailed {
    /// The original hook-failure error (exit code + stderr).
    source: anyhow::Error,
    /// The created-and-kept worktree.
    pub worktree_path: std::path::PathBuf,
    /// The branch the worktree is on.
    pub branch_name: String,
    /// Whether *this command* created `branch_name`. When false (an
    /// existing-branch checkout), the branch pre-existed the command, so the
    /// recovery hint must NOT suggest `daft remove <branch>` — that would
    /// delete a branch the user already had (#765 review, finding 1).
    was_new_branch: bool,
}

impl std::fmt::Display for PostCreateHookFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The original failure stays first, then the state the user is left in
        // and every recovery path. (Display embeds the source rather than
        // exposing it via `Error::source`, so anyhow prints this message once
        // — it does not also walk and re-print the chained cause.)
        write!(
            f,
            "{source}\n  \
             The worktree was created and kept at '{path}'; any -x/--exec commands were skipped.\n  ",
            source = self.source,
            path = self.worktree_path.display(),
        )?;
        let rerun = crate::daft_cmd("hooks run worktree-post-create");
        if self.was_new_branch {
            let remove = crate::daft_cmd(&format!("remove {}", self.branch_name));
            write!(
                f,
                "tip: fix the hook and re-run it from that worktree with `{rerun}`, or remove the \
                 worktree with `{remove}`.\n  ",
            )?;
        } else {
            // Existing branch: `daft remove` would delete the pre-existing
            // branch (and its remote), not just the worktree, so point at
            // git's worktree-only removal instead.
            write!(
                f,
                "tip: fix the hook and re-run it from that worktree with `{rerun}`. The branch \
                 '{branch}' already existed and is unchanged; to discard just this worktree run \
                 `git worktree remove {path}`.\n  ",
                branch = self.branch_name,
                path = self.worktree_path.display(),
            )?;
        }
        write!(
            f,
            "To continue past post-create failures instead: \
             `git config daft.hooks.worktreePostCreate.failMode warn`",
        )
    }
}

impl std::error::Error for PostCreateHookFailed {}

/// Wrap a failed `worktree-post-create` run as a [`PostCreateHookFailed`].
/// `was_new_branch` distinguishes the branch-creating cores (`start` /
/// `checkout -b`, which may safely suggest `daft remove`) from the
/// existing-branch checkout, whose branch must not be offered for deletion.
/// Shared by the two creation cores.
pub(crate) fn post_create_failure_error(
    err: anyhow::Error,
    worktree_path: &std::path::Path,
    branch_name: &str,
    was_new_branch: bool,
) -> anyhow::Error {
    anyhow::Error::new(PostCreateHookFailed {
        source: err,
        worktree_path: worktree_path.to_path_buf(),
        branch_name: branch_name.to_string(),
        was_new_branch,
    })
}

/// Make a worktree path absolute and lexically clean, for use immediately
/// **before** a creation core chdirs into the worktree it just made.
///
/// `--at sub/foo` reaches the creation cores as a relative path, and every
/// consumer that runs after the chdir — `shared:` linking, the `copy:` stage
/// (#387), the post-create `HookContext`'s `DAFT_WORKTREE_PATH`, the returned
/// result — would otherwise re-resolve it against the *new* cwd and land on
/// `<worktree>/sub/foo`. Shared linking degrades to a silent no-op there; the
/// copy stage would create that path, fill it with a cache-sized tree, and
/// report success.
///
/// Deliberately not `canonicalize()`: that also resolves symlinks (`/tmp` →
/// `/private/tmp` on macOS), changing the path every downstream face prints
/// for a problem that is purely about relativity. The lexical normalizer is
/// the one every layout-derived path already goes through, so an
/// `--at ../sibling` and its layout-derived twin are spelled identically
/// wherever they surface instead of one of them carrying `main/../sibling`.
///
/// Shared by all three creation cores (checkout, checkout_branch, sandbox).
pub(crate) fn normalize_worktree_path(
    path: std::path::PathBuf,
) -> anyhow::Result<std::path::PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        crate::utils::get_current_directory()?.join(path)
    };
    Ok(crate::core::layout::template::normalize_path(&absolute))
}

/// The plan's row shape from the `shared:` anchor onward, as compact tokens.
///
/// Row identity alone cannot express "the `copy:` anchor sits after the
/// shared section's `EndGroup` and before the post-create step" — a
/// positional claim needs positions. Rendering the tail as strings puts the
/// expected order in the test as one readable list, and makes a mis-splice
/// print as a diff of two short vectors rather than a wall of `Debug`.
///
/// Slicing from the shared anchor deliberately ignores everything before it
/// (fetch, tracking, branch, worktree, carry, push), which differs per
/// creation core and is not what these tests are about. Shared by all three
/// cores' splice tests.
#[cfg(test)]
pub(crate) fn plan_shape_from_shared(rows: &[crate::core::stage::Row]) -> Vec<String> {
    use crate::core::stage::Row;
    let start = rows
        .iter()
        .position(|r| matches!(r, Row::Group { label } if label == "shared files"))
        .expect("the fixture declares `shared:`, so its anchor must be planned");
    rows[start..]
        .iter()
        // Exhaustive on purpose — no wildcard arm. A new `Row` variant
        // appearing in this span is exactly the kind of change these tests
        // exist to notice, and a `_ =>` would render it as something bland
        // and let it slip through.
        .map(|r| match r {
            Row::Group { label } => format!("group:{label}"),
            Row::EndGroup => "endgroup".to_string(),
            Row::Step(spec) => format!("step:{:?}", spec.key.id),
            Row::Note { text } => format!("note:{text}"),
        })
        .collect()
}

/// Resolve the planned Carry row (#651): a clean tree resolves silently —
/// the row vanishes — an applied stash completes, and a conflicted one
/// fails with the recovery hint. Shared by the two creation cores, whose
/// `apply_stash` twins keep their own (byte-pinned) step wording.
pub(crate) fn resolve_carry_row(
    should_carry: bool,
    stash_created: bool,
    stash_applied: bool,
    sink: &mut impl crate::core::ProgressSink,
) {
    use crate::core::stage::{StageEvent, StageId, StepKey};
    if !should_carry {
        return;
    }
    let carry_key = StepKey::new(StageId::Carry);
    if !stash_created {
        sink.on_stage(&carry_key, StageEvent::SkippedSilent);
    } else if stash_applied {
        sink.on_stage(&carry_key, StageEvent::Completed { annotation: None });
    } else {
        sink.on_stage(
            &carry_key,
            StageEvent::Failed {
                detail: "stash conflicts \u{2014} run git stash pop".to_string(),
            },
        );
    }
}

#[cfg(test)]
mod normalize_worktree_path_tests {
    use super::normalize_worktree_path;
    use std::path::PathBuf;

    /// An absolute path keeps its spelling — symlinked prefixes included.
    /// The `/tmp` case is the one that matters on macOS, where
    /// `canonicalize` would rewrite it to `/private/tmp` and change what
    /// every downstream face prints.
    #[test]
    fn absolute_paths_pass_through_without_resolving_symlinks() {
        assert_eq!(
            normalize_worktree_path(PathBuf::from("/tmp/wt/feat-x")).unwrap(),
            PathBuf::from("/tmp/wt/feat-x"),
        );
    }

    /// `..` and `.` collapse lexically, so `--at ../sibling` is spelled the
    /// way its layout-derived twin would be rather than dragging
    /// `main/../sibling` into DAFT_WORKTREE_PATH and `daft list`.
    #[test]
    fn parent_and_current_components_collapse() {
        assert_eq!(
            normalize_worktree_path(PathBuf::from("/repo/main/../sibling")).unwrap(),
            PathBuf::from("/repo/sibling"),
        );
        assert_eq!(
            normalize_worktree_path(PathBuf::from("/repo/./main")).unwrap(),
            PathBuf::from("/repo/main"),
        );
    }

    /// A relative path resolves against the CURRENT directory — the whole
    /// point of calling this before the creation cores chdir into the new
    /// worktree.
    #[test]
    fn relative_paths_resolve_against_the_current_directory() {
        let cwd = crate::utils::get_current_directory().unwrap();
        assert_eq!(
            normalize_worktree_path(PathBuf::from("sub/foo")).unwrap(),
            cwd.join("sub").join("foo"),
        );
        assert_eq!(
            normalize_worktree_path(PathBuf::from("../sibling")).unwrap(),
            cwd.parent().unwrap().join("sibling"),
        );
    }
}

#[cfg(test)]
mod post_create_failure_tests {
    #[test]
    fn new_branch_message_names_state_and_recovery() {
        let err = super::post_create_failure_error(
            anyhow::anyhow!("worktree-post-create hook failed with exit code 1"),
            std::path::Path::new("/tmp/wt/feat-x"),
            "feat-x",
            true,
        );
        let msg = err.to_string();
        // The original failure stays first…
        assert!(msg.contains("worktree-post-create hook failed with exit code 1"));
        // …followed by the state the user is left in and every recovery path.
        assert!(msg.contains("created and kept at '/tmp/wt/feat-x'"));
        assert!(msg.contains("-x/--exec commands were skipped"));
        assert!(msg.contains("hooks run worktree-post-create"));
        // The branch was just created, so offering to remove it is safe.
        assert!(msg.contains("remove feat-x"));
        assert!(msg.contains("daft.hooks.worktreePostCreate.failMode warn"));
    }

    #[test]
    fn existing_branch_message_does_not_offer_to_delete_the_branch() {
        // finding 1: on the existing-branch checkout path the branch pre-existed
        // the command, so the hint must not steer the user at `daft remove`
        // (which deletes the branch), only at git's worktree-only removal.
        let err = super::post_create_failure_error(
            anyhow::anyhow!("worktree-post-create hook failed with exit code 1"),
            std::path::Path::new("/tmp/wt/existing-feature"),
            "existing-feature",
            false,
        );
        let msg = err.to_string();
        assert!(msg.contains("created and kept at '/tmp/wt/existing-feature'"));
        assert!(msg.contains("hooks run worktree-post-create"));
        // No `daft remove` steer — that would destroy the pre-existing branch.
        assert!(!msg.contains("remove existing-feature"));
        // Instead: reassure the branch is untouched, point at git worktree removal.
        assert!(msg.contains("already existed and is unchanged"));
        assert!(msg.contains("git worktree remove /tmp/wt/existing-feature"));
        assert!(msg.contains("daft.hooks.worktreePostCreate.failMode warn"));
    }
}
