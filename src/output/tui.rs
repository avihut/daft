//! Inline TUI renderer for sync and prune operations.
//!
//! Uses ratatui with Viewport::Inline to render an operation header
//! and worktree status table that update in-place as tasks execute.

use crate::core::worktree::list::WorktreeInfo;
use crate::core::worktree::sync_dag::{DagEvent, OperationPhase, SyncDag, SyncTask, TaskStatus};

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
    Skipped,
    Pruned,
    Failed,
}

/// Complete TUI state, rebuilt from DagEvents.
pub struct TuiState {
    pub phases: Vec<PhaseState>,
    pub worktrees: Vec<WorktreeRow>,
    pub done: bool,
    pub tick: usize,
}

/// A single row in the worktree table.
pub struct WorktreeRow {
    pub info: WorktreeInfo,
    pub status: WorktreeStatus,
}

impl TuiState {
    pub fn new(phases: Vec<OperationPhase>, worktree_infos: Vec<WorktreeInfo>) -> Self {
        Self {
            phases: phases
                .into_iter()
                .map(|phase| PhaseState {
                    phase,
                    status: PhaseStatus::Pending,
                })
                .collect(),
            worktrees: worktree_infos
                .into_iter()
                .map(|info| WorktreeRow {
                    info,
                    status: WorktreeStatus::Idle,
                })
                .collect(),
            done: false,
            tick: 0,
        }
    }

    pub fn apply_event(&mut self, event: &DagEvent, dag: &SyncDag) {
        match event {
            DagEvent::TaskStarted { task_idx } => {
                let task = &dag.tasks[*task_idx];
                self.activate_phase(&task.phase);
                let active_label = match &task.phase {
                    OperationPhase::Fetch => "fetching",
                    OperationPhase::Prune => "pruning",
                    OperationPhase::Update => "updating",
                    OperationPhase::Rebase(_) => "rebasing",
                };
                if let Some(row) = self.find_row_mut(&task.branch_name) {
                    row.status = WorktreeStatus::Active(active_label.into());
                }
            }
            DagEvent::TaskCompleted {
                task_idx,
                status,
                message,
            } => {
                let task = &dag.tasks[*task_idx];
                let final_status = Self::map_final_status(task, *status, message);
                if let Some(row) = self.find_row_mut(&task.branch_name) {
                    row.status = WorktreeStatus::Done(final_status);
                }
                self.check_phase_completion(&task.phase);
            }
            DagEvent::AllDone => {
                for phase in &mut self.phases {
                    if phase.status != PhaseStatus::Completed {
                        phase.status = PhaseStatus::Completed;
                    }
                }
                self.done = true;
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

    fn map_final_status(task: &SyncTask, status: TaskStatus, message: &str) -> FinalStatus {
        match status {
            TaskStatus::Failed => FinalStatus::Failed,
            TaskStatus::DepFailed => FinalStatus::Skipped,
            TaskStatus::Skipped => FinalStatus::Skipped,
            TaskStatus::Succeeded => match &task.phase {
                OperationPhase::Prune => FinalStatus::Pruned,
                OperationPhase::Update => {
                    if message.contains("up to date") || message.contains("Already up to date") {
                        FinalStatus::UpToDate
                    } else {
                        FinalStatus::Updated
                    }
                }
                OperationPhase::Rebase(_) => {
                    if message.contains("conflict") {
                        FinalStatus::Conflict
                    } else {
                        FinalStatus::Rebased
                    }
                }
                OperationPhase::Fetch => FinalStatus::Updated,
            },
            TaskStatus::Pending | TaskStatus::Running => FinalStatus::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::list::EntryKind;
    use crate::core::worktree::sync_dag::*;
    use std::path::PathBuf;

    fn make_worktree_info(name: &str) -> WorktreeInfo {
        WorktreeInfo {
            kind: EntryKind::Worktree,
            name: name.to_string(),
            path: Some(PathBuf::from(format!("/p/{name}"))),
            is_current: false,
            is_default_branch: false,
            ahead: None,
            behind: None,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
            last_commit_subject: String::new(),
            branch_creation_timestamp: None,
            base_lines_inserted: None,
            base_lines_deleted: None,
            staged_lines_inserted: None,
            staged_lines_deleted: None,
            unstaged_lines_inserted: None,
            unstaged_lines_deleted: None,
            remote_lines_inserted: None,
            remote_lines_deleted: None,
        }
    }

    fn make_test_state() -> (TuiState, SyncDag) {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ];
        let dag = SyncDag::build_sync(worktrees, vec!["feat/old".into()], None);
        let phases = dag.phases();

        let worktree_infos = vec![
            make_worktree_info("master"),
            make_worktree_info("feat/a"),
            make_worktree_info("feat/old"),
        ];

        let state = TuiState::new(phases, worktree_infos);
        (state, dag)
    }

    #[test]
    fn initial_state_all_pending() {
        let (state, _) = make_test_state();
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
        let (mut state, dag) = make_test_state();
        state.apply_event(&DagEvent::TaskStarted { task_idx: 0 }, &dag);
        assert_eq!(state.phases[0].status, PhaseStatus::Active);
    }

    #[test]
    fn task_completed_updates_row_status() {
        let (mut state, dag) = make_test_state();
        let master_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        state.apply_event(
            &DagEvent::TaskStarted {
                task_idx: master_idx,
            },
            &dag,
        );
        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: master_idx,
                status: TaskStatus::Succeeded,
                message: "Already up to date".into(),
            },
            &dag,
        );

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    #[test]
    fn all_done_sets_done_flag() {
        let (mut state, dag) = make_test_state();
        state.apply_event(&DagEvent::AllDone, &dag);
        assert!(state.done);
    }

    #[test]
    fn prune_task_sets_pruned_status() {
        let (mut state, dag) = make_test_state();
        let prune_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Prune("feat/old".into()))
            .unwrap();

        state.apply_event(
            &DagEvent::TaskStarted {
                task_idx: prune_idx,
            },
            &dag,
        );
        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Active("pruning".into()));

        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: prune_idx,
                status: TaskStatus::Succeeded,
                message: "removed".into(),
            },
            &dag,
        );

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "feat/old")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Pruned));
    }

    #[test]
    fn failed_task_sets_failed_status() {
        let (mut state, dag) = make_test_state();
        let master_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        state.apply_event(
            &DagEvent::TaskStarted {
                task_idx: master_idx,
            },
            &dag,
        );
        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: master_idx,
                status: TaskStatus::Failed,
                message: "pull failed".into(),
            },
            &dag,
        );

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Failed));
    }

    #[test]
    fn dep_failed_sets_skipped_status() {
        let (mut state, dag) = make_test_state();
        let master_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: master_idx,
                status: TaskStatus::DepFailed,
                message: "dependency failed".into(),
            },
            &dag,
        );

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Skipped));
    }

    #[test]
    fn tick_increments_counter() {
        let (mut state, _) = make_test_state();
        assert_eq!(state.tick, 0);
        state.tick();
        assert_eq!(state.tick, 1);
        state.tick();
        assert_eq!(state.tick, 2);
    }

    #[test]
    fn phase_completes_when_no_active_rows() {
        let (mut state, dag) = make_test_state();
        // Start and complete the fetch task (task 0)
        state.apply_event(&DagEvent::TaskStarted { task_idx: 0 }, &dag);
        assert_eq!(state.phases[0].status, PhaseStatus::Active);

        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: 0,
                status: TaskStatus::Succeeded,
                message: "fetched".into(),
            },
            &dag,
        );
        // Fetch phase should now be completed (fetch task has empty branch_name,
        // so no row was Active for it -- but the phase was Active and now no
        // rows show "fetching")
        assert_eq!(state.phases[0].status, PhaseStatus::Completed);
    }

    #[test]
    fn all_done_completes_remaining_phases() {
        let (mut state, dag) = make_test_state();
        // All phases start as Pending
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Pending));

        state.apply_event(&DagEvent::AllDone, &dag);

        // AllDone should mark all phases as Completed
        assert!(state
            .phases
            .iter()
            .all(|p| p.status == PhaseStatus::Completed));
    }

    #[test]
    fn update_with_changes_sets_updated_status() {
        let (mut state, dag) = make_test_state();
        let master_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        state.apply_event(
            &DagEvent::TaskStarted {
                task_idx: master_idx,
            },
            &dag,
        );
        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: master_idx,
                status: TaskStatus::Succeeded,
                message: "Fast-forward".into(),
            },
            &dag,
        );

        let row = state
            .worktrees
            .iter()
            .find(|w| w.info.name == "master")
            .unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::Updated));
    }
}
