//! Dependency graph for parallelized sync/prune operations.
//!
//! Defines task types, status tracking, and operation phases used by
//! the sync and prune TUI renderers.

use super::list::WorktreeInfo;
use crate::hooks::HookType;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Semantic outcomes that a task can produce. Downstream tasks may
/// inspect these as preconditions for whether to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskOutcome {
    /// Rebase had conflicts and was aborted.
    Conflict,
}

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
    /// Push a worktree branch to its remote.
    Push(String),
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
    /// Task checked preconditions and chose not to run.
    PreconditionFailed,
}

impl TaskStatus {
    /// Whether this status is a terminal state (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Skipped
                | Self::DepFailed
                | Self::PreconditionFailed
        )
    }
}

/// Typed task completion message, replacing string sentinels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskMessage {
    /// Generic success with displayable git output.
    Ok(String),
    /// Git pull/rebase reported "Already up to date".
    UpToDate,
    /// Rebase had conflicts and was aborted.
    Conflict,
    /// Worktree/branch was removed by prune.
    Removed,
    /// Prune deferred (current worktree — handled post-TUI).
    Deferred,
    /// Prune found nothing to remove.
    NoActionNeeded,
    /// Prune skipped because worktree has uncommitted changes.
    SkippedDirty,
    /// Update couldn't fast-forward (branch diverged from upstream).
    Diverged,
    /// Push completed successfully.
    Pushed,
    /// Branch has no upstream tracking branch (push skipped).
    NoPushUpstream,
    /// Task failed with error message.
    Failed(String),
}

/// A high-level operation phase shown in the operation header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationPhase {
    Fetch,
    Prune,
    Update,
    Rebase(String),
    Push,
}

impl OperationPhase {
    /// Human-readable label for the operation header.
    pub fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetching remote branches".into(),
            Self::Prune => "Pruning stale branches".into(),
            Self::Update => "Updating worktrees".into(),
            Self::Rebase(branch) => format!("Rebasing onto {branch}"),
            Self::Push => "Pushing to remote".into(),
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
    /// Whether push tasks are included.
    push: bool,
}

impl SyncDag {
    /// Build a DAG for `daft sync` (with optional rebase and push).
    pub fn build_sync(
        worktrees: Vec<(String, PathBuf)>,
        gone_branches: Vec<String>,
        rebase_branch: Option<String>,
        push: bool,
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

        // 3. Update tasks (each depends on fetch). Track indices for rebase/push deps.
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
        // Track the last task index per branch for push dependencies.
        let mut last_task_indices: Vec<(String, PathBuf, usize)> = update_indices
            .iter()
            .map(|(branch, idx)| {
                let path = worktrees
                    .iter()
                    .find(|(b, _)| b == branch)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_default();
                (branch.clone(), path, *idx)
            })
            .collect();

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

                let rebase_idx = push_task(
                    SyncTask {
                        id: TaskId::Rebase(branch.clone()),
                        phase: OperationPhase::Rebase(base_branch.clone()),
                        worktree_path: Some(path.clone()),
                        branch_name: branch.clone(),
                    },
                    deps,
                );

                // Update last task index for this branch to the rebase task.
                if let Some(entry) = last_task_indices.iter_mut().find(|(b, _, _)| b == branch) {
                    entry.2 = rebase_idx;
                }
            }
        }

        // 5. Push tasks if push is enabled.
        if push {
            for (branch, path, last_idx) in &last_task_indices {
                push_task(
                    SyncTask {
                        id: TaskId::Push(branch.clone()),
                        phase: OperationPhase::Push,
                        worktree_path: Some(path.clone()),
                        branch_name: branch.clone(),
                    },
                    vec![*last_idx],
                );
            }
        }

        Self {
            tasks,
            dependencies,
            dependents,
            rebase_branch: stored_rebase_branch,
            push,
        }
    }

    /// Build a DAG for `daft prune`.
    pub fn build_prune(gone_branches: Vec<String>) -> Self {
        Self::build_sync(vec![], gone_branches, None, false)
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

        if self.push {
            phases.push(OperationPhase::Push);
        }

        phases
    }
}

/// Message sent from worker threads to the renderer.
#[derive(Debug, Clone)]
pub enum DagEvent {
    /// A task started running.
    TaskStarted {
        phase: OperationPhase,
        branch_name: String,
    },
    /// A task completed.
    TaskCompleted {
        phase: OperationPhase,
        branch_name: String,
        status: TaskStatus,
        /// Typed result message.
        message: TaskMessage,
        /// Refreshed worktree info after the operation (if applicable).
        updated_info: Option<Box<WorktreeInfo>>,
    },
    /// All tasks are done.
    AllDone,
    /// A hook started running for a branch.
    HookStarted {
        branch_name: String,
        hook_type: HookType,
    },
    /// A hook completed for a branch.
    HookCompleted {
        branch_name: String,
        hook_type: HookType,
        success: bool,
        /// Non-zero exit with FailMode::Warn.
        warned: bool,
        duration: Duration,
        /// Exit code from the hook process, if available.
        exit_code: Option<i32>,
        /// Captured stdout+stderr, only stored on failure/warning.
        output: Option<String>,
    },
    /// A job started running within a hook.
    JobStarted {
        branch_name: String,
        hook_type: HookType,
        job_name: String,
    },
    /// A job completed within a hook.
    JobCompleted {
        branch_name: String,
        hook_type: HookType,
        job_name: String,
        status: JobCompletionStatus,
        duration: Duration,
        skip_reason: Option<String>,
    },
}

/// Terminal status for a job within a hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCompletionStatus {
    Succeeded,
    Failed,
    Skipped,
}

/// Shared mutable state for the worker pool.
struct DagState {
    ready: Vec<usize>,
    status: Vec<TaskStatus>,
    in_degree: Vec<usize>,
    active: usize,
    done: usize,
    total: usize,
}

/// Executes a DAG of sync tasks in parallel.
pub struct DagExecutor {
    dag: SyncDag,
    sender: mpsc::Sender<DagEvent>,
}

impl DagExecutor {
    /// Create a new executor for the given DAG, sending events through `sender`.
    pub fn new(dag: SyncDag, sender: mpsc::Sender<DagEvent>) -> Self {
        Self { dag, sender }
    }

    /// Execute all tasks in the DAG, calling `task_fn` for each task.
    ///
    /// Tasks are executed in parallel (respecting dependencies) using a thread pool.
    /// The closure receives a reference to the `SyncTask` and must return a
    /// `(TaskStatus, TaskMessage, Option<Box<WorktreeInfo>>)` triple indicating
    /// the result status, a typed message, and optionally refreshed worktree info.
    ///
    /// Consumes `self` so that the sender is dropped after `AllDone` is sent,
    /// allowing the receiver to detect channel closure.
    pub fn run<F>(self, task_fn: F)
    where
        F: Fn(&SyncTask) -> (TaskStatus, TaskMessage, Option<Box<WorktreeInfo>>) + Send + Sync,
    {
        let n = self.dag.tasks.len();

        // Compute initial in-degrees from the DAG's dependencies.
        let mut in_degree = vec![0usize; n];
        for (i, deps) in self.dag.dependencies.iter().enumerate() {
            in_degree[i] = deps.len();
        }

        // Find initially ready tasks (in-degree == 0).
        let ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

        let state = Arc::new((
            Mutex::new(DagState {
                ready,
                status: vec![TaskStatus::Pending; n],
                in_degree,
                active: 0,
                done: 0,
                total: n,
            }),
            Condvar::new(),
        ));

        let max_workers = std::thread::available_parallelism()
            .map(|p| p.get())
            .unwrap_or(4);

        let task_fn = &task_fn;
        let dag = &self.dag;
        let sender = &self.sender;

        std::thread::scope(|scope| {
            for _ in 0..max_workers {
                let state = Arc::clone(&state);

                scope.spawn(move || {
                    loop {
                        let task_idx;
                        {
                            let (lock, cvar) = &*state;
                            let mut s = lock.lock().unwrap();

                            loop {
                                // Try to pop a ready task.
                                if let Some(idx) = s.ready.pop() {
                                    task_idx = idx;
                                    s.status[task_idx] = TaskStatus::Running;
                                    s.active += 1;
                                    break;
                                }

                                // No ready tasks: check if we're done.
                                if s.done == s.total {
                                    return;
                                }

                                // Nothing ready but work still in flight — wait.
                                if s.active == 0 && s.ready.is_empty() {
                                    // All tasks are either done or dep-failed; no more work.
                                    return;
                                }

                                s = cvar.wait(s).unwrap();
                            }
                        }

                        // Send TaskStarted event.
                        let _ = sender.send(DagEvent::TaskStarted {
                            phase: dag.tasks[task_idx].phase.clone(),
                            branch_name: dag.tasks[task_idx].branch_name.clone(),
                        });

                        // Execute the task outside the lock.
                        let task = &dag.tasks[task_idx];
                        let (result_status, message, updated_info) = task_fn(task);

                        // Update DAG state.
                        {
                            let (lock, cvar) = &*state;
                            let mut s = lock.lock().unwrap();
                            s.status[task_idx] = result_status;
                            s.active -= 1;
                            s.done += 1;

                            if result_status == TaskStatus::Succeeded
                                || result_status == TaskStatus::Skipped
                            {
                                // Decrement in-degrees of dependents.
                                for &dep_idx in &dag.dependents[task_idx] {
                                    if s.status[dep_idx] == TaskStatus::Pending {
                                        s.in_degree[dep_idx] -= 1;
                                        if s.in_degree[dep_idx] == 0 {
                                            s.ready.push(dep_idx);
                                        }
                                    }
                                }
                            } else if result_status == TaskStatus::Failed {
                                // Cascade DepFailed to all transitive dependents.
                                let mut stack = vec![task_idx];
                                while let Some(idx) = stack.pop() {
                                    for &dep_idx in &dag.dependents[idx] {
                                        if s.status[dep_idx] == TaskStatus::Pending {
                                            s.status[dep_idx] = TaskStatus::DepFailed;
                                            s.done += 1;
                                            stack.push(dep_idx);
                                        }
                                    }
                                }
                            }

                            // Send TaskCompleted event.
                            let _ = sender.send(DagEvent::TaskCompleted {
                                phase: dag.tasks[task_idx].phase.clone(),
                                branch_name: dag.tasks[task_idx].branch_name.clone(),
                                status: result_status,
                                message: message.clone(),
                                updated_info,
                            });

                            // Also send TaskCompleted for any dep-failed dependents.
                            if result_status == TaskStatus::Failed {
                                let mut stack = vec![task_idx];
                                let mut visited = std::collections::HashSet::new();
                                while let Some(idx) = stack.pop() {
                                    for &dep_idx in &dag.dependents[idx] {
                                        if s.status[dep_idx] == TaskStatus::DepFailed
                                            && visited.insert(dep_idx)
                                        {
                                            let _ = sender.send(DagEvent::TaskCompleted {
                                                phase: dag.tasks[dep_idx].phase.clone(),
                                                branch_name: dag.tasks[dep_idx].branch_name.clone(),
                                                status: TaskStatus::DepFailed,
                                                message: TaskMessage::Failed(format!(
                                                    "dependency {:?} failed",
                                                    dag.tasks[task_idx].id
                                                )),
                                                updated_info: None,
                                            });
                                            stack.push(dep_idx);
                                        }
                                    }
                                }
                            }

                            cvar.notify_all();
                        }
                    }
                });
            }
        });

        // All workers are done. Send AllDone and drop sender.
        let _ = self.sender.send(DagEvent::AllDone);
        // self.sender is dropped here when self goes out of scope.
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

        let dag = SyncDag::build_sync(worktrees, gone, None, false);

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

        let dag = SyncDag::build_sync(worktrees, gone, Some("master".into()), false);

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
        let dag = SyncDag::build_sync(worktrees, vec![], None, false);
        let phases = dag.phases();
        assert_eq!(phases.len(), 3); // Fetch, Prune, Update
    }

    #[test]
    fn dag_phases_sync_with_rebase() {
        let worktrees = vec![("master".into(), PathBuf::from("/p/master"))];
        let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()), false);
        let phases = dag.phases();
        assert_eq!(phases.len(), 4); // Fetch, Prune, Update, Rebase
    }

    #[test]
    fn executor_runs_all_tasks() {
        let dag = SyncDag::build_prune(vec!["feat/a".into()]);
        let (tx, rx) = mpsc::channel();

        let executor = DagExecutor::new(dag, tx);
        executor.run(|_task| (TaskStatus::Succeeded, TaskMessage::Ok("ok".into()), None));

        let events: Vec<DagEvent> = rx.iter().collect();
        let starts = events
            .iter()
            .filter(|e| matches!(e, DagEvent::TaskStarted { .. }))
            .count();
        let completes = events
            .iter()
            .filter(|e| matches!(e, DagEvent::TaskCompleted { .. }))
            .count();
        let dones = events
            .iter()
            .filter(|e| matches!(e, DagEvent::AllDone))
            .count();
        assert_eq!(starts, 2);
        assert_eq!(completes, 2);
        assert_eq!(dones, 1);
    }

    #[test]
    fn executor_respects_dependencies() {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ];
        let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()), false);
        let (tx, rx) = mpsc::channel();

        let order = Arc::new(Mutex::new(Vec::new()));
        let order_clone = Arc::clone(&order);

        let executor = DagExecutor::new(dag, tx);
        executor.run(move |task| {
            order_clone.lock().unwrap().push(task.id.clone());
            (TaskStatus::Succeeded, TaskMessage::Ok("ok".into()), None)
        });

        let _events: Vec<DagEvent> = rx.iter().collect();
        let execution_order = order.lock().unwrap();
        // Fetch must come first
        assert_eq!(execution_order[0], TaskId::Fetch);
        // Rebase(feat/a) must come after Update(master)
        let master_pos = execution_order
            .iter()
            .position(|t| *t == TaskId::Update("master".into()))
            .unwrap();
        let rebase_pos = execution_order
            .iter()
            .position(|t| *t == TaskId::Rebase("feat/a".into()))
            .unwrap();
        assert!(master_pos < rebase_pos);
    }

    #[test]
    fn build_sync_dag_with_push_no_rebase() {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ];
        let gone: Vec<String> = vec![];

        let dag = SyncDag::build_sync(worktrees, gone, None, true);

        // 1 fetch + 2 updates + 2 pushes = 5 tasks
        assert_eq!(dag.tasks.len(), 5);

        // Push tasks should depend on their corresponding Update tasks
        for (i, task) in dag.tasks.iter().enumerate() {
            if let TaskId::Push(ref branch) = task.id {
                let update_idx = dag
                    .tasks
                    .iter()
                    .position(|t| t.id == TaskId::Update(branch.clone()))
                    .unwrap();
                assert!(
                    dag.dependencies_of(i).contains(&update_idx),
                    "Push({branch}) should depend on Update({branch})"
                );
            }
        }
    }

    #[test]
    fn build_sync_dag_with_push_and_rebase() {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
            ("feat/b".into(), PathBuf::from("/p/feat-b")),
        ];
        let gone: Vec<String> = vec![];

        let dag = SyncDag::build_sync(worktrees, gone, Some("master".into()), true);

        // 1 fetch + 3 updates + 2 rebases + 3 pushes = 9 tasks
        assert_eq!(dag.tasks.len(), 9);

        // Push(master) depends on Update(master), not Rebase (master is the base branch)
        let push_master_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Push("master".into()))
            .unwrap();
        let update_master_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();
        assert!(dag
            .dependencies_of(push_master_idx)
            .contains(&update_master_idx));

        // Push(feat/a) depends on Rebase(feat/a)
        let push_feat_a_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Push("feat/a".into()))
            .unwrap();
        let rebase_feat_a_idx = dag
            .tasks
            .iter()
            .position(|t| t.id == TaskId::Rebase("feat/a".into()))
            .unwrap();
        assert!(
            dag.dependencies_of(push_feat_a_idx)
                .contains(&rebase_feat_a_idx),
            "Push(feat/a) should depend on Rebase(feat/a)"
        );
    }

    #[test]
    fn dag_phases_sync_with_push() {
        let worktrees = vec![("master".into(), PathBuf::from("/p/master"))];
        let dag = SyncDag::build_sync(worktrees, vec![], None, true);
        let phases = dag.phases();
        assert_eq!(phases.len(), 4); // Fetch, Prune, Update, Push
        assert!(phases.contains(&OperationPhase::Push));
    }

    #[test]
    fn executor_cascades_failure() {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ];
        let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()), false);
        let (tx, rx) = mpsc::channel();

        let executor = DagExecutor::new(dag, tx);
        executor.run(|task| match &task.id {
            TaskId::Update(name) if name == "master" => (
                TaskStatus::Failed,
                TaskMessage::Failed("pull failed".into()),
                None,
            ),
            _ => (TaskStatus::Succeeded, TaskMessage::Ok("ok".into()), None),
        });

        let events: Vec<DagEvent> = rx.iter().collect();
        let rebase_event = events.iter().find(|e| {
            matches!(
                e,
                DagEvent::TaskCompleted {
                    status: TaskStatus::DepFailed,
                    ..
                }
            )
        });
        assert!(rebase_event.is_some());
    }

    #[test]
    fn precondition_failed_is_terminal() {
        assert!(TaskStatus::PreconditionFailed.is_terminal());
    }
}
