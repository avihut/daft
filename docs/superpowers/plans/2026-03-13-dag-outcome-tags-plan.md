# DAG Outcome Tags Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development
> (if subagents available) or superpowers:executing-plans to implement this
> plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-branch outcome tags to the sync DAG so that a rebase conflict
prevents the downstream push from running, and the TUI shows the correct final
status.

**Architecture:** Introduce `TaskOutcome` tags stored per-branch in `DagState`.
The executor passes outcomes to the task function and stores the returned set.
Tasks check preconditions against outcomes and return
`TaskStatus::PreconditionFailed` to skip without overwriting the TUI row status.
The sequential (non-TUI) path passes conflicted branch names from rebase to
push.

**Tech Stack:** Rust, ratatui (TUI), std::sync::mpsc (DAG events)

---

## File Structure

| File                            | Responsibility                                                                                                       |
| ------------------------------- | -------------------------------------------------------------------------------------------------------------------- |
| `src/core/worktree/sync_dag.rs` | `TaskOutcome` enum, `PreconditionFailed` variant, `branch_outcomes` in `DagState`, executor changes                  |
| `src/commands/sync.rs`          | Task closure adapts to new signature, rebase/push executors produce/check outcomes, sequential path passes conflicts |
| `src/output/tui/state.rs`       | `PreconditionFailed` handling in `apply_event` (skip row update)                                                     |
| `src/core/worktree/push.rs`     | `execute()` accepts optional exclusion set for sequential path                                                       |

## Chunk 1: DAG Core Changes

### Task 1: Add `TaskOutcome` enum and `PreconditionFailed` status

**Files:**

- Modify: `src/core/worktree/sync_dag.rs:29-48` (TaskStatus enum + is_terminal)
- Modify: `src/core/worktree/sync_dag.rs:1-11` (imports)

- [ ] **Step 1: Write failing test for `PreconditionFailed` terminal status**

In the existing `tests` module at the bottom of `sync_dag.rs`, add:

```rust
#[test]
fn precondition_failed_is_terminal() {
    assert!(TaskStatus::PreconditionFailed.is_terminal());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib precondition_failed_is_terminal` Expected: FAIL - no
variant `PreconditionFailed`

- [ ] **Step 3: Add `TaskOutcome` enum and `PreconditionFailed` variant**

Add to `sync_dag.rs` after the imports (before `TaskId`):

```rust
/// Semantic outcomes that a task can produce. Downstream tasks may
/// inspect these as preconditions for whether to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskOutcome {
    /// Rebase had conflicts and was aborted.
    Conflict,
}
```

Add `PreconditionFailed` to `TaskStatus`:

```rust
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
```

Update `is_terminal()`:

```rust
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
```

Add `use std::collections::{HashMap, HashSet};` to the imports (HashMap and
HashSet are needed for the outcome storage).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib precondition_failed_is_terminal` Expected: PASS

- [ ] **Step 5: Run all existing sync_dag tests to verify no regressions**

Run: `cargo test --lib sync_dag` Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/sync_dag.rs
git commit -m "feat(sync): add TaskOutcome enum and PreconditionFailed status"
```

### Task 2: Add `branch_outcomes` to DagState and wire executor

**Files:**

- Modify: `src/core/worktree/sync_dag.rs:383-572` (DagState, DagExecutor::run)

- [ ] **Step 1: Write failing test for outcome propagation**

Add to the `tests` module in `sync_dag.rs`:

```rust
#[test]
fn executor_propagates_outcomes_to_dependent_tasks() {
    use std::collections::HashSet;
    // Build a DAG with rebase and push: feat/a rebase -> push
    let worktrees = vec![
        ("master".into(), PathBuf::from("/p/master")),
        ("feat/a".into(), PathBuf::from("/p/feat-a")),
    ];
    let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()), true);
    let (tx, rx) = mpsc::channel();

    let received_outcomes: Arc<Mutex<Vec<(TaskId, HashSet<TaskOutcome>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let outcomes_clone = Arc::clone(&received_outcomes);

    let executor = DagExecutor::new(dag, tx);
    executor.run(move |task, outcomes| {
        outcomes_clone
            .lock()
            .unwrap()
            .push((task.id.clone(), outcomes.clone()));

        match &task.id {
            TaskId::Rebase(name) if name == "feat/a" => {
                // Simulate conflict: return Conflict outcome
                let mut out = outcomes.clone();
                out.insert(TaskOutcome::Conflict);
                (TaskStatus::Succeeded, TaskMessage::Conflict, out, None)
            }
            TaskId::Push(name) if name == "feat/a" => {
                // Push should receive the Conflict outcome
                if outcomes.contains(&TaskOutcome::Conflict) {
                    (
                        TaskStatus::PreconditionFailed,
                        TaskMessage::Failed("rebase conflict".into()),
                        outcomes.clone(),
                        None,
                    )
                } else {
                    (
                        TaskStatus::Succeeded,
                        TaskMessage::Pushed,
                        outcomes.clone(),
                        None,
                    )
                }
            }
            _ => (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
                None,
            ),
        }
    });

    let _events: Vec<DagEvent> = rx.iter().collect();
    let recorded = received_outcomes.lock().unwrap();

    // Find what Push(feat/a) received
    let push_entry = recorded
        .iter()
        .find(|(id, _)| *id == TaskId::Push("feat/a".into()));
    assert!(push_entry.is_some(), "Push(feat/a) should have been called");
    let (_, push_outcomes) = push_entry.unwrap();
    assert!(
        push_outcomes.contains(&TaskOutcome::Conflict),
        "Push(feat/a) should receive Conflict outcome from Rebase(feat/a)"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib executor_propagates_outcomes` Expected: FAIL - closure
signature mismatch (task_fn doesn't accept outcomes)

- [ ] **Step 3: Add `branch_outcomes` to `DagState` and update executor**

In `DagState`, add the outcomes field:

```rust
struct DagState {
    ready: Vec<usize>,
    status: Vec<TaskStatus>,
    in_degree: Vec<usize>,
    active: usize,
    done: usize,
    total: usize,
    /// Per-branch outcome tags. Tasks read and write through this map.
    branch_outcomes: HashMap<String, HashSet<TaskOutcome>>,
}
```

Initialize in `DagExecutor::run`:

```rust
let state = Arc::new((
    Mutex::new(DagState {
        ready,
        status: vec![TaskStatus::Pending; n],
        in_degree,
        active: 0,
        done: 0,
        total: n,
        branch_outcomes: HashMap::new(),
    }),
    Condvar::new(),
));
```

Change the task function signature:

```rust
pub fn run<F>(self, task_fn: F)
where
    F: Fn(
            &SyncTask,
            &HashSet<TaskOutcome>,
        )
            -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>, Option<Box<WorktreeInfo>>)
        + Send
        + Sync,
```

Before calling `task_fn`, read the branch outcomes (outside the lock, snapshot
the set while holding the lock briefly):

```rust
// Read branch outcomes for this task (snapshot under lock)
let branch_outcomes = {
    let (lock, _) = &*state;
    let s = lock.lock().unwrap();
    s.branch_outcomes
        .get(&dag.tasks[task_idx].branch_name)
        .cloned()
        .unwrap_or_default()
};

// Execute the task outside the lock.
let task = &dag.tasks[task_idx];
let (result_status, message, returned_outcomes, updated_info) =
    task_fn(task, &branch_outcomes);
```

After calling `task_fn`, store the returned outcomes (inside the existing lock
acquisition):

```rust
{
    let (lock, cvar) = &*state;
    let mut s = lock.lock().unwrap();

    // Store returned outcomes for the branch.
    let branch = &dag.tasks[task_idx].branch_name;
    if !branch.is_empty() {
        s.branch_outcomes
            .insert(branch.clone(), returned_outcomes);
    }

    s.status[task_idx] = result_status;
    s.active -= 1;
    s.done += 1;

    if result_status == TaskStatus::Succeeded
        || result_status == TaskStatus::Skipped
        || result_status == TaskStatus::PreconditionFailed
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
        // Cascade DepFailed (existing logic, unchanged)
        // ...
    }
    // ... rest of event sending (unchanged)
}
```

- [ ] **Step 4: Fix all existing tests to use the new closure signature**

Every call to `executor.run(|task| ...)` in the test module must change to
`executor.run(|task, outcomes| ...)` and return a 4-tuple with outcomes passed
through. For example:

```rust
// Before:
executor.run(|_task| (TaskStatus::Succeeded, TaskMessage::Ok("ok".into()), None));

// After:
executor.run(|_task, outcomes| {
    (TaskStatus::Succeeded, TaskMessage::Ok("ok".into()), outcomes.clone(), None)
});
```

Update all existing executor tests:

- `executor_runs_all_tasks`
- `executor_respects_dependencies`
- `executor_cascades_failure`

- [ ] **Step 5: Run all sync_dag tests**

Run: `cargo test --lib sync_dag` Expected: all tests pass including the new
outcome propagation test

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/sync_dag.rs
git commit -m "feat(sync): add branch_outcomes to DagState and propagate through executor"
```

### Task 3: Write test for cross-branch outcome isolation

**Files:**

- Modify: `src/core/worktree/sync_dag.rs` (tests module)

- [ ] **Step 1: Write test verifying outcomes don't leak across branches**

```rust
#[test]
fn outcomes_do_not_leak_across_branches() {
    use std::collections::HashSet;
    let worktrees = vec![
        ("master".into(), PathBuf::from("/p/master")),
        ("feat/a".into(), PathBuf::from("/p/feat-a")),
        ("feat/b".into(), PathBuf::from("/p/feat-b")),
    ];
    let dag = SyncDag::build_sync(worktrees, vec![], Some("master".into()), true);
    let (tx, rx) = mpsc::channel();

    let received_outcomes: Arc<Mutex<Vec<(TaskId, HashSet<TaskOutcome>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let outcomes_clone = Arc::clone(&received_outcomes);

    let executor = DagExecutor::new(dag, tx);
    executor.run(move |task, outcomes| {
        outcomes_clone
            .lock()
            .unwrap()
            .push((task.id.clone(), outcomes.clone()));

        match &task.id {
            // feat/a conflicts
            TaskId::Rebase(name) if name == "feat/a" => {
                let mut out = outcomes.clone();
                out.insert(TaskOutcome::Conflict);
                (TaskStatus::Succeeded, TaskMessage::Conflict, out, None)
            }
            // feat/b succeeds
            TaskId::Rebase(name) if name == "feat/b" => (
                TaskStatus::Succeeded,
                TaskMessage::Ok("rebased".into()),
                outcomes.clone(),
                None,
            ),
            _ => (
                TaskStatus::Succeeded,
                TaskMessage::Ok("ok".into()),
                outcomes.clone(),
                None,
            ),
        }
    });

    let _events: Vec<DagEvent> = rx.iter().collect();
    let recorded = received_outcomes.lock().unwrap();

    // Push(feat/a) should see Conflict
    let push_a = recorded
        .iter()
        .find(|(id, _)| *id == TaskId::Push("feat/a".into()))
        .unwrap();
    assert!(push_a.1.contains(&TaskOutcome::Conflict));

    // Push(feat/b) should NOT see Conflict
    let push_b = recorded
        .iter()
        .find(|(id, _)| *id == TaskId::Push("feat/b".into()))
        .unwrap();
    assert!(
        !push_b.1.contains(&TaskOutcome::Conflict),
        "feat/b's push should not see feat/a's Conflict outcome"
    );
}
```

- [ ] **Step 2: Run test**

Run: `cargo test --lib outcomes_do_not_leak` Expected: PASS (this validates the
per-branch storage design)

- [ ] **Step 3: Commit**

```bash
git add src/core/worktree/sync_dag.rs
git commit -m "test(sync): verify outcome tags are isolated per branch"
```

## Chunk 2: Sync Command + TUI Changes

### Task 4: Update `execute_rebase_task` to return outcomes

**Files:**

- Modify: `src/commands/sync.rs:585-630` (execute_rebase_task)
- Modify: `src/commands/sync.rs:360-453` (task closure in run_tui)

- [ ] **Step 1: Update `execute_rebase_task` signature and return outcomes**

Change the function signature to accept and return outcomes:

```rust
fn execute_rebase_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    base_branch: &str,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force: bool,
    autostash: bool,
    branch_outcomes: &HashSet<TaskOutcome>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
```

In the conflict case, add `TaskOutcome::Conflict` to the returned set:

```rust
} else if result.conflict {
    let mut out = branch_outcomes.clone();
    out.insert(TaskOutcome::Conflict);
    (TaskStatus::Succeeded, TaskMessage::Conflict, out)
```

All other return paths pass through `branch_outcomes.clone()`.

- [ ] **Step 2: Update `execute_push_task` to check preconditions**

Change the signature:

```rust
fn execute_push_task(
    branch_name: &str,
    worktree_path: Option<&PathBuf>,
    project_root: &std::path::Path,
    settings: &DaftSettings,
    force_with_lease: bool,
    branch_outcomes: &HashSet<TaskOutcome>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>) {
```

Add precondition check at the top of the function body (after the `Some`
worktree_path check):

```rust
if branch_outcomes.contains(&TaskOutcome::Conflict) {
    return (
        TaskStatus::PreconditionFailed,
        TaskMessage::Failed("rebase conflict".into()),
        branch_outcomes.clone(),
    );
}
```

All other return paths pass through `branch_outcomes.clone()`.

- [ ] **Step 3: Update the task closure in `run_tui`**

The closure at line 360-453 must change to accept outcomes and return the
4-tuple. The `executor.run` call becomes:

```rust
executor.run(
    move |task: &SyncTask,
          outcomes: &HashSet<TaskOutcome>|
          -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>, Option<Box<list::WorktreeInfo>>) {
```

For each task type, thread outcomes through:

**Fetch and Prune:** return `outcomes.clone()` as-is (they don't interact with
outcomes).

**Update:** return `outcomes.clone()` as-is.

**Rebase:** call updated `execute_rebase_task` with outcomes, destructure the
3-tuple into `(status, message, new_outcomes)`, return with `updated_info`.

**Push:** call updated `execute_push_task` with outcomes, destructure similarly.

Add `use crate::core::worktree::sync_dag::TaskOutcome;` and
`use std::collections::HashSet;` to the imports at the top of `sync.rs`.

- [ ] **Step 4: Run clippy and fix any warnings**

Run: `mise run clippy` Expected: zero warnings

- [ ] **Step 5: Run unit tests**

Run: `mise run test:unit` Expected: all pass

- [ ] **Step 6: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): wire outcome tags through rebase/push task executors"
```

### Task 5: Update TUI state to handle `PreconditionFailed`

**Files:**

- Modify: `src/output/tui/state.rs:20-30` (WorktreeRow struct)
- Modify: `src/output/tui/state.rs:153-194` (apply_event)
- Modify: `src/output/tui/state.rs:383-418` (map_final_status)

**Key issue:** When `TaskStarted` fires for the push phase, it overwrites the
row from `Done(Conflict)` to `Active("pushing")`. Then when `TaskCompleted` with
`PreconditionFailed` arrives, if we just skip the row update, the row is stuck
on `Active("pushing")`. We solve this by saving the previous terminal status on
the row and restoring it on `PreconditionFailed`.

- [ ] **Step 1: Add `prev_terminal_status` to `WorktreeRow`**

Find the `WorktreeRow` struct and add the field:

```rust
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
```

Update all `WorktreeRow` construction sites to include
`prev_terminal_status: None` (in `TuiState::new` and any auto-creation in
`apply_event`).

- [ ] **Step 2: Save previous status in `TaskStarted` handler**

In the `TaskStarted` handler, before overwriting the status, save if it's Done:

```rust
if let Some(row) = self.find_row_mut(branch_name) {
    // Save terminal status so PreconditionFailed can restore it.
    if matches!(row.status, WorktreeStatus::Done(_)) {
        row.prev_terminal_status = Some(row.status.clone());
    }
    row.status = WorktreeStatus::Active(active_label.into());
}
```

- [ ] **Step 3: Write failing test**

Create a test helper that includes Rebase and Push phases:

```rust
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
        crate::core::worktree::list::Stat::None,
        0,
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
```

- [ ] **Step 4: Run test to verify it fails**

Run: `cargo test --lib precondition_failed_restores` Expected: FAIL

- [ ] **Step 5: Update `apply_event` for `PreconditionFailed`**

In `apply_event`, change the `TaskCompleted` handler:

```rust
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
            row.status = WorktreeStatus::Done(final_status);
            if let Some(new_info) = updated_info {
                row.info = *new_info.clone();
            }
        }
        self.check_phase_completion(phase);
    }
}
```

- [ ] **Step 6: Add `PreconditionFailed` to `map_final_status` catch-all**

Even though `map_final_status` is not called for `PreconditionFailed`, the Rust
compiler requires exhaustive matching. Add it to the catch-all arm:

```rust
TaskStatus::Pending | TaskStatus::Running | TaskStatus::PreconditionFailed => {
    FinalStatus::Failed
}
```

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test --lib precondition_failed_restores` Expected: PASS

- [ ] **Step 8: Run all TUI state tests**

Run: `cargo test --lib tui::state` Expected: all pass

- [ ] **Step 9: Commit**

```bash
git add src/output/tui/state.rs
git commit -m "feat(sync): PreconditionFailed restores previous TUI row status"
```

## Chunk 3: Sequential Path + Integration Tests

### Task 6: Update sequential path to skip push for conflicted branches

**Files:**

- Modify: `src/commands/sync.rs:140-167` (sequential orchestration)
- Modify: `src/commands/sync.rs:892-931` (run_rebase_phase)
- Modify: `src/commands/sync.rs:1033-1068` (run_push_phase)
- Modify: `src/core/worktree/push.rs:64-96` (execute)

- [ ] **Step 1: Change `run_rebase_phase` to return `RebaseResult`**

Change signature from `-> Result<()>` to `-> Result<RebaseResult>` and return
the result:

```rust
fn run_rebase_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    base_branch: &str,
    force: bool,
    autostash: bool,
) -> Result<RebaseResult> {
    // ... existing code ...
    let result = exec_result?;
    render_rebase_result(&result, output);

    if result.conflict_count() > 0 {
        output.warning(&format!(
            "{} worktree(s) had conflicts and were aborted",
            result.conflict_count()
        ));
    }

    Ok(result)
}
```

- [ ] **Step 2: Add exclusion set parameter to `push::execute`**

In `src/core/worktree/push.rs`, add an `exclude_branches` parameter:

```rust
pub fn execute(
    params: &PushParams,
    git: &GitCommand,
    project_root: &Path,
    progress: &mut dyn ProgressSink,
    exclude_branches: &HashSet<String>,
) -> Result<PushResult> {
```

Add `use std::collections::HashSet;` to the push module imports.

In the loop, skip excluded branches:

```rust
for (path, branch) in &worktrees {
    if exclude_branches.contains(branch) {
        continue;
    }
    // ... existing push logic ...
}
```

- [ ] **Step 3: Update `run_push_phase` to accept and pass exclusion set**

```rust
fn run_push_phase(
    output: &mut dyn Output,
    settings: &DaftSettings,
    force_with_lease: bool,
    skip_branches: &HashSet<String>,
) -> Result<()> {
    // ... existing setup ...
    let mut sink = OutputSink(output);
    push::execute(&params, &git, &project_root, &mut sink, skip_branches)
    // ... rest unchanged ...
}
```

- [ ] **Step 4: Add `branch_name` to `WorktreeRebaseResult` and
      `rebase_single_worktree`**

`WorktreeRebaseResult` stores `worktree_name` (relative path like
`../feat/branch`), but `push::execute` filters by branch name. Add `branch_name`
to the result struct and thread it through.

In `src/core/worktree/rebase.rs`, update the struct:

```rust
pub struct WorktreeRebaseResult {
    pub worktree_name: String,
    pub branch_name: String,
    pub success: bool,
    pub skipped: bool,
    pub conflict: bool,
    pub already_rebased: bool,
    pub message: String,
}
```

Update `rebase_single_worktree` signature to accept `branch_name`:

```rust
pub fn rebase_single_worktree(
    git: &GitCommand,
    worktree_path: &Path,
    worktree_name: &str,
    branch_name: &str,
    base_branch: &str,
    force: bool,
    autostash: bool,
    progress: &mut dyn ProgressSink,
) -> WorktreeRebaseResult {
```

In every `WorktreeRebaseResult` construction inside `rebase_single_worktree`,
add `branch_name: branch_name.to_string()`. There are 5 return sites: directory
not found, skipped (uncommitted changes), status check error, success, and
conflict.

Update the call site in `execute()` (line 93-101 in rebase.rs) to pass the
branch name:

```rust
let result = rebase_single_worktree(
    git,
    path,
    &worktree_name,
    branch,           // <-- NEW: pass branch name
    &params.base_branch,
    params.force,
    params.autostash,
    progress,
);
```

Update the call site in `sync.rs:execute_rebase_task` (line 611) to pass
`branch_name`:

```rust
let result = rebase::rebase_single_worktree(
    &git,
    target_path,
    &worktree_name,
    branch_name,      // <-- NEW: pass branch name
    base_branch,
    force,
    autostash,
    &mut sink,
);
```

- [ ] **Step 5: Update the sequential orchestration to thread conflict info**

In the sequential path (around lines 153-167 in `sync.rs`):

```rust
// Phase 3: Rebase
let conflicted_branches: HashSet<String> = if let Some(ref base_branch) = args.rebase {
    let result =
        run_rebase_phase(&mut output, &settings, base_branch, force, args.autostash)?;
    result
        .results
        .iter()
        .filter(|r| r.conflict)
        .map(|r| r.branch_name.clone())
        .collect()
} else {
    HashSet::new()
};

// Phase 4: Push
if args.push {
    run_push_phase(
        &mut output,
        &settings,
        args.force_with_lease,
        &conflicted_branches,
    )?;
}
```

Add `use std::collections::HashSet;` to the top of `sync.rs` if not already
present.

- [ ] **Step 5: Run clippy**

Run: `mise run clippy` Expected: zero warnings

- [ ] **Step 6: Run unit tests**

Run: `mise run test:unit` Expected: all pass

- [ ] **Step 7: Commit**

```bash
git add src/commands/sync.rs src/core/worktree/push.rs src/core/worktree/rebase.rs
git commit -m "feat(sync): skip push for conflicted branches in sequential path"
```

### Task 7: Add integration test for rebase conflict stopping push

**Files:**

- Modify: `tests/integration/test_sync.sh`

- [ ] **Step 1: Add integration test**

Add before `run_sync_tests()`:

```bash
# Test sync --rebase --push skips push when rebase conflicts
test_sync_rebase_conflict_skips_push() {
    local remote_repo=$(create_test_remote "test-repo-sync-conflict-push" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-sync-conflict-push"

    # Create a feature worktree
    git-worktree-checkout develop || return 1

    # Make a local commit on develop that will conflict with main
    (
        cd develop
        echo "develop conflicting content" > README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Conflicting change on develop" >/dev/null 2>&1
    ) >/dev/null 2>&1

    # Push a conflicting change to main via remote
    local temp_clone="$TEMP_BASE_DIR/temp_sync_conflict_push_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        echo "main conflicting content" > README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Conflicting change on main" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Record develop's commit before sync
    local develop_commit_before=$(cd develop && git rev-parse HEAD)

    # Record remote develop ref before sync
    local remote_develop_before=$(git ls-remote "$remote_repo" develop 2>/dev/null | awk '{print $1}')

    # Run sync with --rebase --push (use -vv for sequential mode)
    # This should NOT fail -- conflicts are warnings, not errors
    git-sync --rebase main --push --force-with-lease --verbose --verbose 2>&1 || true

    # Verify develop branch was NOT changed (rebase aborted)
    local develop_commit_after=$(cd develop && git rev-parse HEAD)
    if [[ "$develop_commit_after" != "$develop_commit_before" ]]; then
        log_error "Develop branch should not have changed after aborted rebase"
        return 1
    fi

    # Verify push was NOT attempted (remote develop should be unchanged)
    local remote_develop_after=$(git ls-remote "$remote_repo" develop 2>/dev/null | awk '{print $1}')
    if [[ "$remote_develop_after" != "$remote_develop_before" ]]; then
        log_error "Push should have been skipped for branch with rebase conflict"
        return 1
    fi

    return 0
}
```

Add to `run_sync_tests()`:

```bash
run_test "sync_rebase_conflict_skips_push" "test_sync_rebase_conflict_skips_push"
```

- [ ] **Step 2: Run the integration test**

Run: `mise run test:integration` or directly:
`bash tests/integration/test_sync.sh` Expected: PASS for the new test (and all
existing tests still pass)

- [ ] **Step 3: Commit**

```bash
git add tests/integration/test_sync.sh
git commit -m "test(sync): integration test for rebase conflict skipping push"
```

### Task 8: Final verification

- [ ] **Step 1: Run full CI check**

Run: `mise run ci` Expected: all checks pass (fmt, clippy, unit tests,
integration tests)

- [ ] **Step 2: Run fmt**

Run: `mise run fmt` Expected: no changes (code already formatted)

- [ ] **Step 3: Verify man pages are up to date**

Run: `mise run man:verify` Expected: pass (no command help text changed)
