# Rich Sync TUI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Replace sequential text output of `daft sync` and `daft prune` with an
inline ratatui TUI showing live operation progress and a worktree table, backed
by parallelized execution via a dependency graph.

**Architecture:** A `SyncDag` orchestrates fine-grained tasks (fetch, prune,
update, rebase) in a worker pool. Workers send status updates through an `mpsc`
channel to the main thread, which owns a ratatui inline viewport and re-renders
on each update. The TUI shows an operation header (phases with spinners) above a
worktree table (same columns as `daft list` plus a Status column). Non-TTY
environments fall back to sequential text output with the same execution model.

**Tech Stack:** ratatui (Viewport::Inline), crossterm, std::sync (Mutex,
Condvar), std::sync::mpsc

**Design doc:** `docs/plans/2026-03-09-rich-sync-tui-design.md`

---

## Phase 1: Dependencies and Core Types

### Task 1: Add ratatui and crossterm dependencies

**Files:**

- Modify: `Cargo.toml`

**Step 1: Add dependencies to Cargo.toml**

Add `ratatui` and `crossterm` to `[dependencies]`:

```toml
ratatui = { version = "0.29", default-features = false, features = ["crossterm"] }
crossterm = "0.28"
```

Use `default-features = false` with just the crossterm feature to avoid pulling
in unnecessary backends.

**Step 2: Verify it compiles**

Run: `cargo check` Expected: compiles with no errors

**Step 3: Commit**

```
chore: add ratatui and crossterm dependencies
```

---

### Task 2: Define sync DAG types

**Files:**

- Create: `src/core/worktree/sync_dag.rs`
- Modify: `src/core/worktree/mod.rs` (add `pub mod sync_dag;`)

**Step 1: Write unit tests for type construction**

```rust
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
        assert_eq!(
            OperationPhase::Fetch.label(),
            "Fetching remote branches"
        );
        assert_eq!(
            OperationPhase::Rebase("master".into()).label(),
            "Rebasing onto master"
        );
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib core::worktree::sync_dag` Expected: FAIL — module
doesn't exist yet

**Step 3: Implement the types**

In `src/core/worktree/sync_dag.rs`:

```rust
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
```

Add `pub mod sync_dag;` to `src/core/worktree/mod.rs`.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::worktree::sync_dag` Expected: PASS

**Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 6: Commit**

```
feat(sync): define sync DAG task types and operation phases
```

---

## Phase 2: DAG Construction and Execution

### Task 3: Implement DAG builder

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`

This task adds the `SyncDag` struct and `build_sync_dag` / `build_prune_dag`
functions that construct the dependency graph from worktree info and gone
branches.

**Step 1: Write tests for DAG construction**

```rust
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
    let worktrees = vec![
        ("master".into(), PathBuf::from("/p/master")),
    ];
    let dag = SyncDag::build_sync(worktrees, vec![], None);
    let phases = dag.phases();
    assert_eq!(phases.len(), 3); // Fetch, Prune, Update
}

#[test]
fn dag_phases_sync_with_rebase() {
    let worktrees = vec![
        ("master".into(), PathBuf::from("/p/master")),
    ];
    let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()));
    let phases = dag.phases();
    assert_eq!(phases.len(), 4); // Fetch, Prune, Update, Rebase
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib core::worktree::sync_dag` Expected: FAIL —
`SyncDag` not defined

**Step 3: Implement SyncDag and DAG builders**

Add to `src/core/worktree/sync_dag.rs`:

```rust
/// The dependency graph for a sync or prune operation.
#[derive(Debug)]
pub struct SyncDag {
    /// All tasks in topological-ish order (fetch first).
    pub tasks: Vec<SyncTask>,
    /// For each task index, the indices of tasks it depends on.
    dependencies: Vec<Vec<usize>>,
    /// For each task index, the indices of tasks that depend on it.
    dependents: Vec<Vec<usize>>,
}

impl SyncDag {
    /// Build a DAG for `daft sync` (with optional rebase).
    pub fn build_sync(
        worktrees: Vec<(String, PathBuf)>,
        gone_branches: Vec<String>,
        rebase_branch: Option<String>,
    ) -> Self {
        let mut tasks = Vec::new();
        let mut dependencies: Vec<Vec<usize>> = Vec::new();
        let mut dependents: Vec<Vec<usize>> = Vec::new();

        let mut add_task = |task: SyncTask, deps: Vec<usize>| -> usize {
            let idx = tasks.len();
            tasks.push(task);
            dependencies.push(deps.clone());
            dependents.push(Vec::new());
            for &dep in &deps {
                dependents[dep].push(idx);
            }
            idx
        };

        // Task 0: Fetch remote
        let fetch_idx = add_task(
            SyncTask {
                id: TaskId::Fetch,
                phase: OperationPhase::Fetch,
                worktree_path: None,
                branch_name: String::new(),
            },
            vec![],
        );

        // Prune tasks (depend on fetch)
        for branch in &gone_branches {
            add_task(
                SyncTask {
                    id: TaskId::Prune(branch.clone()),
                    phase: OperationPhase::Prune,
                    worktree_path: None,
                    branch_name: branch.clone(),
                },
                vec![fetch_idx],
            );
        }

        // Update tasks (depend on fetch)
        let mut update_indices: Vec<(String, usize)> = Vec::new();
        for (branch, path) in &worktrees {
            let idx = add_task(
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

        // Rebase tasks (depend on update of the base branch)
        if let Some(ref base) = rebase_branch {
            let base_update_idx = update_indices
                .iter()
                .find(|(name, _)| name == base)
                .map(|(_, idx)| *idx);

            if let Some(base_idx) = base_update_idx {
                for (branch, path) in &worktrees {
                    if branch == base {
                        continue; // Don't rebase the base branch onto itself
                    }
                    add_task(
                        SyncTask {
                            id: TaskId::Rebase(branch.clone()),
                            phase: OperationPhase::Rebase(base.clone()),
                            worktree_path: Some(path.clone()),
                            branch_name: branch.clone(),
                        },
                        vec![base_idx],
                    );
                }
            }
        }

        Self {
            tasks,
            dependencies,
            dependents,
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
    pub fn phases(&self) -> Vec<OperationPhase> {
        let mut phases = Vec::new();
        let mut seen_fetch = false;
        let mut seen_prune = false;
        let mut seen_update = false;
        let mut seen_rebase: Option<String> = None;

        for task in &self.tasks {
            match &task.phase {
                OperationPhase::Fetch if !seen_fetch => {
                    phases.push(OperationPhase::Fetch);
                    seen_fetch = true;
                }
                OperationPhase::Prune if !seen_prune => {
                    phases.push(OperationPhase::Prune);
                    seen_prune = true;
                }
                OperationPhase::Update if !seen_update => {
                    phases.push(OperationPhase::Update);
                    seen_update = true;
                }
                OperationPhase::Rebase(branch) if seen_rebase.is_none() => {
                    phases.push(OperationPhase::Rebase(branch.clone()));
                    seen_rebase = Some(branch.clone());
                }
                _ => {}
            }
        }

        // Always include Prune and Update in sync even if no tasks exist
        if seen_fetch && !seen_prune {
            phases.insert(1, OperationPhase::Prune);
        }
        if seen_fetch && !seen_update && seen_rebase.is_none() {
            // Only for sync, not prune-only
        }

        phases
    }
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::worktree::sync_dag` Expected: PASS

**Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 6: Commit**

```
feat(sync): implement sync DAG builder with dependency tracking
```

---

### Task 4: Implement DAG executor (worker pool)

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`

This task adds the execution engine: a `DagExecutor` that runs tasks in a thread
pool, respecting dependencies, and sends status updates through a channel.

**Step 1: Define the executor types and channel messages**

```rust
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

/// Message sent from worker threads to the renderer.
#[derive(Debug, Clone)]
pub enum DagEvent {
    /// A task started running.
    TaskStarted { task_idx: usize },
    /// A task completed.
    TaskCompleted {
        task_idx: usize,
        status: TaskStatus,
        /// Human-readable result message.
        message: String,
    },
    /// All tasks are done.
    AllDone,
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
```

**Step 2: Write tests for executor behavior**

```rust
#[test]
fn executor_runs_all_tasks() {
    let dag = SyncDag::build_prune(vec!["feat/a".into()]);
    let (tx, rx) = mpsc::channel();

    // We need a task runner that just succeeds immediately
    let executor = DagExecutor::new(dag, tx);
    executor.run(|_task| (TaskStatus::Succeeded, "ok".into()));

    let events: Vec<DagEvent> = rx.iter().collect();
    // Should have: 2 TaskStarted + 2 TaskCompleted + 1 AllDone = 5
    let starts = events.iter().filter(|e| matches!(e, DagEvent::TaskStarted { .. })).count();
    let completes = events.iter().filter(|e| matches!(e, DagEvent::TaskCompleted { .. })).count();
    let dones = events.iter().filter(|e| matches!(e, DagEvent::AllDone)).count();
    assert_eq!(starts, 2);
    assert_eq!(completes, 2);
    assert_eq!(dones, 1);
}

#[test]
fn executor_respects_dependencies() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let worktrees = vec![
        ("master".into(), PathBuf::from("/p/master")),
        ("feat/a".into(), PathBuf::from("/p/feat-a")),
    ];
    let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()));
    let (tx, rx) = mpsc::channel();

    let order = Arc::new(Mutex::new(Vec::new()));
    let order_clone = Arc::clone(&order);

    let executor = DagExecutor::new(dag, tx);
    executor.run(move |task| {
        order_clone.lock().unwrap().push(task.id.clone());
        (TaskStatus::Succeeded, "ok".into())
    });

    let execution_order = order.lock().unwrap();
    // Fetch must come first
    assert_eq!(execution_order[0], TaskId::Fetch);
    // Rebase(feat/a) must come after Update(master)
    let master_pos = execution_order.iter().position(|t| *t == TaskId::Update("master".into())).unwrap();
    let rebase_pos = execution_order.iter().position(|t| *t == TaskId::Rebase("feat/a".into())).unwrap();
    assert!(master_pos < rebase_pos);
}

#[test]
fn executor_cascades_failure() {
    let worktrees = vec![
        ("master".into(), PathBuf::from("/p/master")),
        ("feat/a".into(), PathBuf::from("/p/feat-a")),
    ];
    let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()));
    let (tx, rx) = mpsc::channel();

    let executor = DagExecutor::new(dag, tx);
    executor.run(|task| {
        match &task.id {
            TaskId::Update(name) if name == "master" => {
                (TaskStatus::Failed, "pull failed".into())
            }
            _ => (TaskStatus::Succeeded, "ok".into()),
        }
    });

    let events: Vec<DagEvent> = rx.iter().collect();
    // Rebase(feat/a) should be DepFailed because Update(master) failed
    let rebase_event = events.iter().find(|e| matches!(
        e,
        DagEvent::TaskCompleted { status: TaskStatus::DepFailed, .. }
    ));
    assert!(rebase_event.is_some());
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test -p daft --lib core::worktree::sync_dag` Expected: FAIL —
`DagExecutor` not defined

**Step 4: Implement the executor**

```rust
impl DagExecutor {
    pub fn new(dag: SyncDag, sender: mpsc::Sender<DagEvent>) -> Self {
        Self { dag, sender }
    }

    /// Run all tasks in the DAG using a thread pool.
    ///
    /// `task_fn` is called for each task and must return (status, message).
    /// It is called from worker threads and must be Send + Sync.
    pub fn run<F>(self, task_fn: F)
    where
        F: Fn(&SyncTask) -> (TaskStatus, String) + Send + Sync,
    {
        let task_count = self.dag.tasks.len();
        if task_count == 0 {
            let _ = self.sender.send(DagEvent::AllDone);
            return;
        }

        // Compute initial in-degrees
        let mut in_degree = vec![0usize; task_count];
        for (i, deps) in self.dag.dependencies.iter().enumerate() {
            in_degree[i] = deps.len();
        }

        // Find initially ready tasks
        let ready: Vec<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &deg)| deg == 0)
            .map(|(i, _)| i)
            .collect();

        let state = Arc::new((
            Mutex::new(DagState {
                ready,
                status: vec![TaskStatus::Pending; task_count],
                in_degree,
                active: 0,
                done: 0,
                total: task_count,
            }),
            Condvar::new(),
        ));

        let max_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        let task_fn = Arc::new(task_fn);

        std::thread::scope(|scope| {
            for _ in 0..max_workers {
                let state = Arc::clone(&state);
                let sender = self.sender.clone();
                let task_fn = Arc::clone(&task_fn);
                let tasks = &self.dag.tasks;
                let dependents = &self.dag.dependents;

                scope.spawn(move || {
                    loop {
                        let task_idx = {
                            let (lock, cvar) = &*state;
                            let mut s = lock.lock().unwrap();

                            loop {
                                if let Some(idx) = s.ready.pop() {
                                    s.status[idx] = TaskStatus::Running;
                                    s.active += 1;
                                    break Some(idx);
                                }
                                if s.done + s.active >= s.total {
                                    break None; // All work claimed or done
                                }
                                s = cvar.wait(s).unwrap();
                            }
                        };

                        let Some(task_idx) = task_idx else {
                            break;
                        };

                        let _ = sender.send(DagEvent::TaskStarted { task_idx });

                        let (status, message) = task_fn(&tasks[task_idx]);

                        let _ = sender.send(DagEvent::TaskCompleted {
                            task_idx,
                            status,
                            message,
                        });

                        // Update DAG state and unlock dependents
                        {
                            let (lock, cvar) = &*state;
                            let mut s = lock.lock().unwrap();
                            s.status[task_idx] = status;
                            s.active -= 1;
                            s.done += 1;

                            if status == TaskStatus::Succeeded
                                || status == TaskStatus::Skipped
                            {
                                // Unlock dependents
                                for &dep_idx in &dependents[task_idx] {
                                    if s.status[dep_idx] == TaskStatus::Pending {
                                        s.in_degree[dep_idx] -= 1;
                                        if s.in_degree[dep_idx] == 0 {
                                            s.ready.push(dep_idx);
                                        }
                                    }
                                }
                            } else {
                                // Cascade failure
                                Self::cascade_dep_failed(
                                    &mut s.status,
                                    dependents,
                                    task_idx,
                                    &mut s.done,
                                    &sender,
                                );
                            }

                            cvar.notify_all();
                        }
                    }
                });
            }
        });

        let _ = self.sender.send(DagEvent::AllDone);
    }

    fn cascade_dep_failed(
        status: &mut [TaskStatus],
        dependents: &[Vec<usize>],
        failed_idx: usize,
        done: &mut usize,
        sender: &mpsc::Sender<DagEvent>,
    ) {
        let mut stack = vec![failed_idx];
        while let Some(idx) = stack.pop() {
            for &dep_idx in &dependents[idx] {
                if status[dep_idx] == TaskStatus::Pending {
                    status[dep_idx] = TaskStatus::DepFailed;
                    *done += 1;
                    let _ = sender.send(DagEvent::TaskCompleted {
                        task_idx: dep_idx,
                        status: TaskStatus::DepFailed,
                        message: "dependency failed".into(),
                    });
                    stack.push(dep_idx);
                }
            }
        }
    }
}
```

**Step 5: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::worktree::sync_dag` Expected: PASS

**Step 6: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 7: Commit**

```
feat(sync): implement parallel DAG executor with dependency tracking
```

---

## Phase 3: TUI Renderer

### Task 5: Create TUI state model and event types

**Files:**

- Create: `src/output/tui.rs`
- Modify: `src/output/mod.rs` (add `pub mod tui;`)

This task defines the renderer's internal state model — what it tracks to know
how to draw each frame.

**Step 1: Define the TUI state types**

```rust
//! Inline TUI renderer for sync and prune operations.
//!
//! Uses ratatui with Viewport::Inline to render an operation header
//! and worktree status table that update in-place as tasks execute.

use crate::core::worktree::list::WorktreeInfo;
use crate::core::worktree::sync_dag::{DagEvent, OperationPhase, TaskStatus};

/// Status of a high-level operation phase in the header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseStatus {
    /// Not yet started.
    Pending,
    /// At least one task in this phase is running.
    Active,
    /// All tasks in this phase have completed.
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
    /// No operation has touched this worktree yet.
    Idle,
    /// Currently being operated on.
    Active(String), // e.g. "updating", "rebasing"
    /// Operation completed with this final status.
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
    /// Operation phases for the header.
    pub phases: Vec<PhaseState>,
    /// Per-worktree display info (from daft list) plus current status.
    pub worktrees: Vec<WorktreeRow>,
    /// Whether all operations are complete.
    pub done: bool,
    /// Spinner tick counter for animation.
    pub tick: usize,
}

/// A single row in the worktree table.
pub struct WorktreeRow {
    /// The worktree info from `daft list` (branch name, path, etc.).
    pub info: WorktreeInfo,
    /// Current operation status.
    pub status: WorktreeStatus,
}
```

**Step 2: Verify it compiles**

Run: `cargo check` Expected: compiles

**Step 3: Commit**

```
feat(sync): define TUI state model for sync renderer
```

---

### Task 6: Implement TUI state update logic

**Files:**

- Modify: `src/output/tui.rs`

This task adds the logic to update `TuiState` from `DagEvent` messages — the
bridge between the executor and the renderer.

**Step 1: Write tests for state transitions**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::worktree::sync_dag::*;
    use std::path::PathBuf;

    fn make_test_state() -> (TuiState, SyncDag) {
        let worktrees = vec![
            ("master".into(), PathBuf::from("/p/master")),
            ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ];
        let dag = SyncDag::build_sync(worktrees, vec!["feat/old".into()], None);
        let phases = dag.phases();

        let worktree_infos = vec![
            // Minimal WorktreeInfo stubs for testing
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
        assert!(state.phases.iter().all(|p| p.status == PhaseStatus::Pending));
        assert!(state.worktrees.iter().all(|w| w.status == WorktreeStatus::Idle));
        assert!(!state.done);
    }

    #[test]
    fn task_started_activates_phase_and_row() {
        let (mut state, dag) = make_test_state();
        // Fetch task started (index 0)
        state.apply_event(&DagEvent::TaskStarted { task_idx: 0 }, &dag);
        assert_eq!(state.phases[0].status, PhaseStatus::Active);
    }

    #[test]
    fn task_completed_updates_row_status() {
        let (mut state, dag) = make_test_state();

        // Find update(master) task index
        let master_idx = dag.tasks.iter()
            .position(|t| t.id == TaskId::Update("master".into()))
            .unwrap();

        state.apply_event(&DagEvent::TaskStarted { task_idx: master_idx }, &dag);
        state.apply_event(
            &DagEvent::TaskCompleted {
                task_idx: master_idx,
                status: TaskStatus::Succeeded,
                message: "Already up to date".into(),
            },
            &dag,
        );

        let row = state.worktrees.iter().find(|w| w.info.name == "master").unwrap();
        assert_eq!(row.status, WorktreeStatus::Done(FinalStatus::UpToDate));
    }

    #[test]
    fn all_done_sets_done_flag() {
        let (mut state, dag) = make_test_state();
        state.apply_event(&DagEvent::AllDone, &dag);
        assert!(state.done);
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib output::tui` Expected: FAIL — methods not
implemented

**Step 3: Implement state update logic**

```rust
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

    /// Apply a DAG event to update the TUI state.
    pub fn apply_event(&mut self, event: &DagEvent, dag: &SyncDag) {
        match event {
            DagEvent::TaskStarted { task_idx } => {
                let task = &dag.tasks[*task_idx];
                // Activate the phase
                self.activate_phase(&task.phase);
                // Update the worktree row
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
                // Update the worktree row with final status
                let final_status = Self::map_final_status(task, *status, message);
                if let Some(row) = self.find_row_mut(&task.branch_name) {
                    row.status = WorktreeStatus::Done(final_status);
                }
                // Check if this phase is now complete
                self.check_phase_completion(&task.phase, dag);
            }
            DagEvent::AllDone => {
                // Mark all phases as completed
                for phase in &mut self.phases {
                    if phase.status != PhaseStatus::Completed {
                        phase.status = PhaseStatus::Completed;
                    }
                }
                self.done = true;
            }
        }
    }

    fn activate_phase(&mut self, phase: &OperationPhase) {
        if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase) {
            if ps.status == PhaseStatus::Pending {
                ps.status = PhaseStatus::Active;
            }
        }
    }

    fn check_phase_completion(&mut self, phase: &OperationPhase, dag: &SyncDag) {
        // A phase is complete when all its tasks are in a terminal state
        let all_done = dag
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| &t.phase == *phase)
            .all(|(i, _)| {
                // We check via worktree row status, but for Fetch
                // (which has no row) we just mark complete on TaskCompleted
                true // Simplified — full impl checks DagState
            });
        // Simplified: mark complete when we get a TaskCompleted for this phase
        // and no tasks in this phase are still Active in worktree rows
        let any_active = self.worktrees.iter().any(|w| {
            if let WorktreeStatus::Active(label) = &w.status {
                match phase {
                    OperationPhase::Fetch => label == "fetching",
                    OperationPhase::Prune => label == "pruning",
                    OperationPhase::Update => label == "updating",
                    OperationPhase::Rebase(_) => label == "rebasing",
                }
            } else {
                false
            }
        });
        if !any_active {
            if let Some(ps) = self.phases.iter_mut().find(|ps| &ps.phase == phase) {
                ps.status = PhaseStatus::Completed;
            }
        }
    }

    fn find_row_mut(&mut self, branch_name: &str) -> Option<&mut WorktreeRow> {
        self.worktrees.iter_mut().find(|w| w.info.name == branch_name)
    }

    fn map_final_status(
        task: &SyncTask,
        status: TaskStatus,
        message: &str,
    ) -> FinalStatus {
        match status {
            TaskStatus::Failed => FinalStatus::Failed,
            TaskStatus::DepFailed => FinalStatus::Skipped,
            TaskStatus::Skipped => FinalStatus::Skipped,
            TaskStatus::Succeeded => match &task.phase {
                OperationPhase::Prune => FinalStatus::Pruned,
                OperationPhase::Update => {
                    if message.contains("up to date") || message.contains("Already up to date")
                    {
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
                OperationPhase::Fetch => FinalStatus::Updated, // Not displayed in table
            },
            _ => FinalStatus::Failed,
        }
    }
}
```

Note: The test helper `make_worktree_info` will need to construct minimal
`WorktreeInfo` instances. This may require making some fields optional or
creating a test constructor. Adapt to what the compiler requires.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p daft --lib output::tui` Expected: PASS

**Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 6: Commit**

```
feat(sync): implement TUI state update logic from DAG events
```

---

### Task 7: Implement ratatui inline renderer — operation header

**Files:**

- Modify: `src/output/tui.rs`

This task builds the ratatui rendering function for the operation header region
(the list of phases with spinner/checkmark indicators).

**Step 1: Add rendering imports and spinner constants**

```rust
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const CHECKMARK: &str = "✓";
```

**Step 2: Implement render_header function**

```rust
impl TuiState {
    /// Render the operation header into the given area.
    pub fn render_header(&self, frame: &mut Frame, area: Rect) {
        let lines: Vec<Line> = self
            .phases
            .iter()
            .map(|ps| {
                let (indicator, style) = match ps.status {
                    PhaseStatus::Pending => (
                        "  ".into(),
                        Style::default().fg(Color::DarkGray),
                    ),
                    PhaseStatus::Active => (
                        format!(
                            "{} ",
                            SPINNER_FRAMES[self.tick % SPINNER_FRAMES.len()]
                        ),
                        Style::default().fg(Color::Yellow),
                    ),
                    PhaseStatus::Completed => (
                        format!("{CHECKMARK} "),
                        Style::default().fg(Color::Green),
                    ),
                };

                let label_style = match ps.status {
                    PhaseStatus::Completed => {
                        Style::default().add_modifier(Modifier::DIM)
                    }
                    _ => style,
                };

                Line::from(vec![
                    Span::styled(indicator, style),
                    Span::styled(ps.phase.label(), label_style),
                ])
            })
            .collect();

        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }

    /// Advance the spinner tick.
    pub fn tick(&mut self) {
        self.tick += 1;
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo check` Expected: compiles

**Step 4: Commit**

```
feat(sync): implement TUI operation header renderer
```

---

### Task 8: Implement ratatui inline renderer — worktree table

**Files:**

- Modify: `src/output/tui.rs`

This task builds the worktree table renderer with the Status column and column
priority system for narrow terminals.

**Step 1: Define column priority types**

```rust
use ratatui::widgets::{Cell, Row, Table};

/// Column definition with priority for narrow terminal degradation.
#[derive(Debug, Clone, Copy)]
enum Column {
    Status,
    Annotation,
    Branch,
    Path,
    Base,
    Remote,
    Changes,
    Age,
    LastCommit,
}

impl Column {
    /// Priority (lower = higher priority, always shown first).
    fn priority(self) -> u8 {
        match self {
            Self::Status => 0,
            Self::Annotation => 1,
            Self::Branch => 2,
            Self::Path => 3,
            Self::Base => 4,
            Self::Remote => 5,
            Self::Changes => 6,
            Self::Age => 7,
            Self::LastCommit => 8,
        }
    }

    /// Minimum width needed for this column.
    fn min_width(self) -> u16 {
        match self {
            Self::Status => 12,     // "⟳ rebasing" = ~10 chars
            Self::Annotation => 3,  // "> ◉"
            Self::Branch => 10,
            Self::Path => 8,
            Self::Base => 5,
            Self::Remote => 5,
            Self::Changes => 7,
            Self::Age => 4,
            Self::LastCommit => 4,  // Minimum: just age
        }
    }
}
```

**Step 2: Write tests for column selection**

```rust
#[test]
fn column_selection_wide_terminal() {
    let cols = select_columns(120);
    assert_eq!(cols.len(), 9); // All columns
}

#[test]
fn column_selection_narrow_drops_last_commit() {
    let cols = select_columns(60);
    assert!(!cols.contains(&Column::LastCommit));
}

#[test]
fn column_selection_very_narrow_keeps_essentials() {
    let cols = select_columns(30);
    assert!(cols.contains(&Column::Status));
    assert!(cols.contains(&Column::Branch));
}
```

**Step 3: Implement column selection and table rendering**

```rust
/// Select which columns to show given the terminal width.
fn select_columns(width: u16) -> Vec<Column> {
    let all_columns = [
        Column::Status,
        Column::Annotation,
        Column::Branch,
        Column::Path,
        Column::Base,
        Column::Remote,
        Column::Changes,
        Column::Age,
        Column::LastCommit,
    ];

    let mut columns: Vec<Column> = all_columns.to_vec();
    let mut total_min: u16 = columns.iter().map(|c| c.min_width()).sum();

    // Drop columns from lowest priority until we fit
    while total_min > width && columns.len() > 3 {
        // Always keep Status, Annotation, Branch
        let lowest = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.priority() > 2)
            .max_by_key(|(_, c)| c.priority());

        if let Some((idx, _)) = lowest {
            total_min -= columns[idx].min_width();
            columns.remove(idx);
        } else {
            break;
        }
    }

    columns
}

impl TuiState {
    /// Render the worktree table into the given area.
    pub fn render_table(&self, frame: &mut Frame, area: Rect) {
        let columns = select_columns(area.width);

        let header_cells: Vec<Cell> = columns
            .iter()
            .map(|col| {
                let label = match col {
                    Column::Status => "Status",
                    Column::Annotation => "",
                    Column::Branch => "Branch",
                    Column::Path => "Path",
                    Column::Base => "Base",
                    Column::Remote => "Remote",
                    Column::Changes => "Changes",
                    Column::Age => "Age",
                    Column::LastCommit => "Last Commit",
                };
                Cell::from(label).style(Style::default().add_modifier(Modifier::DIM))
            })
            .collect();

        let header = Row::new(header_cells);

        let rows: Vec<Row> = self
            .worktrees
            .iter()
            .map(|wr| {
                let cells: Vec<Cell> = columns
                    .iter()
                    .map(|col| self.render_cell(wr, *col))
                    .collect();
                Row::new(cells)
            })
            .collect();

        let widths: Vec<Constraint> = columns
            .iter()
            .map(|col| match col {
                Column::Status => Constraint::Length(14),
                Column::Annotation => Constraint::Length(3),
                Column::Branch => Constraint::Fill(2),
                Column::Path => Constraint::Fill(2),
                Column::Base => Constraint::Length(8),
                Column::Remote => Constraint::Length(8),
                Column::Changes => Constraint::Length(10),
                Column::Age => Constraint::Length(4),
                Column::LastCommit => Constraint::Fill(3),
            })
            .collect();

        let table = Table::new(rows, widths)
            .header(header)
            .column_spacing(1);

        frame.render_widget(table, area);
    }

    fn render_cell(&self, row: &WorktreeRow, col: Column) -> Cell {
        match col {
            Column::Status => self.render_status_cell(row),
            Column::Annotation => self.render_annotation_cell(row),
            Column::Branch => Cell::from(row.info.name.clone()),
            Column::Path => {
                // Relative path display — adapt from list.rs logic
                Cell::from(
                    row.info
                        .path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                )
            }
            // Base, Remote, Changes, Age, LastCommit:
            // Reuse formatting logic from list.rs (extract to shared module)
            _ => Cell::from(""), // Placeholder — filled in during integration
        }
    }

    fn render_status_cell(&self, row: &WorktreeRow) -> Cell {
        match &row.status {
            WorktreeStatus::Idle => Cell::from(""),
            WorktreeStatus::Active(label) => {
                let spinner = SPINNER_FRAMES[self.tick % SPINNER_FRAMES.len()];
                Cell::from(format!("{spinner} {label}"))
                    .style(Style::default().fg(Color::Yellow))
            }
            WorktreeStatus::Done(final_status) => {
                let (icon, text, color) = match final_status {
                    FinalStatus::Updated => (CHECKMARK, "updated", Color::Green),
                    FinalStatus::UpToDate => (CHECKMARK, "up to date", Color::DarkGray),
                    FinalStatus::Rebased => (CHECKMARK, "rebased", Color::Green),
                    FinalStatus::Conflict => ("✗", "conflict", Color::Red),
                    FinalStatus::Skipped => ("⊘", "skipped", Color::Yellow),
                    FinalStatus::Pruned => ("—", "pruned", Color::Red),
                    FinalStatus::Failed => ("✗", "failed", Color::Red),
                };
                Cell::from(format!("{icon} {text}"))
                    .style(Style::default().fg(color))
            }
        }
    }

    fn render_annotation_cell(&self, row: &WorktreeRow) -> Cell {
        let mut annotation = String::new();
        if row.info.is_current {
            annotation.push('>');
        }
        if row.info.is_default_branch {
            if !annotation.is_empty() {
                annotation.push(' ');
            }
            annotation.push('\u{25C9}');
        }
        Cell::from(annotation).style(Style::default().fg(Color::Cyan))
    }
}
```

**Step 4: Run tests and verify they pass**

Run: `cargo test -p daft --lib output::tui` Expected: PASS

**Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 6: Commit**

```
feat(sync): implement TUI worktree table with column priority
```

---

### Task 9: Implement the TUI main loop

**Files:**

- Modify: `src/output/tui.rs`

This task creates the `TuiRenderer` struct that owns the ratatui Terminal and
runs the render loop, receiving events from the DAG executor.

**Step 1: Implement TuiRenderer**

```rust
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Inline TUI renderer for sync/prune operations.
pub struct TuiRenderer {
    state: TuiState,
    dag: Arc<SyncDag>,
    receiver: mpsc::Receiver<DagEvent>,
}

impl TuiRenderer {
    pub fn new(
        state: TuiState,
        dag: Arc<SyncDag>,
        receiver: mpsc::Receiver<DagEvent>,
    ) -> Self {
        Self {
            state,
            dag,
            receiver,
        }
    }

    /// Run the render loop until all tasks complete.
    /// Returns the final TuiState for post-render summary.
    pub fn run(mut self) -> anyhow::Result<TuiState> {
        let header_height = self.state.phases.len() as u16 + 1; // +1 blank line
        let table_height = self.state.worktrees.len() as u16 + 2; // +1 header +1 padding
        let viewport_height = header_height + table_height;

        let backend = ratatui::backend::CrosstermBackend::new(std::io::stderr());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(viewport_height),
            },
        )?;

        let tick_rate = Duration::from_millis(80);
        let mut last_tick = Instant::now();

        loop {
            // Render current state
            terminal.draw(|frame| {
                let area = frame.area();
                let chunks = Layout::vertical([
                    Constraint::Length(header_height),
                    Constraint::Fill(1),
                ])
                .split(area);

                self.state.render_header(frame, chunks[0]);
                self.state.render_table(frame, chunks[1]);
            })?;

            // Process all pending events
            loop {
                match self.receiver.try_recv() {
                    Ok(event) => {
                        let is_done = matches!(event, DagEvent::AllDone);
                        self.state.apply_event(&event, &self.dag);
                        if is_done {
                            // Final render
                            terminal.draw(|frame| {
                                let area = frame.area();
                                let chunks = Layout::vertical([
                                    Constraint::Length(header_height),
                                    Constraint::Fill(1),
                                ])
                                .split(area);

                                self.state.render_header(frame, chunks[0]);
                                self.state.render_table(frame, chunks[1]);
                            })?;
                            return Ok(self.state);
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Ok(self.state);
                    }
                }
            }

            // Tick the spinner
            if last_tick.elapsed() >= tick_rate {
                self.state.tick();
                last_tick = Instant::now();
            }

            // Small sleep to avoid busy-waiting
            std::thread::sleep(Duration::from_millis(16));
        }
    }
}
```

**Step 2: Verify it compiles**

Run: `cargo check` Expected: compiles

**Step 3: Commit**

```
feat(sync): implement TUI render loop with inline viewport
```

---

## Phase 4: Extract Per-Worktree Operations

### Task 10: Extract per-worktree prune function

**Files:**

- Modify: `src/core/worktree/prune.rs`

Extract the per-branch prune logic into a public function that can be called by
a DAG worker. The existing `execute()` function continues to work by calling
this extracted function internally.

**Step 1: Create a public per-branch prune function**

Extract from the loop at lines 147-218 of `prune.rs`. The new function should:

- Accept a branch name, the `PruneContext`, worktree map, force flag, and a
  `ProgressSink + HookRunner`
- Return a `PrunedBranchDetail` plus mutation deltas (branches_deleted,
  worktrees_removed)
- Handle the three cases: main worktree, linked worktree, no worktree

```rust
/// Result of pruning a single branch.
pub struct SingleBranchPruneResult {
    pub detail: PrunedBranchDetail,
    pub branches_deleted: u32,
    pub worktrees_removed: u32,
    /// Whether this branch was deferred (current worktree).
    pub deferred: bool,
}

/// Prune a single branch. Called by the DAG executor for parallel pruning.
///
/// Returns None if the branch should be deferred (current worktree).
pub fn prune_single_branch(
    ctx: &PruneContext,
    branch_name: &str,
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    is_bare_layout: bool,
    current_wt_path: &Option<PathBuf>,
    current_branch: &Option<String>,
    params: &PruneParams,
    sink: &mut (impl ProgressSink + HookRunner),
) -> SingleBranchPruneResult {
    // ... extract logic from the loop at lines 147-218
}
```

Make `PruneContext` and `identify_gone_branches` public so the DAG builder can
call them.

**Step 2: Refactor execute() to call the new function**

The existing `execute()` should call `prune_single_branch` in its loop, keeping
behavior identical.

**Step 3: Run existing tests**

Run: `mise run test:unit` Expected: PASS — refactor is behavior-preserving

**Step 4: Run integration tests**

Run: `mise run test:integration` Expected: PASS

**Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 6: Commit**

```
refactor(prune): extract per-branch prune logic for DAG workers
```

---

### Task 11: Extract per-worktree update function

**Files:**

- Modify: `src/core/worktree/fetch.rs`

The `process_worktree` function at line 307 is already mostly extracted. Make it
public and ensure it can be called independently by a DAG worker without needing
the full `FetchParams` orchestration.

**Step 1: Make process_worktree and helpers public**

```rust
/// Update a single worktree by pulling from its tracking branch.
/// Called by DAG workers for parallel updating.
pub fn update_single_worktree(
    git: &GitCommand,
    target_path: &Path,
    worktree_name: &str,
    pull_args: &[String],
    params: &FetchParams,
    progress: &mut dyn ProgressSink,
) -> WorktreeFetchResult {
    // Delegate to existing process_worktree with a same-branch refspec
    let refspec = UpdateRefSpec {
        source: worktree_name.to_string(),
        destination: worktree_name.to_string(),
    };
    process_worktree(git, target_path, worktree_name, pull_args, params, &refspec, progress)
}
```

Also make `get_all_worktrees_with_branches` public (already used by rebase.rs).

**Step 2: Run existing tests**

Run: `mise run test:unit && mise run test:integration` Expected: PASS

**Step 3: Commit**

```
refactor(fetch): expose per-worktree update for DAG workers
```

---

### Task 12: Extract per-worktree rebase function

**Files:**

- Modify: `src/core/worktree/rebase.rs`

Extract the per-worktree rebase logic from the loop at lines 73-159 into a
standalone public function.

**Step 1: Create a public per-worktree rebase function**

```rust
/// Rebase a single worktree onto the base branch.
/// Called by DAG workers for parallel rebasing.
pub fn rebase_single_worktree(
    git: &GitCommand,
    worktree_path: &Path,
    worktree_name: &str,
    base_branch: &str,
    force: bool,
    progress: &mut dyn ProgressSink,
) -> WorktreeRebaseResult {
    // ... extract logic from lines 78-158 of execute()
}
```

**Step 2: Refactor execute() to call the new function**

**Step 3: Run existing tests**

Run: `mise run test:unit && mise run test:integration` Expected: PASS

**Step 4: Commit**

```
refactor(rebase): extract per-worktree rebase for DAG workers
```

---

## Phase 5: Wire Up Commands

### Task 13: Integrate TUI into sync command

**Files:**

- Modify: `src/commands/sync.rs`

Replace the current three-phase sequential flow with the DAG + TUI approach.
Keep the existing rendering code as the non-TTY fallback.

**Step 1: Add the TUI code path**

```rust
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-sync"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;

    if std::io::stderr().is_terminal() && !args.verbose {
        run_tui(args, settings)
    } else {
        run_sequential(args, settings) // Existing logic, moved to a function
    }
}

fn run_tui(args: Args, settings: DaftSettings) -> Result<()> {
    // 1. Collect worktree info for the table
    // 2. Do the fetch + identify gone branches (pre-TUI, with simple spinner)
    // 3. Build the DAG
    // 4. Create TUI state from worktree info + DAG phases
    // 5. Spawn DAG executor on worker threads
    // 6. Run TUI renderer on main thread
    // 7. Handle cd_target from prune results
}
```

The actual task runner function passed to `DagExecutor::run` dispatches based on
`TaskId`:

```rust
let task_fn = move |task: &SyncTask| -> (TaskStatus, String) {
    match &task.id {
        TaskId::Fetch => {
            // Already done pre-TUI
            (TaskStatus::Succeeded, "fetched".into())
        }
        TaskId::Prune(branch) => {
            // Call prune_single_branch
        }
        TaskId::Update(branch) => {
            // Call update_single_worktree
        }
        TaskId::Rebase(branch) => {
            // Call rebase_single_worktree
        }
    }
};
```

**Step 2: Test manually**

Run: `cargo build && ./target/debug/daft sync` Expected: TUI renders inline with
operation header and worktree table

**Step 3: Run all tests**

Run: `mise run test` Expected: PASS (integration tests use non-TTY path)

**Step 4: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

**Step 5: Commit**

```
feat(sync): integrate DAG executor and TUI renderer
```

---

### Task 14: Integrate TUI into prune command

**Files:**

- Modify: `src/commands/prune.rs`

Same pattern as sync but simpler — only fetch + prune phases, no update or
rebase.

**Step 1: Add the TUI code path**

Follow the same pattern as Task 13 but with `SyncDag::build_prune()` and only
the fetch + prune phases in the operation header.

**Step 2: Test manually**

Run: `cargo build && ./target/debug/daft prune`

**Step 3: Run all tests**

Run: `mise run test` Expected: PASS

**Step 4: Commit**

```
feat(prune): integrate DAG executor and TUI renderer
```

---

### Task 15: Non-TTY fallback renderer

**Files:**

- Modify: `src/commands/sync.rs`
- Modify: `src/commands/prune.rs`

Ensure the existing sequential text output (the `run_sequential` path) uses the
new status wording (no square brackets) and works correctly when stderr is not a
TTY.

**Step 1: Update status formatting in sequential path**

Replace `[updated]`, `[pruned]`, etc. with plain `✓ updated`, `— pruned`, etc.
in the sequential rendering functions.

**Step 2: Run integration tests**

Run: `mise run test:integration` Expected: PASS — integration tests run in
non-TTY mode

**Step 3: Commit**

```
refactor(sync): update sequential fallback to use new status formatting
```

---

## Phase 6: Shared Formatting and Polish

### Task 16: Extract shared formatting from list.rs

**Files:**

- Create: `src/output/format.rs` (or add to existing output module)
- Modify: `src/commands/list.rs` — use shared formatters
- Modify: `src/output/tui.rs` — use shared formatters

Extract the column formatting functions from `list.rs` that the TUI table also
needs:

- `format_ahead_behind` (line 558)
- `format_head_status` (line 587)
- `format_remote_status` (line 621)
- `shorthand_from_seconds` (line 652)
- `format_shorthand_age` (line 681)
- `relative_display_path` (line 519)

Move these to a shared location and have both `list.rs` and `tui.rs` import
them. The existing tests in `list.rs` for `shorthand_from_seconds` move with the
functions.

**Step 1: Move formatters to shared module**

**Step 2: Update list.rs to import from shared module**

**Step 3: Wire up TUI table cells to use real formatters**

Replace the placeholder `Cell::from("")` in `render_cell` for Base, Remote,
Changes, Age, and LastCommit columns.

**Step 4: Run all tests**

Run: `mise run test` Expected: PASS

**Step 5: Commit**

```
refactor: extract shared column formatters from list.rs
```

---

### Task 17: End-to-end manual testing and polish

**Files:**

- Various — bug fixes found during testing

**Step 1: Test sync with multiple worktrees**

Test cases:

- `daft sync` — prune + update, no rebase
- `daft sync --rebase master` — full flow
- `daft sync --force` — with dirty worktrees
- `daft sync` in a narrow terminal (< 80 cols)
- `daft sync` piped (`daft sync 2>&1 | cat`) — non-TTY fallback
- `daft prune` standalone

**Step 2: Fix any rendering issues**

Adjust column widths, alignment, spinner timing, etc.

**Step 3: Run full CI simulation**

Run: `mise run ci` Expected: PASS

**Step 4: Commit any fixes**

```
fix(sync): polish TUI rendering [details]
```

---

## Task Dependency Overview

```
Task 1 (deps) ─► Task 2 (types) ─► Task 3 (DAG builder) ─► Task 4 (executor)
                                                                    │
Task 5 (TUI state) ─► Task 6 (state logic) ─► Task 7 (header) ─► Task 8 (table)
                                                                    │
                                                               Task 9 (main loop)
                                                                    │
Task 10 (extract prune) ─┐                                         │
Task 11 (extract fetch) ─┼─► Task 13 (wire sync) ─► Task 15 (fallback)
Task 12 (extract rebase) ┘         │                      │
                              Task 14 (wire prune)   Task 16 (shared fmt)
                                                          │
                                                     Task 17 (polish)
```

Tasks 1-4 and 5-9 can be developed in parallel. Tasks 10-12 can be developed in
parallel. Tasks 13-14 depend on both streams. Task 16-17 are final polish.
