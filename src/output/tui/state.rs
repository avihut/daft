use crate::core::sort::SortSpec;
use crate::core::worktree::info_field::FieldSet;
use crate::core::worktree::list::{EntryKind, Stat, WorktreeInfo};
use crate::core::worktree::sync_dag::{
    DagEvent, DagHookPhase, DeferReason, JobCompletionStatus, OperationPhase, TaskMessage,
    TaskStatus, ThrottleReason,
};
use std::path::PathBuf;
use std::time::Duration;

use super::columns::Column;
use super::live_table::{LiveTable, LiveTableConfig};

/// Status of a high-level operation phase in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseStatus {
    Pending,
    Active,
    Completed,
}

/// State of a single operation phase for the header display.
#[derive(Debug, Clone)]
pub struct PhaseState {
    pub phase: OperationPhase,
    pub status: PhaseStatus,
}

/// Display status for a single worktree row in the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeStatus {
    Idle,
    /// Ready to run but held back by the resource governor (#678),
    /// e.g. "held: memory". Rendered dim, like `Idle`.
    Throttled(String),
    Active(String), // e.g. "updating", "rebasing"
    Done(FinalStatus),
}

/// Final status after an operation completes for a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalStatus {
    Updated,
    UpToDate,
    Rebased,
    Conflict,
    Diverged,
    Skipped,
    Pruned,
    /// Prune skipped because the worktree has uncommitted changes.
    Dirty,
    Failed,
    Pushed,
    NoPushUpstream,
}

/// Status of a single hook sub-row (for -v mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookSubStatus {
    Running,
    Succeeded(Duration),
    Warned(Duration),
    Failed(Duration),
}

/// A hook sub-row displayed beneath a worktree row in -v mode.
#[derive(Debug, Clone)]
pub struct HookSubRow {
    pub hook_type: DagHookPhase,
    pub status: HookSubStatus,
    pub job_sub_rows: Vec<JobSubRow>,
}

/// Status of a single job sub-row within a hook (for -v mode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobSubStatus {
    Running,
    Succeeded(Duration),
    Failed(Duration),
    Skipped { duration: Duration, reason: String },
}

/// A job sub-row displayed beneath a hook sub-row in -v mode.
#[derive(Debug, Clone)]
pub struct JobSubRow {
    pub name: String,
    pub status: JobSubStatus,
}

/// Entry for the post-TUI hook summary (printed after TUI exits on warning/failure).
#[derive(Debug, Clone)]
pub struct HookSummaryEntry {
    pub branch_name: String,
    pub hook_type: DagHookPhase,
    pub success: bool,
    pub warned: bool,
    pub duration: Duration,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
}

/// Accumulated resource-governor visibility (#678), surfaced as a one-line
/// post-TUI summary ("2 pushes throttled 14s to preserve memory headroom").
#[derive(Debug, Clone, Default)]
pub struct GovernorSummary {
    /// Distinct pushes the governor ever held back.
    pub throttled_pushes: usize,
    /// Total held time, summed across pushes.
    pub throttled_total: Duration,
}

/// Complete TUI state, rebuilt from DagEvents.
pub struct TuiState {
    pub phases: Vec<PhaseState>,
    pub done: bool,
    pub tick: usize,
    pub hook_summaries: Vec<HookSummaryEntry>,
    pub show_hook_sub_rows: bool,
    /// Wall-clock duration since the renderer started. Updated by `tick()`
    /// from the driver's render-loop start instant. Used by the verbose
    /// footer.
    pub render_start_elapsed: std::time::Duration,
    /// Worktree-rows widget (rows, sort, partition, patch application).
    pub live: LiveTable,
    /// Governor throttle accounting for the post-TUI summary (#678).
    pub governor: GovernorSummary,
    /// Branches already counted in `governor.throttled_pushes`.
    governor_throttled_seen: std::collections::HashSet<String>,
}

/// A single row in the worktree table.
pub struct WorktreeRow {
    pub info: WorktreeInfo,
    pub status: WorktreeStatus,
    /// Saved terminal status. When `TaskStarted` overwrites a `Done(...)`
    /// status with `Active(...)`, the previous Done value is saved here.
    /// `PreconditionFailed` restores it.
    pub prev_terminal_status: Option<WorktreeStatus>,
    pub hook_warned: bool,
    pub hook_failed: bool,
    pub hook_sub_rows: Vec<HookSubRow>,
    /// Human-readable reason for a `FinalStatus::Failed` outcome, if available.
    pub failure_reason: Option<String>,
    /// When the governor first deferred this row's current wait (#678);
    /// cleared (and accumulated into the governor summary) on `TaskStarted`.
    pub throttled_since: Option<std::time::Instant>,
}

impl WorktreeRow {
    pub(crate) fn idle(info: WorktreeInfo) -> Self {
        Self {
            info,
            status: WorktreeStatus::Idle,
            prev_terminal_status: None,
            hook_warned: false,
            hook_failed: false,
            hook_sub_rows: Vec::new(),
            failure_reason: None,
            throttled_since: None,
        }
    }

    pub(crate) fn placeholder() -> Self {
        Self::idle(WorktreeInfo::empty(""))
    }
}

impl TuiState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        phases: Vec<OperationPhase>,
        worktree_infos: Vec<WorktreeInfo>,
        project_root: PathBuf,
        cwd: PathBuf,
        stat: Stat,
        verbose: u8,
        columns: Option<Vec<Column>>,
        columns_explicit: bool,
        unowned_start_index: Option<usize>,
        sort_spec: Option<SortSpec>,
        pin_default_branch: bool,
        partition_by_owner: bool,
        seeded_fields: FieldSet,
    ) -> Self {
        // For prune/sync the caller passes `partition_by_owner: false` and
        // injects an externally-computed `unowned_start_index` (richer
        // predicate via `is_branch_included` with include_filters). Setting
        // `partition_by_owner: true` would let
        // `LiveTable::resort_and_repartition` overwrite that injected value,
        // so callers that supply an external boundary MUST pass `false`.
        // `daft list` (Phase 2) sets `true` to let LiveTable own partitioning.
        let cfg = LiveTableConfig {
            stat,
            columns,
            columns_explicit,
            sort_spec,
            pin_default_branch,
            partition_by_owner,
            project_root,
            cwd,
            seeded_fields,
        };
        let mut live = LiveTable::new(worktree_infos, cfg);
        live.unowned_start_index = unowned_start_index;
        Self {
            phases: phases
                .into_iter()
                .map(|phase| PhaseState {
                    phase,
                    status: PhaseStatus::Pending,
                })
                .collect(),
            done: false,
            tick: 0,
            hook_summaries: Vec::new(),
            show_hook_sub_rows: verbose >= 1,
            render_start_elapsed: Duration::ZERO,
            live,
            governor: GovernorSummary::default(),
            governor_throttled_seen: std::collections::HashSet::new(),
        }
    }

    /// True when the table has reached a terminal state and the renderer
    /// should exit. For commands with phases (prune/sync/clone), this means
    /// `done` was set by `DagEvent::AllDone`. For phase-less commands
    /// (`daft list`), it also returns true once
    /// `live.collection_complete` is set by `WorktreeInfoCollectionDone`.
    pub fn is_complete(&self) -> bool {
        self.done || (self.phases.is_empty() && self.live.collection_complete)
    }

    pub fn apply_event(&mut self, event: &DagEvent) {
        match event {
            DagEvent::TaskStarted { phase, branch_name } => {
                self.activate_phase(phase);
                let active_label = match phase {
                    OperationPhase::Fetch => "fetching",
                    OperationPhase::Prune => "pruning",
                    OperationPhase::Update => "updating",
                    OperationPhase::Rebase(_) => "rebasing",
                    OperationPhase::Push => "pushing",
                    OperationPhase::Setup => "setting up",
                    OperationPhase::RemoveRepo => "removing",
                };
                // Auto-create row for newly discovered branches (e.g., gone branches
                // found after fetch completes while TUI is already running).
                if !branch_name.is_empty() && self.find_row_mut(branch_name).is_none() {
                    let kind = if matches!(phase, OperationPhase::Prune) {
                        EntryKind::LocalBranch
                    } else {
                        EntryKind::Worktree
                    };
                    self.live.push_row(WorktreeInfo {
                        kind,
                        ..WorktreeInfo::empty(branch_name)
                    });
                }
                let mut throttled_for = None;
                if let Some(row) = self.find_row_mut(branch_name) {
                    // Save terminal status so PreconditionFailed can restore it.
                    if matches!(row.status, WorktreeStatus::Done(_)) {
                        row.prev_terminal_status = Some(row.status.clone());
                    }
                    row.status = WorktreeStatus::Active(active_label.into());
                    // A held push finally launched: fold its wait into the
                    // governor summary (#678).
                    throttled_for = row.throttled_since.take().map(|since| since.elapsed());
                }
                if let Some(waited) = throttled_for {
                    self.governor.throttled_total += waited;
                }
            }
            DagEvent::TaskCompleted {
                phase,
                branch_name,
                status,
                message,
            } => {
                if *status == TaskStatus::PreconditionFailed {
                    // Restore the previous terminal status (saved by TaskStarted).
                    if let Some(row) = self.find_row_mut(branch_name)
                        && let Some(prev) = row.prev_terminal_status.take()
                    {
                        row.status = prev;
                    }
                    self.check_phase_completion(phase);
                } else {
                    let final_status = Self::map_final_status(phase, *status, message);
                    let failure_reason = if *status == TaskStatus::Failed {
                        if let TaskMessage::Failed(reason) = message {
                            Some(reason.clone())
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(row) = self.find_row_mut(branch_name) {
                        row.prev_terminal_status = None;
                        row.status = WorktreeStatus::Done(final_status);
                        if failure_reason.is_some() {
                            row.failure_reason = failure_reason;
                        }
                        // Refreshed WorktreeInfo cells now flow as
                        // `WorktreeInfoUpdated` patches with
                        // `PatchSource::PostTask(phase)` — see `LiveTable`.
                    }
                    self.check_phase_completion(phase);
                }
            }
            DagEvent::TaskThrottled {
                phase,
                branch_name,
                reason,
            } => {
                // The phase is genuinely in progress — work is queued and
                // deliberately held, not merely pending.
                self.activate_phase(phase);
                let label = match reason {
                    ThrottleReason::Deferred(DeferReason::ClassCap) => "held: capped",
                    ThrottleReason::Deferred(
                        DeferReason::MemoryPressure | DeferReason::KillCooldown,
                    ) => "held: memory",
                    ThrottleReason::Frozen => "held: frozen",
                    ThrottleReason::Evicted { .. } => "held: retry",
                };
                if self.governor_throttled_seen.insert(branch_name.clone()) {
                    self.governor.throttled_pushes += 1;
                }
                if let Some(row) = self.find_row_mut(branch_name) {
                    // Save the prior terminal status (normally the branch's
                    // update/rebase outcome) exactly like TaskStarted does —
                    // by the time the admitted task starts, `row.status` is
                    // Throttled, so TaskStarted's own save can't.
                    if matches!(row.status, WorktreeStatus::Done(_)) {
                        row.prev_terminal_status = Some(row.status.clone());
                    }
                    row.status = WorktreeStatus::Throttled(label.into());
                    // A freeze pauses mid-run work; only queue-waits count
                    // toward the throttle summary clock.
                    if matches!(
                        reason,
                        ThrottleReason::Deferred(_) | ThrottleReason::Evicted { .. }
                    ) && row.throttled_since.is_none()
                    {
                        row.throttled_since = Some(std::time::Instant::now());
                    }
                }
            }
            DagEvent::TaskResumed { phase, branch_name } => {
                // A thawed unit is running again — restore the active label
                // its TaskStarted originally set.
                let _ = phase;
                if let Some(row) = self.find_row_mut(branch_name)
                    && matches!(row.status, WorktreeStatus::Throttled(_))
                {
                    row.status = WorktreeStatus::Active("pushing".into());
                }
            }
            DagEvent::AllDone => {
                for phase in &mut self.phases {
                    if phase.status != PhaseStatus::Completed {
                        phase.status = PhaseStatus::Completed;
                    }
                }
                // Mark any worktrees that were never touched as up-to-date.
                // This covers prune (where only gone branches get events) and
                // any other scenario where some rows stay idle.
                for wt in &mut self.live.rows {
                    if wt.status == WorktreeStatus::Idle {
                        wt.status = WorktreeStatus::Done(FinalStatus::UpToDate);
                    }
                }
                self.done = true;
            }
            DagEvent::HookStarted {
                branch_name,
                hook_type,
            } => {
                let show_sub_rows = self.show_hook_sub_rows;
                if let Some(row) = self.find_row_mut(branch_name) {
                    // Update status label to show current hook phase.
                    // Short labels stay within STATUS_MAX_WIDTH and avoid
                    // column width jumps in the table layout.
                    row.status = WorktreeStatus::Active(hook_type.label().to_string());
                    // Add sub-row if in verbose TUI mode
                    if show_sub_rows {
                        row.hook_sub_rows.push(HookSubRow {
                            hook_type: *hook_type,
                            status: HookSubStatus::Running,
                            job_sub_rows: Vec::new(),
                        });
                    }
                }
            }
            DagEvent::HookCompleted {
                branch_name,
                hook_type,
                success,
                warned,
                duration,
                exit_code,
                output,
            } => {
                let show_sub_rows = self.show_hook_sub_rows;
                if let Some(row) = self.find_row_mut(branch_name) {
                    if *warned {
                        row.hook_warned = true;
                    }
                    if !success && !warned {
                        row.hook_failed = true;
                    }
                    // Update sub-row status
                    if show_sub_rows
                        && let Some(sub) = row
                            .hook_sub_rows
                            .iter_mut()
                            .rfind(|s| s.hook_type == *hook_type)
                    {
                        sub.status = if *warned {
                            HookSubStatus::Warned(*duration)
                        } else if *success {
                            HookSubStatus::Succeeded(*duration)
                        } else {
                            HookSubStatus::Failed(*duration)
                        };
                    }
                }
                // Accumulate for post-TUI summary if non-success
                if *warned || !success {
                    self.hook_summaries.push(HookSummaryEntry {
                        branch_name: branch_name.clone(),
                        hook_type: *hook_type,
                        success: *success,
                        warned: *warned,
                        duration: *duration,
                        exit_code: *exit_code,
                        output: output.clone(),
                    });
                }
            }
            DagEvent::JobStarted {
                branch_name,
                hook_type,
                job_name,
            } => {
                if self.show_hook_sub_rows
                    && let Some(row) = self.find_row_mut(branch_name)
                    && let Some(hook_sub) = row
                        .hook_sub_rows
                        .iter_mut()
                        .rfind(|s| s.hook_type == *hook_type)
                {
                    hook_sub.job_sub_rows.push(JobSubRow {
                        name: job_name.clone(),
                        status: JobSubStatus::Running,
                    });
                }
            }
            DagEvent::JobCompleted {
                branch_name,
                hook_type,
                job_name,
                status,
                duration,
                skip_reason,
            } => {
                if self.show_hook_sub_rows
                    && let Some(row) = self.find_row_mut(branch_name)
                    && let Some(hook_sub) = row
                        .hook_sub_rows
                        .iter_mut()
                        .rfind(|s| s.hook_type == *hook_type)
                    && let Some(job_sub) = hook_sub
                        .job_sub_rows
                        .iter_mut()
                        .rfind(|j| j.name == *job_name)
                {
                    job_sub.status = match status {
                        JobCompletionStatus::Succeeded => JobSubStatus::Succeeded(*duration),
                        JobCompletionStatus::Failed => JobSubStatus::Failed(*duration),
                        JobCompletionStatus::Skipped => JobSubStatus::Skipped {
                            duration: *duration,
                            reason: skip_reason.clone().unwrap_or_default(),
                        },
                    };
                }
            }
            DagEvent::WorktreeInfoUpdated { .. } | DagEvent::WorktreeInfoCollectionDone => {
                self.live.apply_event(event);
            }
        }
    }

    pub fn tick(&mut self) {
        self.tick += 1;
        self.live.tick();
    }

    fn activate_phase(&mut self, phase: &OperationPhase) {
        if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase)
            && ps.status == PhaseStatus::Pending
        {
            ps.status = PhaseStatus::Active;
        }
    }

    fn check_phase_completion(&mut self, phase: &OperationPhase) {
        // A phase is complete when all tasks belonging to it have a terminal status
        // in the worktree rows, and no rows show an Active label for this phase.
        let phase_active_label = match phase {
            OperationPhase::Fetch => "fetching",
            OperationPhase::Prune => "pruning",
            OperationPhase::Update => "updating",
            OperationPhase::Rebase(_) => "rebasing",
            OperationPhase::Push => "pushing",
            OperationPhase::Setup => "setting up",
            OperationPhase::RemoveRepo => "removing",
        };
        let any_active = self.live.rows.iter().any(
            |w| matches!(&w.status, WorktreeStatus::Active(label) if label == phase_active_label),
        );
        if !any_active
            && let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase)
            && ps.status == PhaseStatus::Active
        {
            ps.status = PhaseStatus::Completed;
        }
    }

    fn find_row_mut(&mut self, branch_name: &str) -> Option<&mut WorktreeRow> {
        self.live
            .rows
            .iter_mut()
            .find(|w| w.info.name == branch_name)
    }

    fn map_final_status(
        phase: &OperationPhase,
        status: TaskStatus,
        message: &TaskMessage,
    ) -> FinalStatus {
        match status {
            TaskStatus::Failed => FinalStatus::Failed,
            TaskStatus::DepFailed => FinalStatus::Skipped,
            TaskStatus::Skipped => FinalStatus::Skipped,
            TaskStatus::Succeeded => match phase {
                OperationPhase::Prune => match message {
                    TaskMessage::Removed | TaskMessage::Deferred => FinalStatus::Pruned,
                    TaskMessage::SkippedDirty => FinalStatus::Dirty,
                    TaskMessage::SkippedRefined | TaskMessage::SkippedUnmerged => {
                        FinalStatus::Skipped
                    }
                    TaskMessage::NoActionNeeded => FinalStatus::UpToDate,
                    _ => FinalStatus::UpToDate,
                },
                OperationPhase::Update => match message {
                    TaskMessage::UpToDate => FinalStatus::UpToDate,
                    TaskMessage::Diverged => FinalStatus::Diverged,
                    _ => FinalStatus::Updated,
                },
                OperationPhase::Rebase(_) => match message {
                    TaskMessage::Conflict => FinalStatus::Conflict,
                    _ => FinalStatus::Rebased,
                },
                OperationPhase::Push => match message {
                    TaskMessage::UpToDate => FinalStatus::UpToDate,
                    TaskMessage::NoPushUpstream => FinalStatus::NoPushUpstream,
                    TaskMessage::Pushed | TaskMessage::Ok(_) => FinalStatus::Pushed,
                    _ => FinalStatus::Diverged,
                },
                OperationPhase::Fetch => FinalStatus::Updated,
                OperationPhase::Setup => match message {
                    TaskMessage::Created => FinalStatus::Updated,
                    TaskMessage::BaseCreated => FinalStatus::Updated,
                    TaskMessage::NotFound => FinalStatus::Skipped,
                    _ => FinalStatus::Updated,
                },
                OperationPhase::RemoveRepo => match message {
                    TaskMessage::Removed | TaskMessage::Deferred => FinalStatus::Pruned,
                    TaskMessage::SkippedDirty => FinalStatus::Dirty,
                    TaskMessage::SkippedRefined | TaskMessage::SkippedUnmerged => {
                        FinalStatus::Skipped
                    }
                    TaskMessage::NoActionNeeded => FinalStatus::UpToDate,
                    _ => FinalStatus::UpToDate,
                },
            },
            TaskStatus::PreconditionFailed => FinalStatus::Skipped,
            // Cancelled is a user decision, not a failure — render like a
            // skip so check_tui_failures never counts it. The run-level
            // "cancelled" state comes from the live region, and the exit
            // code (130) from the sync command itself.
            TaskStatus::Cancelled => FinalStatus::Skipped,
            // Evicted is transient (the executor requeues it and never puts
            // it in a TaskCompleted event) — defensive arm only.
            TaskStatus::Pending | TaskStatus::Running | TaskStatus::Evicted => FinalStatus::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::sync_dag::*;
    use crate::hooks::HookType;

    fn make_test_state() -> TuiState {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
        ];

        let worktree_infos = vec![
            WorktreeInfo::empty("master"),
            WorktreeInfo::empty("feat/a"),
            WorktreeInfo::empty("feat/old"),
        ];

        TuiState::new(
            phases,
            worktree_infos,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        )
    }

    fn make_verbose_test_state() -> TuiState {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
        ];

        let worktree_infos = vec![
            WorktreeInfo::empty("master"),
            WorktreeInfo::empty("feat/a"),
            WorktreeInfo::empty("feat/old"),
        ];

        TuiState::new(
            phases,
            worktree_infos,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            1,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        )
    }

    #[test]
    fn task_throttled_sets_held_status_then_start_accumulates() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::TaskThrottled {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
            reason: ThrottleReason::Deferred(DeferReason::MemoryPressure),
        });
        assert!(matches!(
            &state.find_row_mut("feat/a").unwrap().status,
            WorktreeStatus::Throttled(label) if label == "held: memory"
        ));
        assert_eq!(state.governor.throttled_pushes, 1);

        // Re-throttling the same branch relabels but never double-counts.
        state.apply_event(&DagEvent::TaskThrottled {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
            reason: ThrottleReason::Deferred(DeferReason::ClassCap),
        });
        assert!(matches!(
            &state.find_row_mut("feat/a").unwrap().status,
            WorktreeStatus::Throttled(label) if label == "held: capped"
        ));
        assert_eq!(state.governor.throttled_pushes, 1);

        // Admission (TaskStarted) folds the wait into the summary and
        // clears the row's throttle stamp.
        std::thread::sleep(std::time::Duration::from_millis(15));
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
        });
        let row = state.find_row_mut("feat/a").unwrap();
        assert!(matches!(&row.status, WorktreeStatus::Active(label) if label == "pushing"));
        assert!(row.throttled_since.is_none());
        assert!(state.governor.throttled_total >= std::time::Duration::from_millis(10));
    }

    #[test]
    fn throttled_row_restores_prior_terminal_status_on_precondition_failed() {
        let mut state = make_test_state();
        // The branch's update completed…
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "feat/a".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Ok("ok".into()),
        });
        // …then its push is held, admitted, and declines to run
        // (e.g. rebase-conflict precondition).
        state.apply_event(&DagEvent::TaskThrottled {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
            reason: ThrottleReason::Deferred(DeferReason::MemoryPressure),
        });
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
            status: TaskStatus::PreconditionFailed,
            message: TaskMessage::Failed("rebase conflict".into()),
        });
        // The update outcome survives the throttle → start → decline arc.
        assert_eq!(
            state.find_row_mut("feat/a").unwrap().status,
            WorktreeStatus::Done(FinalStatus::Updated)
        );
    }

    /// Build a TuiState with a caller-specified phase list. Thin wrapper
    /// over `TuiState::new` that supplies sensible defaults for the rest of
    /// the args (no worktrees, Stat::Summary, default columns/sort, etc.).
    fn make_test_state_with_phases(phases: Vec<OperationPhase>) -> TuiState {
        TuiState::new(
            phases,
            Vec::new(),
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        )
    }

    #[test]
    fn is_complete_false_initially() {
        let state = make_test_state_with_phases(vec![OperationPhase::Fetch]);
        assert!(!state.is_complete());
    }

    #[test]
    fn is_complete_true_after_all_done_event() {
        let mut state = make_test_state_with_phases(vec![OperationPhase::Fetch]);
        state.apply_event(&DagEvent::AllDone);
        assert!(state.is_complete());
    }

    #[test]
    fn is_complete_true_after_collection_done_when_no_phases() {
        let mut state = make_test_state_with_phases(vec![]);
        state.apply_event(&DagEvent::WorktreeInfoCollectionDone);
        assert!(state.is_complete());
    }

    #[test]
    fn is_complete_false_after_collection_done_when_phases_present() {
        // When phases exist, completion still requires AllDone — collection
        // finishing is just one input among many.
        let mut state = make_test_state_with_phases(vec![OperationPhase::Fetch]);
        state.apply_event(&DagEvent::WorktreeInfoCollectionDone);
        assert!(!state.is_complete());
    }

    #[test]
    fn initial_state_all_pending() {
        let state = make_test_state();
        assert!(
            state
                .phases
                .iter()
                .all(|p| p.status == PhaseStatus::Pending)
        );
        assert!(
            state
                .live
                .rows
                .iter()
                .all(|w| w.status == WorktreeStatus::Idle)
        );
        assert!(!state.done);
    }

    #[test]
    fn task_started_activates_phase_and_row() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
        });
        assert_eq!(state.phases[0].status, PhaseStatus::Active);
    }

    #[test]
    fn task_completed_updates_row_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::UpToDate,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    #[test]
    fn all_done_sets_done_flag() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::AllDone);
        assert!(state.done);
    }

    #[test]
    fn prune_task_sets_pruned_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
        });
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pruning".into()));

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Removed,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Pruned));
    }

    #[test]
    fn prune_skipped_dirty_sets_dirty_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::SkippedDirty,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Dirty));
    }

    #[test]
    fn failed_task_sets_failed_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Failed,
            message: TaskMessage::Failed("pull failed".into()),
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Failed));
    }

    #[test]
    fn dep_failed_sets_skipped_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::DepFailed,
            message: TaskMessage::Failed("dependency failed".into()),
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Skipped));
    }

    #[test]
    fn tick_increments_counter() {
        let mut state = make_test_state();
        assert_eq!(state.tick, 0);
        state.tick();
        assert_eq!(state.tick, 1);
        state.tick();
        assert_eq!(state.tick, 2);
    }

    #[test]
    fn phase_completes_when_no_active_rows() {
        let mut state = make_test_state();
        // Start and complete the fetch task
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
        });
        assert_eq!(state.phases[0].status, PhaseStatus::Active);

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Fetch,
            branch_name: String::new(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Ok("fetched".into()),
        });
        // Fetch phase should now be completed (fetch task has empty branch_name,
        // so no row was Active for it -- but the phase was Active and now no
        // rows show "fetching")
        assert_eq!(state.phases[0].status, PhaseStatus::Completed);
    }

    #[test]
    fn all_done_completes_remaining_phases() {
        let mut state = make_test_state();
        // All phases start as Pending
        assert!(
            state
                .phases
                .iter()
                .all(|p| p.status == PhaseStatus::Pending)
        );

        state.apply_event(&DagEvent::AllDone);

        // AllDone should mark all phases as Completed
        assert!(
            state
                .phases
                .iter()
                .all(|p| p.status == PhaseStatus::Completed)
        );
    }

    #[test]
    fn update_with_changes_sets_updated_status() {
        let mut state = make_test_state();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Ok("Fast-forward".into()),
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Updated));
    }

    #[test]
    fn repo_remove_full_event_flow_keeps_one_row_per_seeded_worktree() {
        // Reproduces a user-reported scenario: removing a single-worktree
        // (non-daft layout) repo appeared to show two `master` rows in the
        // TUI ("waiting" + "pruned"). This test pins the state-machine side:
        // a row seeded by `build_tui_rows` plus the full event flow (worktree
        // task + bare task) must leave exactly one data row.
        let phases = vec![OperationPhase::RemoveRepo];
        let mut master_info = WorktreeInfo::empty("master");
        master_info.path = Some(PathBuf::from("/tmp/repo/main"));
        let mut state = TuiState::new(
            phases,
            vec![master_info],
            PathBuf::from("/tmp/repo"),
            PathBuf::from("/tmp"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            false,
            false,
            FieldSet::ALL,
        );

        // Worktree task: matches the seeded row by name; no auto-create.
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::RemoveRepo,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::RemoveRepo,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Removed,
        });

        // Bare task: empty branch_name suppresses auto-create.
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::RemoveRepo,
            branch_name: String::new(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::RemoveRepo,
            branch_name: String::new(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Removed,
        });

        assert_eq!(
            state.live.rows.len(),
            1,
            "expected exactly 1 row after full repo-remove event flow, got {}",
            state.live.rows.len()
        );
        assert_eq!(state.live.rows[0].info.name, "master");
        assert!(matches!(
            state.live.rows[0].status,
            WorktreeStatus::Done(FinalStatus::Pruned)
        ));
    }

    #[test]
    fn task_started_with_empty_branch_name_does_not_auto_create_row() {
        // Regression for the `(bare)` row reappearing in `daft repo remove`:
        // the bare-removal task fires `TaskStarted` with an empty branch_name
        // (so auto-create is skipped). The phase header still activates;
        // only the row creation is suppressed.
        let mut state = make_test_state();
        let initial_rows = state.live.rows.len();

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::RemoveRepo,
            branch_name: String::new(),
        });

        assert_eq!(
            state.live.rows.len(),
            initial_rows,
            "TaskStarted with empty branch_name must not auto-create a row",
        );
    }

    #[test]
    fn auto_creates_row_for_unknown_branch() {
        let mut state = make_test_state();
        assert_eq!(state.live.rows.len(), 3);

        // A TaskStarted for an unknown branch should auto-create a row
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/discovered".into(),
        });

        assert_eq!(state.live.rows.len(), 4);
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/discovered")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pruning".into()));
    }

    #[test]
    fn all_done_marks_idle_rows_as_up_to_date() {
        let mut state = make_test_state();

        // Only one branch gets a prune event; the others stay Idle.
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Removed,
        });

        state.apply_event(&DagEvent::AllDone);

        // feat/old was pruned
        let pruned = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(pruned.status, WorktreeStatus::Done(FinalStatus::Pruned));

        // The remaining idle rows should now be up-to-date
        let master = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(master.status, WorktreeStatus::Done(FinalStatus::UpToDate));

        let feat_a = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(feat_a.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    // The previous `task_completed_with_updated_info_merges_into_row` test
    // was deleted: refreshed `WorktreeInfo` cells now flow as
    // `WorktreeInfoUpdated` patches with `PatchSource::PostTask(phase)` rather
    // than as a `Box<WorktreeInfo>` riding on `TaskCompleted`. The patch
    // pipeline is covered by the streaming collector's own tests.

    #[test]
    fn hook_started_updates_status_label() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/old".into(),
            hook_type: HookType::PreRemove.into(),
        });
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pre-remove".into()));
    }

    #[test]
    fn hook_completed_warn_sets_flag() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::HookCompleted {
            branch_name: "feat/old".into(),
            hook_type: HookType::PreRemove.into(),
            success: false,
            warned: true,
            duration: Duration::from_millis(100),
            exit_code: Some(1),
            output: Some("warning output".into()),
        });
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert!(row.hook_warned);
        assert!(!row.hook_failed);
        assert_eq!(state.hook_summaries.len(), 1);
        assert_eq!(state.hook_summaries[0].branch_name, "feat/old");
        assert!(state.hook_summaries[0].warned);
    }

    #[test]
    fn hook_completed_success_no_summary() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::HookCompleted {
            branch_name: "master".into(),
            hook_type: HookType::PostRemove.into(),
            success: true,
            warned: false,
            duration: Duration::from_millis(50),
            exit_code: Some(0),
            output: None,
        });
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert!(!row.hook_warned);
        assert!(!row.hook_failed);
        assert!(state.hook_summaries.is_empty());
    }

    #[test]
    fn hook_sub_rows_populated_when_verbose() {
        let mut state = make_verbose_test_state();
        assert!(state.show_hook_sub_rows);

        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostRemove.into(),
        });

        {
            let row = state
                .live
                .rows
                .iter()
                .find(|w| w.info.name == "feat/a")
                .unwrap();
            assert_eq!(row.hook_sub_rows.len(), 1);
            assert_eq!(row.hook_sub_rows[0].hook_type, HookType::PostRemove.into());
            assert_eq!(row.hook_sub_rows[0].status, HookSubStatus::Running);
        }

        let dur = Duration::from_millis(200);
        state.apply_event(&DagEvent::HookCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostRemove.into(),
            success: true,
            warned: false,
            duration: dur,
            exit_code: Some(0),
            output: None,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(row.hook_sub_rows.len(), 1);
        assert_eq!(
            row.hook_sub_rows[0].status,
            HookSubStatus::Succeeded(Duration::from_millis(200))
        );
    }

    #[test]
    fn job_started_creates_sub_row() {
        let mut state = make_verbose_test_state();

        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
            job_name: "build".into(),
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(row.hook_sub_rows.len(), 1);
        assert_eq!(row.hook_sub_rows[0].job_sub_rows.len(), 1);
        assert_eq!(row.hook_sub_rows[0].job_sub_rows[0].name, "build");
        assert_eq!(
            row.hook_sub_rows[0].job_sub_rows[0].status,
            JobSubStatus::Running
        );
    }

    #[test]
    fn job_completed_updates_status() {
        let mut state = make_verbose_test_state();

        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
            job_name: "build".into(),
        });
        state.apply_event(&DagEvent::JobCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
            job_name: "build".into(),
            status: JobCompletionStatus::Succeeded,
            duration: Duration::from_millis(150),
            skip_reason: None,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(
            row.hook_sub_rows[0].job_sub_rows[0].status,
            JobSubStatus::Succeeded(Duration::from_millis(150))
        );
    }

    #[test]
    fn multiple_jobs_within_hook() {
        let mut state = make_verbose_test_state();

        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove.into(),
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove.into(),
            job_name: "cleanup".into(),
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove.into(),
            job_name: "notify".into(),
        });
        state.apply_event(&DagEvent::JobCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove.into(),
            job_name: "cleanup".into(),
            status: JobCompletionStatus::Succeeded,
            duration: Duration::from_millis(100),
            skip_reason: None,
        });
        state.apply_event(&DagEvent::JobCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove.into(),
            job_name: "notify".into(),
            status: JobCompletionStatus::Failed,
            duration: Duration::from_millis(200),
            skip_reason: None,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        let jobs = &row.hook_sub_rows[0].job_sub_rows;
        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs[0].status,
            JobSubStatus::Succeeded(Duration::from_millis(100))
        );
        assert_eq!(
            jobs[1].status,
            JobSubStatus::Failed(Duration::from_millis(200))
        );
    }

    #[test]
    fn push_phase_maps_pushed_status() {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
            OperationPhase::Push,
        ];
        let worktree_infos = vec![WorktreeInfo::empty("master"), WorktreeInfo::empty("feat/a")];
        let mut state = TuiState::new(
            phases,
            worktree_infos,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        );

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "master".into(),
        });
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pushing".into()));

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Pushed,
        });
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Pushed));
    }

    #[test]
    fn push_phase_maps_no_upstream_status() {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
            OperationPhase::Push,
        ];
        let worktree_infos = vec![WorktreeInfo::empty("feat/a")];
        let mut state = TuiState::new(
            phases,
            worktree_infos,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        );

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: "feat/a".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::NoPushUpstream,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(
            row.status,
            WorktreeStatus::Done(FinalStatus::NoPushUpstream)
        );
    }

    #[test]
    fn push_phase_maps_up_to_date_status() {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
            OperationPhase::Push,
        ];
        let worktree_infos = vec![WorktreeInfo::empty("master")];
        let mut state = TuiState::new(
            phases,
            worktree_infos,
            PathBuf::from("/tmp/test"),
            PathBuf::from("/tmp/test"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        );

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "master".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::UpToDate,
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    #[test]
    fn job_events_ignored_when_not_verbose() {
        let mut state = make_test_state();
        assert!(!state.show_hook_sub_rows);

        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate.into(),
            job_name: "build".into(),
        });

        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert!(row.hook_sub_rows.is_empty());
    }

    fn make_rebase_push_test_state() -> TuiState {
        let phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
            OperationPhase::Rebase("master".into()),
            OperationPhase::Push,
        ];
        let infos = vec![
            WorktreeInfo::empty("master"),
            WorktreeInfo::empty("feat/old"),
        ];
        TuiState::new(
            phases,
            infos,
            PathBuf::from("/projects/test"),
            PathBuf::from("/projects/test/master"),
            Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
            FieldSet::EMPTY,
        )
    }

    #[test]
    fn precondition_failed_restores_previous_terminal_status() {
        let mut state = make_rebase_push_test_state();

        // Rebase completes with conflict
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Rebase("master".into()),
            branch_name: "feat/old".into(),
        });
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Rebase("master".into()),
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Conflict,
        });

        // Verify row shows conflict
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Conflict));

        // Push TaskStarted overwrites to Active
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "feat/old".into(),
        });

        // Push completes with PreconditionFailed
        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: "feat/old".into(),
            status: TaskStatus::PreconditionFailed,
            message: TaskMessage::Failed("rebase conflict".into()),
        });

        // Row should show conflict again (restored from prev_terminal_status)
        let row = state
            .live
            .rows
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(
            row.status,
            WorktreeStatus::Done(FinalStatus::Conflict),
            "PreconditionFailed push should restore conflict status"
        );
    }
}
