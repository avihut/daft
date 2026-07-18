//! Stage vocabulary for the plan-then-execute timeline (#651).
//!
//! Cores describe their work as an ordered plan of *stages* and then narrate
//! execution as structured events against those stages. The vocabulary is
//! deliberately presentation-free: labels, tenses, glyphs, and colors live in
//! `crate::output::timeline`; this module only names the steps and their
//! lifecycle so that core logic stays UI-agnostic (the same rule that keeps
//! `ProgressSink` free of `Output`).
//!
//! A core emits `ProgressSink::on_plan` exactly once — at its *commit point*:
//! every row of the plan is known, every interactive prompt has fired, and
//! only planned work remains. Long-running resolve work may itself be planned
//! — checkout's remote fetch runs as the rail's first rows rather than as a
//! spinner before it — and facts that resolve mid-plan (the branch's resolved
//! base) reach their rows via [`StageEvent::Note`]. Early-return paths that
//! bail before the commit point never render a timeline at all.

use std::time::Duration;

/// Identity of a plan step, independent of any run.
///
/// One variant per user-meaningful step across the create/remove/clone
/// journeys. Presentation (label text per tense) is keyed off this in
/// `crate::output::timeline`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum StageId {
    // ── Creation (go / start) ────────────────────────────────────────────
    /// Resolve a forge PR/MR reference (`pr:123` / `mr:45` / a PR URL) to its
    /// source branch via `gh`/`glab`. Rendered pre-completed — resolution runs
    /// under the planning face (it determines the plan), and this row is its
    /// receipt.
    ResolveRef,
    /// Fetch from the remote before resolving branches (`daft.checkout.fetch`).
    Fetch,
    /// Ensure remote-tracking refs exist for every remote branch (the
    /// `+refs/heads/*:refs/remotes/<remote>/*` fetch that follows the
    /// general one). Planned right after [`StageId::Fetch`].
    Tracking,
    /// Carry uncommitted changes into the new worktree (stash + apply).
    Carry,
    /// `worktree-pre-create` hooks.
    PreCreateHooks,
    /// Create the new local branch (`daft start`).
    CreateBranch,
    /// Materialize the branch checkout (both journeys).
    CheckOut,
    /// Create the worktree directory.
    CreateWorktree,
    /// Push the new branch and set upstream (`daft start`).
    Push,
    /// Link one declared shared file into the new worktree. Always scoped by
    /// the file's relative path; the row's label is the path itself (set via
    /// [`StepSpec::with_label`]), planned under a `shared files` group.
    SharedFile,
    /// `worktree-post-create` hooks.
    PostCreateHooks,

    // ── Removal (remove) ─────────────────────────────────────────────────
    /// `worktree-pre-remove` hooks.
    PreRemoveHooks,
    /// Delete the branch on the remote (runs first — hardest to recreate).
    DeleteRemote,
    /// Remove the worktree directory.
    RemoveWorktree,
    /// Delete the local branch ref.
    DeleteLocalBranch,
    /// `worktree-post-remove` hooks.
    PostRemoveHooks,

    // ── Clone ────────────────────────────────────────────────────────────
    /// Bare clone of the repository (rendered pre-completed; it finishes
    /// before the layout prompt, which precedes the plan commit).
    CloneBare,
    /// Create the initial worktree for the default (or requested) branch.
    CreateBaseWorktree,
    /// `post-clone` hooks.
    PostCloneHooks,
    /// `daft install` requested via `--install`.
    Install,

    // ── Exec (multi-worktree command runner) ─────────────────────────────
    /// One command run against one worktree in a `daft exec` fleet. The row's
    /// identity is its subject (the worktree label, or the command text in a
    /// multi-command pipeline), so it always carries a fixed label override
    /// and the tense table is a fallback only.
    ExecCommand,

    // ── Run (user tasks) ─────────────────────────────────────────────────
    /// A `daft run` task rendering as a rail section — multi-job tasks only
    /// (a single-job invocation passes the terminal through and never plans
    /// a timeline). Always carries the task name as a fixed label override;
    /// the tense table is a fallback only.
    Task,
    // ── Push (worktree-correct pre-push) ─────────────────────────────────
    /// Resolve the pushed branch to its owning worktree (`daft push`) — the
    /// cwd the shared `pre-push` hook will run in, which is the command's
    /// entire reason to exist (#600).
    ResolveWorktree,
}

impl StageId {
    /// True for stages that render as an embedded hook block when they run
    /// (the plan row is replaced by the hook renderer's own output).
    /// [`Self::Task`] qualifies: a `daft run` task expands into the same
    /// rail-native job section as a lifecycle hook phase.
    pub fn is_hook_phase(self) -> bool {
        matches!(
            self,
            Self::PreCreateHooks
                | Self::PostCreateHooks
                | Self::PreRemoveHooks
                | Self::PostRemoveHooks
                | Self::PostCloneHooks
                | Self::Task
        )
    }

    /// The plan stage a lifecycle hook renders as. `None` for hook types the
    /// timeline never plans (merge hooks — merge keeps its own output).
    pub fn for_hook_type(hook_type: crate::hooks::HookType) -> Option<Self> {
        use crate::hooks::HookType;
        match hook_type {
            HookType::PreCreate => Some(Self::PreCreateHooks),
            HookType::PostCreate => Some(Self::PostCreateHooks),
            HookType::PreRemove => Some(Self::PreRemoveHooks),
            HookType::PostRemove => Some(Self::PostRemoveHooks),
            HookType::PostClone => Some(Self::PostCloneHooks),
            HookType::PreMerge | HookType::PostMerge => None,
        }
    }
}

/// A stage instance within one plan.
///
/// `scope` disambiguates repeated stages — multi-branch `daft remove` runs
/// the same removal stages once per branch, scoped by branch name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct StepKey {
    pub id: StageId,
    pub scope: Option<String>,
}

impl StepKey {
    pub fn new(id: StageId) -> Self {
        Self { id, scope: None }
    }

    pub fn scoped(id: StageId, scope: impl Into<String>) -> Self {
        Self {
            id,
            scope: Some(scope.into()),
        }
    }
}

/// Lifecycle event for one plan step.
///
/// Owned strings by design: events are rare (a handful per command) and an
/// owned payload keeps the sink trait object-safe and storage-friendly.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StageEvent {
    /// The step began executing.
    Started,
    /// The step finished successfully. `annotation`, when present, replaces
    /// the row's annotation (e.g. the branch's resolved provenance).
    Completed { annotation: Option<String> },
    /// The step failed. The label stays imperative (the fact never
    /// happened); `detail` is appended as the annotation.
    Failed { detail: String },
    /// The step was cancelled mid-run (SIGINT). Renders the yellow `⊘` face
    /// with a `cancelled` annotation and the elapsed duration — `daft exec`'s
    /// interrupted workers. The label stays imperative like a failure.
    Cancelled,
    /// The step resolved without running, and that is the expected quiet
    /// case (config off, nothing to do). Renders dim.
    SkippedExpected { reason: String },
    /// The step resolved without running for an attention-worthy reason
    /// (repository not trusted, `--skip-hooks`). Renders yellow.
    SkippedAttention { reason: String },
    /// The step resolved as a no-op that warrants no record (carry with a
    /// clean source tree). The row is removed — the finished rail lists only
    /// steps that actually happened.
    SkippedSilent,
    /// Update the row's annotation while the step is pending or active.
    Note(String),
}

/// One row of the committed plan.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Row {
    /// An executable step.
    Step(StepSpec),
    /// A dim structural anchor grouping the rows below it (multi-branch
    /// remove renders the branch name this way). Its span runs to the next
    /// `Group`, an `EndGroup`, or the end of the plan — whichever comes
    /// first.
    Group { label: String },
    /// Invisible terminator closing the innermost open `Group` span: rows
    /// after it are ungrouped. Needed when grouped rows are followed by
    /// ungrouped ones (the shared-files section sits before the ungrouped
    /// hooks row); plans grouped end-to-end (multi-branch remove) don't
    /// need it.
    EndGroup,
    /// A non-step annotation rendered at its plan position (e.g. remove's
    /// "no remote branch" when remote deletion is on but has no target).
    Note { text: String },
}

/// Specification of a single plan step at commit time.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StepSpec {
    pub key: StepKey,
    /// Fixed label overriding the stage's tense table in every phase. For
    /// rows whose identity IS their subject (a shared file's path); the
    /// row's state then lives entirely in the face glyph.
    pub label: Option<String>,
    /// Second-column annotation (path, `← origin/x`, `→ origin/x`, job
    /// count…). May be patched later via `StageEvent`.
    pub annotation: Option<String>,
    /// Render the row as already done, with this duration. Used by clone
    /// for the bare-clone phase, which completes before the plan can be
    /// committed (the layout prompt sits between them).
    pub pre_completed: Option<Duration>,
}

impl StepSpec {
    pub fn new(key: StepKey) -> Self {
        Self {
            key,
            label: None,
            annotation: None,
            pre_completed: None,
        }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn with_annotation(mut self, annotation: impl Into<String>) -> Self {
        self.annotation = Some(annotation.into());
        self
    }

    pub fn pre_completed(mut self, elapsed: Duration) -> Self {
        self.pre_completed = Some(elapsed);
        self
    }
}

/// The plan a core commits right before mutation begins.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PlanCommit {
    /// Optional resolved replacement for the header text seeded at
    /// `Timeline::new`. The seed is built by the command layer from raw
    /// args; a core sets this when resolution improves on it (`daft remove
    /// .` resolves the worktree-path shorthand to its branch name).
    pub header: Option<String>,
    /// Optional annotation appended to the timeline header (e.g. `← master`
    /// once the base branch is resolved). The header text itself is seeded
    /// by the command layer, which knows the verb and target.
    pub header_annotation: Option<String>,
    pub rows: Vec<Row>,
}

impl PlanCommit {
    pub fn new(rows: Vec<Row>) -> Self {
        Self {
            header: None,
            header_annotation: None,
            rows,
        }
    }

    pub fn with_header(mut self, header: impl Into<String>) -> Self {
        self.header = Some(header.into());
        self
    }

    pub fn with_header_annotation(mut self, annotation: impl Into<String>) -> Self {
        self.header_annotation = Some(annotation.into());
        self
    }

    /// Convenience: the specs of all `Row::Step` rows, in plan order.
    pub fn steps(&self) -> impl Iterator<Item = &StepSpec> {
        self.rows.iter().filter_map(|r| match r {
            Row::Step(spec) => Some(spec),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_phases_are_identified() {
        assert!(StageId::PostCreateHooks.is_hook_phase());
        assert!(StageId::PreRemoveHooks.is_hook_phase());
        assert!(StageId::PostCloneHooks.is_hook_phase());
        assert!(!StageId::CreateWorktree.is_hook_phase());
        assert!(!StageId::Push.is_hook_phase());
    }

    #[test]
    fn scoped_keys_differ_by_scope() {
        let a = StepKey::scoped(StageId::RemoveWorktree, "feat/a");
        let b = StepKey::scoped(StageId::RemoveWorktree, "feat/b");
        assert_ne!(a, b);
        assert_eq!(a, StepKey::scoped(StageId::RemoveWorktree, "feat/a"));
    }

    #[test]
    fn plan_steps_iterates_step_rows_only() {
        let plan = PlanCommit::new(vec![
            Row::Group {
                label: "feat/a".into(),
            },
            Row::Step(StepSpec::new(StepKey::new(StageId::RemoveWorktree))),
            Row::Note {
                text: "no remote branch".into(),
            },
            Row::Step(StepSpec::new(StepKey::new(StageId::DeleteLocalBranch))),
        ]);
        assert_eq!(plan.steps().count(), 2);
    }
}
