use crate::core::worktree::list::{Stat, WorktreeInfo};
use crate::core::worktree::sync_dag::{DagEvent, OperationPhase, TaskMessage, TaskStatus};
use std::path::PathBuf;

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
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    pub stat: Stat,
}

/// A single row in the worktree table.
pub struct WorktreeRow {
    pub info: WorktreeInfo,
    pub status: WorktreeStatus,
}

impl TuiState {
    pub fn new(
        phases: Vec<OperationPhase>,
        worktree_infos: Vec<WorktreeInfo>,
        project_root: PathBuf,
        cwd: PathBuf,
        stat: Stat,
    ) -> Self {
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
            project_root,
            cwd,
            stat,
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
                };
                // Auto-create row for newly discovered branches (e.g., gone branches
                // found after fetch completes while TUI is already running).
                if !branch_name.is_empty() && self.find_row_mut(branch_name).is_none() {
                    self.worktrees.push(WorktreeRow {
                        info: WorktreeInfo::empty(branch_name),
                        status: WorktreeStatus::Idle,
                    });
                }
                if let Some(row) = self.find_row_mut(branch_name) {
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
                let final_status = Self::map_final_status(phase, *status, message);
                if let Some(row) = self.find_row_mut(branch_name) {
                    row.status = WorktreeStatus::Done(final_status);
                    if let Some(new_info) = updated_info {
                        row.info = *new_info.clone();
                    }
                }
                self.check_phase_completion(phase);
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
            DagEvent::HookStarted { .. } | DagEvent::HookCompleted { .. } => {}
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
                    TaskMessage::NoActionNeeded => FinalStatus::UpToDate,
                    _ => FinalStatus::Pruned,
                },
                OperationPhase::Update => match message {
                    TaskMessage::UpToDate => FinalStatus::UpToDate,
                    _ => FinalStatus::Updated,
                },
                OperationPhase::Rebase(_) => match message {
                    TaskMessage::Conflict => FinalStatus::Conflict,
                    _ => FinalStatus::Rebased,
                },
                OperationPhase::Fetch => FinalStatus::Updated,
            },
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
}
