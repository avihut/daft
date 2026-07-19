//! Ports owned by the worktree subsystem.
//!
//! Consumer-owns-port (see ARCHITECTURE.md "Hexagonal at subsystem
//! boundaries"): the traits here describe what worktree operations need from
//! the outside world, in the worktree subsystem's own vocabulary. Adapters
//! live with their implementing subsystem. This is the first port outside
//! `src/coordinator/` and follows the same shape.

use crate::core::worktree::forge_ref::ForgeBranchRef;
use crate::executor::presenter::JobPresenter;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;

/// One ref a push is updating, in the exact shape git feeds a `pre-push`
/// hook on stdin: `<local-ref> <local-oid> <remote-ref> <remote-oid>` per
/// line. Deletes use `(delete)` and the zero oid on the local side; refs
/// new to the remote use the zero oid on the remote side.
#[derive(Debug, Clone)]
pub struct PushRef {
    pub local_ref: String,
    pub local_oid: String,
    pub remote_ref: String,
    pub remote_oid: String,
}

/// Result of running a daft-managed hook stage.
#[derive(Debug, Clone)]
pub struct StageOutcome {
    /// Whether the stage completed successfully (gates the push).
    pub success: bool,
    /// Whether the stage was skipped rather than run.
    pub skipped: bool,
    /// Reason for skipping or failing, if any.
    pub reason: Option<String>,
}

/// Port for daft-managed hook stages around git operations (issue #599
/// declares it; #468 implements it).
///
/// A "stage" is a named gate like `pre-push`. When an adapter manages a
/// stage, daft runs the stage itself (with per-job reporting through the
/// presenter) and then performs the git operation with git's own hook
/// dispatch suppressed, so a foreign incumbent hook is never double-fired —
/// chaining it is the adapter's job. When no adapter manages the stage,
/// callers fall back to Path A: git dispatches whatever `pre-push` hook is
/// installed (native, lefthook, husky, pre-commit) as one opaque subprocess.
///
/// Trust gating lives behind this port too: an adapter must answer `false`
/// for repos whose daft config is untrusted, degrading to Path A (git still
/// runs the repo's own hooks; daft's stage is skipped).
pub trait StageRunner {
    /// Whether daft manages the given stage for the repo seen from
    /// `repo_cwd` (a directory inside the repo — normally the worktree the
    /// operation runs in; the adapter resolves git dirs itself).
    ///
    /// Called on every push, so implementations must stay cheap: stat-level
    /// probes only, no subprocess spawns.
    fn manages_stage(&self, stage: &str, repo_cwd: &Path) -> bool;

    /// Run the stage. `worktree_cwd` is the worktree the triggering
    /// operation targets; `refs` pins what the push is about to update.
    /// Reporting flows through `presenter` (same surface lifecycle hooks
    /// use), giving per-job fidelity that Path A's single opaque subprocess
    /// cannot.
    fn run_stage(
        &self,
        stage: &str,
        worktree_cwd: Option<&Path>,
        refs: &[PushRef],
        presenter: Arc<dyn JobPresenter>,
    ) -> Result<StageOutcome>;
}

/// Port for asking the forge whether a branch's pull/merge request was
/// merged (issue #737).
///
/// Exists because the git-side probes cannot see every merge. A squash whose
/// content was altered on the way in — a conflict resolved by the merger,
/// a maintainer's edit before the button — matches no patch and no tree, yet
/// the forge watched it happen and is authoritative about it.
///
/// The signal is **positive-only**: it can conclude "merged", never "not
/// merged". Anything short of proof — the forge unreachable, the PR still
/// open, a field the platform did not supply — must answer `None` and leave
/// the verdict to the git probes, which default to unmerged. Reporting
/// "merged" is what authorizes deleting a branch, so an adapter that cannot
/// prove the claim must not make it.
pub trait ForgeMergedWitness: Send + Sync {
    /// The PR/MR that merged `branch`, if the forge proves one did.
    ///
    /// Implementations must satisfy all of:
    ///
    /// - **Freshly fetched.** A cached state is a stale hint, not a witness.
    /// - **`tip_oid` is the PR's head.** Branch names get reused; pinning the
    ///   commit is what stops an old merged PR from vouching for whatever
    ///   work later took its name.
    /// - **`target_branch` is the PR's base.** A stacked PR merged into
    ///   another feature branch, or one merged into a release line, is
    ///   genuinely "merged" and still absent from the branch this caller
    ///   asked about.
    /// - **Nothing open shadows it.** An open PR on the same branch means
    ///   the work continues regardless of what merged before.
    fn merged_pr(&self, branch: &str, tip_oid: &str, target_branch: &str)
    -> Option<ForgeBranchRef>;
}

/// The witness for repos with no forge, and for every test that has no
/// business talking to one. Mirrors the `NoopStageRunner` pattern below.
pub struct NoopForgeWitness;

impl ForgeMergedWitness for NoopForgeWitness {
    fn merged_pr(
        &self,
        _branch: &str,
        _tip_oid: &str,
        _target_branch: &str,
    ) -> Option<ForgeBranchRef> {
        None
    }
}

/// The adapter used until #468 ships: daft manages no stages, so every push
/// takes Path A. Mirrors the `NoopHookRunner` pattern in `core/mod.rs`.
pub struct NoopStageRunner;

impl StageRunner for NoopStageRunner {
    fn manages_stage(&self, _stage: &str, _repo_cwd: &Path) -> bool {
        false
    }

    fn run_stage(
        &self,
        _stage: &str,
        _worktree_cwd: Option<&Path>,
        _refs: &[PushRef],
        _presenter: Arc<dyn JobPresenter>,
    ) -> Result<StageOutcome> {
        Ok(StageOutcome {
            success: true,
            skipped: true,
            reason: Some("no stage runner configured".to_string()),
        })
    }
}
