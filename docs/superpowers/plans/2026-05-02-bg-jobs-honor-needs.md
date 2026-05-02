# Background Jobs Honor `needs:` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the background coordinator honor `needs:` between background
jobs, so that bg→bg dependencies are scheduled in topological order with
parallel waves, matching the contract foreground jobs already enjoy.

**Architecture:** Replace the coordinator's unconditional fan-out loop in
`src/coordinator/process.rs::run_all_with_cancel` with a
`DagGraph::run_parallel` call (the same scheduler primitive
`runner.rs::run_dag_execution` uses). The existing per-job logic
(`run_single_background_job`) becomes the closure body; its only required change
is to return `NodeStatus` so the DAG can cascade failures and cancellations to
dependents.

**Tech Stack:** Rust, existing `DagGraph` primitive (`src/executor/dag.rs`),
existing coordinator infrastructure.

**Reference issue:** [daft#454](https://github.com/avihut/daft/issues/454).

---

## File Structure

### Modified files

| File                                                               | Change                                                                                             |
| ------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------- |
| `src/coordinator/process.rs`                                       | Replace fan-out loop with `DagGraph::run_parallel`; change `run_single_background_job` return type |
| `src/coordinator/mod.rs`                                           | (No change expected — verify exports if tests need new visibility)                                 |
| `tests/manual/scenarios/hooks/bg-needs-ordering.yml`               | New YAML scenario asserting bg→bg ordering via timestamp comparison                                |
| `docs/superpowers/specs/2026-03-27-background-hook-jobs-design.md` | Already updated in the spec section that ships with this plan                                      |

### Files NOT changed

- `src/executor/dag.rs` — used as-is.
- `src/executor/runner.rs` — foreground path unchanged.
- `src/hooks/yaml_executor/partition.rs` — partition logic correct, no changes
  needed.
- `docs/guide/hooks.md` — user-facing behavior already documented; the bug was
  purely an implementation drift.
- `SKILL.md` — same reason.

---

## Task 1: Update spec doc to nail down the runtime contract

**Files:**

- Verify already-applied:
  `docs/superpowers/specs/2026-03-27-background-hook-jobs-design.md`

This update was applied alongside this plan. It adds a "Runtime contract for
`needs:` (binding)" subsection under "DAG Integration" that:

1. States bg jobs MUST wait for `needs:` deps to terminate.
2. Pins down satisfied = `JobStatus::Completed`.
3. Pins down `Failed`/`Cancelled`/`Skipped` deps cascade to `DepFailed`
   dependents.
4. Mandates cycle/missing-dep detection.
5. Describes the closure status-mapping table (`Completed → Succeeded`; anything
   else → `Failed`).

- [ ] **Step 1: Confirm the spec section exists**

```bash
rg -n "Runtime contract for" docs/superpowers/specs/2026-03-27-background-hook-jobs-design.md
```

Expected: matches the heading line. If missing, re-apply the edit from the plan
PR description.

- [ ] **Step 2: Commit (if uncommitted)**

```bash
git add docs/superpowers/specs/2026-03-27-background-hook-jobs-design.md
git commit -m "docs(spec): pin down bg-coordinator runtime contract for needs:"
```

---

## Task 2: Write a failing ordering regression test

**Files:**

- Modify: `src/coordinator/process.rs` (inside `mod tests`)

This test reproduces the issue ticket directly: B `needs: [A]` where A sleeps
200ms; assert `B.started_at >= A.finished_at` from the on-disk `meta.json`.

- [ ] **Step 1: Write the failing test**

Add at the bottom of `mod tests` in `src/coordinator/process.rs`:

```rust
#[test]
fn bg_dependent_waits_for_dep_to_finish() {
    // Regression test for daft#454: B `needs: [A]` must not start until A
    // has terminated.
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) =
        make_test_state();

    let mut state = CoordinatorState::new("test-repo", "inv-needs-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feat/x");

    state.add_job(JobSpec {
        name: "dep-a".to_string(),
        command: "sleep 0.2 && echo done".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        ..Default::default()
    });
    state.add_job(JobSpec {
        name: "dep-b".to_string(),
        command: "echo b".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        needs: vec!["dep-a".to_string()],
        ..Default::default()
    });

    state
        .run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs)
        .unwrap();

    let dir_a = store.base_dir.join("inv-needs-1").join("dep-a");
    let dir_b = store.base_dir.join("inv-needs-1").join("dep-b");
    let meta_a = store.read_meta(&dir_a).expect("meta-a");
    let meta_b = store.read_meta(&dir_b).expect("meta-b");

    let a_finished = meta_a.finished_at.expect("a finished_at");
    let b_started = meta_b.started_at;

    assert!(
        b_started >= a_finished,
        "dep-b started ({b_started}) before dep-a finished ({a_finished})"
    );
    assert!(matches!(meta_a.status, JobStatus::Completed));
    assert!(matches!(meta_b.status, JobStatus::Completed));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --lib -p daft coordinator::process::tests::bg_dependent_waits_for_dep_to_finish
```

Expected: FAIL — `dep-b started ... before dep-a finished ...` (the bug).

- [ ] **Step 3: Commit the failing test**

```bash
git add src/coordinator/process.rs
git commit -m "test(coordinator): add failing regression test for bg needs: ordering"
```

---

## Task 3: Make `run_single_background_job` return `NodeStatus`

**Files:**

- Modify: `src/coordinator/process.rs` (the `run_single_background_job` fn,
  lines 152-363)

This change is a precondition for plugging the function into
`DagGraph::run_parallel`. The DAG closure must return `NodeStatus`. Today the
function returns `()` and pushes the result to a shared vec; we keep the push
side-effect (used by tests and the `JobResult` collector) and add the return
value on top.

- [ ] **Step 1: Change the function signature and add the return statement**

In `src/coordinator/process.rs`, edit the function declaration on line 152:

```rust
fn run_single_background_job(
    job: &JobSpec,
    ctx: &JobInvocationContext<'_>,
    store: &LogStore,
    results: &Arc<Mutex<Vec<JobResult>>>,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
) -> NodeStatus {
```

In the early-return cancel_all check (around line 169), change `return;` to
`return NodeStatus::Failed;`:

```rust
if cancel_all.load(Ordering::Relaxed) {
    results.lock().unwrap().push(JobResult {
        name: job.name.clone(),
        status: NodeStatus::Skipped,
        duration: start.elapsed(),
        exit_code: None,
        stdout: String::new(),
        stderr: "Cancelled before start".to_string(),
    });
    return NodeStatus::Failed;
}
```

In the create-job-dir error path (around line 184), change `return;` to
`return NodeStatus::Failed;`:

```rust
let job_dir = match store.create_job_dir(ctx.invocation_id, &job.name) {
    Ok(dir) => dir,
    Err(e) => {
        eprintln!("daft: failed to create log dir for '{}': {e}", job.name);
        results.lock().unwrap().push(JobResult {
            name: job.name.clone(),
            status: NodeStatus::Failed,
            duration: start.elapsed(),
            exit_code: None,
            stdout: String::new(),
            stderr: format!("Failed to create log dir: {e}"),
        });
        return NodeStatus::Failed;
    }
};
```

At the bottom of the function (after the existing
`results.lock().unwrap().push(JobResult { ... })` call near the end of the fn),
add the final return:

```rust
    results.lock().unwrap().push(JobResult {
        name: job.name.clone(),
        status: node_status,
        duration,
        exit_code,
        stdout,
        stderr,
    });

    // Map outcome to a DAG-cascade-friendly status. Cancelled and Skipped
    // collapse to Failed because the dep did not produce its work product,
    // so dependents must DepFailed via cascade. JobMeta on disk preserves
    // the Completed/Failed/Cancelled distinction for `daft hooks jobs`.
    if matches!(node_status, NodeStatus::Succeeded) {
        NodeStatus::Succeeded
    } else {
        NodeStatus::Failed
    }
}
```

- [ ] **Step 2: Update existing test call sites**

The standalone tests `run_single_background_job_registers_and_deregisters_pid`
(line 1062), `per_job_cancel_marks_status_cancelled_not_failed` (line 1101),
`silent_bg_output_deletes_log_on_success` (line 1148),
`silent_bg_output_keeps_log_on_failure` (line 1181),
`non_silent_bg_output_always_writes_log` (line 1216), and
`silent_bg_output_keeps_log_when_status_is_cancelled` (line 1245) call
`run_single_background_job` and discard the return value. Update each call to
discard explicitly with `let _ =`:

```rust
let _ = run_single_background_job(
    &job,
    &ctx,
    &store,
    &results,
    &child_pids,
    &cancel_all,
    &cancelled_jobs,
);
```

This is needed because Rust will warn about unused `NodeStatus` Result-like
values if not handled (and because clippy enforces `#![warn(unused_must_use)]`
in the project).

- [ ] **Step 3: Build to confirm the signature change compiles**

```bash
cargo build -p daft
```

Expected: clean build. If errors complain about callers, those are caught
already in step 2 — the only callers are the tests.

- [ ] **Step 4: Run all coordinator tests**

```bash
cargo test --lib -p daft coordinator::process::tests
```

Expected: all existing tests still pass (PID registration, cancel, silent
output, etc.). The new ordering test (`bg_dependent_waits_for_dep_to_finish`)
still fails — it depends on Task 4.

- [ ] **Step 5: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "refactor(coordinator): return NodeStatus from run_single_background_job"
```

---

## Task 4: Replace fan-out loop with `DagGraph::run_parallel`

**Files:**

- Modify: `src/coordinator/process.rs`, the body of `run_all_with_cancel` (lines
  88-139)

This is the core fix.

- [ ] **Step 1: Add the new imports**

At the top of `src/coordinator/process.rs`, alongside the existing executor
imports (currently `use crate::executor::command::run_command;` and
`use crate::executor::{JobResult, JobSpec, NodeStatus};`), extend the executor
import to bring `DagGraph`:

```rust
use crate::executor::command::run_command;
use crate::executor::dag::DagGraph;
use crate::executor::{JobResult, JobSpec, NodeStatus};
```

- [ ] **Step 2: Replace the fan-out loop body**

Replace the body of `run_all_with_cancel` (lines 88-139). The new body:

```rust
fn run_all_with_cancel(
    &self,
    store: &LogStore,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
) -> Result<Vec<JobResult>> {
    if self.jobs.is_empty() {
        return Ok(Vec::new());
    }

    // Build the DAG from `needs:`. Reusing the same scheduler the foreground
    // runner uses so foreground and background share one ordering contract.
    let nodes: Vec<(String, Vec<String>)> = self
        .jobs
        .iter()
        .map(|j| (j.name.clone(), j.needs.clone()))
        .collect();
    let graph = DagGraph::new(nodes)
        .map_err(|e| anyhow::anyhow!("invalid background job DAG: {e}"))?;

    let job_map: HashMap<&str, &JobSpec> = self
        .jobs
        .iter()
        .map(|j| (j.name.as_str(), j))
        .collect();

    let results = Arc::new(Mutex::new(Vec::<JobResult>::new()));
    let max_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    graph.run_parallel(
        |_idx, name| {
            let Some(job) = job_map.get(name).copied() else {
                return NodeStatus::Failed;
            };
            let ctx = JobInvocationContext {
                invocation_id: &self.invocation_id,
                hook_type: &self.hook_type,
                worktree: &self.worktree,
            };
            run_single_background_job(
                job,
                &ctx,
                store,
                &results,
                child_pids,
                cancel_all,
                cancelled_jobs,
            )
        },
        max_workers,
    );

    let results = match Arc::try_unwrap(results) {
        Ok(mutex) => mutex.into_inner().unwrap_or_default(),
        Err(arc) => arc.lock().unwrap().clone(),
    };

    Ok(results)
}
```

Notes for the implementer:

- The closure captures `self`, `store`, `child_pids`, `cancel_all`,
  `cancelled_jobs`, `results`, and `job_map` by reference. Each is `Sync`
  (Arc/Mutex/AtomicBool/&LogStore/HashMap-of-refs), so the resulting closure is
  `Fn + Send + Sync` as required by `DagGraph::run_parallel`.
- The previous code constructed a fresh `LogStore::new(store_base)` per thread;
  this is unnecessary because `LogStore` is `{ base_dir: PathBuf }` and
  trivially `Sync`. Pass `store` directly.
- `Arc::try_unwrap` succeeds because `run_parallel` joins all workers before
  returning, so no Arc clones survive.
- `DagGraph::new` validates the graph: cycles and missing dependencies are
  reported via `DagError`, which we wrap in `anyhow::Error` for the
  `Result<Vec<JobResult>>` return.

- [ ] **Step 3: Build**

```bash
cargo build -p daft
```

Expected: clean build.

- [ ] **Step 4: Run the regression test**

```bash
cargo test --lib -p daft coordinator::process::tests::bg_dependent_waits_for_dep_to_finish
```

Expected: PASS — `dep-b` now waits for `dep-a` to finish.

- [ ] **Step 5: Run all coordinator tests**

```bash
cargo test --lib -p daft coordinator
```

Expected: all pass, including the existing
`test_run_all_with_cancel_skips_when_cancelled` (which has one job and no deps —
closure runs once, sees cancel_all, pushes `Skipped` JobResult, returns Failed;
results vec has one Skipped entry — same as before).

- [ ] **Step 6: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "fix(coordinator): honor needs: between background jobs

Closes #454.

The coordinator's run_all_with_cancel previously spawned every background
job as a thread immediately and joined them at the end, ignoring needs:.
Reuses DagGraph from the foreground runner so foreground and background
share the same scheduler primitive."
```

---

## Task 5: Add a failure-cascade unit test

**Files:**

- Modify: `src/coordinator/process.rs` (inside `mod tests`)

Verifies that when A fails, B `needs: [A]` is not spawned.

- [ ] **Step 1: Write the test**

Append to `mod tests`:

```rust
#[test]
fn bg_dependent_skipped_when_dep_fails() {
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) =
        make_test_state();

    let mut state = CoordinatorState::new("test-repo", "inv-needs-fail-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feat/x");

    state.add_job(JobSpec {
        name: "fails".to_string(),
        command: "exit 7".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        ..Default::default()
    });
    state.add_job(JobSpec {
        name: "dependent".to_string(),
        command: "touch /tmp/should-never-exist-bg-needs-fail".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        needs: vec!["fails".to_string()],
        ..Default::default()
    });

    // Make sure the marker file isn't lying around from a prior run.
    let _ = std::fs::remove_file("/tmp/should-never-exist-bg-needs-fail");

    state
        .run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs)
        .unwrap();

    let meta_fails = store
        .read_meta(&store.base_dir.join("inv-needs-fail-1").join("fails"))
        .expect("meta fails");
    assert!(matches!(meta_fails.status, JobStatus::Failed));

    // The dependent's closure was never invoked, so no meta exists.
    let dep_dir = store.base_dir.join("inv-needs-fail-1").join("dependent");
    assert!(
        !dep_dir.exists(),
        "dependent should not have a job dir — its closure must not have run"
    );

    // Cleanup safety check.
    assert!(
        !std::path::Path::new("/tmp/should-never-exist-bg-needs-fail").exists(),
        "dependent ran its command despite dep failing"
    );
}
```

- [ ] **Step 2: Run it**

```bash
cargo test --lib -p daft coordinator::process::tests::bg_dependent_skipped_when_dep_fails
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "test(coordinator): assert failed bg dep cascades skip to dependents"
```

---

## Task 6: Add a cancellation-cascade unit test

**Files:**

- Modify: `src/coordinator/process.rs` (inside `mod tests`)

Verifies that when A is per-job-cancelled mid-run, B `needs: [A]` is not
spawned.

- [ ] **Step 1: Write the test**

Append to `mod tests`:

```rust
#[test]
fn bg_dependent_skipped_when_dep_cancelled() {
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) =
        make_test_state();

    let mut state = CoordinatorState::new("test-repo", "inv-needs-cancel-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feat/x");

    state.add_job(JobSpec {
        name: "long".to_string(),
        command: "sleep 5".to_string(),
        working_dir: std::env::temp_dir(),
        timeout: std::time::Duration::from_secs(30),
        background: true,
        ..Default::default()
    });
    state.add_job(JobSpec {
        name: "after".to_string(),
        command: "touch /tmp/should-never-exist-bg-needs-cancel".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        needs: vec!["long".to_string()],
        ..Default::default()
    });

    let _ = std::fs::remove_file("/tmp/should-never-exist-bg-needs-cancel");

    let pids = Arc::clone(&child_pids);
    let cancelled = Arc::clone(&cancelled_jobs);
    let store_for_killer = store.clone();
    let killer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = cancel_single_job("long", &pids, &cancelled, &store_for_killer);
    });

    state
        .run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs)
        .unwrap();
    killer.join().unwrap();

    let meta_long = store
        .read_meta(&store.base_dir.join("inv-needs-cancel-1").join("long"))
        .expect("meta long");
    assert!(
        matches!(meta_long.status, JobStatus::Cancelled),
        "long should be Cancelled, got {:?}",
        meta_long.status
    );

    let after_dir = store.base_dir.join("inv-needs-cancel-1").join("after");
    assert!(
        !after_dir.exists(),
        "after's closure ran even though its dep was cancelled"
    );
    assert!(
        !std::path::Path::new("/tmp/should-never-exist-bg-needs-cancel").exists(),
        "after ran its command despite dep being cancelled"
    );
}
```

- [ ] **Step 2: Run it**

```bash
cargo test --lib -p daft coordinator::process::tests::bg_dependent_skipped_when_dep_cancelled
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "test(coordinator): assert cancelled bg dep cascades skip to dependents"
```

---

## Task 7: Add cycle / missing-dep error tests

**Files:**

- Modify: `src/coordinator/process.rs` (inside `mod tests`)

Verifies that cycles and missing-dep references in the bg bucket are surfaced as
errors rather than silently fanning out (per the Runtime Contract, clause 4).

- [ ] **Step 1: Write the tests**

Append to `mod tests`:

```rust
#[test]
fn bg_cycle_in_needs_returns_error() {
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) =
        make_test_state();

    let mut state = CoordinatorState::new("test-repo", "inv-cycle-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feat/x");

    state.add_job(JobSpec {
        name: "a".to_string(),
        command: "echo a".to_string(),
        background: true,
        needs: vec!["b".to_string()],
        ..Default::default()
    });
    state.add_job(JobSpec {
        name: "b".to_string(),
        command: "echo b".to_string(),
        background: true,
        needs: vec!["a".to_string()],
        ..Default::default()
    });

    let result =
        state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs);
    assert!(result.is_err(), "cycle should be reported as an error");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("invalid background job DAG"),
        "error should mention invalid DAG, got: {msg}"
    );
}

#[test]
fn bg_missing_dep_in_needs_returns_error() {
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) =
        make_test_state();

    let mut state = CoordinatorState::new("test-repo", "inv-missing-1")
        .with_metadata("worktree-post-create", "worktree-post-create", "feat/x");

    state.add_job(JobSpec {
        name: "only".to_string(),
        command: "echo only".to_string(),
        background: true,
        needs: vec!["does-not-exist".to_string()],
        ..Default::default()
    });

    let result =
        state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs);
    assert!(
        result.is_err(),
        "missing dep should be reported as an error"
    );
}
```

- [ ] **Step 2: Run them**

```bash
cargo test --lib -p daft coordinator::process::tests::bg_cycle_in_needs_returns_error coordinator::process::tests::bg_missing_dep_in_needs_returns_error
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/coordinator/process.rs
git commit -m "test(coordinator): bg DAG errors on cycles and missing deps"
```

---

## Task 8: Add a YAML scenario for end-to-end ordering

**Files:**

- Create: `tests/manual/scenarios/hooks/bg-needs-ordering.yml`

End-to-end test: two background jobs, second `needs:` first; assert ordering via
timestamps in the recorded `meta.json`.

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/bg-needs-ordering.yml`:

```yaml
name: Background bg→bg ordering honors needs
description:
  "Two background jobs where the second `needs:` the first. The second job's
  meta.json must record started_at >= the first's finished_at — i.e., the
  coordinator must not race them. Regression test for daft#454."

repos:
  - name: test-bg-needs
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# bg needs ordering"
        commits:
          - message: "Initial commit"
      - name: feature/bg-needs
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: dep-a
              background: true
              run: |
                sleep 0.5
                date -u +%s%N > "$DAFT_WORKTREE_PATH/.a-finished-ns"
            - name: dep-b
              background: true
              needs: [dep-a]
              run: |
                date -u +%s%N > "$DAFT_WORKTREE_PATH/.b-started-ns"

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_BG_NEEDS
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-bg-needs/main"

  - name: Trust the repository
    run: daft hooks trust --force 2>&1
    cwd: "$WORK_DIR/test-bg-needs/main"
    expect:
      exit_code: 0

  - name: Checkout feature branch (triggers worktree-post-create hooks)
    run: git-worktree-checkout feature/bg-needs 2>&1
    cwd: "$WORK_DIR/test-bg-needs/main"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-bg-needs/feature/bg-needs"

  - name: Wait for both background jobs to finish (poll for B's marker)
    run: |
      for i in $(seq 1 40); do
        if [ -f "$WORK_DIR/test-bg-needs/feature/bg-needs/.b-started-ns" ]; then
          echo "BOTH_MARKERS_PRESENT"
          exit 0
        fi
        sleep 0.5
      done
      echo "BG_NEEDS_TIMEOUT"
      exit 1
    expect:
      exit_code: 0
      output_contains:
        - "BOTH_MARKERS_PRESENT"

  - name: Assert dep-b started AFTER dep-a finished
    run: |
      A_FINISHED=$(cat "$WORK_DIR/test-bg-needs/feature/bg-needs/.a-finished-ns")
      B_STARTED=$(cat "$WORK_DIR/test-bg-needs/feature/bg-needs/.b-started-ns")
      echo "a_finished_ns=$A_FINISHED"
      echo "b_started_ns=$B_STARTED"
      if [ "$B_STARTED" -lt "$A_FINISHED" ]; then
        echo "ORDERING_VIOLATION: dep-b started before dep-a finished"
        exit 1
      fi
      echo "ORDERING_OK"
    expect:
      exit_code: 0
      output_contains:
        - "ORDERING_OK"
```

Notes:

- macOS `date` does not support `+%s%N` as nanoseconds; on Linux it does. GitHub
  CI runs on both. The CI matrix runs `mise run test:integration` which executes
  scenarios via the YAML harness. If `%N` proves unportable, swap to a
  `python -c "import time; print(time.time_ns())"` invocation in step 4.
  **Implementer: verify on macOS first** — run
  `mise run test:manual -- --ci hooks/bg-needs-ordering` locally and check
  whether `%N` produces nanoseconds or a literal `N`. If the latter, fall back
  to `python -c 'import time; print(time.time_ns())'`.

- [ ] **Step 2: Run the scenario locally**

```bash
mise run test:manual -- --ci hooks/bg-needs-ordering
```

Expected: PASS. If `%N` returns a literal `N` on macOS, edit the scenario to use
`python -c 'import time; print(time.time_ns())'` for both timestamps, re-run,
and re-check.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/bg-needs-ordering.yml
git commit -m "test(hooks): YAML scenario for bg→bg needs: ordering (regresses #454)"
```

---

## Task 9: Run the full test suite, lint, format

**Files:**

- None — verification only.

- [ ] **Step 1: Format**

```bash
mise run fmt
```

Expected: no diffs (or all generated diffs are intentional).

- [ ] **Step 2: Format check**

```bash
mise run fmt:check
```

Expected: clean.

- [ ] **Step 3: Clippy**

```bash
mise run clippy
```

Expected: zero warnings. If clippy complains about the `Fn` closure captures,
prefer adjusting the closure body rather than `#[allow]`-ing.

- [ ] **Step 4: Unit tests**

```bash
mise run test:unit
```

Expected: all pass.

- [ ] **Step 5: Integration tests**

```bash
mise run test:integration
```

Expected: all pass.

- [ ] **Step 6: Commit any fmt/clippy fixups**

```bash
git status
# If fmt/clippy made changes:
git add -p
git commit -m "chore: fmt/clippy fixups for bg coordinator scheduler"
```

(If no changes, skip.)

---

## Task 10: Push and open the PR

**Files:**

- None.

- [ ] **Step 1: Push**

```bash
git push -u origin daft-454/fix/background-job-coordinator-ignores-needs
```

- [ ] **Step 2: Open PR**

PR title (conventional commit, ≤70 chars):

> `fix(coordinator): honor needs: between background jobs (#454)`

PR body — copy-paste:

```markdown
## Summary

- Reuse the foreground `DagGraph` scheduler in the background coordinator so
  bg→bg `needs:` is honored at runtime, not just on paper.
- `run_single_background_job` now returns `NodeStatus`; the closure passed to
  `DagGraph::run_parallel` carries the existing per-job logic (PID registration,
  log streaming, `meta.json` lifecycle) unchanged.
- Cancelled / failed deps cascade `DepFailed` to dependents (no spawn).

Fixes #454.

## Test plan

- [x] Unit: `bg_dependent_waits_for_dep_to_finish` (regression for #454)
- [x] Unit: `bg_dependent_skipped_when_dep_fails`
- [x] Unit: `bg_dependent_skipped_when_dep_cancelled`
- [x] Unit: `bg_cycle_in_needs_returns_error`,
      `bg_missing_dep_in_needs_returns_error`
- [x] YAML scenario: `tests/manual/scenarios/hooks/bg-needs-ordering.yml`
- [x] All existing coordinator tests still pass
- [x] `mise run fmt:check`, `mise run clippy`, `mise run test:unit`,
      `mise run test:integration`
```

PR tags (per `CLAUDE.md`):

- Assignee: `avihut`
- Label: `fix`
- Milestone: `Public Launch`

```bash
gh pr create \
  --title "fix(coordinator): honor needs: between background jobs (#454)" \
  --assignee avihut \
  --label fix \
  --milestone "Public Launch" \
  --body-file <(cat <<'EOF'
## Summary

- Reuse the foreground `DagGraph` scheduler in the background coordinator so
  bg→bg `needs:` is honored at runtime, not just on paper.
- `run_single_background_job` now returns `NodeStatus`; the closure passed
  to `DagGraph::run_parallel` carries the existing per-job logic unchanged.
- Cancelled / failed deps cascade `DepFailed` to dependents (no spawn).

Fixes #454.

## Test plan

- [x] Unit: `bg_dependent_waits_for_dep_to_finish` (regression for #454)
- [x] Unit: `bg_dependent_skipped_when_dep_fails`
- [x] Unit: `bg_dependent_skipped_when_dep_cancelled`
- [x] Unit: `bg_cycle_in_needs_returns_error`, `bg_missing_dep_in_needs_returns_error`
- [x] YAML scenario: `tests/manual/scenarios/hooks/bg-needs-ordering.yml`
- [x] All existing coordinator tests still pass
- [x] `mise run fmt:check`, `mise run clippy`, `mise run test:unit`, `mise run test:integration`
EOF
)
```

- [ ] **Step 3: Confirm PR is open and tagged**

```bash
gh pr view --json title,labels,assignees,milestone
```

Expected: title matches, label `fix`, assignee `avihut`, milestone
`Public Launch`.

---

## Self-review checklist

- [x] Spec coverage: each requirement in the Runtime Contract section maps to at
      least one task (1–7).
- [x] No placeholders: every step contains the actual code or command.
- [x] Type consistency: `NodeStatus`, `JobStatus`, `JobSpec`, `JobMeta`,
      `LogStore`, `DagGraph` names are used identically across tasks; the
      `run_single_background_job` signature change is propagated to all caller
      sites in Task 3.
- [x] All file paths are absolute-from-repo-root.
- [x] All commands include the daft `mise` task or `cargo` invocation as used by
      `CLAUDE.md`.
