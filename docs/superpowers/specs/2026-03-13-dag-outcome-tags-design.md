# DAG Outcome Tags: Precondition-Based Task Gating

**Date:** 2026-03-13 **Branch:** fix/rebase-failure-should-stop-push **Status:**
Draft

## Problem

When `daft sync --rebase <branch> --push` encounters a rebase conflict, the
rebase is correctly aborted and the branch is rolled back to its pre-rebase
state. However, two things go wrong:

1. **Push still runs.** The rebase conflict returns `TaskStatus::Succeeded` with
   `TaskMessage::Conflict`, so the DAG treats it as a success and proceeds to
   push. The push finds nothing to do and returns "up to date."

2. **The final status is misleading.** Each phase's `TaskCompleted` event
   overwrites the worktree row's status in the TUI. The push's "up to date"
   overwrites the rebase's "conflict", so the user sees a green checkmark for a
   branch that failed to rebase.

The root cause: the DAG has no mechanism for a task to communicate semantic
state ("this branch is in conflict") that downstream tasks can use as a
precondition for whether to run.

## Design

### Outcome Tags

A new concept: **outcome tags** are semantic labels that tasks produce to
describe the state of the entity (branch/worktree) they operated on. They are
not success/failure signals -- they are state. A conflict is a state, not a
failure; the rebase resolved cleanly by aborting.

```rust
/// Semantic outcomes that a task can produce. Downstream tasks may
/// inspect these as preconditions for whether to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskOutcome {
    /// Rebase had conflicts and was aborted.
    Conflict,
}
```

New variants can be added as needed without changing the mechanism.

### Per-Branch Outcome Storage

Outcomes are stored **per branch** in the `DagState`, not on DAG edges:

```rust
struct DagState {
    ready: Vec<usize>,
    status: Vec<TaskStatus>,
    in_degree: Vec<usize>,
    active: usize,
    done: usize,
    total: usize,
    // NEW: per-branch accumulated outcome tags
    branch_outcomes: HashMap<String, HashSet<TaskOutcome>>,
}
```

Why per-branch and not per-edge:

- **Cross-branch dependencies don't leak.** Rebase(feat) depends on
  Update(master), but master's outcomes should not contaminate feat's state.
- **Diamond convergence is coherent.** When multiple parents converge on a task,
  edge-based unions can produce contradictory state. Per-branch storage has one
  authoritative set per entity.
- **Remediation is straightforward.** A remediation task removes specific tags
  from the branch's set; later tasks see the clean state.

Tasks for the same branch are sequential in the current DAG (Update -> Rebase ->
Push), so there are no concurrent writes to the same branch's outcome set. The
Fetch task has an empty `branch_name` -- its outcomes are stored under `""` but
never read by any downstream task (Fetch has no branch-specific dependents that
share its key).

### Task Function Signature

The task function receives the current outcome set for its branch and returns an
updated set:

```rust
F: Fn(&SyncTask, &HashSet<TaskOutcome>)
    -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>, Option<Box<WorktreeInfo>>)
```

- **Rebase task (conflict):** receives `{}`, returns `{Conflict}`
- **Rebase task (success):** receives `{}`, returns `{}`
- **Push task:** receives outcomes, checks preconditions, either proceeds or
  returns `PreconditionFailed`
- **Remediation task (future):** receives `{Conflict}`, succeeds, removes
  `Conflict`, returns remaining tags. On failure, returns input unchanged.

Every task function **must** faithfully pass through all outcome tags it does
not explicitly add or remove. This is a contract: a task that receives
`{Conflict, Dirty}` and only cares about `Conflict` must still return `Dirty` in
its output set. Dropping tags silently breaks downstream preconditions.

### Precondition Checking

Before executing, a task can inspect the branch's outcome set and decide not to
run. This is expressed via the return value, not a separate mechanism -- the
task function is called, receives the outcomes, and can immediately return
`TaskStatus::PreconditionFailed` without doing any work.

For the current use case, `execute_push_task` checks:

```rust
fn execute_push_task(
    // ... existing params ...
    branch_outcomes: &HashSet<TaskOutcome>,
) -> (TaskStatus, TaskMessage, HashSet<TaskOutcome>, Option<Box<WorktreeInfo>>) {
    if branch_outcomes.contains(&TaskOutcome::Conflict) {
        return (
            TaskStatus::PreconditionFailed,
            // Message is not displayed (TUI skips the row update for
            // PreconditionFailed), but carried for logging/debugging.
            TaskMessage::Failed("rebase conflict".into()),
            branch_outcomes.clone(),
            None,
        );
    }
    // ... existing push logic ...
}
```

### New TaskStatus Variant

```rust
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    DepFailed,
    /// Task checked preconditions and chose not to run.
    PreconditionFailed,
}
```

`PreconditionFailed` is terminal (`is_terminal()` must return `true`). In the
executor:

- It decrements in-degrees of dependents (like `Succeeded`) so the pipeline
  continues for this branch. Downstream tasks receive the unchanged outcomes and
  can make their own precondition decisions.
- It stores the branch outcomes unchanged (the task didn't modify state).

### Executor Flow

When a worker picks up a task:

1. Send `TaskStarted` event. (The task function may return `PreconditionFailed`
   near-instantly, causing a brief "pushing" flicker in the TUI. This is
   acceptable -- the precondition check is near-instantaneous and avoids
   complicating the executor with a two-phase call.)
2. Look up `branch_outcomes[task.branch_name]` (default: empty set).
3. Call `task_fn(task, &outcomes)`.
4. Store the returned outcomes under `branch_outcomes[task.branch_name]`.
5. If status is `PreconditionFailed`:
   - Decrement in-degrees of dependents (pipeline continues).
   - Emit `TaskCompleted` with `PreconditionFailed` status.
6. If status is `Succeeded` or `Skipped`:
   - Decrement in-degrees of dependents (existing behavior).
   - Emit `TaskCompleted`.
7. If status is `Failed`:
   - Cascade `DepFailed` to transitive dependents (existing behavior).
   - Emit `TaskCompleted`.

### TUI Handling

The TUI's `apply_event` handler for `TaskCompleted`:

- **`PreconditionFailed`:** Process the event for phase progress tracking (phase
  header shows completion) but **do not update** the worktree row's status. The
  row retains whatever the last actually-executed task set (e.g., "conflict"
  from the rebase phase).
- All other statuses: existing behavior (overwrite the row status).

This preserves "latest wins" semantics. The display always shows the outcome of
the last task that actually ran. A precondition-skipped task didn't run, so it
doesn't contribute to the display.

### Sequential Path

The sequential (non-TUI) path (`run_rebase_phase` / `run_push_phase`) does not
use the DAG. For consistency:

1. Change `run_rebase_phase` to return `Result<RebaseResult>` instead of
   `Result<()>`. Currently the `RebaseResult` is consumed internally for
   rendering and then discarded.
2. At the call site (lines 160-166 in `sync.rs`), collect the set of branch
   names that had conflicts from the returned `RebaseResult`.
3. Change `run_push_phase` to accept an `Option<HashSet<String>>` of branch
   names to skip.
4. Thread this exclusion set into `push::execute` (which currently discovers
   worktrees internally), either by adding a filter parameter or by filtering
   the worktree list before passing it in.

## Affected Files

| File                            | Change                                                                                                                                                                                                                                           |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/core/worktree/sync_dag.rs` | Add `TaskOutcome` enum, `PreconditionFailed` variant, `branch_outcomes` to `DagState`, update executor flow                                                                                                                                      |
| `src/commands/sync.rs`          | Update `execute_rebase_task` to return outcomes, update `execute_push_task` to check preconditions, update `task_fn` closure signature, pass rebase result to sequential push phase                                                              |
| `src/output/tui/state.rs`       | Handle `PreconditionFailed` in `apply_event` (skip row update), update `map_final_status`                                                                                                                                                        |
| `src/output/tui/columns.rs`     | No change needed (no new `FinalStatus` variant)                                                                                                                                                                                                  |
| `src/output/tui/render.rs`      | No change needed                                                                                                                                                                                                                                 |
| `src/commands/sync_shared.rs`   | No change needed. `check_tui_failures` counts `FinalStatus::Failed` rows. `PreconditionFailed` does not produce a `FinalStatus::Failed` row (the TUI skips the row update), and `FinalStatus::Conflict` is not a failure for exit-code purposes. |

## Testing

- **Unit test (sync_dag):** Rebase task returns `{Conflict}` -> push task
  receives `{Conflict}`, returns `PreconditionFailed` -> dependents of push
  still proceed -> verify outcomes stored correctly per branch.
- **Unit test (sync_dag):** Multi-branch: one branch conflicts, another succeeds
  -> push runs for the successful branch, skipped for the conflicting one.
- **Unit test (state):** `PreconditionFailed` event does not overwrite worktree
  row status but phase progress still advances.
- **Unit test (state):** After rebase conflict + precondition-failed push, row
  shows "conflict" not "up to date".
- **Integration test:** `daft sync --rebase master --push` with a worktree that
  will conflict -> verify push is not attempted, final output shows conflict
  status.
- **Integration test (sequential):** Same scenario without TUI -> verify push is
  skipped for conflicting branch.

## Future Extensions

- **New outcome tags:** `Diverged`, `Dirty`, etc. -- any semantic state that
  downstream tasks might gate on.
- **Remediation tasks:** A task that depends on a conflicting rebase, attempts
  automatic resolution, and removes the `Conflict` tag on success. Later tasks
  (push) see the clean state and proceed.
- **User-visible outcome reporting:** Surface the outcome tags in verbose output
  so users can see why a task was skipped.
