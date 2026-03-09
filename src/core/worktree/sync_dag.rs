//! Dependency graph for parallelized sync/prune operations.
//!
//! Defines task types, status tracking, and operation phases used by
//! the sync and prune TUI renderers.

use std::path::PathBuf;

/// Identifies a single executable task in the sync DAG.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskId {
    /// Fetch from remote with --prune.
    Fetch,
    /// Prune a single gone branch (and its worktree if present).
    Prune(String),
    /// Update (pull) a single worktree.
    Update(String),
    /// Rebase a worktree onto a base branch.
    Rebase(String),
}

/// Execution status of a single task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    /// Skipped because a dependency failed.
    DepFailed,
}

impl TaskStatus {
    /// Whether this status is a terminal state (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::DepFailed
        )
    }
}

/// A high-level operation phase shown in the operation header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationPhase {
    Fetch,
    Prune,
    Update,
    Rebase(String),
}

impl OperationPhase {
    /// Human-readable label for the operation header.
    pub fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetching remote branches".into(),
            Self::Prune => "Pruning stale branches".into(),
            Self::Update => "Updating worktrees".into(),
            Self::Rebase(branch) => format!("Rebasing onto {branch}"),
        }
    }
}

/// A node in the sync dependency graph.
#[derive(Debug)]
pub struct SyncTask {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Which operation phase this task belongs to.
    pub phase: OperationPhase,
    /// Worktree path (None for Fetch tasks).
    pub worktree_path: Option<PathBuf>,
    /// Branch name associated with this task.
    pub branch_name: String,
}

/// The dependency graph for a sync or prune operation.
#[derive(Debug)]
pub struct SyncDag {
    /// All tasks in topological-ish order (fetch first).
    pub tasks: Vec<SyncTask>,
    /// For each task index, the indices of tasks it depends on.
    dependencies: Vec<Vec<usize>>,
    /// For each task index, the indices of tasks that depend on it.
    pub(crate) dependents: Vec<Vec<usize>>,
    /// The rebase base branch, if rebase was requested.
    rebase_branch: Option<String>,
}

impl SyncDag {
    /// Build a DAG for `daft sync` (with optional rebase).
    pub fn build_sync(
        worktrees: Vec<(String, PathBuf)>,
        gone_branches: Vec<String>,
        rebase_branch: Option<String>,
    ) -> Self {
        let stored_rebase_branch = rebase_branch.clone();
        let mut tasks = Vec::new();
        let mut dependencies: Vec<Vec<usize>> = Vec::new();
        let mut dependents: Vec<Vec<usize>> = Vec::new();

        // Helper to push a task and its deps, returning the new index.
        let mut push_task = |task: SyncTask, deps: Vec<usize>| -> usize {
            let idx = tasks.len();
            tasks.push(task);
            // For each dependency, record that this new task depends on it.
            for &dep in &deps {
                // Grow dependents vec if needed.
                if dependents.len() <= dep {
                    dependents.resize_with(dep + 1, Vec::new);
                }
                dependents[dep].push(idx);
            }
            dependencies.push(deps);
            // Ensure dependents vec covers the new index too.
            if dependents.len() <= idx {
                dependents.resize_with(idx + 1, Vec::new);
            }
            idx
        };

        // 1. Fetch task (index 0, no deps).
        let fetch_idx = push_task(
            SyncTask {
                id: TaskId::Fetch,
                phase: OperationPhase::Fetch,
                worktree_path: None,
                branch_name: String::new(),
            },
            vec![],
        );

        // 2. Prune tasks (each depends on fetch).
        for branch in &gone_branches {
            push_task(
                SyncTask {
                    id: TaskId::Prune(branch.clone()),
                    phase: OperationPhase::Prune,
                    worktree_path: None,
                    branch_name: branch.clone(),
                },
                vec![fetch_idx],
            );
        }

        // 3. Update tasks (each depends on fetch). Track indices for rebase deps.
        let mut update_indices: Vec<(String, usize)> = Vec::new();
        for (branch, path) in &worktrees {
            let idx = push_task(
                SyncTask {
                    id: TaskId::Update(branch.clone()),
                    phase: OperationPhase::Update,
                    worktree_path: Some(path.clone()),
                    branch_name: branch.clone(),
                },
                vec![fetch_idx],
            );
            update_indices.push((branch.clone(), idx));
        }

        // 4. Rebase tasks if rebase_branch is specified.
        if let Some(ref base_branch) = rebase_branch {
            // Find the update task for the base branch.
            let base_update_idx = update_indices
                .iter()
                .find(|(b, _)| b == base_branch)
                .map(|(_, idx)| *idx);

            for (branch, path) in &worktrees {
                // Don't rebase the base branch onto itself.
                if branch == base_branch {
                    continue;
                }

                // Find the update task for this branch.
                let this_update_idx = update_indices
                    .iter()
                    .find(|(b, _)| b == branch)
                    .map(|(_, idx)| *idx);

                let mut deps = Vec::new();
                if let Some(idx) = base_update_idx {
                    deps.push(idx);
                }
                if let Some(idx) = this_update_idx {
                    deps.push(idx);
                }

                push_task(
                    SyncTask {
                        id: TaskId::Rebase(branch.clone()),
                        phase: OperationPhase::Rebase(base_branch.clone()),
                        worktree_path: Some(path.clone()),
                        branch_name: branch.clone(),
                    },
                    deps,
                );
            }
        }

        Self {
            tasks,
            dependencies,
            dependents,
            rebase_branch: stored_rebase_branch,
        }
    }

    /// Build a DAG for `daft prune`.
    pub fn build_prune(gone_branches: Vec<String>) -> Self {
        Self::build_sync(vec![], gone_branches, None)
    }

    /// Get the dependency indices for a task.
    pub fn dependencies_of(&self, task_idx: usize) -> &[usize] {
        &self.dependencies[task_idx]
    }

    /// Get the dependent indices for a task.
    pub fn dependents_of(&self, task_idx: usize) -> &[usize] {
        &self.dependents[task_idx]
    }

    /// Get the ordered list of operation phases for the header display.
    ///
    /// For sync, always includes Fetch, Prune, Update even if no tasks exist
    /// for those phases. Only includes Rebase if a rebase branch was specified.
    pub fn phases(&self) -> Vec<OperationPhase> {
        let mut phases = vec![
            OperationPhase::Fetch,
            OperationPhase::Prune,
            OperationPhase::Update,
        ];

        if let Some(ref branch) = self.rebase_branch {
            phases.push(OperationPhase::Rebase(branch.clone()));
        }

        phases
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_display() {
        let id = TaskId::Fetch;
        assert_eq!(format!("{id:?}"), "Fetch");

        let id = TaskId::Prune("feat/old".into());
        assert_eq!(format!("{id:?}"), "Prune(\"feat/old\")");
    }

    #[test]
    fn task_status_is_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Succeeded.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Skipped.is_terminal());
        assert!(TaskStatus::DepFailed.is_terminal());
    }

    #[test]
    fn operation_phase_label() {
        assert_eq!(OperationPhase::Fetch.label(), "Fetching remote branches");
        assert_eq!(
            OperationPhase::Rebase("master".into()).label(),
            "Rebasing onto master"
        );
    }

    #[test]
    fn build_sync_dag_no_rebase() {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ];
        let gone: Vec<String> = vec!["feat/old".into()];

        let dag = SyncDag::build_sync(worktrees, gone, None);

        // 1 fetch + 1 prune + 2 updates = 4 tasks
        assert_eq!(dag.tasks.len(), 4);
        // Fetch has no dependencies
        assert!(dag.dependencies_of(0).is_empty());
        // All others depend on fetch (index 0)
        for i in 1..dag.tasks.len() {
            assert!(dag.dependencies_of(i).contains(&0));
        }
    }

    #[test]
    fn build_sync_dag_with_rebase() {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
            ("feat/b".into(), PathBuf::from("/p/feat-b")),
        ];
        let gone: Vec<String> = vec![];

        let dag = SyncDag::build_sync(worktrees, gone, Some("master".into()));

        // 1 fetch + 3 updates + 2 rebases = 6 tasks
        assert_eq!(dag.tasks.len(), 6);

        // Find the update(master) task index
        let master_update_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        // Rebase tasks depend on update(master)
        for (i, task) in dag.tasks.iter().enumerate() {
            if matches!(&task.id, TaskId::Rebase(_)) {
                assert!(
                    dag.dependencies_of(i).contains(&master_update_idx),
                    "Rebase task should depend on update(master)"
                );
            }
        }
    }

    #[test]
    fn build_prune_dag() {
        let gone = vec!["feat/old".into(), "feat/stale".into()];
        let dag = SyncDag::build_prune(gone);

        // 1 fetch + 2 prunes = 3 tasks
        assert_eq!(dag.tasks.len(), 3);
    }

    #[test]
    fn dag_phases_sync() {
        let worktrees = vec![("master".into(), PathBuf::from("/p/master"))];
        let dag = SyncDag::build_sync(worktrees, vec![], None);
        let phases = dag.phases();
        assert_eq!(phases.len(), 3); // Fetch, Prune, Update
    }

    #[test]
    fn dag_phases_sync_with_rebase() {
        let worktrees = vec![("master".into(), PathBuf::from("/p/master"))];
        let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()));
        let phases = dag.phases();
        assert_eq!(phases.len(), 4); // Fetch, Prune, Update, Rebase
    }
}
