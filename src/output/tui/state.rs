use crate::core::sort::SortSpec;
use crate::core::worktree::list::{EntryKind, Stat, WorktreeInfo};
use crate::core::worktree::sync_dag::{
    DagEvent, JobCompletionStatus, OperationPhase, TaskMessage, TaskStatus,
};
use crate::hooks::HookType;
use std::path::PathBuf;
use std::time::Duration;

use super::columns::Column;

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
    pub hook_type: HookType,
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
    pub hook_type: HookType,
    pub success: bool,
    pub warned: bool,
    pub duration: Duration,
    pub exit_code: Option<i32>,
    pub output: Option<String>,
}

/// Complete TUI state, rebuilt from DagEvents.
pub struct TuiState {
    pub phases: Vec<PhaseState>,
    pub worktrees: Vec<WorktreeRow>,
    pub done: bool,
    pub tick: usize,
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub stat: Stat,
    pub hook_summaries: Vec<HookSummaryEntry>,
    pub show_hook_sub_rows: bool,
    /// User-selected columns (None = use responsive selection).
    pub columns: Option<Vec<Column>>,
    /// If true, the user explicitly chose columns (replace mode) — disables responsive dropping.
    pub columns_explicit: bool,
    /// Index of the first unowned worktree row (None if no unowned section).
    pub unowned_start_index: Option<usize>,
    /// User-specified sort order (None = default alphabetical).
    pub sort_spec: Option<SortSpec>,
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
    ) -> Self {
        let mut worktrees: Vec<WorktreeRow> = worktree_infos
            .into_iter()
            .map(|info| WorktreeRow {
                info,
                status: WorktreeStatus::Idle,
                prev_terminal_status: None,
                hook_warned: false,
                hook_failed: false,
                hook_sub_rows: Vec::new(),
            })
            .collect();
        worktrees.sort_by(|a, b| {
            // Default branch always first.
            let default_order = |w: &WorktreeRow| u8::from(!w.info.is_default_branch);
            let kind_order = |k: &EntryKind| match k {
                EntryKind::Worktree => 0,
                EntryKind::LocalBranch => 1,
                EntryKind::RemoteBranch => 2,
            };
            default_order(a)
                .cmp(&default_order(b))
                .then_with(|| kind_order(&a.info.kind).cmp(&kind_order(&b.info.kind)))
                .then_with(|| match &sort_spec {
                    Some(spec) => spec.compare(&a.info, &b.info),
                    None => a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()),
                })
        });
        Self {
            phases: phases
                .into_iter()
                .map(|phase| PhaseState {
                    phase,
                    status: PhaseStatus::Pending,
                })
                .collect(),
            worktrees,
            done: false,
            tick: 0,
            project_root,
            cwd,
            stat,
            hook_summaries: Vec::new(),
            show_hook_sub_rows: verbose >= 1,
            columns,
            columns_explicit,
            unowned_start_index,
            sort_spec,
        }
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
                };
                // Auto-create row for newly discovered branches (e.g., gone branches
                // found after fetch completes while TUI is already running).
                if !branch_name.is_empty() && self.find_row_mut(branch_name).is_none() {
                    let kind = if matches!(phase, OperationPhase::Prune) {
                        EntryKind::LocalBranch
                    } else {
                        EntryKind::Worktree
                    };
                    self.worktrees.push(WorktreeRow {
                        info: WorktreeInfo {
                            kind,
                            ..WorktreeInfo::empty(branch_name)
                        },
                        status: WorktreeStatus::Idle,
                        prev_terminal_status: None,
                        hook_warned: false,
                        hook_failed: false,
                        hook_sub_rows: Vec::new(),
                    });
                }
                if let Some(row) = self.find_row_mut(branch_name) {
                    // Save terminal status so PreconditionFailed can restore it.
                    if matches!(row.status, WorktreeStatus::Done(_)) {
                        row.prev_terminal_status = Some(row.status.clone());
                    }
                    row.status = WorktreeStatus::Active(active_label.into());
                }
            }
            DagEvent::TaskCompleted {
                phase,
                branch_name,
                status,
                message,
                updated_info,
            } => {
                if *status == TaskStatus::PreconditionFailed {
                    // Restore the previous terminal status (saved by TaskStarted).
                    if let Some(row) = self.find_row_mut(branch_name) {
                        if let Some(prev) = row.prev_terminal_status.take() {
                            row.status = prev;
                        }
                    }
                    self.check_phase_completion(phase);
                } else {
                    let final_status = Self::map_final_status(phase, *status, message);
                    if let Some(row) = self.find_row_mut(branch_name) {
                        row.prev_terminal_status = None;
                        row.status = WorktreeStatus::Done(final_status);
                        if let Some(new_info) = updated_info {
                            row.info = *new_info.clone();
                        }
                    }
                    self.check_phase_completion(phase);
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
                for wt in &mut self.worktrees {
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
                    // Use short labels to stay within STATUS_MAX_WIDTH and avoid
                    // column width jumps in the table layout.
                    let label = match hook_type {
                        HookType::PreRemove => "pre-remove",
                        HookType::PostRemove => "post-remove",
                        HookType::PreCreate => "pre-create",
                        HookType::PostCreate => "post-create",
                        HookType::PostClone => "post-clone",
                    };
                    row.status = WorktreeStatus::Active(label.to_string());
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
                    if show_sub_rows {
                        if let Some(sub) = row
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
                if self.show_hook_sub_rows {
                    if let Some(row) = self.find_row_mut(branch_name) {
                        if let Some(hook_sub) = row
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
                if self.show_hook_sub_rows {
                    if let Some(row) = self.find_row_mut(branch_name) {
                        if let Some(hook_sub) = row
                            .hook_sub_rows
                            .iter_mut()
                            .rfind(|s| s.hook_type == *hook_type)
                        {
                            if let Some(job_sub) = hook_sub
                                .job_sub_rows
                                .iter_mut()
                                .rfind(|j| j.name == *job_name)
                            {
                                job_sub.status = match status {
                                    JobCompletionStatus::Succeeded => {
                                        JobSubStatus::Succeeded(*duration)
                                    }
                                    JobCompletionStatus::Failed => JobSubStatus::Failed(*duration),
                                    JobCompletionStatus::Skipped => JobSubStatus::Skipped {
                                        duration: *duration,
                                        reason: skip_reason.clone().unwrap_or_default(),
                                    },
                                };
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn tick(&mut self) {
        self.tick += 1;
    }

    fn activate_phase(&mut self, phase: &OperationPhase) {
        if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase) {
            if ps.status == PhaseStatus::Pending {
                ps.status = PhaseStatus::Active;
            }
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
        };
        let any_active = self.worktrees.iter().any(
            |w| matches!(&w.status, WorktreeStatus::Active(label) if label == phase_active_label),
        );
        if !any_active {
            if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase) {
                if ps.status == PhaseStatus::Active {
                    ps.status = PhaseStatus::Completed;
                }
            }
        }
    }

    fn find_row_mut(&mut self, branch_name: &str) -> Option<&mut WorktreeRow> {
        self.worktrees
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
            },
            TaskStatus::PreconditionFailed => FinalStatus::Skipped,
            TaskStatus::Pending | TaskStatus::Running => FinalStatus::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::sync_dag::*;

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
        )
    }

    #[test]
    fn initial_state_all_pending() {
        let state = make_test_state();
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));
        assert!(state
            .worktrees
            .iter()
            .all(|w| w.status == WorktreeStatus::Idle));
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
            updated_info: None,
        });

        let row = state
            .worktrees
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
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pruning".into()));

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Prune,
            branch_name: "feat/old".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Removed,
            updated_info: None,
        });

        let row = state
            .worktrees
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
            updated_info: None,
        });

        let row = state
            .worktrees
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
            updated_info: None,
        });

        let row = state
            .worktrees
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
            updated_info: None,
        });

        let row = state
            .worktrees
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
            updated_info: None,
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
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));

        state.apply_event(&DagEvent::AllDone);

        // AllDone should mark all phases as Completed
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Completed));
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
            updated_info: None,
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Updated));
    }

    #[test]
    fn auto_creates_row_for_unknown_branch() {
        let mut state = make_test_state();
        assert_eq!(state.worktrees.len(), 3);

        // A TaskStarted for an unknown branch should auto-create a row
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Prune,
            branch_name: "feat/discovered".into(),
        });

        assert_eq!(state.worktrees.len(), 4);
        let row = state
            .worktrees
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
            updated_info: None,
        });

        state.apply_event(&DagEvent::AllDone);

        // feat/old was pruned
        let pruned = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(pruned.status, WorktreeStatus::Done(FinalStatus::Pruned));

        // The remaining idle rows should now be up-to-date
        let master = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(master.status, WorktreeStatus::Done(FinalStatus::UpToDate));

        let feat_a = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/a")
            .unwrap();
        assert_eq!(feat_a.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    #[test]
    fn task_completed_with_updated_info_merges_into_row() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
        });

        let mut new_info = WorktreeInfo::empty("master");
        new_info.remote_ahead = Some(0);
        new_info.remote_behind = Some(0);
        new_info.ahead = Some(0);
        new_info.behind = Some(0);

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Update,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Ok("Fast-forward".into()),
            updated_info: Some(Box::new(new_info)),
        });

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.info.remote_ahead, Some(0));
        assert_eq!(row.info.remote_behind, Some(0));
        assert_eq!(row.info.ahead, Some(0));
        assert_eq!(row.info.behind, Some(0));
    }

    #[test]
    fn hook_started_updates_status_label() {
        let mut state = make_test_state();
        state.apply_event(&DagEvent::HookStarted {
            branch_name: "feat/old".into(),
            hook_type: HookType::PreRemove,
        });
        let row = state
            .worktrees
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
            hook_type: HookType::PreRemove,
            success: false,
            warned: true,
            duration: Duration::from_millis(100),
            exit_code: Some(1),
            output: Some("warning output".into()),
        });
        let row = state
            .worktrees
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
            hook_type: HookType::PostRemove,
            success: true,
            warned: false,
            duration: Duration::from_millis(50),
            exit_code: Some(0),
            output: None,
        });
        let row = state
            .worktrees
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
            hook_type: HookType::PostRemove,
        });

        {
            let row = state
                .worktrees
                .iter()
                .find(|w| w.info.name == "feat/a")
                .unwrap();
            assert_eq!(row.hook_sub_rows.len(), 1);
            assert_eq!(row.hook_sub_rows[0].hook_type, HookType::PostRemove);
            assert_eq!(row.hook_sub_rows[0].status, HookSubStatus::Running);
        }

        let dur = Duration::from_millis(200);
        state.apply_event(&DagEvent::HookCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostRemove,
            success: true,
            warned: false,
            duration: dur,
            exit_code: Some(0),
            output: None,
        });

        let row = state
            .worktrees
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
            hook_type: HookType::PostCreate,
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate,
            job_name: "build".into(),
        });

        let row = state
            .worktrees
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
            hook_type: HookType::PostCreate,
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate,
            job_name: "build".into(),
        });
        state.apply_event(&DagEvent::JobCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate,
            job_name: "build".into(),
            status: JobCompletionStatus::Succeeded,
            duration: Duration::from_millis(150),
            skip_reason: None,
        });

        let row = state
            .worktrees
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
            hook_type: HookType::PreRemove,
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove,
            job_name: "cleanup".into(),
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove,
            job_name: "notify".into(),
        });
        state.apply_event(&DagEvent::JobCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove,
            job_name: "cleanup".into(),
            status: JobCompletionStatus::Succeeded,
            duration: Duration::from_millis(100),
            skip_reason: None,
        });
        state.apply_event(&DagEvent::JobCompleted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PreRemove,
            job_name: "notify".into(),
            status: JobCompletionStatus::Failed,
            duration: Duration::from_millis(200),
            skip_reason: None,
        });

        let row = state
            .worktrees
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
        );

        state.apply_event(&DagEvent::TaskStarted {
            phase: OperationPhase::Push,
            branch_name: "master".into(),
        });
        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pushing".into()));

        state.apply_event(&DagEvent::TaskCompleted {
            phase: OperationPhase::Push,
            branch_name: "master".into(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::Pushed,
            updated_info: None,
        });
        let row = state
            .worktrees
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
            updated_info: None,
        });

        let row = state
            .worktrees
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
            updated_info: None,
        });

        let row = state
            .worktrees
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
            hook_type: HookType::PostCreate,
        });
        state.apply_event(&DagEvent::JobStarted {
            branch_name: "feat/a".into(),
            hook_type: HookType::PostCreate,
            job_name: "build".into(),
        });

        let row = state
            .worktrees
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
            updated_info: None,
        });

        // Verify row shows conflict
        let row = state
            .worktrees
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
            updated_info: None,
        });

        // Row should show conflict again (restored from prev_terminal_status)
        let row = state
            .worktrees
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
