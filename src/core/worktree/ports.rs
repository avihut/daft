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
/// open, a field the platform did not supply — must answer
/// [`ForgeWitness::Unproven`] and leave the verdict to the git probes, which
/// default to unmerged. Reporting "merged" is what authorizes deleting a
/// branch, so an adapter that cannot prove the claim must not make it.
///
/// Adapters answer from whatever listing they can fetch, and those listings
/// are windowed — the live adapter sees only the most recently merged PRs.
/// A branch whose PR merged long enough ago to have fallen out of that window
/// is therefore `Unproven` rather than merged: correct (nothing was proved),
/// but it means the witness is weakest for the longest-abandoned branches.
/// Widening the window is a fetch-cost tradeoff, not a correctness fix.
pub trait ForgeMergedWitness: Send + Sync {
    /// What the forge can prove about `branch`'s merge into `target_branch`.
    ///
    /// Implementations must satisfy all of:
    ///
    /// - **Freshly fetched.** A cached state is a stale hint, not a witness.
    /// - **`target_branch` is the PR's base.** A stacked PR merged into
    ///   another feature branch, or one merged into a release line, is
    ///   genuinely "merged" and still absent from the branch this caller
    ///   asked about.
    /// - **Nothing open shadows it.** An open PR on the same branch means
    ///   the work continues regardless of what merged before.
    ///
    /// `tip_oid` is the local branch tip. An implementation reports whether
    /// the merged PR's head matched it but must not decide what a mismatch
    /// means — that needs git, which the caller has and the forge does not.
    fn witness(&self, branch: &str, tip_oid: &str, target_branch: &str) -> ForgeWitness;
}

/// What a [`ForgeMergedWitness`] found. Positive-only: no variant asserts
/// "not merged", only degrees of proof that it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeWitness {
    /// Nothing usable — no merged PR, an open one shadows the name, the forge
    /// was unreachable, or the platform withheld a field the pins need.
    Unproven,
    /// A merged PR against the right base whose head is exactly the local tip.
    MergedAtTip(ForgeBranchRef),
    /// A merged PR against the right base whose head is a *different* commit:
    /// the PR advanced remotely — a suggestion accepted in the web UI, a push
    /// from another machine — past the tip this caller holds.
    ///
    /// Merged, as far as the forge is concerned. Whether that merge carried
    /// *this* branch's work depends on whether the local tip is an ancestor of
    /// `head_oid`, which only the caller can answer.
    MergedAtOtherHead {
        pr: ForgeBranchRef,
        head_oid: String,
    },
}

/// The witness for repos with no forge, and for every test that has no
/// business talking to one. Mirrors the `NoopStageRunner` pattern below.
pub struct NoopForgeWitness;

impl ForgeMergedWitness for NoopForgeWitness {
    fn witness(&self, _branch: &str, _tip_oid: &str, _target_branch: &str) -> ForgeWitness {
        ForgeWitness::Unproven
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
