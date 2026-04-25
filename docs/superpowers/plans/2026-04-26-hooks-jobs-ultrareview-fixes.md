# Hooks Jobs Ultrareview Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the four ultrareview findings on the `feat/background-hook-jobs`
branch: non-functional background-job cancellation, documented `log:` /
`background_output:` config fields that are silent no-ops, `RepoPolicy` clobber
on every hook fire, and cleanup-summary accounting bugs.

**Architecture:** Four independent fixes scoped to existing modules
(`coordinator/`, `executor/`, `hooks/`). No new modules. Each fix lands as
TDD-driven commits with regression coverage.

**Tech Stack:** Rust (anyhow, std::sync::mpsc, libc, serde, walkdir, chrono),
YAML manual-test scenarios.

---

## Decision Log

| ID     | Decision                                    | Choice                                                                                                                                                                                       | Rationale                                                                                                                                                                                                      |
| ------ | ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **D1** | How to plumb child PID out of `run_command` | Add `pid_sender: Option<Sender<u32>>` parameter; send `child.id()` immediately after spawn                                                                                                   | Mirrors existing `line_sender: Option<Sender<String>>` parameter shape — no closure plumbing, no shared mutex inside `run_command`                                                                             |
| **D2** | Per-job cancel signal                       | Add `cancelled_jobs: Arc<Mutex<HashSet<String>>>` shared state in coordinator; `cancel_single_job` inserts; classifier checks both `cancel_all` and the per-job set                          | Lets per-job cancel produce `JobStatus::Cancelled` instead of `Failed` without flipping the global `cancel_all`                                                                                                |
| **D3** | Worktree-removal cancel comparison          | Pass `branch_name: &str` (slug) into `cancel_background_jobs_for_worktree`; drop the unused `_project_root` arg                                                                              | `JobInfo.worktree` is unambiguously a branch slug (set from `ctx.branch_name`); slug-to-slug comparison is the only correct one                                                                                |
| **D4** | `background_output: silent` semantics       | Always write to `output.log`; on success completion, delete the log; suppress the failure stderr notification when `silent`                                                                  | Simplest correct implementation; matches the doc table (`Written only on failure / No notification`)                                                                                                           |
| **D5** | `log.path` custom-path field                | **Strip the field.** Remove `LogConfig.path`, doc row, merge entry, and the `log-cleanup-custom-path-untouched.yml` scenario                                                                 | The whole hooks-jobs/log subsystem is new on this branch (no prior release exposed `log.path`), so this isn't a breaking change in practice. Smallest diff, no new behavior to test, eliminates the no-op gap. |
| **D6** | Top-level `log:` propagation                | `yaml_jobs_to_specs` accepts a new `repo_log: Option<&LogConfig>`; merges via `merge_log_configs(per_job.unwrap_or_default(), repo_log)` so per-job overrides win, repo-level fills the rest | Reuses existing merge helper; per-job semantics preserved; repo-level fields finally reach `build_repo_policy`                                                                                                 |
| **D7** | Repo policy persistence                     | Field-merge in `write_repo_policy`: read on-disk first, then keep `Some(_)` from the new policy, else fall back to on-disk                                                                   | "Most-recent-write wins" only for _explicitly set_ fields. Hooks without log config no longer wipe persisted tuning.                                                                                           |
| **D8** | `enforce_budget` return type                | New `BudgetOutcome { evicted_invocations, freed_bytes, freed_jobs }` struct in `clean_policy.rs`; both call sites accumulate all three fields                                                | Counts are already computed locally — surfacing them is a bounded refactor (2 call sites)                                                                                                                      |
| **D9** | Dry-run invocation tally                    | Hoist `candidates_per_inv` build above the `policy.dry_run` early return; in dry-run, count entries where `count >= jobs_per_inv[inv]` and assign to `summary.removed_invocations`           | Mirrors live-path logic; no new computation                                                                                                                                                                    |

---

## D5 resolution

Decided: **Option A — strip `log.path`.** The hooks-jobs/log subsystem is new on
this branch and hasn't shipped in a release, so removing the field is not a
breaking change for any actual user. Task 5 below drops it; the Appendix-B
alternative (implement custom paths) is left in place for reference but is not
part of execution.

---

## File Structure

| File                              | Change                                                                                 | Bug          |
| --------------------------------- | -------------------------------------------------------------------------------------- | ------------ |
| `src/executor/command.rs`         | Add `pid_sender` parameter                                                             | Bug 1        |
| `src/coordinator/process.rs`      | Wire PID channel; add `cancelled_jobs`; silent-aware logging                           | Bug 1, Bug 2 |
| `src/core/worktree/prune.rs`      | Cancel helper takes branch slug                                                        | Bug 1        |
| `src/hooks/job_adapter.rs`        | Accept `repo_log` and merge                                                            | Bug 2        |
| `src/hooks/yaml_executor/mod.rs`  | Pass `&config.log` to adapter                                                          | Bug 2        |
| `src/executor/mod.rs`             | Drop `LogConfig.path` field                                                            | Bug 2        |
| `src/hooks/yaml_config_loader.rs` | Drop `path` from `merge_log_configs`                                                   | Bug 2        |
| `src/coordinator/log_store.rs`    | `write_repo_policy` field-merge; `enforce_budget` returns summary; dry-run tally hoist | Bug 3, Bug 4 |
| `src/coordinator/clean_policy.rs` | New `BudgetOutcome` struct                                                             | Bug 4        |
| `src/log_clean.rs`                | Accumulate budget freed_bytes/jobs                                                     | Bug 4        |
| `src/commands/hooks/jobs.rs`      | Accumulate budget freed_bytes/jobs in CLI                                              | Bug 4        |
| `docs/guide/hooks.md`             | Remove `log.path` row                                                                  | Bug 2        |
| `tests/manual/scenarios/hooks/*`  | New regression scenarios                                                               | All          |

---

# Tasks

## Task 1: Surface child PID from `run_command` (Bug 1, D1)

**Files:**

- Modify: `src/executor/command.rs`
- Test: `src/executor/command.rs` (existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test**

Append to the existing tests module in `src/executor/command.rs`:

```rust
#[test]
fn run_command_sends_child_pid_on_pid_sender() {
    use std::sync::mpsc;
    use std::time::Duration;

    let (pid_tx, pid_rx) = mpsc::channel::<u32>();
    let env = std::collections::HashMap::new();
    let _ = run_command(
        "true",
        &env,
        std::path::Path::new("."),
        Duration::from_secs(5),
        None,
        Some(pid_tx),
    )
    .unwrap();
    let pid = pid_rx.recv_timeout(Duration::from_secs(1)).expect("pid not sent");
    assert!(pid > 0, "pid should be a positive integer");
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p daft --lib executor::command::tests::run_command_sends_child_pid_on_pid_sender
```

Expected: FAIL — wrong number of arguments to `run_command`.

- [ ] **Step 3: Add the parameter and send the PID**

Edit `src/executor/command.rs` `run_command` signature (line 46) to add the new
parameter:

```rust
pub fn run_command(
    cmd: &str,
    env: &HashMap<String, String>,
    working_dir: &Path,
    timeout: Duration,
    line_sender: Option<std::sync::mpsc::Sender<String>>,
    pid_sender: Option<std::sync::mpsc::Sender<u32>>,
) -> Result<CommandResult> {
```

Immediately after the existing `let mut child = command.spawn()` block (around
line 64-66), insert:

```rust
    if let Some(tx) = pid_sender {
        let _ = tx.send(child.id());
    }
```

- [ ] **Step 4: Update all existing callers to pass `None` for the new
      parameter**

```bash
rg "run_command\(" src --type rust -l
```

For each non-test caller, append `, None` before the closing paren. Existing
test callers in `command.rs` itself also need updating — they pass 5 args today.

- [ ] **Step 5: Run the new test plus the full unit suite**

```bash
mise run test:unit
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add -p src/executor/command.rs
git commit -m "$(cat <<'EOF'
feat(executor): plumb child PID out of run_command

Adds optional pid_sender parameter. Used by the coordinator to
register background child PIDs for cancel/shutdown handling.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Register PID + per-job cancel signal (Bug 1, D1 + D2 — merged)

**Why merged:** `cancelled_jobs` and the PID-registration plumbing both alter
`run_single_background_job`'s signature. Splitting them produces a Step-1
failing-test that doesn't compile (uses an arg from the next task), which
destroys the TDD signal.

**Files:**

- Modify: `src/coordinator/process.rs` — `ChildPidMap` types (line 27),
  `run_all_with_cancel` (line 77), `JobInvocationContext` (line 129),
  `run_single_background_job` (line 138-303), `start_socket_listener` (line
  313), `handle_client_connection` (line 364), the `CancelJob` dispatch arm
  (line 403), `cancel_single_job` (line 482), `fork_coordinator` (line 533+)

**Reference signatures (from current code):**

```rust
type ChildPidMap = Arc<Mutex<HashMap<String, u32>>>;

struct JobInvocationContext<'a> {
    invocation_id: &'a str,
    hook_type: &'a str,
    worktree: &'a str,
}

fn run_single_background_job(
    job: &JobSpec,
    ctx: &JobInvocationContext<'_>,
    store: &LogStore,
    results: &Arc<Mutex<Vec<JobResult>>>,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
) { ... }
```

- [ ] **Step 1: Add the new `CancelledJobs` type alias and update the function
      signatures**

In `src/coordinator/process.rs`, near line 27:

```rust
type ChildPidMap = Arc<Mutex<HashMap<String, u32>>>;
type CancelledJobs = Arc<Mutex<std::collections::HashSet<String>>>;
```

Update `run_single_background_job` signature at line 138 to accept the new arg:

```rust
fn run_single_background_job(
    job: &JobSpec,
    ctx: &JobInvocationContext<'_>,
    store: &LogStore,
    results: &Arc<Mutex<Vec<JobResult>>>,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
) {
```

Update `run_all_with_cancel` (line 77) to accept and thread `cancelled_jobs`:

```rust
fn run_all_with_cancel(
    &self,
    store: &LogStore,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
) -> Result<Vec<JobResult>> {
    // ... clone cancelled_jobs into each thread alongside child_pids ...
    run_single_background_job(
        &job, &ctx, &local_store, &results,
        &child_pids, &cancel_all, &cancelled_jobs,
    );
}
```

Update `start_socket_listener` (line 313) and `handle_client_connection`
(line 364) to take and forward `cancelled_jobs`. In `fork_coordinator` (line
565), construct it:

```rust
let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(std::collections::HashSet::new()));
```

Pass it into `start_socket_listener` and
`state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs)`.

- [ ] **Step 2: Write the merged failing test**

Add to `#[cfg(test)] mod tests` in `src/coordinator/process.rs` (create the
module if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    fn make_ctx<'a>(inv: &'a str) -> JobInvocationContext<'a> {
        JobInvocationContext {
            invocation_id: inv,
            hook_type: "worktree-post-create",
            worktree: "feat/x",
        }
    }

    fn make_test_state() -> (
        TempDir, LogStore, ChildPidMap,
        Arc<AtomicBool>, CancelledJobs, Arc<Mutex<Vec<JobResult>>>,
    ) {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().join("jobs").join("test-repo"));
        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancel_all = Arc::new(AtomicBool::new(false));
        let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));
        let results = Arc::new(Mutex::new(Vec::new()));
        (tmp, store, child_pids, cancel_all, cancelled_jobs, results)
    }

    #[test]
    fn run_single_background_job_registers_and_deregisters_pid() {
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, results) = make_test_state();
        let job = JobSpec {
            name: "sleep-job".to_string(),
            command: "sleep 0.4 && echo done".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        };
        let ctx = make_ctx("00000000-0000-0000-0000-000000000001");

        let pids_probe = Arc::clone(&child_pids);
        let probe = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            pids_probe.lock().unwrap().clone()
        });

        run_single_background_job(&job, &ctx, &store, &results,
            &child_pids, &cancel_all, &cancelled_jobs);

        let mid = probe.join().unwrap();
        assert!(mid.contains_key("sleep-job"), "PID should be registered mid-run");
        assert!(!child_pids.lock().unwrap().contains_key("sleep-job"),
            "PID should be deregistered after completion");
    }

    #[test]
    fn per_job_cancel_marks_status_cancelled_not_failed() {
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, results) = make_test_state();
        let job = JobSpec {
            name: "long-job".to_string(),
            command: "sleep 5".to_string(),
            working_dir: std::env::temp_dir(),
            timeout: std::time::Duration::from_secs(30),
            background: true,
            ..Default::default()
        };
        let inv_id = "00000000-0000-0000-0000-000000000002".to_string();
        let ctx = make_ctx(&inv_id);

        let killer = {
            let pids = Arc::clone(&child_pids);
            let cancelled = Arc::clone(&cancelled_jobs);
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                cancelled.lock().unwrap().insert("long-job".to_string());
                if let Some(&pid) = pids.lock().unwrap().get("long-job") {
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
                }
            })
        };

        run_single_background_job(&job, &ctx, &store, &results,
            &child_pids, &cancel_all, &cancelled_jobs);
        killer.join().unwrap();

        let job_dir = store.base_dir.join(&inv_id).join("long-job");
        let meta = store.read_meta(&job_dir).expect("meta should exist");
        assert!(matches!(meta.status, JobStatus::Cancelled),
            "expected Cancelled, got {:?}", meta.status);
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cargo test -p daft --lib coordinator::process::tests
```

Expected: both new tests FAIL — PID never registered, status reported as
`Failed` not `Cancelled`.

- [ ] **Step 4: Wire the PID channel + cancellation classifier**

In `run_single_background_job` body — replace the block from line 219 through
line 264 (inclusive of the writer thread spawn and post-wait classifier).

The current code at line 219-220 moves `log_path` into the writer closure; we
need a clone before the move (will also be needed for Task 8). Replace the
`log_path` binding and the `run_command` block:

```rust
    // 4. Spawn a log writer thread that reads from the channel and writes
    //    to output.log.
    let log_path = LogStore::log_path(&job_dir);
    let log_path_for_writer = log_path.clone();
    let log_writer_handle = std::thread::spawn(move || {
        let file = OpenOptions::new()
            .create(true).append(true).open(&log_path_for_writer);
        match file {
            Ok(mut f) => {
                for line in rx { let _ = writeln!(f, "{line}"); }
            }
            Err(e) => {
                eprintln!("daft: failed to open log file: {e}");
                for _line in rx {}
            }
        }
    });

    // 5. Set up a one-shot PID channel; register PID in child_pids when run_command spawns.
    let (pid_tx, pid_rx) = std::sync::mpsc::channel::<u32>();
    let job_name_for_register = job.name.clone();
    let child_pids_for_register = Arc::clone(&Arc::new(child_pids.clone()));
    // Note: child_pids is &ChildPidMap (= &Arc<Mutex<...>>), so .clone() yields a new Arc.
    let registrar = std::thread::spawn(move || {
        if let Ok(pid) = pid_rx.recv() {
            child_pids_for_register.lock().unwrap().insert(job_name_for_register, pid);
        }
    });

    // 6. Execute the command (now also passing pid_sender).
    let cmd_result = run_command(
        &job.command, &job.env, &job.working_dir, job.timeout,
        Some(tx), Some(pid_tx),
    );

    let _ = registrar.join();
    child_pids.lock().unwrap().remove(&job.name);

    log_writer_handle.join().ok();
    let duration = start.elapsed();

    // 7. Classify final status — both global and per-job cancel signals route to Cancelled.
    let was_cancelled_globally = cancel_all.load(Ordering::Relaxed);
    let was_cancelled_per_job = cancelled_jobs.lock().unwrap().contains(&job.name);
    let was_cancelled = was_cancelled_globally || was_cancelled_per_job;

    let (status, node_status, exit_code) = if was_cancelled {
        (JobStatus::Cancelled, NodeStatus::Skipped, None)
    } else {
        match &cmd_result {
            Ok(cr) if cr.success => (JobStatus::Completed, NodeStatus::Succeeded, cr.exit_code),
            Ok(cr) => (JobStatus::Failed, NodeStatus::Failed, cr.exit_code),
            Err(_) => (JobStatus::Failed, NodeStatus::Failed, None),
        }
    };
```

(Note: `child_pids` is `&Arc<Mutex<...>>`, so the existing call sites pass it by
reference. The `Arc::clone(&Arc::new(child_pids.clone()))` line above is a
placeholder — the cleaner pattern is to take `child_pids: &ChildPidMap` and just
call `child_pids.clone()` to get a new `Arc` for the thread. Adjust to match the
borrowing the surrounding code already uses.)

- [ ] **Step 5: Update `cancel_single_job` to insert into `cancelled_jobs`**

```rust
fn cancel_single_job(
    name: &str,
    child_pids: &ChildPidMap,
    cancelled_jobs: &CancelledJobs,
    _store: &LogStore,
) -> CoordinatorResponse {
    let pids = child_pids.lock().unwrap();
    if let Some(&pid) = pids.get(name) {
        cancelled_jobs.lock().unwrap().insert(name.to_string());
        unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM); }
        CoordinatorResponse::Ack { message: format!("Cancelled job: {name}") }
    } else {
        CoordinatorResponse::Error { message: format!("Job not found or not running: {name}") }
    }
}
```

Update the call site in `handle_client_connection` (line 403):

```rust
CoordinatorRequest::CancelJob { name } =>
    cancel_single_job(&name, child_pids, cancelled_jobs, store),
```

- [ ] **Step 6: Update existing tests that construct `child_pids` to also build
      `cancelled_jobs`**

```bash
rg "ChildPidMap|run_single_background_job|run_all_with_cancel" src --type rust -l
```

For each test/site, add the missing `cancelled_jobs` construction.

- [ ] **Step 7: Run all tests + clippy**

```bash
mise run test:unit && mise run clippy
```

Expected: all tests pass, including both new ones.

- [ ] **Step 8: Commit**

```bash
git add -p src/coordinator/process.rs
git commit -m "$(cat <<'EOF'
fix(coordinator): register child PIDs and honor per-job cancel

run_single_background_job now (a) registers the spawned child's
PID via a one-shot channel from run_command, so cancel/shutdown
handlers actually have something to SIGTERM, and (b) consults a
new CancelledJobs shared set so per-job cancel records
JobStatus::Cancelled instead of Failed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Worktree-removal cancel uses branch slug (Bug 1, D3)

**Files:**

- Modify: `src/core/worktree/prune.rs:828-870` (signature) and `:706` (caller)

- [ ] **Step 1: Write the failing test**

In `src/core/worktree/prune.rs` tests (or wherever similar prune-helpers are
tested), add:

```rust
#[test]
fn cancel_helper_matches_on_branch_slug_not_filesystem_path() {
    use crate::coordinator::log_store::JobStatus;
    use crate::coordinator::ipc::JobInfo;
    let job = JobInfo {
        name: "warm-build".into(),
        worktree: "feat/x".into(),
        hook_type: "worktree-post-create".into(),
        status: JobStatus::Running,
        invocation_id: "abc".into(),
        started_at: chrono::Utc::now(),
        elapsed_secs: Some(5),
    };
    assert!(super::worktree_matches_job(&job, "feat/x"));
    assert!(!super::worktree_matches_job(&job, "/repo/feat/x"));
}
```

If the JobInfo struct shape differs, adapt to whatever it actually is — the goal
is one passing positive case (slug equality) and one failing case
(path-vs-slug).

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p daft --lib worktree::prune::tests::cancel_helper_matches_on_branch_slug_not_filesystem_path
```

Expected: FAIL — `worktree_matches_job` doesn't exist yet.

- [ ] **Step 3: Extract the matcher and switch comparison to slug**

Add a small helper near `cancel_background_jobs_for_worktree`:

```rust
fn worktree_matches_job(job: &crate::coordinator::ipc::JobInfo, branch_slug: &str) -> bool {
    job.worktree == branch_slug
}
```

Change `cancel_background_jobs_for_worktree`:

```rust
fn cancel_background_jobs_for_worktree(branch_slug: &str, sink: &mut dyn ProgressSink) {
    use crate::coordinator::client::CoordinatorClient;
    use crate::coordinator::log_store::JobStatus;

    let repo_hash = match crate::core::repo_identity::compute_repo_id() {
        Ok(id) => id,
        Err(_) => return,
    };
    let mut client = match CoordinatorClient::connect(&repo_hash) {
        Ok(Some(c)) => c,
        _ => return,
    };
    let jobs = match client.list_jobs() {
        Ok(j) => j,
        Err(_) => return,
    };

    for job in jobs {
        if matches!(job.status, JobStatus::Running) && worktree_matches_job(&job, branch_slug) {
            sink.on_step(&format!("Stopping background job '{}'...", job.name));
            match client.cancel_job(&job.name) {
                Ok(_) => sink.on_step(&format!("Stopped background job '{}'", job.name)),
                Err(e) => sink.on_warning(&format!(
                    "Failed to cancel background job '{}': {e}",
                    job.name
                )),
            }
        }
    }
}
```

Update the caller at line 706:

```rust
cancel_background_jobs_for_worktree(branch_name, sink);
```

(`branch_name` is already in scope at that callsite.)

- [ ] **Step 4: Run unit + clippy**

```bash
mise run test:unit && mise run clippy
```

- [ ] **Step 5: Commit**

```bash
git add -p src/core/worktree/prune.rs
git commit -m "$(cat <<'EOF'
fix(worktree): cancel bg jobs by branch slug, not filesystem path

JobInfo.worktree carries a branch slug (e.g. "feat/x"); previously
we compared it against the filesystem worktree path, so the
auto-cancel-on-removal flow silently never matched.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Manual scenario for cancel (Bug 1)

**Template:** Copy the structure from
`tests/manual/scenarios/hooks/log-cleanup-dry-run.yml` (uses `repos:`,
`daft_yml:`, `steps:` with `cwd:`, `expect.exit_code` and `output_contains`).
The framework does NOT support `write_file:` or `jq:` — use shell `python3 -c` /
`grep` / `cat` for assertions.

**Files:**

- Create: `tests/manual/scenarios/hooks/bg-job-cancel-by-name.yml`
- Create: `tests/manual/scenarios/hooks/bg-job-cancel-on-worktree-remove.yml`

- [ ] **Step 1: Author the per-job-cancel scenario**

```yaml
name: Per-job cancel terminates child and records Cancelled status
description:
  "A background job sleeping for 30s is cancelled by name; its meta.json status
  flips to cancelled and `daft hooks jobs --format json` reflects this."

env:
  DAFT_NO_LOG_CLEAN: "1"

repos:
  - name: test-bg-cancel-by-name
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# Cancel test"
        commits:
          - message: "Initial commit"
      - name: feat/x
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: long-sleep
              run: sleep 30
              background: true

steps:
  - name: Clone with --trust-hooks
    run:
      env -u DAFT_TESTING git-worktree-clone --trust-hooks --layout contained
      $REMOTE_TEST_BG_CANCEL_BY_NAME 2>&1
    expect:
      exit_code: 0

  - name: Checkout feat/x (fires the bg hook)
    run: env -u DAFT_TESTING git-worktree-checkout feat/x 2>&1
    cwd: "$WORK_DIR/test-bg-cancel-by-name/main"
    expect:
      exit_code: 0

  - name: Wait briefly so the bg job is in 'running' state
    run: sleep 1
    expect:
      exit_code: 0

  - name: Cancel by name
    run: daft hooks jobs cancel long-sleep 2>&1
    cwd: "$WORK_DIR/test-bg-cancel-by-name/feat/x"
    expect:
      exit_code: 0
      output_contains:
        - "Cancelled job: long-sleep"

  - name: Wait for status to settle
    run: sleep 1
    expect:
      exit_code: 0

  - name: Verify status is cancelled in JSON output
    run: daft hooks jobs --format json 2>&1
    cwd: "$WORK_DIR/test-bg-cancel-by-name/feat/x"
    expect:
      exit_code: 0
      output_contains:
        - '"name":"long-sleep"'
        - '"status":"cancelled"'
```

- [ ] **Step 2: Author the worktree-removal-cancel scenario**

Same structure, but the cancel happens via `git-worktree-prune feat/x` (or
whatever the project's removal command is). Assert the
`Stopping background job 'long-sleep'` line appears.

```yaml
name: Removing a worktree cancels its running background jobs
description:
  "Removing a worktree should auto-cancel any background jobs associated with
  that worktree's branch slug."

env:
  DAFT_NO_LOG_CLEAN: "1"

repos:
  - name: test-bg-cancel-on-rm
    default_branch: main
    branches:
      - name: main
        files: [{ path: README.md, content: "# Auto-cancel" }]
        commits: [{ message: "Initial commit" }]
      - name: feat/x
        from: main
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: long-sleep
              run: sleep 30
              background: true

steps:
  - name: Clone with --trust-hooks
    run:
      env -u DAFT_TESTING git-worktree-clone --trust-hooks --layout contained
      $REMOTE_TEST_BG_CANCEL_ON_RM 2>&1
    expect:
      exit_code: 0

  - name: Checkout feat/x
    run: env -u DAFT_TESTING git-worktree-checkout feat/x 2>&1
    cwd: "$WORK_DIR/test-bg-cancel-on-rm/main"
    expect:
      exit_code: 0

  - name: Settle
    run: sleep 1
    expect: { exit_code: 0 }

  - name: Remove worktree (auto-cancel must fire)
    run: env -u DAFT_TESTING git-worktree-prune feat/x --yes 2>&1
    cwd: "$WORK_DIR/test-bg-cancel-on-rm/main"
    expect:
      exit_code: 0
      output_contains:
        - "Stopping background job 'long-sleep'"
```

(Verify the exact prune-command name with `daft --help` — the project uses
`git-worktree-prune` per CLAUDE.md.)

- [ ] **Step 3: Run both scenarios**

```bash
mise run test:manual -- --ci hooks
```

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/hooks/bg-job-cancel-by-name.yml tests/manual/scenarios/hooks/bg-job-cancel-on-worktree-remove.yml
git commit -m "$(cat <<'EOF'
test(hooks): bg-job cancel scenarios (per-job + worktree-remove)

Locks in cancel semantics: per-job cancel SIGTERMs and records
Cancelled, worktree removal triggers auto-cancel via slug match.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Strip `LogConfig.path` field (Bug 2, D5 — Option A)

**Files:**

- Modify: `src/executor/mod.rs:42`
- Modify: `src/hooks/yaml_config_loader.rs:227-236`
- Modify: `docs/guide/hooks.md` (the `log:` section)
- Possibly: any test fixtures referencing `path:` under `log:`

- [ ] **Step 1: Find every reference**

```bash
rg "LogConfig\s*\{" src tests | head -30
rg "\blog\.path\b|\.path\s*[:=]" src/executor src/hooks src/coordinator | head -20
rg "^\s*path:" tests/manual/scenarios/hooks
```

Note all locations.

- [ ] **Step 2: Remove the field from the struct**

In `src/executor/mod.rs`, delete lines 40-42:

```rust
    /// Override log file path. Absolute or relative to worktree root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
```

- [ ] **Step 3: Remove the merge entry**

In `src/hooks/yaml_config_loader.rs:227-236`, delete the `path:` line in
`merge_log_configs`:

```rust
pub fn merge_log_configs(o: LogConfig, b: LogConfig) -> LogConfig {
    LogConfig {
        retention: o.retention.or(b.retention),
        max_log_size: o.max_log_size.or(b.max_log_size),
        max_total_size: o.max_total_size.or(b.max_total_size),
        keep_last: o.keep_last.or(b.keep_last),
        stale_running_after: o.stale_running_after.or(b.stale_running_after),
    }
}
```

- [ ] **Step 4: Update or delete
      `tests/manual/scenarios/hooks/log-cleanup-custom-path-untouched.yml`**

It tests behavior we're stripping. Either delete it or rewrite to assert the
field is no longer accepted (parser yields a structured error).

- [ ] **Step 5: Update `docs/guide/hooks.md`**

Find the `path:` row in the log: config table and remove it. If a "custom paths"
subsection exists, remove it too.

- [ ] **Step 6: Run unit + integration**

```bash
mise run fmt && mise run clippy && mise run test:unit
```

Expected: passes.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor(hooks): drop unimplemented LogConfig.path field

The path field was parsed and merged but no execution code path
read it; both writers hard-coded the XDG state-dir location.
Stripping is preferable to shipping a documented no-op. The
hooks-jobs/log subsystem is new on this branch and hasn't been
released, so this isn't a breaking change for any real user.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Top-level `log:` defaults reach jobs (Bug 2, D6)

**Files:**

- Modify: `src/hooks/job_adapter.rs:42-145`
- Modify: `src/hooks/yaml_executor/mod.rs:235-243`

**Note:** `HookContext::test_default()` does NOT exist — only
`HookContext::new(...)` (positional args) and `with_*` builders. Use the real
constructor in tests; check the signature with:

```bash
grep -n "pub fn new" src/hooks/environment.rs
```

- [ ] **Step 1: Write the failing test**

Add to `src/hooks/job_adapter.rs` `#[cfg(test)] mod tests`. The `make_test_ctx`
helper below uses the real `HookContext::new` shape — verify the argument list
against the actual signature in `src/hooks/environment.rs:91` and adjust if it
has changed.

```rust
#[cfg(test)]
mod log_merge_tests {
    use super::*;
    use crate::executor::LogConfig;
    use crate::hooks::environment::HookContext;
    use crate::hooks::yaml_config::JobDef;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_test_ctx() -> HookContext {
        // Adjust args to match HookContext::new signature exactly.
        HookContext::new(
            crate::hooks::environment::HookType::PostCreate,
            PathBuf::from("/tmp/repo"),
            PathBuf::from("/tmp/repo/feat-x"),
            "feat/x".to_string(),
        )
    }

    fn job_def(name: &str, run: &str, log: Option<LogConfig>) -> JobDef {
        JobDef {
            name: Some(name.to_string()),
            run: Some(serde_yaml::Value::String(run.to_string())),
            log,
            ..Default::default()
        }
    }

    #[test]
    fn merges_repo_log_into_jobs_without_per_job_log() {
        let job = job_def("build", "cargo build", None);
        let repo_log = LogConfig {
            max_total_size: Some("1GB".to_string()),
            keep_last: Some(5),
            ..Default::default()
        };
        let ctx = make_test_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &[job], &ctx, &HashMap::new(), ".daft",
            std::path::Path::new("/tmp"), None, None,
            Some(&repo_log),
        );
        assert_eq!(kept.len(), 1);
        let lc = kept[0].log_config.as_ref().expect("log_config should be Some");
        assert_eq!(lc.max_total_size.as_deref(), Some("1GB"));
        assert_eq!(lc.keep_last, Some(5));
    }

    #[test]
    fn per_job_log_overrides_repo_log() {
        let job = job_def("build", "cargo build", Some(LogConfig {
            retention: Some("1d".to_string()),
            max_log_size: Some("1MB".to_string()),
            ..Default::default()
        }));
        let repo_log = LogConfig {
            retention: Some("30d".to_string()),
            max_total_size: Some("1GB".to_string()),
            ..Default::default()
        };
        let ctx = make_test_ctx();
        let (kept, _) = yaml_jobs_to_specs(
            &[job], &ctx, &HashMap::new(), ".daft",
            std::path::Path::new("/tmp"), None, None,
            Some(&repo_log),
        );
        let lc = kept[0].log_config.as_ref().unwrap();
        assert_eq!(lc.retention.as_deref(), Some("1d"), "per-job retention wins");
        assert_eq!(lc.max_log_size.as_deref(), Some("1MB"));
        assert_eq!(lc.max_total_size.as_deref(), Some("1GB"), "repo-level fills in");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p daft --lib hooks::job_adapter::tests::yaml_jobs_to_specs_merges_repo_log_into_jobs_without_per_job_log
```

Expected: FAIL — wrong number of args.

- [ ] **Step 3: Add `repo_log` parameter and merge logic**

Modify `yaml_jobs_to_specs` signature in `src/hooks/job_adapter.rs:42`:

```rust
pub fn yaml_jobs_to_specs(
    jobs: &[JobDef],
    ctx: &HookContext,
    hook_env: &HashMap<String, String>,
    source_dir: &str,
    working_dir: &Path,
    rc: Option<&str>,
    hook_background: Option<bool>,
    repo_log: Option<&crate::executor::LogConfig>,
) -> (Vec<JobSpec>, Vec<SkippedJob>) {
```

Replace the `log_config: job.log.clone()` line (line 132) with:

```rust
        log_config: merge_job_log(job.log.clone(), repo_log),
```

Add this helper at the bottom of the file (or beside `yaml_jobs_to_specs`):

```rust
fn merge_job_log(
    per_job: Option<crate::executor::LogConfig>,
    repo: Option<&crate::executor::LogConfig>,
) -> Option<crate::executor::LogConfig> {
    match (per_job, repo) {
        (None, None) => None,
        (Some(j), None) => Some(j),
        (None, Some(r)) => Some(r.clone()),
        (Some(j), Some(r)) => Some(crate::hooks::yaml_config_loader::merge_log_configs(j, r.clone())),
    }
}
```

- [ ] **Step 4: Update the caller in `src/hooks/yaml_executor/mod.rs`**

At line 235, pass `config.log.as_ref()`:

```rust
    let (specs, skipped_jobs) = crate::hooks::job_adapter::yaml_jobs_to_specs(
        &jobs,
        ctx,
        &hook_env,
        source_dir,
        working_dir,
        rc,
        hook_def.background,
        config.log.as_ref(),
    );
```

(`config` here is the loaded `YamlConfig`; verify the variable name in the
surrounding code.)

- [ ] **Step 5: Update any other callers**

```bash
rg "yaml_jobs_to_specs\(" src --type rust
```

Each caller needs `, None` (or the appropriate `Some(&log)`) appended.

- [ ] **Step 6: Run unit + clippy**

```bash
mise run test:unit && mise run clippy
```

- [ ] **Step 7: Commit**

```bash
git add -p src/hooks/
git commit -m "$(cat <<'EOF'
fix(hooks): top-level log: defaults propagate into job log_config

yaml_jobs_to_specs now accepts the repo-level LogConfig and
merges it under each per-job log block, so max_total_size /
keep_last / stale_running_after configured at the top level
finally reach build_repo_policy.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `background_output: silent` is honored (Bug 2, D4)

**Files:**

- Modify: `src/coordinator/process.rs:215-288`

- [ ] **Step 1: Write the failing tests**

Add to the same `tests` module created in Task 2 in
`src/coordinator/process.rs`. Reuses the `make_test_state` and `make_ctx`
helpers from Task 2.

```rust
#[test]
fn silent_bg_output_deletes_log_on_success() {
    use crate::executor::BackgroundOutput;
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, results) = make_test_state();
    let job = JobSpec {
        name: "silent-ok".to_string(),
        command: "echo hello".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        background_output: Some(BackgroundOutput::Silent),
        ..Default::default()
    };
    let inv_id = "00000000-0000-0000-0000-000000000003".to_string();
    let ctx = make_ctx(&inv_id);

    run_single_background_job(&job, &ctx, &store, &results,
        &child_pids, &cancel_all, &cancelled_jobs);

    let job_dir = store.base_dir.join(&inv_id).join("silent-ok");
    let log_path = LogStore::log_path(&job_dir);
    assert!(!log_path.exists(), "silent + success should leave no log file");
}

#[test]
fn silent_bg_output_keeps_log_on_failure() {
    use crate::executor::BackgroundOutput;
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, results) = make_test_state();
    let job = JobSpec {
        name: "silent-fail".to_string(),
        command: "echo whoops; exit 1".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        background_output: Some(BackgroundOutput::Silent),
        ..Default::default()
    };
    let inv_id = "00000000-0000-0000-0000-000000000004".to_string();
    let ctx = make_ctx(&inv_id);

    run_single_background_job(&job, &ctx, &store, &results,
        &child_pids, &cancel_all, &cancelled_jobs);

    let job_dir = store.base_dir.join(&inv_id).join("silent-fail");
    let log_path = LogStore::log_path(&job_dir);
    assert!(log_path.exists(), "silent + failure should preserve log file");
    let contents = std::fs::read_to_string(&log_path).unwrap();
    assert!(contents.contains("whoops"));
}

#[test]
fn non_silent_bg_output_always_writes_log() {
    let (_tmp, store, child_pids, cancel_all, cancelled_jobs, results) = make_test_state();
    let job = JobSpec {
        name: "loud-ok".to_string(),
        command: "echo loud".to_string(),
        working_dir: std::env::temp_dir(),
        background: true,
        background_output: None,
        ..Default::default()
    };
    let inv_id = "00000000-0000-0000-0000-000000000005".to_string();
    let ctx = make_ctx(&inv_id);

    run_single_background_job(&job, &ctx, &store, &results,
        &child_pids, &cancel_all, &cancelled_jobs);

    let job_dir = store.base_dir.join(&inv_id).join("loud-ok");
    let log_path = LogStore::log_path(&job_dir);
    assert!(log_path.exists(), "non-silent should always retain log");
}
```

(The stderr-suppression assertion is hard to do reliably in a unit test; cover
it in the manual scenario in Task 11 instead.)

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — log is always written.

- [ ] **Step 3: Make the writer + notifier silent-aware**

In `src/coordinator/process.rs`:

1. After `let start = Instant::now();` (line 146), add:

```rust
    let is_silent = matches!(
        job.background_output,
        Some(crate::executor::BackgroundOutput::Silent)
    );
```

2. The `log_path` binding is already cloned
   (`let log_path_for_writer = log_path.clone();`) by Task 2, so `log_path`
   remains in scope after `log_writer_handle.join()`. After that join (around
   line 249), add:

```rust
    // Silent mode: only retain the log file if the job failed.
    if is_silent && matches!(&cmd_result, Ok(cr) if cr.success) {
        let _ = std::fs::remove_file(&log_path);
    }
```

3. Wrap the existing `writeln!(std::io::stderr(), ...)` block (line 287, inside
   `if node_status == NodeStatus::Failed`):

```rust
    if node_status == NodeStatus::Failed && !is_silent {
        // existing notification block
    }
```

- [ ] **Step 4: Run tests**

```bash
mise run test:unit
```

- [ ] **Step 5: Commit**

```bash
git add -p src/coordinator/process.rs
git commit -m "$(cat <<'EOF'
fix(coordinator): honor background_output: silent

Previously silent was parsed and validated but had no execution
effect — output.log was always written and the failure stderr
notification always fired. Now: log is deleted on success,
notification suppressed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: `write_repo_policy` field-merges with on-disk (Bug 3, D7)

**Files:**

- Modify: `src/coordinator/log_store.rs:635-666`
- Test: `src/coordinator/log_store.rs` tests module

- [ ] **Step 1: Write the failing test**

Add to `src/coordinator/log_store.rs` tests:

```rust
#[test]
fn write_repo_policy_preserves_unset_fields_from_on_disk() {
    use crate::coordinator::clean_policy::RepoPolicy;
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().join("store"));

    // First write: user sets max_total_size + keep_last.
    let first = RepoPolicy {
        version: RepoPolicy::VERSION,
        max_total_size_bytes: Some(100 * 1024 * 1024),
        keep_last: Some(5),
        stale_running_after_seconds: None,
    };
    store.write_repo_policy(&first).unwrap();

    // Second write: a hook with no log block submits all-None.
    let second = RepoPolicy::defaults();
    store.write_repo_policy(&second).unwrap();

    // The on-disk policy should still have the user's values.
    let read = store.read_repo_policy();
    assert_eq!(read.max_total_size_bytes, Some(100 * 1024 * 1024));
    assert_eq!(read.keep_last, Some(5));
}

#[test]
fn write_repo_policy_overrides_explicitly_set_fields() {
    use crate::coordinator::clean_policy::RepoPolicy;
    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().join("store"));

    let first = RepoPolicy {
        version: RepoPolicy::VERSION,
        max_total_size_bytes: Some(100 * 1024 * 1024),
        keep_last: Some(5),
        stale_running_after_seconds: None,
    };
    store.write_repo_policy(&first).unwrap();

    let second = RepoPolicy {
        version: RepoPolicy::VERSION,
        max_total_size_bytes: Some(200 * 1024 * 1024),
        keep_last: None,
        stale_running_after_seconds: None,
    };
    store.write_repo_policy(&second).unwrap();

    let read = store.read_repo_policy();
    assert_eq!(read.max_total_size_bytes, Some(200 * 1024 * 1024), "explicit set wins");
    assert_eq!(read.keep_last, Some(5), "unset preserves on-disk");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p daft --lib coordinator::log_store::tests::write_repo_policy_preserves_unset_fields_from_on_disk
```

Expected: FAIL — current implementation truncate-writes.

- [ ] **Step 3: Implement field-merge**

Replace `write_repo_policy` body in `src/coordinator/log_store.rs`:

```rust
pub fn write_repo_policy(
    &self,
    policy: &crate::coordinator::clean_policy::RepoPolicy,
) -> Result<()> {
    fs::create_dir_all(&self.base_dir)
        .with_context(|| format!("Failed to create base dir: {}", self.base_dir.display()))?;

    let on_disk = self.read_repo_policy();
    let merged = crate::coordinator::clean_policy::RepoPolicy {
        version: policy.version,
        max_total_size_bytes: policy.max_total_size_bytes.or(on_disk.max_total_size_bytes),
        keep_last: policy.keep_last.or(on_disk.keep_last),
        stale_running_after_seconds: policy
            .stale_running_after_seconds
            .or(on_disk.stale_running_after_seconds),
    };

    let json = serde_json::to_string_pretty(&merged)?;
    let path = self.repo_policy_path();
    fs::write(&path, json)
        .with_context(|| format!("Failed to write repo policy: {}", path.display()))?;
    Ok(())
}
```

- [ ] **Step 4: Run tests**

```bash
mise run test:unit
```

- [ ] **Step 5: Commit**

```bash
git add -p src/coordinator/log_store.rs
git commit -m "$(cat <<'EOF'
fix(coordinator): write_repo_policy field-merges with on-disk

Hooks without a log block submit an all-None RepoPolicy. The
previous truncate-write silently wiped the user's persisted
tuning back to defaults on every such fire. Now: only explicitly
set fields override on-disk; unset fields preserve.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: `enforce_budget` returns full accounting (Bug 4, D8)

**Files:**

- Modify: `src/coordinator/clean_policy.rs` (add struct)
- Modify: `src/coordinator/log_store.rs:476-540`
- Modify: `src/log_clean.rs:124-127`
- Modify: `src/commands/hooks/jobs.rs:1633-1637`

- [ ] **Step 1: Write the failing test (with seed helper inline)**

`log_store.rs` has no pre-existing seed helpers (verified with
`grep "fn seed\|fn make"` — only `create_job_dir` exists). Define one inline at
the top of the test we're adding, in `src/coordinator/log_store.rs`
`#[cfg(test)] mod tests`:

```rust
#[test]
fn enforce_budget_returns_freed_bytes_and_jobs() {
    use crate::coordinator::clean_policy::{BudgetOutcome, RepoPolicy};

    fn seed_inv_with_jobs(
        store: &LogStore,
        inv_id: &str,
        worktree: &str,
        started_at: chrono::DateTime<chrono::Utc>,
        n_jobs: usize,
        log_bytes: usize,
    ) {
        // Write the invocation sidecar.
        let inv_dir = store.base_dir.join(inv_id);
        std::fs::create_dir_all(&inv_dir).unwrap();
        let inv_meta = serde_json::json!({
            "invocation_id": inv_id,
            "worktree": worktree,
            "hook_type": "worktree-post-create",
            "trigger_command": "test",
            "created_at": started_at.to_rfc3339(),
        });
        std::fs::write(
            inv_dir.join("invocation.json"),
            serde_json::to_string(&inv_meta).unwrap(),
        ).unwrap();

        // Write each job's meta + a synthetic log file of `log_bytes` bytes.
        for i in 0..n_jobs {
            let name = format!("job-{i}");
            let job_dir = store.create_job_dir(inv_id, &name).unwrap();
            let meta = JobMeta {
                name: name.clone(),
                hook_type: "worktree-post-create".to_string(),
                worktree: worktree.to_string(),
                command: "echo x".to_string(),
                working_dir: "/tmp".to_string(),
                env: HashMap::new(),
                started_at,
                status: JobStatus::Completed,
                exit_code: Some(0),
                pid: None,
                background: true,
                finished_at: Some(started_at),
                needs: Vec::new(),
                retention_seconds: None,
                max_log_size_bytes: None,
                log_truncated: false,
                original_size_bytes: None,
            };
            store.write_meta(&job_dir, &meta).unwrap();
            let log_path = LogStore::log_path(&job_dir);
            std::fs::write(&log_path, vec![b'x'; log_bytes]).unwrap();
        }
    }

    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().join("jobs").join("test-repo"));

    let now = chrono::Utc::now();
    seed_inv_with_jobs(&store, "inv-old", "feat/x", now - chrono::Duration::days(2), 2, 1_048_576);
    seed_inv_with_jobs(&store, "inv-new", "feat/x", now, 2, 1_048_576);

    let policy = RepoPolicy {
        version: RepoPolicy::VERSION,
        max_total_size_bytes: Some(1_500_000), // forces eviction of inv-old
        keep_last: Some(1),
        stale_running_after_seconds: None,
    };

    let outcome: BudgetOutcome = store.enforce_budget(&policy).unwrap();
    assert_eq!(outcome.evicted_invocations, 1);
    assert_eq!(outcome.freed_jobs, 2);
    assert!(outcome.freed_bytes >= 2 * 1_048_576);
}
```

If `JobMeta`'s field set has changed, adjust the constructor — verify with
`grep "pub struct JobMeta"` in `src/coordinator/log_store.rs`.

- [ ] **Step 2: Run test**

Expected: FAIL — `outcome` is `usize`, doesn't have `.evicted_invocations`.

- [ ] **Step 3: Add the struct and change return type**

In `src/coordinator/clean_policy.rs`, after `RepoPolicy`:

```rust
/// Outcome of a single budget-enforcement pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BudgetOutcome {
    pub evicted_invocations: usize,
    pub freed_bytes: u64,
    pub freed_jobs: usize,
}
```

In `src/coordinator/log_store.rs:479`:

```rust
pub fn enforce_budget(
    &self,
    policy: &crate::coordinator::clean_policy::RepoPolicy,
) -> Result<crate::coordinator::clean_policy::BudgetOutcome> {
    use crate::coordinator::clean_policy::BudgetOutcome;
    let budget = policy.max_total_size_resolved();
    let keep_last = policy.keep_last_resolved();

    let mut total = self.total_size_bytes()?;
    if total <= budget {
        return Ok(BudgetOutcome::default());
    }

    // List invocations with (worktree, inv_id, created_at, total_size, job_count).
    let mut invs: Vec<(String, String, chrono::DateTime<chrono::Utc>, u64, usize)> = Vec::new();
    for inv in self.list_invocations()? {
        let inv_dir = self.base_dir.join(&inv.invocation_id);
        let mut size: u64 = 0;
        let mut job_count: usize = 0;
        for entry in walkdir::WalkDir::new(&inv_dir).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                size += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if entry.file_type().is_dir() && entry.depth() == 1 {
                job_count += 1;
            }
        }
        invs.push((inv.worktree.clone(), inv.invocation_id.clone(), inv.created_at, size, job_count));
    }

    let mut per_wt_count: std::collections::BTreeMap<String, usize> = Default::default();
    for (wt, _, _, _, _) in &invs {
        *per_wt_count.entry(wt.clone()).or_default() += 1;
    }

    invs.sort_by_key(|(_, _, ts, _, _)| *ts);

    let mut outcome = BudgetOutcome::default();
    for (wt, inv_id, _, size, jobs) in invs {
        if total <= budget { break; }
        if let Some(count) = per_wt_count.get_mut(&wt) {
            if *count <= keep_last { continue; }
            *count -= 1;
        }
        let inv_dir = self.base_dir.join(&inv_id);
        let trash = self.base_dir.join(format!(".deleting-{inv_id}"));
        if fs::rename(&inv_dir, &trash).is_ok() {
            let _ = fs::remove_dir_all(&trash);
            total = total.saturating_sub(size);
            outcome.evicted_invocations += 1;
            outcome.freed_bytes += size;
            outcome.freed_jobs += jobs;
        }
    }
    Ok(outcome)
}
```

(`depth() == 1` counts immediate child directories, which under our layout are
job dirs. Adjust if the layout has a different shape.)

- [ ] **Step 4: Update both call sites**

In `src/log_clean.rs:124-127`:

```rust
let bo = store.enforce_budget(&repo_policy).unwrap_or_default();
total_summary.removed_invocations += bo.evicted_invocations;
total_summary.removed_jobs += bo.freed_jobs;
total_summary.freed_bytes += bo.freed_bytes;
```

In `src/commands/hooks/jobs.rs:1633-1637`:

```rust
if !dry_run {
    let bo = store.enforce_budget(&repo_policy).unwrap_or_default();
    summary.removed_invocations += bo.evicted_invocations;
    summary.removed_jobs += bo.freed_jobs;
    summary.freed_bytes += bo.freed_bytes;
}
```

- [ ] **Step 5: Run tests + clippy + fmt**

```bash
mise run test:unit && mise run clippy && mise run fmt
```

- [ ] **Step 6: Commit**

```bash
git add -p src/
git commit -m "$(cat <<'EOF'
fix(coordinator): enforce_budget returns full accounting

Returns BudgetOutcome { evicted_invocations, freed_bytes,
freed_jobs } instead of just the eviction count, so cleanup
summaries accurately reflect what was deleted. Fixes the
'No old logs to clean.' message printing despite hundreds
of MB freed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: `prune --dry-run` reports correct invocation count (Bug 4, D9)

**Files:**

- Modify: `src/coordinator/log_store.rs:285-322`

- [ ] **Step 1: Write the failing test (reuse the seed helper from Task 9)**

If Tasks 9 and 10 land in the same module, refactor the inline
`seed_inv_with_jobs` helper into a module-level `fn` so both tests share it.
Then:

```rust
#[test]
fn dry_run_tallies_removed_invocations() {
    use crate::coordinator::clean_policy::{CleanPolicy, RepoPolicy};

    let tmp = TempDir::new().unwrap();
    let store = LogStore::new(tmp.path().join("jobs").join("test-repo"));

    let now = chrono::Utc::now();
    // 1 invocation with 2 jobs, both far older than retention (override below).
    seed_inv_with_jobs(&store, "inv-old", "feat/x", now - chrono::Duration::days(30), 2, 100);

    let policy = CleanPolicy {
        repo_policy: RepoPolicy::defaults(),
        dry_run: true,
        retention_override: Some(chrono::Duration::seconds(1)),
        ..CleanPolicy::default()
    };
    let summary = store.clean(&policy).unwrap();
    assert_eq!(summary.removed_jobs, 2);
    assert_eq!(summary.removed_invocations, 1, "dry-run should tally would-be-removed invocations");
}
```

Verify the actual fields on `CleanPolicy` first — adjust if `retention_override`
/ `default_retention` etc. have different names in the current source.

- [ ] **Step 2: Run test to verify it fails**

Expected: FAIL — `removed_invocations` is 0 in dry-run.

- [ ] **Step 3: Hoist `candidates_per_inv` build above the dry-run early
      return**

In `src/coordinator/log_store.rs`, move the lines currently at 326-329 (the
`candidates_per_inv` computation) to just before the `if policy.dry_run { ... }`
at line 318, and extend the dry-run branch to compute `removed_invocations`:

```rust
        let mut candidates_per_inv: std::collections::BTreeMap<String, usize> = Default::default();
        for (_, _, inv_id) in &candidates {
            *candidates_per_inv.entry(inv_id.clone()).or_default() += 1;
        }

        if policy.dry_run {
            summary.freed_bytes = candidates.iter().map(|(_, s, _)| s).sum();
            summary.removed_jobs = candidates.len();
            for (inv_id, count) in &candidates_per_inv {
                let total = jobs_per_inv.get(inv_id).copied().unwrap_or(0);
                if *count >= total && total > 0 {
                    summary.removed_invocations += 1;
                }
            }
            return Ok(summary);
        }
```

Then in the live path (after the early return), the existing
`candidates_per_inv` declaration is now dead — delete it.

- [ ] **Step 4: Run tests**

```bash
mise run test:unit
```

- [ ] **Step 5: Commit**

```bash
git add -p src/coordinator/log_store.rs
git commit -m "$(cat <<'EOF'
fix(coordinator): dry-run prune tallies removed_invocations

Hoists candidates_per_inv build above the dry-run early return so
the same logic that counts invocation removals in the live path
(count >= total) also fires in dry-run.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 11: Manual scenarios for cleanup accounting (Bug 2, 3, 4)

**Files:**

- Create: `tests/manual/scenarios/hooks/repo-log-defaults-applied.yml`
- Create: `tests/manual/scenarios/hooks/repo-policy-not-clobbered.yml`
- Create: `tests/manual/scenarios/hooks/prune-budget-accounting.yml`
- Create: `tests/manual/scenarios/hooks/prune-dry-run-invocations.yml`
- Create: `tests/manual/scenarios/hooks/bg-output-silent.yml`

- [ ] **Step 1: Author each scenario using `log-cleanup-dry-run.yml` as the
      template**

Each scenario follows the same shape: `repos:` (with `daft_yml:`), then `steps:`
with `cwd:`, `expect.exit_code` and `output_contains` / `output_not_contains`.
Use shell tools (`python3`, `find`, `grep`, `cat`) for assertions; the framework
has no `jq:` matcher.

Each scenario must:

1. Define a `daft.yml` under `repos[].daft_yml:` exercising the specific
   feature.
2. Clone with `git-worktree-clone --trust-hooks`.
3. Trigger the relevant hook (typically a `git-worktree-checkout`).
4. Assert observable disk/text state via `output_contains`.

Specific assertions per scenario:

- `repo-log-defaults-applied.yml` — top-level
  `log: { max_total_size: 100MB, keep_last: 5 }` with no per-job log block;
  assert `repo-policy.json` contains `"max_total_size_bytes": 104857600` and
  `"keep_last": 5`.
- `repo-policy-not-clobbered.yml` — fire one hook with top-level `log:`, then a
  second hook (different worktree) with no `log:` block; assert
  `repo-policy.json` still contains the user values.
- `prune-budget-accounting.yml` — seed enough invocations to exceed a tight
  `max_total_size`; backdate metas with `python3` (use the recipe from
  `log-cleanup-dry-run.yml`); run `daft hooks jobs prune`; assert summary
  contains a non-zero `freed` figure (use `grep` for `freed [1-9]`).
- `prune-dry-run-invocations.yml` — same setup; run
  `daft hooks jobs prune --dry-run`; assert
  `output_contains: ["Would remove", "across 1 invocation"]` (or whatever count
  seeded).
- `bg-output-silent.yml` — bg job with `background_output: silent` succeeding;
  after settling, assert `output.log` does NOT exist (`find` returning empty).

- [ ] **Step 2: Run the new scenarios**

```bash
mise run test:manual -- --ci hooks
```

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/
git commit -m "$(cat <<'EOF'
test(hooks): regression scenarios for ultrareview fixes

Locks in: top-level log: defaults reach jobs, RepoPolicy not
clobbered by hooks without log blocks, budget eviction reports
full accounting, dry-run reports correct invocation count,
silent background output behavior.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: Final verification

- [ ] **Step 1: Full CI parity**

```bash
mise run ci
```

Expected: passes — fmt, clippy, unit, integration, manual.

- [ ] **Step 2: Manual sandbox sanity**

In a scratch repo with a `daft.yml` that defines a long-sleeping bg job, walk
through:

- `daft hooks jobs cancel <name>` — child terminates, status `cancelled`.
- `daft hooks jobs cancel --all` — all children terminate, all `cancelled`.
- `daft wt rm <feature>` — `Stopping background job '...'` line appears, child
  terminates.
- A daft.yml with top-level `log: { max_total_size: 100MB }` —
  `repo-policy.json` shows `max_total_size_bytes: 104857600`.
- A subsequent unrelated hook fire — `repo-policy.json` still shows `100MB`.
- `daft hooks jobs prune --dry-run` against a repo with retention-expired
  invocations — output reads `Would remove N job(s) across M invocation(s)` with
  M > 0.
- `daft hooks jobs prune` against a repo over budget — summary reads
  `Removed N job(s) ... freed XX MB` with non-zero MB.
- A bg job with `background_output: silent` succeeds — no `output.log` file
  remains. Same job fails — `output.log` is preserved and no stderr
  `daft: background job ... failed` line printed.

- [ ] **Step 3: Final commit (if any verification fixes needed) and summary**

If `ci` and the sanity walk pass cleanly, no further commit needed. Otherwise
fix forward.

---

## Out of scope (deliberate)

- **`log.path` template substitution** (`{branch}`, `{worktree_path}`). Stripped
  per D5; can be re-added as its own feature later if user demand exists.
- **`background_output: pager`** behavior changes. Not flagged by the review.
- **CLI surface for cancel telemetry** (e.g., showing which jobs got SIGTERM'd
  by `cancel --all`). Out of scope; the existing `Cancelled N job(s)` message is
  now accurate.
- **CleanPolicy refactor**. We keep the existing
  `(removed_jobs, removed_invocations, freed_bytes, ...)` shape and only fix the
  dry-run branch and budget call sites; a broader refactor is unrelated to the
  four findings.

---

## Verification matrix

| Bug                            | Unit tests            | Manual scenarios | Verification                                      |
| ------------------------------ | --------------------- | ---------------- | ------------------------------------------------- |
| Bug 1 cancel                   | Tasks 1, 2, 3         | Task 4           | `mise run test:unit` + sandbox walk               |
| Bug 2 silent + repo-log + path | Tasks 5 (gated), 6, 7 | Task 11          | `mise run test:unit` + manual silent scenario     |
| Bug 3 policy clobber           | Task 8                | Task 11          | `mise run test:unit` + sandbox repeated-fire walk |
| Bug 4 accounting               | Tasks 9, 10           | Task 11          | `mise run test:unit` + manual prune scenarios     |

All four bugs have at least one unit test and one manual scenario gating the
fix.

---

## Appendix: Option B for D5 (implement `log.path`)

If the user picks Option B in the open question above, replace Task 5 with:

**Files:**

- Modify: `src/coordinator/process.rs:219` — resolve `lc.path` against
  `job.working_dir` when present
- Modify: `src/executor/log_sink.rs` (the `BufferingLogSink::on_job_complete`
  path) — same resolution
- Modify: `docs/guide/hooks.md` — strip the `{branch}` / `{worktree_path}`
  template bullet, add a callout that custom-path logs are user-managed (no
  auto-cleanup)

Resolution helper:

```rust
fn resolve_log_path(default: &Path, custom: Option<&str>, working_dir: &Path) -> PathBuf {
    match custom {
        None => default.to_path_buf(),
        Some(p) => {
            let pb = PathBuf::from(p);
            if pb.is_absolute() { pb } else { working_dir.join(pb) }
        }
    }
}
```

Test: assert that `lc.path = "build.log"` writes to
`working_dir.join("build.log")` and that `lc.path = "/tmp/abs.log"` writes there
directly. The cleanup tests for `log-cleanup-custom-path-untouched.yml` continue
to apply unchanged.
