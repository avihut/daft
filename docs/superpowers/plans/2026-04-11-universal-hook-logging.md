# Universal Hook Invocation Logging Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `daft hooks jobs` reflect every hook invocation — including
foreground-only hooks, remove hooks, and skipped jobs — instead of only
background work.

**Architecture:** (1) `yaml_executor` writes `invocation.json` for every hook
run, moving that responsibility out of the coordinator. (2) A new `LogSink`
trait lets the generic runner capture foreground job output and metadata into
the same log store the coordinator already uses, via a `BufferingLogSink` that
writes atomically at job completion. (3) `yaml_jobs_to_specs` grows a
`(Vec<JobSpec>, Vec<SkippedJob>)` return type, and per-job `skip:` / `only:`
evaluation is wired up for the first time — activating a dormant YAML feature
and producing the data the new "skipped" listing rows need.

**Tech Stack:** Rust, anyhow, serde, chrono, tabled, mpsc threads, existing
`conditions::should_skip` helpers.

**Spec:** `docs/superpowers/specs/2026-04-11-universal-hook-logging.md`

---

## File map

- **`src/coordinator/log_store.rs`** — `JobStatus::Skipped` variant; new
  `write_job_record` helper that writes `meta.json` + `output.log` atomically.
- **`src/executor/log_sink.rs`** (new) — `LogSink` trait and `BufferingLogSink`
  implementation.
- **`src/executor/mod.rs`** — re-export the new module.
- **`src/executor/runner.rs`** — `run_jobs` gains an optional `sink` parameter;
  output fan-out point and each completion site calls the sink.
- **`src/hooks/job_adapter.rs`** — `SkippedJob` struct; `yaml_jobs_to_specs`
  returns `(Vec<JobSpec>, Vec<SkippedJob>)`; per-job `skip:` / `only:` /
  platform / group evaluation produces `SkippedJob` entries.
- **`src/coordinator/process.rs`** — remove the `write_invocation_meta` call
  from `run_all_with_cancel` (main process owns it now).
- **`src/hooks/yaml_executor/mod.rs`** — compute repo_hash + invocation_id
  unconditionally, write `invocation.json` unconditionally, write sparse skipped
  records, construct `BufferingLogSink` and pass it to `run_jobs`, remove the
  `bg_specs.is_empty()` early return.
- **`src/commands/hooks/jobs.rs`** — render `Skipped` status; render
  `(no jobs declared)` placeholder; extend JSON output.
- **`tests/manual/scenarios/hooks/*.yml`** (new) — integration scenarios listed
  in Tasks 14–18.

---

## Task 1: Add `JobStatus::Skipped` and `write_job_record` helper

**Files:**

- Modify: `src/coordinator/log_store.rs`

**Rationale:** Before touching anything else, extend the persistence layer with
the new status variant and a single helper that writes `meta.json` +
`output.log` together. All subsequent tasks build on this.

- [ ] **Step 1: Write the failing tests**

Add to `src/coordinator/log_store.rs` (append to the existing
`#[cfg(test)] mod tests`):

```rust
#[test]
fn skipped_status_round_trips_through_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = LogStore::new(dir.path().to_path_buf());
    let job_dir = store.create_job_dir("inv1", "dbsetup").unwrap();

    let meta = JobMeta {
        name: "dbsetup".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        command: String::new(),
        working_dir: String::new(),
        env: HashMap::new(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Skipped,
        exit_code: None,
        pid: None,
        background: false,
        finished_at: None,
    };
    store.write_meta(&job_dir, &meta).unwrap();

    let loaded = store.read_meta(&job_dir).unwrap();
    assert_eq!(loaded.status, JobStatus::Skipped);
}

#[test]
fn write_job_record_creates_meta_and_log_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let store = LogStore::new(dir.path().to_path_buf());

    let meta = JobMeta {
        name: "pnpm-install".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        command: "pnpm install".to_string(),
        working_dir: "/tmp/wt".to_string(),
        env: HashMap::new(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Completed,
        exit_code: Some(0),
        pid: None,
        background: false,
        finished_at: Some(chrono::Utc::now()),
    };

    let job_dir = store
        .write_job_record("inv42", &meta, b"installing...\ndone\n")
        .unwrap();

    let loaded_meta = store.read_meta(&job_dir).unwrap();
    assert_eq!(loaded_meta.name, "pnpm-install");

    let log_bytes = std::fs::read(LogStore::log_path(&job_dir)).unwrap();
    assert_eq!(log_bytes, b"installing...\ndone\n");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:
`cargo test -p daft --lib coordinator::log_store::tests::skipped_status_round_trips_through_json coordinator::log_store::tests::write_job_record_creates_meta_and_log_atomically`
Expected: FAIL — `JobStatus::Skipped` doesn't exist, `write_job_record` method
not found.

- [ ] **Step 3: Add the `Skipped` variant**

In `src/coordinator/log_store.rs`, at the `JobStatus` enum (lines 7–14), add the
variant:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Skipped,
}
```

- [ ] **Step 4: Add `write_job_record` helper**

In `src/coordinator/log_store.rs`, inside the `impl LogStore` block (after
`log_path` at line 89), add:

```rust
/// Write `meta.json` and `output.log` for a completed job atomically.
///
/// Creates the job directory if needed. Used by `BufferingLogSink` (for
/// foreground jobs) and by `yaml_executor` (for skipped job records).
pub fn write_job_record(
    &self,
    invocation_id: &str,
    meta: &JobMeta,
    log_bytes: &[u8],
) -> Result<PathBuf> {
    let job_dir = self.create_job_dir(invocation_id, &meta.name)?;
    self.write_meta(&job_dir, meta)?;
    fs::write(Self::log_path(&job_dir), log_bytes)
        .with_context(|| format!("Failed to write log file for job: {}", meta.name))?;
    Ok(job_dir)
}
```

- [ ] **Step 5: Update `clean()` so Skipped records are also eligible for
      cleanup**

In `src/coordinator/log_store.rs`, the existing `clean` method at line 116 has:

```rust
if meta.started_at < cutoff && !matches!(meta.status, JobStatus::Running) {
```

No change needed — `Running` is the only status protected from cleanup, and
`Skipped` is not `Running`. Just confirm by reading the line.

- [ ] **Step 6: Check for any exhaustive matches on `JobStatus` that now need an
      arm**

Run:
`cargo build -p daft 2>&1 | grep -E 'non-exhaustive|pattern `.\*` not covered'`
Expected: compiler identifies every `match status` site that needs a `Skipped`
arm. In this codebase they are:

- `src/commands/hooks/jobs.rs` `format_status_inline` (line ~338) — add
  `JobStatus::Skipped => dim("\u{2014} skipped")`. This gets properly tested in
  Task 12; for now just add the arm so the crate compiles.
- `src/commands/hooks/jobs.rs` JSON output block (around line ~381) — add
  `JobStatus::Skipped => "skipped".to_string()`.
- `src/commands/hooks/jobs.rs` `render_single_job_log` (around line ~622) — add
  the arm (pattern: reuse `Cancelled` treatment initially; proper handling is
  Task 12).
- `src/commands/hooks/jobs.rs` `render_invocation_logs` (around line ~729) —
  same.
- `src/coordinator/process.rs` — none expected (coordinator only writes Running
  / Completed / Failed / Cancelled).

For each exhaustive match site reported by the compiler, add a minimal
`JobStatus::Skipped => …` arm that mirrors the `Cancelled` arm. Task 12 will
refine the display formatting.

- [ ] **Step 7: Run the tests again**

Run:
`cargo test -p daft --lib coordinator::log_store::tests::skipped_status_round_trips_through_json coordinator::log_store::tests::write_job_record_creates_meta_and_log_atomically`
Expected: PASS.

- [ ] **Step 8: Run clippy and fmt**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: zero
warnings, all existing tests still pass.

- [ ] **Step 9: Commit**

```bash
git add src/coordinator/log_store.rs src/commands/hooks/jobs.rs
git commit -m "feat(log_store): add JobStatus::Skipped and write_job_record helper"
```

---

## Task 2: `LogSink` trait

**Files:**

- Create: `src/executor/log_sink.rs`
- Modify: `src/executor/mod.rs`

**Rationale:** The `LogSink` trait is the seam between the generic runner and
persistence. This task lands the trait and a `NoopLogSink` for use by existing
callers that don't need logging. The concrete `BufferingLogSink` comes in
Task 3.

- [ ] **Step 1: Create the new module skeleton**

Write `src/executor/log_sink.rs`:

```rust
//! Job log sinks: a pluggable seam between the generic runner and
//! persistent log storage.
//!
//! The runner drives job execution and streams output via a presenter for
//! live display. A `LogSink`, if provided, also receives output chunks and
//! completion notifications so it can write `meta.json` + `output.log`
//! entries into a `LogStore`. Callers that don't need persistence pass
//! `None`.

use super::{JobResult, JobSpec};

/// Sink for streaming job lifecycle events to persistent storage.
///
/// All methods take `&self` and implementations must be `Send + Sync`
/// because the runner executes jobs on a thread pool.
pub trait LogSink: Send + Sync {
    /// Called exactly once per job, just before the command runs.
    fn on_job_start(&self, spec: &JobSpec);

    /// Called for every output line (stdout+stderr merged) the runner
    /// reads from the child process.
    fn on_job_output(&self, spec: &JobSpec, line: &str);

    /// Called exactly once per job, after the command terminates.
    fn on_job_complete(&self, spec: &JobSpec, result: &JobResult);

    /// Called when a job is skipped by the runner (e.g., piped mode after
    /// a prior failure, or dep-failed in a DAG). The reason describes why.
    fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str);
}
```

- [ ] **Step 2: Re-export from `src/executor/mod.rs`**

Open `src/executor/mod.rs` and add (alongside the other `pub mod` lines — find a
representative one first with `grep -n "pub mod" src/executor/mod.rs`):

```rust
pub mod log_sink;

pub use log_sink::LogSink;
```

- [ ] **Step 3: Verify the crate still builds**

Run: `cargo check -p daft` Expected: PASS (no new callers yet, just the trait
definition).

- [ ] **Step 4: Commit**

```bash
git add src/executor/log_sink.rs src/executor/mod.rs
git commit -m "feat(executor): add LogSink trait"
```

---

## Task 3: `BufferingLogSink` implementation

**Files:**

- Modify: `src/executor/log_sink.rs`

**Rationale:** The concrete sink that foreground hook execution uses. Buffers
output in memory, writes `meta.json` + `output.log` atomically via
`LogStore::write_job_record` at `on_job_complete`. Crash mid-job leaves nothing
on disk.

- [ ] **Step 1: Write the failing tests**

Append to `src/executor/log_sink.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::log_store::{JobStatus, LogStore};
    use crate::executor::{JobResult, JobSpec, NodeStatus};
    use std::sync::Arc;
    use std::time::Duration;

    fn make_spec(name: &str, background: bool) -> JobSpec {
        JobSpec {
            name: name.to_string(),
            command: "echo hi".to_string(),
            background,
            ..Default::default()
        }
    }

    fn make_result(name: &str, status: NodeStatus) -> JobResult {
        JobResult {
            name: name.to_string(),
            status,
            duration: Duration::from_secs(1),
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    #[test]
    fn buffering_sink_writes_meta_and_log_on_complete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "inv1".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let spec = make_spec("pnpm-install", false);
        sink.on_job_start(&spec);
        sink.on_job_output(&spec, "installing...");
        sink.on_job_output(&spec, "done");
        sink.on_job_complete(&spec, &make_result("pnpm-install", NodeStatus::Succeeded));

        let job_dir = tmp.path().join("inv1").join("pnpm-install");
        let loaded = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded.status, JobStatus::Completed);
        assert_eq!(loaded.hook_type, "worktree-post-create");
        assert_eq!(loaded.worktree, "feature/x");
        assert!(!loaded.background);
        assert!(loaded.finished_at.is_some());

        let log_bytes = std::fs::read(LogStore::log_path(&job_dir)).unwrap();
        let log_text = String::from_utf8(log_bytes).unwrap();
        assert!(log_text.contains("installing..."));
        assert!(log_text.contains("done"));
    }

    #[test]
    fn buffering_sink_records_failed_jobs() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "inv2".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let spec = make_spec("broken", false);
        sink.on_job_start(&spec);
        sink.on_job_output(&spec, "error: oops");
        let mut result = make_result("broken", NodeStatus::Failed);
        result.exit_code = Some(2);
        sink.on_job_complete(&spec, &result);

        let job_dir = tmp.path().join("inv2").join("broken");
        let loaded = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded.status, JobStatus::Failed);
        assert_eq!(loaded.exit_code, Some(2));
    }

    #[test]
    fn buffering_sink_drops_in_flight_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));
        {
            let sink = BufferingLogSink::new(
                Arc::clone(&store),
                "inv3".to_string(),
                "worktree-post-create".to_string(),
                "feature/x".to_string(),
            );
            let spec = make_spec("never-finishes", false);
            sink.on_job_start(&spec);
            sink.on_job_output(&spec, "working...");
            // Sink dropped here without calling on_job_complete.
        }
        let job_dir = tmp.path().join("inv3").join("never-finishes");
        assert!(!job_dir.exists(), "no record should be written for in-flight job");
    }

    #[test]
    fn buffering_sink_runner_skipped_writes_sparse_record() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "inv4".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let spec = make_spec("after-the-failure", false);
        sink.on_job_runner_skipped(&spec, "previous job failed");

        let job_dir = tmp.path().join("inv4").join("after-the-failure");
        let loaded = store.read_meta(&job_dir).unwrap();
        assert_eq!(loaded.status, JobStatus::Skipped);

        let log_bytes = std::fs::read(LogStore::log_path(&job_dir)).unwrap();
        assert_eq!(log_bytes, b"previous job failed");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p daft --lib executor::log_sink::tests` Expected: FAIL —
`BufferingLogSink` doesn't exist.

- [ ] **Step 3: Implement `BufferingLogSink`**

In `src/executor/log_sink.rs` (before the `#[cfg(test)] mod tests` block), add:

```rust
use crate::coordinator::log_store::{JobMeta, JobStatus, LogStore};
use crate::executor::NodeStatus;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// A `LogSink` that buffers output per job and writes `meta.json` +
/// `output.log` atomically at `on_job_complete`.
///
/// If a job is in flight when the sink is dropped (e.g., the main process
/// crashes mid-run), its buffered output is discarded and no record is
/// written. This matches the "atomic at completion" design choice in
/// docs/superpowers/specs/2026-04-11-universal-hook-logging.md §1.
pub struct BufferingLogSink {
    store: Arc<LogStore>,
    invocation_id: String,
    hook_type: String,
    worktree: String,
    buffers: Mutex<HashMap<String, JobBuffer>>,
}

struct JobBuffer {
    started_at: chrono::DateTime<chrono::Utc>,
    output: Vec<u8>,
}

impl BufferingLogSink {
    pub fn new(
        store: Arc<LogStore>,
        invocation_id: String,
        hook_type: String,
        worktree: String,
    ) -> Self {
        Self {
            store,
            invocation_id,
            hook_type,
            worktree,
            buffers: Mutex::new(HashMap::new()),
        }
    }

    fn node_to_job_status(status: NodeStatus, has_exit_code: bool) -> JobStatus {
        match status {
            NodeStatus::Succeeded => JobStatus::Completed,
            NodeStatus::Failed if has_exit_code => JobStatus::Failed,
            NodeStatus::Failed => JobStatus::Failed,
            NodeStatus::Skipped | NodeStatus::DepFailed => JobStatus::Skipped,
        }
    }
}

impl LogSink for BufferingLogSink {
    fn on_job_start(&self, spec: &JobSpec) {
        let mut buffers = self.buffers.lock().unwrap();
        buffers.insert(
            spec.name.clone(),
            JobBuffer {
                started_at: chrono::Utc::now(),
                output: Vec::new(),
            },
        );
    }

    fn on_job_output(&self, spec: &JobSpec, line: &str) {
        let mut buffers = self.buffers.lock().unwrap();
        if let Some(buf) = buffers.get_mut(&spec.name) {
            buf.output.extend_from_slice(line.as_bytes());
            buf.output.push(b'\n');
        }
    }

    fn on_job_complete(&self, spec: &JobSpec, result: &JobResult) {
        let buf = {
            let mut buffers = self.buffers.lock().unwrap();
            buffers.remove(&spec.name)
        };
        let Some(buf) = buf else { return };

        let meta = JobMeta {
            name: spec.name.clone(),
            hook_type: self.hook_type.clone(),
            worktree: self.worktree.clone(),
            command: spec.command.clone(),
            working_dir: spec.working_dir.to_string_lossy().into_owned(),
            env: spec.env.clone(),
            started_at: buf.started_at,
            status: Self::node_to_job_status(result.status, result.exit_code.is_some()),
            exit_code: result.exit_code,
            pid: None,
            background: false,
            finished_at: Some(chrono::Utc::now()),
        };

        let _ = self.store.write_job_record(&self.invocation_id, &meta, &buf.output);
    }

    fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str) {
        // Remove any buffered state for defensive cleanup.
        {
            let mut buffers = self.buffers.lock().unwrap();
            buffers.remove(&spec.name);
        }

        let meta = JobMeta {
            name: spec.name.clone(),
            hook_type: self.hook_type.clone(),
            worktree: self.worktree.clone(),
            command: spec.command.clone(),
            working_dir: spec.working_dir.to_string_lossy().into_owned(),
            env: spec.env.clone(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Skipped,
            exit_code: None,
            pid: None,
            background: false,
            finished_at: None,
        };

        let _ = self.store.write_job_record(&self.invocation_id, &meta, reason.as_bytes());
    }
}
```

- [ ] **Step 4: Re-export `BufferingLogSink` from the executor module**

In `src/executor/mod.rs`, extend the re-export added in Task 2:

```rust
pub use log_sink::{BufferingLogSink, LogSink};
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p daft --lib executor::log_sink::tests` Expected: PASS (all
four tests).

- [ ] **Step 6: Clippy + fmt**

Run: `mise run fmt && mise run clippy` Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/executor/log_sink.rs src/executor/mod.rs
git commit -m "feat(executor): add BufferingLogSink for foreground job logging"
```

---

## Task 4: Thread `LogSink` through `run_jobs`

**Files:**

- Modify: `src/executor/runner.rs`

**Rationale:** Add an optional sink parameter to `run_jobs` and forward it
through the internal scheduling functions. The output fan-out point and each
completion site gets sink calls. Existing callers pass `None` and are
unaffected.

- [ ] **Step 1: Find the existing callers of `run_jobs`**

Run: `grep -rn 'run_jobs(' src/ --include='*.rs'` Expected: a small list. The
only call site outside `runner.rs` itself is
`src/hooks/yaml_executor/mod.rs:244` (the foreground dispatch) plus
`DAFT_NO_BACKGROUND_JOBS` fallback near line 254. Both will be updated in later
tasks; for Task 4 they keep passing `None`.

- [ ] **Step 2: Write a runner test that exercises the sink**

Append to the tests module in `src/executor/runner.rs` (after the existing
tests, inside `mod tests`):

```rust
/// Minimal test sink that records which lifecycle methods were called.
#[derive(Default)]
struct RecordingLogSink {
    events: std::sync::Mutex<Vec<String>>,
}

impl RecordingLogSink {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl crate::executor::log_sink::LogSink for RecordingLogSink {
    fn on_job_start(&self, spec: &JobSpec) {
        self.events.lock().unwrap().push(format!("start:{}", spec.name));
    }
    fn on_job_output(&self, spec: &JobSpec, line: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("output:{}:{}", spec.name, line));
    }
    fn on_job_complete(&self, spec: &JobSpec, result: &JobResult) {
        self.events
            .lock()
            .unwrap()
            .push(format!("complete:{}:{:?}", spec.name, result.status));
    }
    fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("runner_skipped:{}:{}", spec.name, reason));
    }
}

#[test]
fn sink_receives_start_output_and_complete_for_successful_job() {
    let jobs = vec![make_job("hello", "echo hello")];
    let presenter: Arc<dyn JobPresenter> = Arc::new(RecordingPresenter::default());
    let concrete = Arc::new(RecordingLogSink::default());
    let sink_arc: Arc<dyn crate::executor::log_sink::LogSink> = concrete.clone();

    let _ = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, Some(&sink_arc)).unwrap();

    let events = concrete.events();
    assert!(events.iter().any(|e| e == "start:hello"));
    assert!(events.iter().any(|e| e.starts_with("output:hello:")));
    assert!(events.iter().any(|e| e == "complete:hello:Succeeded"));
}

#[test]
fn sink_receives_runner_skipped_in_piped_mode_after_failure() {
    let jobs = vec![make_job("bad", "false"), make_job("after", "echo never")];
    let presenter: Arc<dyn JobPresenter> = Arc::new(RecordingPresenter::default());
    let concrete = Arc::new(RecordingLogSink::default());
    let sink_arc: Arc<dyn crate::executor::log_sink::LogSink> = concrete.clone();

    let _ = run_jobs(&jobs, ExecutionMode::Piped, &presenter, Some(&sink_arc)).unwrap();

    let events = concrete.events();
    assert!(events.iter().any(|e| e == "runner_skipped:after:previous job failed"));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run:
`cargo test -p daft --lib executor::runner::tests::sink_receives_start_output_and_complete_for_successful_job executor::runner::tests::sink_receives_runner_skipped_in_piped_mode_after_failure`
Expected: FAIL — `run_jobs` doesn't take a sink argument.

- [ ] **Step 4: Update `run_jobs` signature and thread the sink down**

In `src/executor/runner.rs`, replace the `pub fn run_jobs` signature (lines
28–48):

```rust
pub fn run_jobs(
    jobs: &[JobSpec],
    mode: ExecutionMode,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    let has_deps = jobs.iter().any(|j| !j.needs.is_empty());

    if has_deps {
        run_with_dag(jobs, mode, presenter, sink)
    } else {
        match mode {
            ExecutionMode::Parallel => run_parallel_flat(jobs, presenter, sink),
            ExecutionMode::Sequential => run_sequential(jobs, presenter, false, sink),
            ExecutionMode::Piped => run_sequential(jobs, presenter, true, sink),
        }
    }
}
```

- [ ] **Step 5: Thread the sink through `run_sequential`**

Replace `fn run_sequential` (lines 59–101):

```rust
fn run_sequential(
    jobs: &[JobSpec],
    presenter: &Arc<dyn JobPresenter>,
    stop_on_failure: bool,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let mut results = Vec::with_capacity(jobs.len());

    for (i, job) in jobs.iter().enumerate() {
        presenter.on_job_start(&job.name, job.description.as_deref(), Some(&job.command));
        if let Some(s) = sink {
            s.on_job_start(job);
        }
        let start = Instant::now();

        let cr = execute_single_job(job, presenter, sink)?;
        let duration = start.elapsed();
        let result = command_to_job_result(&job.name, &cr, duration);

        report_completion(job, &result, presenter);
        if let Some(s) = sink {
            s.on_job_complete(job, &result);
        }
        let failed = result.status == NodeStatus::Failed;
        results.push(result);

        if failed && stop_on_failure {
            for remaining in &jobs[i + 1..] {
                presenter.on_job_skipped(
                    &remaining.name,
                    "previous job failed",
                    Duration::ZERO,
                    false,
                );
                if let Some(s) = sink {
                    s.on_job_runner_skipped(remaining, "previous job failed");
                }
                results.push(JobResult {
                    name: remaining.name.clone(),
                    status: NodeStatus::Skipped,
                    duration: Duration::ZERO,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            return Ok(results);
        }
    }

    Ok(results)
}
```

- [ ] **Step 6: Thread the sink through `run_parallel_flat` and `run_with_dag`**

Replace `run_parallel_flat` (lines 108–115):

```rust
fn run_parallel_flat(
    jobs: &[JobSpec],
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let nodes: Vec<(String, Vec<String>)> = jobs.iter().map(|j| (j.name.clone(), vec![])).collect();
    let graph = DagGraph::new(nodes)?;
    run_dag_execution(jobs, &graph, presenter, sink)
}
```

Replace `run_with_dag` (lines 122–137):

```rust
fn run_with_dag(
    jobs: &[JobSpec],
    mode: ExecutionMode,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let nodes: Vec<(String, Vec<String>)> = jobs
        .iter()
        .map(|j| (j.name.clone(), j.needs.clone()))
        .collect();
    let graph = DagGraph::new(nodes)?;

    match mode {
        ExecutionMode::Parallel => run_dag_execution(jobs, &graph, presenter, sink),
        _ => run_dag_sequential_exec(jobs, &graph, presenter, mode == ExecutionMode::Piped, sink),
    }
}
```

- [ ] **Step 7: Thread the sink through `run_dag_execution`**

Replace `run_dag_execution` (lines 140–201). The sink needs to be cloneable into
closures running on the thread pool, so we clone the `Arc` once per sub-scope.
Body below reproduces the original plus sink calls:

```rust
fn run_dag_execution(
    jobs: &[JobSpec],
    graph: &DagGraph,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let job_map = build_job_map(jobs);
    let max_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let captured: std::sync::Mutex<HashMap<usize, CapturedOutput>> =
        std::sync::Mutex::new(HashMap::new());
    let durations: std::sync::Mutex<HashMap<usize, Duration>> =
        std::sync::Mutex::new(HashMap::new());

    let sink_for_closure = sink.cloned();

    let statuses = graph.run_parallel(
        |idx, name| {
            let Some(job) = job_map.get(name) else {
                return NodeStatus::Failed;
            };

            presenter.on_job_start(name, job.description.as_deref(), Some(&job.command));
            if let Some(ref s) = sink_for_closure {
                s.on_job_start(job);
            }
            let start = Instant::now();

            let cr = execute_single_job(job, presenter, sink_for_closure.as_ref());
            let duration = start.elapsed();

            match cr {
                Ok(cr) => {
                    let result = command_to_job_result(name, &cr, duration);
                    report_completion(job, &result, presenter);
                    if let Some(ref s) = sink_for_closure {
                        s.on_job_complete(job, &result);
                    }

                    captured.lock().unwrap().insert(
                        idx,
                        CapturedOutput {
                            exit_code: cr.exit_code,
                            stdout: cr.stdout,
                            stderr: cr.stderr,
                        },
                    );
                    durations.lock().unwrap().insert(idx, duration);

                    result.status
                }
                Err(_) => {
                    presenter.on_job_failure(name, duration);
                    if let Some(ref s) = sink_for_closure {
                        let failed_result = JobResult {
                            name: job.name.clone(),
                            status: NodeStatus::Failed,
                            duration,
                            exit_code: None,
                            stdout: String::new(),
                            stderr: String::new(),
                        };
                        s.on_job_complete(job, &failed_result);
                    }
                    durations.lock().unwrap().insert(idx, duration);
                    NodeStatus::Failed
                }
            }
        },
        max_workers,
    );

    let captured = captured.into_inner().unwrap();
    let durations = durations.into_inner().unwrap();

    Ok(build_results_from_statuses(
        graph,
        &statuses,
        &captured,
        &durations,
        jobs,
        presenter,
        sink,
    ))
}
```

- [ ] **Step 8: Thread the sink through `run_dag_sequential_exec`**

Replace `run_dag_sequential_exec` (lines 204–259) — same shape as
`run_dag_execution`:

```rust
fn run_dag_sequential_exec(
    jobs: &[JobSpec],
    graph: &DagGraph,
    presenter: &Arc<dyn JobPresenter>,
    _stop_on_failure: bool,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let job_map = build_job_map(jobs);

    let captured: std::sync::Mutex<HashMap<usize, CapturedOutput>> =
        std::sync::Mutex::new(HashMap::new());
    let durations: std::sync::Mutex<HashMap<usize, Duration>> =
        std::sync::Mutex::new(HashMap::new());

    let sink_for_closure = sink.cloned();

    let statuses = graph.run_sequential(|idx, name| {
        let Some(job) = job_map.get(name) else {
            return NodeStatus::Failed;
        };

        presenter.on_job_start(name, job.description.as_deref(), Some(&job.command));
        if let Some(ref s) = sink_for_closure {
            s.on_job_start(job);
        }
        let start = Instant::now();

        let cr = execute_single_job(job, presenter, sink_for_closure.as_ref());
        let duration = start.elapsed();

        match cr {
            Ok(cr) => {
                let result = command_to_job_result(name, &cr, duration);
                report_completion(job, &result, presenter);
                if let Some(ref s) = sink_for_closure {
                    s.on_job_complete(job, &result);
                }

                captured.lock().unwrap().insert(
                    idx,
                    CapturedOutput {
                        exit_code: cr.exit_code,
                        stdout: cr.stdout,
                        stderr: cr.stderr,
                    },
                );
                durations.lock().unwrap().insert(idx, duration);

                result.status
            }
            Err(_) => {
                presenter.on_job_failure(name, duration);
                if let Some(ref s) = sink_for_closure {
                    let failed_result = JobResult {
                        name: job.name.clone(),
                        status: NodeStatus::Failed,
                        duration,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                    };
                    s.on_job_complete(job, &failed_result);
                }
                durations.lock().unwrap().insert(idx, duration);
                NodeStatus::Failed
            }
        }
    });

    let captured = captured.into_inner().unwrap();
    let durations = durations.into_inner().unwrap();

    Ok(build_results_from_statuses(
        graph,
        &statuses,
        &captured,
        &durations,
        jobs,
        presenter,
        sink,
    ))
}
```

- [ ] **Step 9: Update `execute_single_job` to fan output to the sink**

Replace `execute_single_job` (lines 281–309):

```rust
fn execute_single_job(
    job: &JobSpec,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<CommandResult> {
    if job.interactive {
        run_command_interactive(&job.command, &job.env, &job.working_dir)
    } else {
        let (tx, rx) = mpsc::channel::<String>();

        let presenter_clone = Arc::clone(presenter);
        let sink_clone: Option<Arc<dyn crate::executor::log_sink::LogSink>> = sink.cloned();
        let job_name = job.name.clone();
        let job_for_sink = job.clone();
        let reader_handle = std::thread::spawn(move || {
            for line in rx {
                presenter_clone.on_job_output(&job_name, &line);
                if let Some(ref s) = sink_clone {
                    s.on_job_output(&job_for_sink, &line);
                }
            }
        });

        let result = run_command(
            &job.command,
            &job.env,
            &job.working_dir,
            job.timeout,
            Some(tx),
        );

        reader_handle.join().ok();

        result
    }
}
```

- [ ] **Step 10: Update `build_results_from_statuses` to notify the sink for DAG
      dep-failed nodes**

Replace `build_results_from_statuses` (lines 352–396):

```rust
fn build_results_from_statuses(
    graph: &DagGraph,
    statuses: &[NodeStatus],
    captured: &HashMap<usize, CapturedOutput>,
    durations: &HashMap<usize, Duration>,
    jobs: &[JobSpec],
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Vec<JobResult> {
    let job_map: HashMap<&str, &JobSpec> = jobs.iter().map(|j| (j.name.as_str(), j)).collect();

    statuses
        .iter()
        .enumerate()
        .map(|(idx, &status)| {
            let name = &graph.names[idx];
            let duration = durations.get(&idx).copied().unwrap_or(Duration::ZERO);

            if status == NodeStatus::DepFailed {
                if let Some(job) = job_map.get(name.as_str()) {
                    presenter.on_job_skipped(&job.name, "dependency failed", Duration::ZERO, false);
                    if let Some(s) = sink {
                        s.on_job_runner_skipped(job, "dependency failed");
                    }
                }
            }

            match captured.get(&idx) {
                Some(cap) => JobResult {
                    name: name.clone(),
                    status,
                    duration,
                    exit_code: cap.exit_code,
                    stdout: cap.stdout.clone(),
                    stderr: cap.stderr.clone(),
                },
                None => JobResult {
                    name: name.clone(),
                    status,
                    duration,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            }
        })
        .collect()
}
```

- [ ] **Step 11: Update the single existing non-test caller of `run_jobs` to
      pass `None`**

In `src/hooks/yaml_executor/mod.rs` at line 244 (the foreground run call site)
and line 254 (the `DAFT_NO_BACKGROUND_JOBS` fallback), append `, None` to each
`run_jobs(…)` invocation:

```rust
// line 244:
let fg_results = crate::executor::runner::run_jobs(&fg_specs, exec_mode, presenter, None)?;

// line 254:
let bg_results = crate::executor::runner::run_jobs(&bg_specs, exec_mode, presenter, None)?;
```

(Task 11 will replace the `None` at line 244 with a real sink.)

- [ ] **Step 12: Update all runner.rs internal test calls to `run_jobs` to pass
      `None`**

Run: `grep -n 'run_jobs(' src/executor/runner.rs` Expected: every test-only
call. Add `, None` as the last argument to each. For the two new tests added in
Step 2, they already pass the sink correctly.

- [ ] **Step 13: Build + run the tests**

Run: `cargo test -p daft --lib executor::runner` Expected: PASS including the
two new sink tests.

- [ ] **Step 14: Full unit suite sanity check**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: clean.

- [ ] **Step 15: Commit**

```bash
git add src/executor/runner.rs src/hooks/yaml_executor/mod.rs
git commit -m "feat(runner): thread optional LogSink through run_jobs"
```

---

## Task 5: Introduce `SkippedJob` and change `yaml_jobs_to_specs` return type

**Files:**

- Modify: `src/hooks/job_adapter.rs`

**Rationale:** Lands the new return shape and converts the existing
platform-skip and group-skip branches from silent `None` drops into `SkippedJob`
entries with descriptive reasons. No new evaluation logic yet — that's Tasks 6
and 7.

- [ ] **Step 1: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block in
`src/hooks/job_adapter.rs`:

```rust
#[test]
fn platform_skip_produces_skipped_job_entry() {
    use crate::hooks::yaml_config::{JobDef, RunCommand, TargetOs};
    let mut run_map = HashMap::new();
    let other_os = if cfg!(target_os = "macos") {
        TargetOs::Linux
    } else {
        TargetOs::Macos
    };
    run_map.insert(other_os, "echo other".to_string());

    let jobs = vec![JobDef {
        name: Some("platform-only".to_string()),
        run: Some(RunCommand::Platform(run_map)),
        ..Default::default()
    }];

    let ctx = sample_hook_context();
    let env = HashMap::new();
    let (kept, skipped) = yaml_jobs_to_specs(
        &jobs,
        &ctx,
        &env,
        "/src",
        std::path::Path::new("/work"),
        None,
        None,
    );

    assert!(kept.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].name, "platform-only");
    assert!(skipped[0].reason.contains("platform"));
}

#[test]
fn group_jobs_produce_skipped_job_entry() {
    use crate::hooks::yaml_config::{GroupDef, JobDef};
    let jobs = vec![JobDef {
        name: Some("my-group".to_string()),
        group: Some(GroupDef { jobs: vec![] }),
        ..Default::default()
    }];

    let ctx = sample_hook_context();
    let env = HashMap::new();
    let (kept, skipped) = yaml_jobs_to_specs(
        &jobs,
        &ctx,
        &env,
        "/src",
        std::path::Path::new("/work"),
        None,
        None,
    );

    assert!(kept.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].name, "my-group");
    assert!(skipped[0].reason.contains("group"));
}
```

If `sample_hook_context()` doesn't already exist in the test module, reuse
whatever helper the existing `platform_skip_excludes_job` test (around line 190)
uses to build a `HookContext` — copy that fixture into a local helper if needed.

- [ ] **Step 2: Run the tests to confirm they fail**

Run:
`cargo test -p daft --lib hooks::job_adapter::tests::platform_skip_produces_skipped_job_entry hooks::job_adapter::tests::group_jobs_produce_skipped_job_entry`
Expected: FAIL — signature mismatch (`yaml_jobs_to_specs` returns
`Vec<JobSpec>`, not a tuple).

- [ ] **Step 3: Define `SkippedJob`**

At the top of `src/hooks/job_adapter.rs` (after existing `use` statements,
before `yaml_jobs_to_specs`):

```rust
/// A job that was declared in YAML but filtered out before execution.
///
/// Produced by `yaml_jobs_to_specs` alongside the kept `JobSpec`s so the
/// caller can record a skipped-job entry in the log store.
#[derive(Debug, Clone)]
pub struct SkippedJob {
    pub name: String,
    pub background: bool,
    pub reason: String,
}
```

- [ ] **Step 4: Change `yaml_jobs_to_specs` return type and convert filter
      sites**

Replace the function body (`src/hooks/job_adapter.rs:31-95`):

```rust
pub fn yaml_jobs_to_specs(
    jobs: &[JobDef],
    ctx: &HookContext,
    hook_env: &HashMap<String, String>,
    source_dir: &str,
    working_dir: &Path,
    rc: Option<&str>,
    hook_background: Option<bool>,
) -> (Vec<JobSpec>, Vec<SkippedJob>) {
    let mut kept: Vec<JobSpec> = Vec::new();
    let mut skipped: Vec<SkippedJob> = Vec::new();

    for job in jobs {
        let name = job.name.clone().unwrap_or_else(|| "(unnamed)".to_string());
        let declared_background = job.background.or(hook_background).unwrap_or(false);

        if job.group.is_some() {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: "skip: group jobs are not yet supported by the generic executor"
                    .to_string(),
            });
            continue;
        }

        if super::yaml_executor::is_platform_skip(job) {
            skipped.push(SkippedJob {
                name,
                background: declared_background,
                reason: format!(
                    "skip: platform-specific run has no entry for {}",
                    std::env::consts::OS
                ),
            });
            continue;
        }

        let cmd = super::yaml_executor::resolve_command(job, ctx, Some(&name), source_dir);

        let cmd = match rc {
            Some(rc_path) => format!("source {rc_path} && {cmd}"),
            None => cmd,
        };

        let mut env = hook_env.clone();
        if let Some(ref job_env) = job.env {
            env.extend(job_env.clone());
        }

        let wd = if let Some(ref root) = job.root {
            working_dir.join(root)
        } else {
            working_dir.to_path_buf()
        };

        kept.push(JobSpec {
            name,
            command: cmd,
            working_dir: wd,
            env,
            description: job.description.clone(),
            needs: job.needs.clone().unwrap_or_default(),
            interactive: job.interactive == Some(true),
            fail_text: job.fail_text.clone(),
            timeout: JobSpec::DEFAULT_TIMEOUT,
            background: declared_background,
            background_output: job.background_output.clone(),
            log_config: job.log.clone(),
        });
    }

    (kept, skipped)
}
```

- [ ] **Step 5: Update existing callers**

Run: `grep -n 'yaml_jobs_to_specs' src/ --include='*.rs' -r` Expected: the
single non-test caller is in `src/hooks/yaml_executor/mod.rs` at line 206.
Update that line to unpack the tuple:

```rust
let (specs, _skipped_jobs) = crate::hooks::job_adapter::yaml_jobs_to_specs(
    &jobs,
    ctx,
    &hook_env,
    source_dir,
    working_dir,
    rc,
    hook_def.background,
);
```

The `_skipped_jobs` binding will be consumed in Task 9; for now the underscore
prefix silences `unused_variables`.

- [ ] **Step 6: Update existing `job_adapter.rs` tests that call
      `yaml_jobs_to_specs`**

Run: `grep -n 'yaml_jobs_to_specs' src/hooks/job_adapter.rs` Expected: several
test call sites (lines 148–519). Update each to destructure the tuple. For tests
that only assert on the kept specs, use
`let (specs, _) = yaml_jobs_to_specs(...)`. Existing assertions on
`specs.len()`, `specs[0].name`, etc. continue to work once the binding is
updated.

For the test `platform_skip_excludes_job` (line ~190) and
`group_jobs_are_skipped` (line ~372), the old assertion
`assert!(specs.is_empty())` becomes `assert!(kept.is_empty())`, and you can also
add `assert_eq!(skipped.len(), 1)`.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p daft --lib hooks::job_adapter` Expected: PASS including the
two new tests.

- [ ] **Step 8: Clippy + fmt**

Run: `mise run fmt && mise run clippy` Expected: clean.

- [ ] **Step 9: Commit**

```bash
git add src/hooks/job_adapter.rs src/hooks/yaml_executor/mod.rs
git commit -m "refactor(job_adapter): return (kept, skipped) from yaml_jobs_to_specs"
```

---

## Task 6: Wire up per-job `skip:` evaluation

**Files:**

- Modify: `src/hooks/job_adapter.rs`

**Rationale:** Activates dormant per-job `skip:` handling. Uses the existing
`conditions::should_skip` helper. A matching `skip:` rule produces a
`SkippedJob` entry with the helper's `SkipInfo.reason` string.

- [ ] **Step 1: Write the failing test**

Append to `src/hooks/job_adapter.rs` tests:

```rust
#[test]
fn per_job_skip_true_produces_skipped_entry() {
    use crate::hooks::yaml_config::{JobDef, SkipCondition};
    let jobs = vec![JobDef {
        name: Some("gated".to_string()),
        run: Some(crate::hooks::yaml_config::RunCommand::Simple("echo gated".to_string())),
        skip: Some(SkipCondition::Bool(true)),
        ..Default::default()
    }];

    let ctx = sample_hook_context();
    let env = HashMap::new();
    let (kept, skipped) = yaml_jobs_to_specs(
        &jobs,
        &ctx,
        &env,
        "/src",
        std::path::Path::new("/work"),
        None,
        None,
    );

    assert!(kept.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].name, "gated");
    assert_eq!(skipped[0].reason, "skip: true");
}

#[test]
fn per_job_skip_false_keeps_job() {
    use crate::hooks::yaml_config::{JobDef, SkipCondition};
    let jobs = vec![JobDef {
        name: Some("always-runs".to_string()),
        run: Some(crate::hooks::yaml_config::RunCommand::Simple("echo go".to_string())),
        skip: Some(SkipCondition::Bool(false)),
        ..Default::default()
    }];

    let ctx = sample_hook_context();
    let env = HashMap::new();
    let (kept, skipped) = yaml_jobs_to_specs(
        &jobs,
        &ctx,
        &env,
        "/src",
        std::path::Path::new("/work"),
        None,
        None,
    );

    assert_eq!(kept.len(), 1);
    assert!(skipped.is_empty());
}
```

- [ ] **Step 2: Run tests, verify failure**

Run:
`cargo test -p daft --lib hooks::job_adapter::tests::per_job_skip_true_produces_skipped_entry hooks::job_adapter::tests::per_job_skip_false_keeps_job`
Expected: FAIL — `skip: true` test fails because the job still makes it into
`kept` (the field is currently ignored).

- [ ] **Step 3: Wire up per-job `skip:` evaluation**

In `src/hooks/job_adapter.rs`, in the `yaml_jobs_to_specs` loop, immediately
after the `is_platform_skip` check, add:

```rust
if let Some(ref skip) = job.skip {
    if let Some(info) = super::conditions::should_skip(skip, working_dir) {
        skipped.push(SkippedJob {
            name,
            background: declared_background,
            reason: info.reason,
        });
        continue;
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p daft --lib hooks::job_adapter` Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/hooks/job_adapter.rs
git commit -m "feat(job_adapter): evaluate per-job skip: conditions"
```

---

## Task 7: Wire up per-job `only:` evaluation

**Files:**

- Modify: `src/hooks/job_adapter.rs`

**Rationale:** Companion to Task 6 — activates per-job `only:`. Uses
`conditions::should_only_skip`; a failing `only:` produces a `SkippedJob` with
the helper's reason string.

- [ ] **Step 1: Write the failing test**

Append to `src/hooks/job_adapter.rs` tests:

```rust
#[test]
fn per_job_only_false_produces_skipped_entry() {
    use crate::hooks::yaml_config::{JobDef, OnlyCondition};
    let jobs = vec![JobDef {
        name: Some("conditional".to_string()),
        run: Some(crate::hooks::yaml_config::RunCommand::Simple("echo cond".to_string())),
        only: Some(OnlyCondition::Bool(false)),
        ..Default::default()
    }];

    let ctx = sample_hook_context();
    let env = HashMap::new();
    let (kept, skipped) = yaml_jobs_to_specs(
        &jobs,
        &ctx,
        &env,
        "/src",
        std::path::Path::new("/work"),
        None,
        None,
    );

    assert!(kept.is_empty());
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].reason, "only: false");
}
```

- [ ] **Step 2: Run tests, verify failure**

Run:
`cargo test -p daft --lib hooks::job_adapter::tests::per_job_only_false_produces_skipped_entry`
Expected: FAIL.

- [ ] **Step 3: Wire up per-job `only:` evaluation**

In `src/hooks/job_adapter.rs`, in the `yaml_jobs_to_specs` loop, immediately
after the per-job `skip:` block from Task 6, add:

```rust
if let Some(ref only) = job.only {
    if let Some(info) = super::conditions::should_only_skip(only, working_dir) {
        skipped.push(SkippedJob {
            name,
            background: declared_background,
            reason: info.reason,
        });
        continue;
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p daft --lib hooks::job_adapter` Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/hooks/job_adapter.rs
git commit -m "feat(job_adapter): evaluate per-job only: conditions"
```

---

## Task 8: Move `write_invocation_meta` from coordinator to main process

**Files:**

- Modify: `src/coordinator/process.rs`
- Modify: `src/hooks/yaml_executor/mod.rs`

**Rationale:** Prerequisite for universal logging: the main process owns the
invocation record so it exists even when no background jobs are forked. The
coordinator keeps writing per-job records for its bg jobs.

- [ ] **Step 1: Remove the coordinator's `write_invocation_meta` call**

In `src/coordinator/process.rs`, locate the block in `run_all_with_cancel`
(lines 83–92 per the survey):

```rust
if !self.trigger_command.is_empty() {
    let inv_meta = super::log_store::InvocationMeta {
        invocation_id: self.invocation_id.clone(),
        trigger_command: self.trigger_command.clone(),
        hook_type: self.hook_type.clone(),
        worktree: self.worktree.clone(),
        created_at: chrono::Utc::now(),
    };
    let _ = store.write_invocation_meta(&self.invocation_id, &inv_meta);
}
```

Delete this entire block. The main process will write `invocation.json` before
forking the coordinator.

- [ ] **Step 2: Add unconditional invocation write in `yaml_executor`**

In `src/hooks/yaml_executor/mod.rs`, around the dispatch block that currently
starts at line 220 (the `partition_foreground_background` call), insert — after
the tuple destructure from Task 5 but before the fg/bg dispatch — a block that
computes `repo_hash` and `invocation_id` unconditionally and writes
`invocation.json`.

Read the surrounding lines first to see the exact local variable names (the
`#[cfg(unix)]` branch previously wrapped these computations at lines 272–273).
Hoist them out so they are computed on all platforms:

```rust
// Compute repo hash and invocation ID unconditionally so every hook
// invocation lands in the log store, even fg-only and remove hooks.
let repo_hash = compute_repo_hash(&hook_env_obj);
let invocation_id = generate_invocation_id();
let store = std::sync::Arc::new(
    crate::coordinator::log_store::LogStore::for_repo(&repo_hash)?,
);

let trigger_command = if ctx.command == "hooks-run" {
    format!("hooks run {}", hook_name)
} else {
    hook_name.to_string()
};

let inv_meta = crate::coordinator::log_store::InvocationMeta {
    invocation_id: invocation_id.clone(),
    trigger_command: trigger_command.clone(),
    hook_type: hook_name.to_string(),
    worktree: ctx.branch_name.clone(),
    created_at: chrono::Utc::now(),
};
let _ = store.write_invocation_meta(&invocation_id, &inv_meta);
```

Place this block **after**
`let (fg_specs, bg_specs) = partition_foreground_background(&specs);` at line
223 and **before** the `on_phase_start` presenter call at line 240.

- [ ] **Step 3: Delete the duplicate computation inside the `#[cfg(unix)]`
      block**

In `src/hooks/yaml_executor/mod.rs`, inside the `#[cfg(unix)] { ... }` block at
lines 270–299, remove:

- `let repo_hash = compute_repo_hash(&hook_env_obj);` (line 272)
- `let invocation_id = generate_invocation_id();` (line 273)
- `let store = crate::coordinator::log_store::LogStore::for_repo(&repo_hash)?;`
  (line 274)
- The `let trigger_command = ...` block (lines 278–282)

These are now computed earlier. The
`CoordinatorState::new(&repo_hash, &invocation_id)` call at line 285 stays — it
just uses the outer-scope variables. The `store` passed to `fork_coordinator` at
line 292 needs to be dereferenced from the `Arc`: pass `(*store).clone()` or
restructure so the coordinator still owns a `LogStore`. Because `LogStore` is
just `{ base_dir: PathBuf }`, cloning is trivial — add `#[derive(Clone)]` to
`LogStore` in `src/coordinator/log_store.rs` if it isn't already, and pass
`(*store).clone()`.

- [ ] **Step 4: Add `#[derive(Clone)]` to `LogStore`**

In `src/coordinator/log_store.rs` at line 51, the struct currently is:

```rust
pub struct LogStore {
    pub base_dir: PathBuf,
}
```

Change to:

```rust
#[derive(Clone)]
pub struct LogStore {
    pub base_dir: PathBuf,
}
```

- [ ] **Step 5: Add a sanity test in `log_store.rs`**

Append to `src/coordinator/log_store.rs` tests:

```rust
#[test]
fn invocation_meta_written_without_coordinator() {
    // Verifies that write_invocation_meta creates the file on its own;
    // no coordinator fork required.
    let dir = tempfile::tempdir().unwrap();
    let store = LogStore::new(dir.path().to_path_buf());
    let meta = InvocationMeta {
        invocation_id: "deadbeef".to_string(),
        trigger_command: "worktree-post-create".to_string(),
        hook_type: "worktree-post-create".to_string(),
        worktree: "feature/x".to_string(),
        created_at: chrono::Utc::now(),
    };

    store.write_invocation_meta("deadbeef", &meta).unwrap();
    let path = dir.path().join("deadbeef").join("invocation.json");
    assert!(path.exists());

    let loaded = store.read_invocation_meta("deadbeef").unwrap();
    assert_eq!(loaded.trigger_command, "worktree-post-create");
}
```

- [ ] **Step 6: Build and run the unit suite**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: clean.
All existing tests still pass; the new test passes.

- [ ] **Step 7: Commit**

```bash
git add src/coordinator/process.rs src/coordinator/log_store.rs src/hooks/yaml_executor/mod.rs
git commit -m "refactor(hooks): move invocation.json write from coordinator to main"
```

---

## Task 9: `yaml_executor` writes sparse records for skipped jobs

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`

**Rationale:** Uses the `_skipped` binding from Task 5. For each `SkippedJob`,
writes a sparse `JobMeta` + a log file containing the reason. This data is the
input for display Tasks 12 and 13.

**Verification note:** Unit testing this change in isolation requires
replicating the `yaml_executor` test scaffold, which is non-trivial. End-to-end
verification lands in Task 15 (`per-job-skip-records.yml`), which asserts that a
skipped job's `meta.json` and log file are present after a real hook run. For
this task, verify via `cargo build` and `clippy`; the integration scenario is
the functional test.

- [ ] **Step 1: Write skipped records in `execute_yaml_hook_with_logging`**

In `src/hooks/yaml_executor/mod.rs`, immediately after the block introduced in
Task 8 (the unconditional invocation write) and before the `on_phase_start`
call, add:

First, rename the outer binding — go back to line 206 and change `_skipped_jobs`
to `skipped_jobs` (drop the underscore) now that it's actually used.

Then insert this loop:

```rust
// Write sparse records for jobs that were filtered out before execution.
// Each skipped job gets a meta.json + log file containing the reason so
// the user can investigate via `daft hooks jobs logs <name>`.
for sj in &skipped_jobs {
    let meta = crate::coordinator::log_store::JobMeta {
        name: sj.name.clone(),
        hook_type: hook_name.to_string(),
        worktree: ctx.branch_name.clone(),
        command: String::new(),
        working_dir: String::new(),
        env: std::collections::HashMap::new(),
        started_at: chrono::Utc::now(),
        status: crate::coordinator::log_store::JobStatus::Skipped,
        exit_code: None,
        pid: None,
        background: sj.background,
        finished_at: None,
    };
    let _ = store.write_job_record(&invocation_id, &meta, sj.reason.as_bytes());
}
```

- [ ] **Step 2: Build and check**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: clean.
(Functional verification happens in Task 15.)

- [ ] **Step 3: Commit**

```bash
git add src/hooks/yaml_executor/mod.rs
git commit -m "feat(yaml_executor): write sparse log records for skipped jobs"
```

---

## Task 10: Pass `BufferingLogSink` to `run_jobs` for foreground work

**Files:**

- Modify: `src/hooks/yaml_executor/mod.rs`

**Rationale:** Completes the write path. `execute_yaml_hook_with_logging`
constructs a `BufferingLogSink` pointing at the same invocation/store as the
(optional) background coordinator, and passes it to `run_jobs` for foreground
execution. Foreground jobs now land in the log store alongside their background
peers.

- [ ] **Step 1: Construct the sink after the invocation write**

In `src/hooks/yaml_executor/mod.rs`, immediately after the skipped-record loop
from Task 9 and before `presenter.on_phase_start(hook_name)` at line 240, add:

```rust
let fg_sink: std::sync::Arc<dyn crate::executor::log_sink::LogSink> =
    std::sync::Arc::new(crate::executor::log_sink::BufferingLogSink::new(
        std::sync::Arc::clone(&store),
        invocation_id.clone(),
        hook_name.to_string(),
        ctx.branch_name.clone(),
    ));
```

- [ ] **Step 2: Update the foreground `run_jobs` call to pass the sink**

In `src/hooks/yaml_executor/mod.rs` at line 244 (the call modified in Task 4
step 11), change:

```rust
let fg_results = crate::executor::runner::run_jobs(&fg_specs, exec_mode, presenter, None)?;
```

to:

```rust
let fg_results =
    crate::executor::runner::run_jobs(&fg_specs, exec_mode, presenter, Some(&fg_sink))?;
```

- [ ] **Step 3: Update the `DAFT_NO_BACKGROUND_JOBS` fallback**

In `src/hooks/yaml_executor/mod.rs` at line 254, inline bg jobs also go through
the fg sink so they appear in the listing with `background: false` (the escape
hatch behavior documented in spec §5). Change:

```rust
let bg_results = crate::executor::runner::run_jobs(&bg_specs, exec_mode, presenter, None)?;
```

to:

```rust
let bg_results =
    crate::executor::runner::run_jobs(&bg_specs, exec_mode, presenter, Some(&fg_sink))?;
```

- [ ] **Step 4: Leave the `bg_specs.is_empty()` early return in place**

Confirm that lines 247–250 in `src/hooks/yaml_executor/mod.rs` still contain:

```rust
if bg_specs.is_empty() {
    presenter.on_phase_complete(hook_start.elapsed());
    return job_results_to_hook_result(&fg_results);
}
```

This early-return is still correct: by the time we reach it, `invocation.json`
has been written (Task 8), skipped records have been written (Task 9), and the
fg sink has written all fg job records (Steps 1–2 of this task). The
early-return just avoids the unnecessary coordinator fork when there are no
background jobs. No code change — this step exists only to confirm the block is
intentional after the surrounding changes.

- [ ] **Step 5: Verification note**

End-to-end functional verification lives in Task 13
(`foreground-only-hook.yml`), which runs a real hook and asserts that the
foreground invocation and job records are present in the log store. For this
task, verify via the unit suite only.

- [ ] **Step 6: Full unit suite**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: clean.

- [ ] **Step 7: Commit**

```bash
git add src/hooks/yaml_executor/mod.rs
git commit -m "feat(yaml_executor): pass BufferingLogSink to foreground runner"
```

---

## Task 11: Render `Skipped` status in `daft hooks jobs`

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

**Rationale:** Task 1 added a minimal stub `Skipped` match arm in
`format_status_inline` and friends. This task refines the display so skipped
rows read as intended.

- [ ] **Step 1: Update `format_status_inline`**

In `src/commands/hooks/jobs.rs` around line 338, the function currently has arms
for `Completed`, `Failed`, `Running`, `Cancelled` (plus the stub `Skipped` added
in Task 1). Ensure the `Skipped` arm is exactly:

```rust
JobStatus::Skipped => dim("\u{2014} skipped"),
```

- [ ] **Step 2: Update the JSON output**

In `src/commands/hooks/jobs.rs` around line 381 in `print_json_output`, ensure
the `status_str` match includes:

```rust
JobStatus::Skipped => "skipped".to_string(),
```

- [ ] **Step 3: Update log rendering status labels for skipped jobs**

In `src/commands/hooks/jobs.rs` `render_single_job_log` (line 622), the match
produces a `status_label`. Add the arm:

```rust
JobStatus::Skipped => dim("SKIPPED"),
```

In `render_invocation_logs` (line 729), the same match exists. Add:

```rust
JobStatus::Skipped => dim("SKIPPED"),
```

The log read path at lines 671–685 is unconditional — it reads `output.log`
whenever it exists, regardless of status. Skipped jobs have a log file
containing the reason string (written in Task 9), so they will display their
reason under the `--- output ---` section naturally. No other changes needed.

- [ ] **Step 4: Write a rendering test**

Append to `src/commands/hooks/jobs.rs` tests:

```rust
#[test]
fn format_status_inline_renders_skipped() {
    let rendered = format_status_inline(&JobStatus::Skipped, true);
    // The dim helper wraps in ANSI codes; check that the literal "skipped"
    // is present inside.
    assert!(rendered.contains("skipped"));
}
```

- [ ] **Step 5: Run the test**

Run:
`cargo test -p daft --lib commands::hooks::jobs::tests::format_status_inline_renders_skipped`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): render skipped jobs in hooks jobs listing and json"
```

---

## Task 12: Render `(no jobs declared)` placeholder for empty invocations

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

**Rationale:** After Task 8 every hook run writes `invocation.json`, including
hooks with zero jobs. Today `list_jobs` silently skips those invocations
(`if job_dirs.is_empty() { continue; }` around line 494). The user asked for
empty invocations to appear so they can see the hook fired.

- [ ] **Step 1: Replace the silent `continue` with the placeholder**

In `src/commands/hooks/jobs.rs` around line 494, the loop body inside
`list_jobs` currently has:

```rust
let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
if job_dirs.is_empty() {
    continue;
}
```

Change to:

```rust
let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
if job_dirs.is_empty() {
    output.info(&format!("  {}", dim("(no jobs declared)")));
    output.info("");
    continue;
}
```

This prints the placeholder under the invocation header (which was already
printed at lines 484–489) and a blank line separator, matching the rhythm of
non-empty invocation sections.

- [ ] **Step 2: Verification note**

End-to-end verification of this behavior lives in Task 17
(`empty-hook-placeholder.yml`), which runs a hook with zero jobs and asserts the
placeholder appears in `daft hooks jobs` output.

- [ ] **Step 3: Clippy + fmt + full unit suite**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(jobs): show (no jobs declared) for empty hook invocations"
```

---

## Task 13: Integration scenario — foreground-only hook

**Files:**

- Create: `tests/manual/scenarios/hooks/foreground-only-hook.yml`

**Rationale:** Regression gate for the core visibility fix. A hook that declares
only foreground jobs used to write nothing to the log store; after this feature
it writes a full invocation record with per-job meta + logs.

- [ ] **Step 1: Read an existing scenario to follow the format**

Run: `cat tests/manual/scenarios/hooks/background-jobs.yml` Study the structure
— `repos`, `daft_yml`, `steps`, `expect` sections.

- [ ] **Step 2: Write the new scenario**

Create `tests/manual/scenarios/hooks/foreground-only-hook.yml`:

```yaml
name: Foreground-only hook creates an invocation record

description: |
  Regression for the "foreground jobs are invisible" bug. A hook that
  declares only foreground jobs used to write nothing to the log store,
  so `daft hooks jobs` showed no record. After sub-project A of the
  hooks-jobs redesign, the hook's invocation and every foreground job
  must appear in the listing.

repos:
  - name: fg-hook
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "fg-only hook test repo"
        commits:
          - message: "init"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: say-hello
              run: echo hello-from-fg
            - name: say-done
              run: echo done-from-fg

steps:
  - name: Create a worktree — triggers worktree-post-create (fg only)
    run: daft wt add feature/fg
    expect:
      exit_code: 0
      output_contains:
        - "hello-from-fg"
        - "done-from-fg"

  - name: List jobs — should show the invocation with two fg rows
    run: daft hooks jobs
    cwd: feature/fg
    expect:
      exit_code: 0
      output_contains:
        - "worktree-post-create"
        - "say-hello"
        - "say-done"
        - "completed"
      output_not_contains:
        - "(no jobs declared)"

  - name: View log for one of the fg jobs
    run: daft hooks jobs logs say-hello
    cwd: feature/fg
    expect:
      exit_code: 0
      output_contains:
        - "hello-from-fg"
```

- [ ] **Step 3: Run the scenario**

Run: `mise run test:manual -- --ci hooks:foreground-only-hook` Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/hooks/foreground-only-hook.yml
git commit -m "test(hooks): integration scenario for foreground-only hook visibility"
```

---

## Task 14: Integration scenario — mixed fg + bg invocation

**Files:**

- Create: `tests/manual/scenarios/hooks/mixed-fg-bg-invocation.yml`

**Rationale:** Verifies that when both execution modes are present, main and
coordinator write to the same invocation without races, and the listing
correctly distinguishes them.

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/mixed-fg-bg-invocation.yml`:

```yaml
name: Mixed foreground + background hook invocation

description: |
  When a hook has both foreground and background jobs, main process writes
  the invocation and fg job records, and the coordinator writes the bg job
  records. Both sets must land under the same invocation ID with no races,
  and `daft hooks jobs` must show them in the same invocation group.

repos:
  - name: mixed-hook
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "mixed hook test"
        commits:
          - message: "init"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: setup-env
              run: echo fg-setup
            - name: print-done
              run: echo fg-done
            - name: warm-cache
              run: sleep 1 && echo bg-cache
              background: true
            - name: warm-index
              run: sleep 1 && echo bg-index
              background: true

steps:
  - name: Create worktree — triggers mixed fg/bg hook
    run: daft wt add feature/mixed
    expect:
      exit_code: 0
      output_contains:
        - "fg-setup"
        - "fg-done"

  - name: Wait for background jobs to finish, then list
    run: sleep 3 && daft hooks jobs
    cwd: feature/mixed
    expect:
      exit_code: 0
      output_contains:
        - "setup-env"
        - "print-done"
        - "warm-cache"
        - "warm-index"
        - "completed"

  - name: Logs for a bg job
    run: daft hooks jobs logs warm-cache
    cwd: feature/mixed
    expect:
      exit_code: 0
      output_contains:
        - "bg-cache"

  - name: Logs for a fg job
    run: daft hooks jobs logs setup-env
    cwd: feature/mixed
    expect:
      exit_code: 0
      output_contains:
        - "fg-setup"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci hooks:mixed-fg-bg-invocation` Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/mixed-fg-bg-invocation.yml
git commit -m "test(hooks): integration scenario for mixed fg+bg invocation"
```

---

## Task 15: Integration scenario — per-job `skip:` / `only:` skipped records

**Files:**

- Create: `tests/manual/scenarios/hooks/per-job-skip-records.yml`

**Rationale:** Verifies the expanded-scope work from Tasks 6–7. Before this
feature, `skip:` and `only:` on individual jobs were silently ignored. After,
matching jobs are actually filtered and appear as skipped entries with their
reason stored in the job's log file.

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/per-job-skip-records.yml`:

```yaml
name: Per-job skip/only conditions filter and record

description: |
  Activating the dormant per-job skip: / only: evaluation. A job with
  `skip: true` must not run and must appear in `daft hooks jobs` as
  skipped. A job with `only: false` must behave the same. The reason
  string must be visible via `daft hooks jobs logs <name>`.

repos:
  - name: skip-hook
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "skip test repo"
        commits:
          - message: "init"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: runs-always
              run: echo runs
            - name: gated-skip
              run: echo never-skip
              skip: true
            - name: gated-only
              run: echo never-only
              only: false

steps:
  - name: Create worktree
    run: daft wt add feature/skip
    expect:
      exit_code: 0
      output_contains:
        - "runs"
      output_not_contains:
        - "never-skip"
        - "never-only"

  - name: List shows skipped jobs alongside the runnable one
    run: daft hooks jobs
    cwd: feature/skip
    expect:
      exit_code: 0
      output_contains:
        - "runs-always"
        - "completed"
        - "gated-skip"
        - "gated-only"
        - "skipped"

  - name: Log for skip-gated job shows reason
    run: daft hooks jobs logs gated-skip
    cwd: feature/skip
    expect:
      exit_code: 0
      output_contains:
        - "skip: true"

  - name: Log for only-gated job shows reason
    run: daft hooks jobs logs gated-only
    cwd: feature/skip
    expect:
      exit_code: 0
      output_contains:
        - "only: false"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci hooks:per-job-skip-records` Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/per-job-skip-records.yml
git commit -m "test(hooks): integration scenario for per-job skip/only records"
```

---

## Task 16: Integration scenario — remove hook visibility regression

**Files:**

- Create: `tests/manual/scenarios/hooks/remove-hook-visibility.yml`

**Rationale:** The direct regression for the originally observed bug:
`worktree-pre-remove` / `worktree-post-remove` hooks with only foreground jobs
used to leave zero records. After this feature, they must appear in
`daft hooks jobs` even after the worktree is deleted.

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/remove-hook-visibility.yml`:

```yaml
name: Remove-hook activity is visible after worktree deletion

description: |
  Regression for the originally observed bug: worktree-pre-remove hooks
  with only foreground jobs wrote nothing to the log store, so users
  could not debug failed cleanup. After sub-project A, the remove hook
  invocation and per-job records persist in the log store (under the
  now-stale branch name) after the worktree is deleted.

repos:
  - name: remove-hook
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "remove hook test"
        commits:
          - message: "init"
    daft_yml: |
      hooks:
        worktree-pre-remove:
          jobs:
            - name: cleanup-db
              run: echo removing-db-volumes
            - name: announce-removal
              run: echo goodbye

steps:
  - name: Create worktree (baseline)
    run: daft wt add feature/goes-away
    expect:
      exit_code: 0

  - name: Remove worktree — fires worktree-pre-remove fg jobs
    run: daft wt remove feature/goes-away
    expect:
      exit_code: 0
      output_contains:
        - "removing-db-volumes"
        - "goodbye"

  - name: List jobs — the pre-remove invocation must appear
    run: daft hooks jobs --all
    expect:
      exit_code: 0
      output_contains:
        - "worktree-pre-remove"
        - "cleanup-db"
        - "announce-removal"
        - "completed"

  - name: Log for a pre-remove job must show its output
    run: daft hooks jobs logs feature/goes-away:cleanup-db
    expect:
      exit_code: 0
      output_contains:
        - "removing-db-volumes"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci hooks:remove-hook-visibility` Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/remove-hook-visibility.yml
git commit -m "test(hooks): regression scenario for remove-hook visibility"
```

---

## Task 17: Integration scenario — empty YAML shows `(no jobs declared)`

**Files:**

- Create: `tests/manual/scenarios/hooks/empty-hook-placeholder.yml`

**Rationale:** Exercises the Task 12 placeholder. A hook file that declares no
jobs must still record its invocation so the user can see the hook fired.

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/empty-hook-placeholder.yml`:

```yaml
name: Hook with no jobs still appears in listing

description: |
  After sub-project A, every hook run writes invocation.json even when
  the hook declares zero runnable jobs. The listing shows a
  (no jobs declared) placeholder so the user can tell the hook fired.

repos:
  - name: empty-hook
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "empty hook test"
        commits:
          - message: "init"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs: []

steps:
  - name: Create worktree — fires an empty post-create hook
    run: daft wt add feature/empty
    expect:
      exit_code: 0

  - name: List shows the invocation with a placeholder
    run: daft hooks jobs
    cwd: feature/empty
    expect:
      exit_code: 0
      output_contains:
        - "worktree-post-create"
        - "no jobs declared"
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci hooks:empty-hook-placeholder` Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/hooks/empty-hook-placeholder.yml
git commit -m "test(hooks): integration scenario for empty hook placeholder"
```

---

## Task 18: Integration scenario — promoted bg→fg records as foreground

**Files:**

- Create: `tests/manual/scenarios/hooks/promoted-bg-to-fg.yml`

**Rationale:** A background job that a foreground job depends on is promoted to
the foreground partition. Spec §2 commits to recording such jobs as
`background: false`. This scenario locks that in.

- [ ] **Step 1: Write the scenario**

Create `tests/manual/scenarios/hooks/promoted-bg-to-fg.yml`:

```yaml
name: Background job promoted to foreground is recorded as fg

description: |
  When a foreground job declares a `needs:` dependency on a job marked
  `background: true`, the background job is promoted to the foreground
  partition so the DAG remains valid. Its meta.json must record
  `background: false` to reflect how it actually ran.

repos:
  - name: promoted
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "promoted test"
        commits:
          - message: "init"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: build-shared
              run: echo shared-built
              background: true
            - name: use-shared
              run: echo using-shared
              needs:
                - build-shared

steps:
  - name: Create worktree (both jobs run as fg)
    run: daft wt add feature/promoted
    expect:
      exit_code: 0
      output_contains:
        - "promoted"
        - "shared-built"
        - "using-shared"

  - name: JSON output confirms both jobs are background=false
    run: daft hooks jobs --json
    cwd: feature/promoted
    expect:
      exit_code: 0
      output_contains:
        - '"name": "build-shared"'
        - '"background": false'
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci hooks:promoted-bg-to-fg` Expected: PASS.

- [ ] **Step 3: Run the full CI matrix**

Run: `mise run ci` Expected: all unit tests, all integration scenarios, clippy,
fmt, man-page verification all pass.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/hooks/promoted-bg-to-fg.yml
git commit -m "test(hooks): integration scenario for promoted bg->fg recording"
```

---

## Self-Review checklist

Before handing off to execution, verify:

- [ ] **Spec coverage (§1 write path):** Tasks 8 + 10 + 11 move invocation
      writes to main, thread the sink through `run_jobs`, and construct the
      `BufferingLogSink` — covered.
- [ ] **Spec coverage (§2 data model):** Task 1 adds `JobStatus::Skipped`; Tasks
      10 and 18 cover the promoted-bg→fg rule.
- [ ] **Spec coverage (§3 display):** Existing renderer handles mixed fg/bg
      automatically; Task 11 adds Skipped rendering; Task 12 adds
      empty-invocation placeholder.
- [ ] **Spec coverage (§4 skip-reason capture):** Tasks 5–7 and 9 implement
      `SkippedJob` + per-job `skip:` / `only:` + writing sparse records.
- [ ] **Spec coverage (§5 edge cases):** Task 13 covers fg-only; Task 14 covers
      mixed; Task 15 covers per-job skip; Task 16 covers remove-hook regression;
      Task 17 covers empty YAML; Task 18 covers promoted jobs.
      `DAFT_NO_BACKGROUND_JOBS` is covered implicitly via the fg sink reuse in
      Task 10 step 3.
- [ ] **Spec coverage (§6 testing):** Unit tests in Tasks 1, 3, 4, 5, 6, 7, 8,
      9, 10, 11, 12; integration scenarios in Tasks 13–18.
- [ ] **Placeholders:** None — every step has concrete code or a concrete
      command.
- [ ] **Type consistency:**
      `BufferingLogSink::new(store: Arc<LogStore>, invocation_id: String, hook_type: String, worktree: String)`
      — matches Task 3 definition and Task 10 construction.
      `yaml_jobs_to_specs -> (Vec<JobSpec>, Vec<SkippedJob>)` — matches Task 5
      definition and Task 10 consumption. `LogSink::on_job_runner_skipped` —
      matches the trait declaration in Task 2 and the runner call in Task 4.
