//! Coordinator process for running background hook jobs.
//!
//! When a hook declares any background jobs, the parent daft process spawns
//! a fresh `daft __coordinator <state-file>` child via `spawn_coordinator`,
//! prints a summary, and returns. The child reads + unlinks the state file,
//! detaches via `setsid()`, runs the background jobs as threads, and exits
//! when done.
//!
//! A Unix socket listener runs in a separate thread alongside job execution,
//! handling IPC requests from CLI commands (`daft hooks jobs`).

use super::adapters::SqliteJobsStore;
use super::log_record::{LogRecord, OutputKind, StatusEvent, record_from, write_log_record};
use super::log_store::{JobMeta, JobStatus, LogStore};
use super::ports::JobsStorePort;
#[cfg(unix)]
use super::{
    CoordinatorRequest, CoordinatorResponse, ErrorCode, JobInfo, PROTOCOL_VERSION, RequestEnvelope,
    ResponseEnvelope, coordinator_pid_path, coordinator_socket_path, framing, types::JobAddress,
};
use crate::executor::command::run_command;
use crate::executor::dag::DagGraph;
use crate::executor::{JobResult, JobSpec, NodeStatus};
use crate::store::models::JobRow;
#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Reconcile `Running`/`Cancelling` rows that this repo still holds by
/// probing each row's recorded PGID. Thin wrapper around
/// [`crate::coordinator::domain::reconcile_active_jobs`] that constructs
/// the concrete `SystemClock` + `UnixProcess` adapters; the actual logic
/// lives in the domain layer so it can be unit-tested with fakes.
///
/// Best-effort: errors writing the updated row are reported via stderr by
/// the domain function and don't abort the boot.
#[cfg(unix)]
fn reconcile_active_jobs(store: &SqliteJobsStore, repo_hash: &str) -> Result<()> {
    use crate::coordinator::adapters::{SystemClock, UnixProcess};
    crate::coordinator::domain::reconcile_active_jobs(store, &UnixProcess, &SystemClock, repo_hash)
        .map(|_| ())
}

#[cfg(not(unix))]
fn reconcile_active_jobs(_store: &SqliteJobsStore, _repo_hash: &str) -> Result<()> {
    // Non-Unix coordinators don't spawn background jobs (see yaml_executor
    // fallback). Nothing to reconcile.
    Ok(())
}

/// Shared state for tracking running child processes.
/// Maps job name to the child process PID, allowing cancellation.
type ChildPidMap = Arc<Mutex<HashMap<String, u32>>>;

/// Shared set of job names that have been cancelled individually
/// (as opposed to a global `cancel_all`). Consulted by the post-run
/// status classifier so per-job cancel records `JobStatus::Cancelled`
/// instead of `JobStatus::Failed`.
type CancelledJobs = Arc<Mutex<HashSet<String>>>;

/// State for a coordinator process managing background jobs.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CoordinatorState {
    pub repo_hash: String,
    pub invocation_id: String,
    pub jobs: Vec<JobSpec>,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    /// Names of background jobs that must be recorded as `DepFailed`
    /// before the wave loop starts. Populated by the caller from the
    /// foreground `JobResult`s: a BG job whose original (pre-partition)
    /// `needs:` referenced a foreground job that did not succeed lands
    /// here so it is not run, and its dependents cascade through the
    /// same `DepFailed` path that handles runtime failures.
    ///
    /// `#[serde(default)]` is load-bearing: older parents that haven't
    /// learned this field serialize a payload without it, and a freshly
    /// spawned coordinator deserializes them as an empty list (i.e. no
    /// prefailed jobs).
    #[serde(default)]
    pub prefailed_jobs: Vec<String>,
}

/// Wire format for state passed from parent to spawned `__coordinator`
/// child via a serde-JSON tempfile (see `spawn_coordinator` /
/// `run_coordinator`). Bundles `LogStore.base_dir` because the spawned
/// child does not share the parent's address space.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct CoordinatorPayload {
    pub state: CoordinatorState,
    pub log_store_base: std::path::PathBuf,
}

impl CoordinatorState {
    pub fn new(repo_hash: &str, invocation_id: &str) -> Self {
        Self {
            repo_hash: repo_hash.to_string(),
            invocation_id: invocation_id.to_string(),
            jobs: Vec::new(),
            trigger_command: String::new(),
            hook_type: String::new(),
            worktree: String::new(),
            prefailed_jobs: Vec::new(),
        }
    }

    pub fn with_metadata(mut self, trigger_command: &str, hook_type: &str, worktree: &str) -> Self {
        self.trigger_command = trigger_command.to_string();
        self.hook_type = hook_type.to_string();
        self.worktree = worktree.to_string();
        self
    }

    /// Set the list of background-job names that should be recorded as
    /// `DepFailed` before the wave loop starts (see [`Self::prefailed_jobs`]).
    pub fn with_prefailed(mut self, names: Vec<String>) -> Self {
        self.prefailed_jobs = names;
        self
    }

    pub fn add_job(&mut self, job: JobSpec) {
        self.jobs.push(job);
    }

    /// Run all background jobs, writing logs and metadata to the store.
    /// Jobs run as threads within this process.
    pub fn run_all(&self, store: &LogStore) -> Result<Vec<JobResult>> {
        self.run_all_with_cancel(
            store,
            &ChildPidMap::default(),
            &Arc::new(AtomicBool::new(false)),
            &CancelledJobs::default(),
            None,
        )
    }

    /// Run all background jobs with cancellation support.
    ///
    /// `child_pids` is shared with the socket listener so it can send
    /// SIGTERM to running child processes. `cancel_all` is a global flag
    /// that, when set, causes new jobs to skip and running jobs to be killed.
    /// `cancelled_jobs` is a per-job cancellation set: when a job's name
    /// appears here, the post-run classifier records it as
    /// `JobStatus::Cancelled` rather than `JobStatus::Failed`.
    /// `job_store`, when present, receives a `JobRow` dual-write at job
    /// start and completion — enables crash-recovery by the next coordinator
    /// invocation and richer cancel filtering (#476 Tier-1).
    fn run_all_with_cancel(
        &self,
        store: &LogStore,
        child_pids: &ChildPidMap,
        cancel_all: &Arc<AtomicBool>,
        cancelled_jobs: &CancelledJobs,
        job_store: Option<&SqliteJobsStore>,
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
        let graph =
            DagGraph::new(nodes).map_err(|e| anyhow::anyhow!("invalid background job DAG: {e}"))?;

        let results = Arc::new(Mutex::new(Vec::<JobResult>::new()));

        // Wave-based scheduler. DagGraph is used for cycle/missing-dep
        // detection and dependents lookup, but each wave is executed with
        // bare `std::thread::spawn` rather than `DagGraph::run_parallel`.
        // Reason: every job in a wave must reach a terminal state before
        // any dependent's wave starts — that's the contract `run_all_with_cancel`
        // exposes to callers (see `next_ready` computation below). A
        // free-running scheduler would advance dependents the moment their
        // own predecessors finish, which violates this barrier and would
        // race with the cancel/skip cascade that fans out from per-wave
        // outcomes. The wave shape costs concurrency in heterogeneous DAGs
        // but is the simplest expression of "outcomes settle before the
        // next layer dispatches"; the previous comment cited a fork-era
        // malloc-arena constraint, which is moot post-spawn.
        let n = graph.len();
        let mut statuses = vec![NodeStatus::Pending; n];
        let in_degree: Vec<usize> = (0..n).map(|i| graph.dependencies_of(i).len()).collect();

        // Apply prefailed cascade before computing the initial ready set.
        // A name in `prefailed_jobs` is treated identically to a runtime
        // failure in this DAG: the node itself is marked `DepFailed` (the
        // wave loop and the post-loop synthesis loop both already handle
        // that variant) and the same transitive cascade used inside the
        // wave loop fans out to all Pending dependents. The names come
        // from the caller's view of foreground outcomes; missing names
        // are ignored (a stale list from an upgrade racing with a config
        // edit shouldn't crash the coordinator).
        if !self.prefailed_jobs.is_empty() {
            let prefailed: HashSet<&str> = self.prefailed_jobs.iter().map(String::as_str).collect();
            for (idx, job) in self.jobs.iter().enumerate() {
                if !prefailed.contains(job.name.as_str()) {
                    continue;
                }
                statuses[idx] = NodeStatus::DepFailed;
                let mut stack = vec![idx];
                while let Some(i) = stack.pop() {
                    for &d in graph.dependents_of(i) {
                        if statuses[d] == NodeStatus::Pending {
                            statuses[d] = NodeStatus::DepFailed;
                            stack.push(d);
                        }
                    }
                }
            }
        }

        let mut in_degree = in_degree;
        let mut ready: Vec<usize> = (0..n)
            .filter(|&i| statuses[i] == NodeStatus::Pending && in_degree[i] == 0)
            .collect();

        // Each iteration of this loop is one wave: every ready node is
        // spawned in parallel and the loop blocks until ALL of them finish
        // before computing the next wave. Less concurrent than a free-running
        // scheduler (a fast independent chain can be held up behind a slow
        // sibling in the same wave), but the dependency-cascade and cancel
        // bookkeeping above relies on per-wave settlement — see the
        // top-of-function comment.
        while !ready.is_empty() {
            // Spawn one worker per ready node. Inputs are cloned per-spawn,
            // matching the pre-fix per-thread cloning pattern.
            let mut handles: Vec<(usize, std::thread::JoinHandle<NodeStatus>)> = Vec::new();
            for &idx in &ready {
                statuses[idx] = NodeStatus::Running;

                let job = self.jobs[idx].clone();
                let inv_id = self.invocation_id.clone();
                let repo_hash = self.repo_hash.clone();
                let store_base = store.base_dir.clone();
                let hook_type = self.hook_type.clone();
                let worktree = self.worktree.clone();
                let results_clone = Arc::clone(&results);
                let child_pids_clone = Arc::clone(child_pids);
                let cancel_all_clone = Arc::clone(cancel_all);
                let cancelled_jobs_clone = Arc::clone(cancelled_jobs);
                let job_store_clone = job_store.cloned();

                let handle = std::thread::spawn(move || {
                    let local_store = LogStore::new(store_base);
                    let ctx = JobInvocationContext {
                        repo_hash: &repo_hash,
                        invocation_id: &inv_id,
                        hook_type: &hook_type,
                        worktree: &worktree,
                        job_store: job_store_clone,
                        results: results_clone,
                        child_pids: child_pids_clone,
                        cancel_all: cancel_all_clone,
                        cancelled_jobs: cancelled_jobs_clone,
                    };
                    run_single_background_job(&job, &ctx, &local_store)
                });
                handles.push((idx, handle));
            }

            // Wait for the wave to finish. A panicked worker is treated as a
            // Failed outcome so the cascade still fires.
            let wave_outcomes: Vec<(usize, NodeStatus)> = handles
                .into_iter()
                .map(|(idx, handle)| (idx, handle.join().unwrap_or(NodeStatus::Failed)))
                .collect();

            // Apply outcomes and compute the next wave.
            let mut next_ready: Vec<usize> = Vec::new();
            for (idx, status) in wave_outcomes {
                statuses[idx] = status;
                match status {
                    // `Skipped` is treated like `Succeeded` for cascade
                    // purposes — both unblock dependents. Today
                    // `run_single_background_job` only ever returns
                    // `Succeeded` or `Failed`, so the `Skipped` arm is
                    // reserved for future closure variants (e.g. an `if:`
                    // conditional skip path).
                    NodeStatus::Succeeded | NodeStatus::Skipped => {
                        for &dep_idx in graph.dependents_of(idx) {
                            if statuses[dep_idx] == NodeStatus::Pending {
                                in_degree[dep_idx] -= 1;
                                if in_degree[dep_idx] == 0 {
                                    next_ready.push(dep_idx);
                                }
                            }
                        }
                    }
                    NodeStatus::Failed => {
                        // Cascade DepFailed to all transitive Pending dependents.
                        let mut stack = vec![idx];
                        while let Some(i) = stack.pop() {
                            for &d in graph.dependents_of(i) {
                                if statuses[d] == NodeStatus::Pending {
                                    statuses[d] = NodeStatus::DepFailed;
                                    stack.push(d);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            ready = next_ready;
        }

        // Synthesize a row + JobResult for jobs the scheduler marked
        // DepFailed (their closure was not invoked, so they would otherwise
        // be invisible to `daft hooks jobs`).
        for (idx, status) in statuses.iter().enumerate() {
            if *status != NodeStatus::DepFailed {
                continue;
            }
            let Some(job) = self.jobs.get(idx) else {
                continue;
            };
            let meta = JobMeta::skipped(
                &job.name,
                &self.hook_type,
                &self.worktree,
                &job.command,
                job.background,
                job.needs.clone(),
            );
            // Create the dir so the writer thread has somewhere to drop
            // an `output.jsonl` next to the terminal-status record.
            // Metadata flows to SQLite via `JobsStorePort::upsert_job`
            // below — no `meta.json` is written.
            if store
                .create_job_dir(&self.invocation_id, &job.name)
                .is_err()
            {
                eprintln!(
                    "daft: failed to create dep-failed log dir for '{}'",
                    job.name
                );
            }
            if let Some(js) = job_store
                && let Err(e) = js.upsert_job(&job_row_from_meta(
                    &meta,
                    &self.repo_hash,
                    &self.invocation_id,
                    &job.tags,
                    None,
                ))
            {
                eprintln!("daft: failed to persist dep-failed job '{}': {e}", job.name);
            }
            results.lock().unwrap().push(JobResult {
                name: job.name.clone(),
                status: NodeStatus::DepFailed,
                duration: std::time::Duration::ZERO,
                exit_code: None,
                stdout: String::new(),
                stderr: "Dependency failed; job did not run".to_string(),
            });
        }

        let results = match Arc::try_unwrap(results) {
            Ok(mutex) => mutex.into_inner().unwrap_or_default(),
            Err(arc) => arc.lock().unwrap().clone(),
        };

        Ok(results)
    }
}

/// Per-invocation context bundle threaded into every background job spawn.
///
/// Replaces a multi-arg signature (#476 Tier-3 API surface). Holds the
/// invocation identifiers as borrowed references and the per-invocation
/// shared state as `Arc` handles so each spawn thread can keep its own
/// reference alive without re-cloning at every call site.
struct JobInvocationContext<'a> {
    repo_hash: &'a str,
    invocation_id: &'a str,
    hook_type: &'a str,
    worktree: &'a str,
    /// Optional dual-write target for the SQLite-backed `JobRow` lifecycle
    /// records. `None` from test paths that don't exercise persistence.
    job_store: Option<SqliteJobsStore>,
    /// Per-invocation completion records appended by every job as it
    /// terminates. Read at end of `run_all_with_cancel`.
    results: Arc<Mutex<Vec<JobResult>>>,
    /// Map of `job_name -> shell PID` for inflight jobs. Used to fan out
    /// SIGTERM via `killpg` when a cancel arrives over IPC.
    child_pids: ChildPidMap,
    /// Hot signal: when any job sets this, subsequent jobs short-circuit
    /// with `Skipped`/`Cancelled` instead of starting.
    cancel_all: Arc<AtomicBool>,
    /// Set of job names already targeted by a cancel — used to label the
    /// per-job terminal status correctly (Cancelled vs Failed).
    cancelled_jobs: CancelledJobs,
}

/// Project a `JobMeta` into a SQLite `JobRow`. `repo_hash`/`invocation_id`/
/// `tags` come from outside `JobMeta` (the coordinator's invocation context),
/// and `child_pid` is the spawned shell's PID — equal to its PGID because
/// `run_command` calls `process_group(0)`.
fn job_row_from_meta(
    meta: &JobMeta,
    repo_hash: &str,
    invocation_id: &str,
    tags: &[String],
    child_pid: Option<u32>,
) -> JobRow {
    JobRow {
        repo_hash: repo_hash.to_string(),
        invocation_id: invocation_id.to_string(),
        name: meta.name.clone(),
        hook_type: meta.hook_type.clone(),
        worktree: meta.worktree.clone(),
        command: meta.command.clone(),
        working_dir: meta.working_dir.clone(),
        env: meta.env.clone(),
        started_at: meta.started_at,
        finished_at: meta.finished_at,
        // SQLite `jobs.status` is TEXT. The serde rename keeps the wire
        // tag stable across enum ↔ string round-trips.
        status: meta.status.as_status_str().to_string(),
        exit_code: meta.exit_code,
        pid: child_pid,
        pgid: child_pid, // PID == PGID via process_group(0)
        background: meta.background,
        needs: meta.needs.clone(),
        tags: tags.to_vec(),
        retention_seconds: meta.retention_seconds,
        max_log_size_bytes: meta.max_log_size_bytes,
    }
}

/// Run a single background job: create log directory, write metadata,
/// stream output to a log file, execute the command, and update metadata
/// with the final status.
fn run_single_background_job(
    job: &JobSpec,
    ctx: &JobInvocationContext<'_>,
    store: &LogStore,
) -> NodeStatus {
    let results = &ctx.results;
    let child_pids = &ctx.child_pids;
    let cancel_all = &ctx.cancel_all;
    let cancelled_jobs = &ctx.cancelled_jobs;
    let start = Instant::now();

    let is_silent = matches!(
        job.background_output,
        Some(crate::executor::BackgroundOutput::Silent)
    );

    // Check if cancellation has been requested before even starting.
    if cancel_all.load(Ordering::Relaxed) {
        results.lock().unwrap().push(JobResult {
            name: job.name.clone(),
            status: NodeStatus::Skipped,
            duration: start.elapsed(),
            exit_code: None,
            stdout: String::new(),
            stderr: "Cancelled before start".to_string(),
        });
        // Note the deliberate split: the in-memory `JobResult.status` is
        // `Skipped` (the user cancelled), but the value returned to the
        // wave scheduler is `Failed` so dependents cascade to `DepFailed`.
        // Same rationale as the final-return mapping at the bottom of this
        // function — the dependency did not produce its work product.
        return NodeStatus::Failed;
    }

    // 1. Create the job log directory.
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

    // 2. Write initial meta with Running status.
    let retention_seconds = job
        .log_config
        .as_ref()
        .and_then(|lc| lc.retention.as_deref())
        .and_then(|s| crate::coordinator::clean_policy::parse_duration_str(s).ok())
        .map(|n| n as i64);
    let max_log_size_bytes = job
        .log_config
        .as_ref()
        .and_then(|lc| lc.max_log_size.as_deref())
        .and_then(|s| crate::coordinator::clean_policy::parse_size(s).ok());

    let mut meta = JobMeta {
        name: job.name.clone(),
        hook_type: ctx.hook_type.to_string(),
        worktree: ctx.worktree.to_string(),
        command: job.command.clone(),
        working_dir: job.working_dir.display().to_string(),
        env: job.env.clone(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Running,
        exit_code: None,
        // Child PID is unknown until `run_command` spawns; the terminal
        // `JobRow` write populates it via `child_pid`.
        pid: None,
        background: true,
        finished_at: None,
        needs: job.needs.clone(),
        retention_seconds,
        max_log_size_bytes,
    };
    // SQLite is the source of truth for job metadata. The initial
    // JobRow lands here; child PID/PGID aren't known yet — `run_command`
    // spawns the shell — so they're populated alongside the terminal
    // write below.
    if let Some(js) = ctx.job_store.as_ref()
        && let Err(e) = js.upsert_job(&job_row_from_meta(
            &meta,
            ctx.repo_hash,
            ctx.invocation_id,
            &job.tags,
            None,
        ))
    {
        eprintln!(
            "daft: failed to write initial JobRow for '{}': {e}",
            job.name
        );
    }

    // 3. Set up an mpsc channel for output streaming.
    let (tx, rx) = std::sync::mpsc::channel::<(OutputKind, String)>();

    // 4. Spawn a log writer thread that reads from the channel and live-
    //    appends structured `LogRecord` entries to `output.jsonl`. Sampling
    //    (`log_config.sampling_every_nth`) drops a record but always
    //    advances the `seq` counter so consumers can detect the gap.
    // The thread returns the next `seq` so the post-run path can emit a
    // terminal `Status::Finished` record with the correct sequence number.
    let jsonl_path = LogStore::jsonl_path(&job_dir);
    let jsonl_path_for_writer = jsonl_path.clone();
    let sampling_every_nth = job.log_config.as_ref().and_then(|lc| lc.sampling_every_nth);
    let log_writer_handle = std::thread::spawn(move || -> u64 {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path_for_writer);
        match file {
            Ok(mut f) => {
                let mut next_seq: u64 = 0;
                for (kind, line) in rx {
                    let seq = next_seq;
                    next_seq += 1;
                    if let Some(n) = sampling_every_nth
                        && n > 1
                        && !seq.is_multiple_of(n as u64)
                    {
                        continue;
                    }
                    let record = record_from(seq, kind, line);
                    let _ = write_log_record(&mut f, &record);
                }
                next_seq
            }
            Err(e) => {
                eprintln!("daft: failed to open log file: {e}");
                for _ in rx {}
                0
            }
        }
    });

    // 5. Set up a one-shot PID channel; register the spawned child's PID
    //    in `child_pids` so the socket listener can SIGTERM it on cancel.
    let (pid_tx, pid_rx) = std::sync::mpsc::channel::<u32>();
    let job_name_for_register = job.name.clone();
    let child_pids_for_register = child_pids.clone();
    let registrar = std::thread::spawn(move || {
        if let Ok(pid) = pid_rx.recv() {
            child_pids_for_register
                .lock()
                .unwrap()
                .insert(job_name_for_register, pid);
        }
    });

    // 6. Call run_command() to execute the shell command (now also
    //    forwarding `pid_tx` so the registrar above can pick up the PID).
    let cmd_result = run_command(
        &job.command,
        &job.env,
        &job.working_dir,
        job.timeout,
        Some(tx),
        Some(pid_tx),
    );

    // Wait for the registrar (if the child died before send, the channel
    // closes and recv() returns Err — registrar exits cleanly either way).
    let _ = registrar.join();

    // Remove from the child PID map and capture the spawned child's PID
    // so the terminal JobRow write can record it. `process_group(0)` makes
    // pid == pgid for every spawned hook job, so the same value is also
    // the PGID that `killpg` would target.
    let child_pid = child_pids.lock().unwrap().remove(&job.name);

    // Wait for the log writer thread to finish; recover the next seq for
    // the terminal Status record.
    let next_seq = log_writer_handle.join().unwrap_or(0);

    let duration = start.elapsed();

    // 7. Determine final status, considering both global and per-job
    //    cancellation. Either signal routes the job to Cancelled rather
    //    than Failed.
    let was_cancelled_globally = cancel_all.load(Ordering::Relaxed);
    let was_cancelled_per_job = cancelled_jobs.lock().unwrap().contains(&job.name);
    let was_cancelled = was_cancelled_globally || was_cancelled_per_job;

    // Clear the per-job cancellation entry (if any) so a re-invocation of
    // the same job name does not inherit a stale `Cancelled` flag.
    cancelled_jobs.lock().unwrap().remove(&job.name);

    let (status, node_status, exit_code) = if was_cancelled {
        (JobStatus::Cancelled, NodeStatus::Skipped, None)
    } else {
        match &cmd_result {
            Ok(cr) if cr.success => (JobStatus::Completed, NodeStatus::Succeeded, cr.exit_code),
            Ok(cr) => (JobStatus::Failed, NodeStatus::Failed, cr.exit_code),
            Err(_) => (JobStatus::Failed, NodeStatus::Failed, None),
        }
    };

    // Append the terminal `Status` record so JSONL readers see lifecycle
    // signals even when the writer thread had already exited.
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_path)
    {
        let event = if was_cancelled {
            StatusEvent::Signaled { signal: 15 } // SIGTERM
        } else {
            StatusEvent::Finished { exit_code }
        };
        let _ = write_log_record(&mut f, &LogRecord::status(next_seq, event));
    }

    // Silent mode: only retain the log file if the job did not succeed
    // (failed or cancelled). On success, the log is best-effort deleted.
    if is_silent && node_status == NodeStatus::Succeeded {
        let _ = std::fs::remove_file(&jsonl_path);
    }

    // 8. Persist the terminal JobRow to SQLite with the captured child
    // PID/PGID. SQLite is the source of truth for job metadata.
    meta.status = status.clone();
    meta.exit_code = exit_code;
    meta.finished_at = Some(chrono::Utc::now());
    if let Some(js) = ctx.job_store.as_ref()
        && let Err(e) = js.upsert_job(&job_row_from_meta(
            &meta,
            ctx.repo_hash,
            ctx.invocation_id,
            &job.tags,
            child_pid,
        ))
    {
        eprintln!(
            "daft: failed to write terminal JobRow for '{}': {e}",
            job.name
        );
    }

    // 9. On failure, print a one-line notification to stderr (best-effort,
    //    catches EPIPE). Suppressed for silent background_output, and under
    //    cfg!(test) so the banner doesn't bleed into unit-test logs.
    if node_status == NodeStatus::Failed && !is_silent && !cfg!(test) {
        let msg = match &cmd_result {
            Ok(cr) => format!(
                "daft: background job '{}' failed (exit code: {})",
                job.name,
                cr.exit_code
                    .map_or("unknown".to_string(), |c| c.to_string())
            ),
            Err(e) => format!("daft: background job '{}' failed: {e}", job.name),
        };
        // Best-effort write to stderr; ignore EPIPE if the parent has closed
        // its end of the pipe.
        let _ = writeln!(std::io::stderr(), "{msg}");
    }

    // 10. Push the JobResult to the shared results vec.
    let (stdout, stderr) = match cmd_result {
        Ok(cr) => (cr.stdout, cr.stderr),
        Err(_) => (String::new(), String::new()),
    };

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

/// Start a Unix socket listener that handles IPC requests from CLI clients.
///
/// The listener runs in a separate thread and processes requests until:
/// - A `Shutdown` request is received
/// - The `shutdown` flag is set (e.g., when all jobs complete)
///
/// Returns a `JoinHandle` for the listener thread.
#[cfg(unix)]
fn start_socket_listener(
    repo_hash: &str,
    store_base: std::path::PathBuf,
    child_pids: ChildPidMap,
    cancel_all: Arc<AtomicBool>,
    cancelled_jobs: CancelledJobs,
    shutdown: Arc<AtomicBool>,
    job_store: Option<SqliteJobsStore>,
) -> Result<(std::thread::JoinHandle<()>, std::path::PathBuf)> {
    let socket_path = coordinator_socket_path(repo_hash)?;

    // Remove stale socket if it exists.
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).ok();
    }

    let listener = std::os::unix::net::UnixListener::bind(&socket_path)
        .map_err(|e| anyhow::anyhow!("Failed to bind socket at {}: {e}", socket_path.display()))?;

    // Set to non-blocking so we can check the shutdown flag periodically.
    listener.set_nonblocking(true)?;

    let socket_path_clone = socket_path.clone();
    let repo_hash = repo_hash.to_string();
    let handle = std::thread::spawn(move || {
        let store = LogStore::new(store_base);
        loop {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }

            match listener.accept() {
                Ok((stream, _)) => {
                    handle_client_connection(
                        stream,
                        &store,
                        &child_pids,
                        &cancel_all,
                        &cancelled_jobs,
                        &shutdown,
                        job_store.as_ref(),
                        &repo_hash,
                    );
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No pending connection; sleep briefly and retry.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => {
                    // Unexpected error (FD exhaustion, protocol error,
                    // etc.). From this point on every IPC client sees
                    // "connection refused" with no breadcrumb in the
                    // coordinator output — surface the underlying error
                    // before exiting so the failure is diagnosable.
                    eprintln!("daft coordinator: socket listener error: {e}");
                    break;
                }
            }
        }

        // Clean up the socket file.
        std::fs::remove_file(&socket_path_clone).ok();
    });

    Ok((handle, socket_path))
}

/// Handle a single client connection: read a request, process it, send a response.
#[cfg(unix)]
#[allow(clippy::too_many_arguments)]
fn handle_client_connection(
    mut stream: std::os::unix::net::UnixStream,
    store: &LogStore,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
    shutdown: &Arc<AtomicBool>,
    job_store: Option<&SqliteJobsStore>,
    repo_hash: &str,
) {
    // On macOS (and other BSDs) `accept(2)` returns a socket that inherits
    // O_NONBLOCK from the listening socket. `start_socket_listener` puts the
    // listener in non-blocking mode so the accept loop can poll the shutdown
    // flag, which means every accepted stream lands here non-blocking on
    // macOS. `read_exact` against a non-blocking socket returns `WouldBlock`
    // before any data arrives and the request silently drops. Force the
    // accepted stream back to blocking so `set_read_timeout` is the only
    // thing bounding reads.
    let _ = stream.set_nonblocking(false);

    // Set a read timeout so a misbehaving client doesn't block the listener.
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();

    let bytes = match framing::read_frame(&mut stream) {
        Ok(b) => b,
        Err(_) => return,
    };
    let env: RequestEnvelope = match serde_json::from_slice(&bytes) {
        Ok(e) => e,
        Err(_) => {
            send_response(
                &stream,
                &CoordinatorResponse::Error {
                    code: ErrorCode::Internal,
                    message: "Invalid request envelope".into(),
                },
            );
            return;
        }
    };
    if env.v != PROTOCOL_VERSION {
        send_response(
            &stream,
            &CoordinatorResponse::Error {
                code: ErrorCode::SchemaMismatch,
                message: format!(
                    "client wire-version {} not supported (server expects {})",
                    env.v, PROTOCOL_VERSION
                ),
            },
        );
        return;
    }
    let request = env.body;

    // `TailLogs` is the only streaming variant today — handle it inline so
    // it can emit multiple `StreamFrame` envelopes before `StreamEnd`.
    if let CoordinatorRequest::TailLogs {
        job,
        follow,
        since_seq,
    } = &request
    {
        // `follow=true` blocks indefinitely; relax the connection's read
        // timeout so the client side staying open isn't read as misbehavior.
        let _ = stream.set_read_timeout(None);
        let _ = stream.set_write_timeout(None);
        handle_tail_logs(&stream, store, job, *follow, *since_seq);
        return;
    }

    let response = match request {
        CoordinatorRequest::ListJobs => {
            let jobs = build_job_list(job_store, repo_hash);
            CoordinatorResponse::Jobs(jobs)
        }
        CoordinatorRequest::CancelJob { name } => {
            cancel_single_job(&name, child_pids, cancelled_jobs, store)
        }
        CoordinatorRequest::CancelAll => {
            cancel_all.store(true, Ordering::Relaxed);
            let pids: Vec<(String, u32)> = child_pids
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();

            let mut count = 0;
            for (name, pid) in &pids {
                // Bg children are process-group leaders (PID == PGID via
                // setpgid). killpg reaches every descendant — e.g. a `sleep`
                // grandchild of the wrapping `sh`.
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(*pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
                count += 1;
                let _ = name; // used for counting
            }

            CoordinatorResponse::Ack {
                message: format!("Cancelled {count} job(s)"),
            }
        }
        CoordinatorRequest::Shutdown => {
            shutdown.store(true, Ordering::Relaxed);
            cancel_all.store(true, Ordering::Relaxed);

            // Kill all running children (via their process groups).
            let pids: Vec<u32> = child_pids.lock().unwrap().values().copied().collect();
            for pid in pids {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid as i32),
                    nix::sys::signal::Signal::SIGTERM,
                );
            }

            CoordinatorResponse::Ack {
                message: "Coordinator shutting down".to_string(),
            }
        }
        CoordinatorRequest::CancelMatching {
            hook,
            worktree,
            tag,
            invocation_prefix,
            older_than_secs,
        } => cancel_matching(
            CancelMatchingArgs {
                hook: hook.as_deref(),
                worktree: worktree.as_deref(),
                tag: tag.as_deref(),
                invocation_prefix: invocation_prefix.as_deref(),
                older_than_secs,
            },
            child_pids,
            cancelled_jobs,
            job_store,
            repo_hash,
        ),
        CoordinatorRequest::TailLogs { .. } => {
            // Handled above (short-circuit at the top of this function);
            // present here only so the match is exhaustive.
            unreachable!("TailLogs is dispatched before the match")
        }
    };

    send_response(&stream, &response);
}

/// Build the list of jobs the coordinator's `ListJobs` IPC returns.
/// Sources every field from SQLite — the meta.json filesystem scan is
/// gone now that the store is the single source of truth.
#[cfg(unix)]
fn build_job_list(job_store: Option<&SqliteJobsStore>, repo_hash: &str) -> Vec<JobInfo> {
    let Some(js) = job_store else {
        return Vec::new();
    };
    let rows = match js.list_jobs_for_repo(repo_hash) {
        Ok(rs) => rs,
        Err(_) => return Vec::new(),
    };
    let now = chrono::Utc::now();
    rows.into_iter()
        .map(|row| {
            let status = JobStatus::from_status_str(&row.status);
            let elapsed_secs = if matches!(status, JobStatus::Running) {
                Some(
                    now.signed_duration_since(row.started_at)
                        .num_seconds()
                        .max(0) as u64,
                )
            } else {
                None
            };
            JobInfo {
                name: row.name,
                hook_type: row.hook_type,
                worktree: row.worktree,
                status,
                elapsed_secs,
                exit_code: row.exit_code,
            }
        })
        .collect()
}

/// Cancel a single job by name. Records the name in `cancelled_jobs` so
/// the post-run classifier reports `JobStatus::Cancelled` rather than
/// `JobStatus::Failed`.
#[cfg(unix)]
fn cancel_single_job(
    name: &str,
    child_pids: &ChildPidMap,
    cancelled_jobs: &CancelledJobs,
    _store: &LogStore,
) -> CoordinatorResponse {
    let pids = child_pids.lock().unwrap();
    if let Some(&pid) = pids.get(name) {
        cancelled_jobs.lock().unwrap().insert(name.to_string());
        // The bg child is a process-group leader (PID == PGID via setpgid),
        // so killpg reaches every descendant.
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid as i32),
            nix::sys::signal::Signal::SIGTERM,
        );
        CoordinatorResponse::Ack {
            message: format!("Cancelled job: {name}"),
        }
    } else {
        CoordinatorResponse::Error {
            code: ErrorCode::JobNotFound,
            message: format!("Job not found or not running: {name}"),
        }
    }
}

/// Predicates for `CancelMatching`. Grouped into a struct so the helper
/// signature stays under clippy's `too_many_arguments` lint without an
/// allow attribute.
struct CancelMatchingArgs<'a> {
    hook: Option<&'a str>,
    worktree: Option<&'a str>,
    tag: Option<&'a str>,
    invocation_prefix: Option<&'a str>,
    older_than_secs: Option<u64>,
}

/// Pure filter for `CancelMatching`: returns rows whose fields satisfy
/// every supplied predicate. `now` is parameterized so unit tests can
/// pin time and exercise `older_than_secs` deterministically.
fn filter_matching_jobs(
    rows: Vec<JobRow>,
    args: &CancelMatchingArgs<'_>,
    now: chrono::DateTime<chrono::Utc>,
) -> Vec<JobRow> {
    rows.into_iter()
        .filter(|r| args.hook.is_none_or(|h| r.hook_type == h))
        .filter(|r| args.worktree.is_none_or(|w| r.worktree == w))
        .filter(|r| args.tag.is_none_or(|t| r.tags.iter().any(|x| x == t)))
        .filter(|r| {
            args.invocation_prefix
                .is_none_or(|p| r.invocation_id.starts_with(p))
        })
        .filter(|r| {
            args.older_than_secs.is_none_or(|secs| {
                let elapsed = now.signed_duration_since(r.started_at).num_seconds();
                elapsed >= 0 && (elapsed as u64) >= secs
            })
        })
        .collect()
}

/// Cancel every active job whose SQLite `JobRow` matches *all* supplied
/// predicates (AND, missing-is-wildcard). Looks up the runtime PID via
/// `child_pids` so we signal exactly the process group that's still alive.
///
/// Rows the coordinator never wrote (e.g. legacy pre-store invocations)
/// can't be filtered by the SQL schema, so they fall through unmatched —
/// the existing per-name `CancelJob` path remains for those.
#[cfg(unix)]
fn cancel_matching(
    args: CancelMatchingArgs<'_>,
    child_pids: &ChildPidMap,
    cancelled_jobs: &CancelledJobs,
    job_store: Option<&SqliteJobsStore>,
    repo_hash: &str,
) -> CoordinatorResponse {
    let Some(js) = job_store else {
        return CoordinatorResponse::Error {
            code: ErrorCode::Internal,
            message: "CancelMatching requires the SQLite job store; not available on this build"
                .into(),
        };
    };

    let active = match js.list_active_jobs(repo_hash) {
        Ok(rows) => rows,
        Err(e) => {
            return CoordinatorResponse::Error {
                code: ErrorCode::Internal,
                message: format!("Failed to list active jobs: {e}"),
            };
        }
    };

    let matched = filter_matching_jobs(active, &args, chrono::Utc::now());

    // Best-effort signal each match. We use `child_pids` (the in-memory
    // PID map this coordinator maintains for its own jobs) — rows whose
    // PID isn't in this map belong to a different coordinator and we
    // shouldn't touch them.
    let pids_snapshot = {
        let g = child_pids.lock().unwrap();
        g.clone()
    };
    let mut names: Vec<String> = Vec::new();
    for row in &matched {
        if let Some(&pid) = pids_snapshot.get(&row.name) {
            cancelled_jobs.lock().unwrap().insert(row.name.clone());
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGTERM,
            );
            names.push(row.name.clone());
        }
    }

    CoordinatorResponse::Cancelled {
        count: names.len(),
        names,
    }
}

/// Send one [`ResponseEnvelope`] as a length-prefixed frame.
///
/// Serialization of a well-formed `CoordinatorResponse` shouldn't fail
/// in practice, but if it does we emit a sentinel `Error{Internal}` frame
/// so the client's blocking `read_frame` doesn't hang waiting for a
/// response that will never arrive.
#[cfg(unix)]
fn send_response(stream: &std::os::unix::net::UnixStream, response: &CoordinatorResponse) {
    let env = ResponseEnvelope {
        v: PROTOCOL_VERSION,
        body: response.clone(),
    };
    let bytes = match serde_json::to_vec(&env) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("daft coordinator: response serialization failed: {e}");
            let fallback = ResponseEnvelope {
                v: PROTOCOL_VERSION,
                body: CoordinatorResponse::Error {
                    code: ErrorCode::Internal,
                    message: format!("response serialization failed: {e}"),
                },
            };
            match serde_json::to_vec(&fallback) {
                Ok(b) => b,
                Err(_) => return,
            }
        }
    };
    let mut writer = stream;
    let _ = framing::write_frame(&mut writer, &bytes);
}

/// Stream a job's `output.jsonl` to the client as a sequence of
/// `StreamFrame` envelopes terminated by a single `StreamEnd`.
///
/// Per-invocation lifecycle means "this coordinator is alive only while
/// jobs are running" — `follow=true` blocks reading the JSONL file until
/// either EOF stays empty for a beat or the connection drops. The
/// foreground client's "coordinator unreachable → file done" fallback
/// (see `commands::hooks::jobs::show_logs`) covers the post-coordinator
/// state.
#[cfg(unix)]
fn handle_tail_logs(
    stream: &std::os::unix::net::UnixStream,
    log_store: &LogStore,
    job: &JobAddress,
    follow: bool,
    since_seq: Option<u64>,
) {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let job_dir = match resolve_tail_job_dir(log_store, job) {
        Ok(dir) => dir,
        Err(e) => {
            send_response(
                stream,
                &CoordinatorResponse::Error {
                    code: ErrorCode::JobNotFound,
                    message: format!("{e}"),
                },
            );
            send_response(stream, &CoordinatorResponse::StreamEnd);
            return;
        }
    };
    let jsonl = LogStore::jsonl_path(&job_dir);

    // Open. If the file doesn't exist yet (job hasn't produced output),
    // and we're following, wait briefly for it to appear.
    let mut file = loop {
        match std::fs::OpenOptions::new().read(true).open(&jsonl) {
            Ok(f) => break f,
            Err(_) if follow => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            Err(_) => {
                send_response(stream, &CoordinatorResponse::StreamEnd);
                return;
            }
        }
    };

    let mut reader = BufReader::new(&file);
    // Walk through existing records first.
    let mut line = String::new();
    let send = |record: &LogRecord| -> bool {
        let value = match serde_json::to_value(record) {
            Ok(v) => v,
            Err(_) => return true,
        };
        send_response(stream, &CoordinatorResponse::StreamFrame(value));
        true
    };

    loop {
        line.clear();
        let n = match reader.read_line(&mut line) {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            if !follow {
                break;
            }
            // EOF while following — wait a beat for new data. The reader
            // already buffered EOF; reset our position to the actual end
            // before sleeping so the next read picks up appends.
            //
            // Known limitation: if the writer thread panics before
            // emitting a terminal `Status::Finished/Signaled/Crashed`
            // record, this loop spins at 200ms/cycle indefinitely (the
            // connection read-timeout was cleared for follow=true in
            // `handle_client_connection`). Closing this requires either
            // (a) a wall-clock idle deadline gated on "row status no
            // longer Running + mtime stale", or (b) a writer-liveness
            // channel surfaced to the tail handler. Tracked as a
            // follow-up; not load-bearing for the typical case where the
            // writer reliably emits a terminal Status before exiting.
            let pos = match file.stream_position() {
                Ok(p) => p,
                Err(_) => break,
            };
            std::thread::sleep(std::time::Duration::from_millis(200));
            // Reopen at the same position to re-read past previous EOF.
            if file.seek(SeekFrom::Start(pos)).is_err() {
                break;
            }
            reader = BufReader::new(&file);
            continue;
        }
        let record: LogRecord = match serde_json::from_str(line.trim_end_matches('\n')) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Some(min) = since_seq
            && record.seq < min
        {
            continue;
        }
        if !send(&record) {
            break;
        }
        // A terminal Status::Finished/Signaled/Crashed record is the
        // natural end of stream — stop following.
        if matches!(
            record.kind,
            crate::coordinator::log_record::LogRecordKind::Status(
                StatusEvent::Finished { .. }
                    | StatusEvent::Signaled { .. }
                    | StatusEvent::Crashed { .. }
            )
        ) {
            break;
        }
    }

    send_response(stream, &CoordinatorResponse::StreamEnd);
}

/// Resolve a `JobAddress` to a concrete `job_dir` for tailing. Reuses
/// `LogStore::list_invocations_for_worktree`/`list_jobs_in_invocation`
/// rather than the CLI's `resolve_job_address` so this stays inside the
/// coordinator layer.
#[cfg(unix)]
fn resolve_tail_job_dir(store: &LogStore, addr: &JobAddress) -> Result<std::path::PathBuf> {
    let invocations = if let Some(wt) = addr.worktree.as_deref() {
        store.list_invocations_for_worktree(wt)?
    } else {
        store.list_invocations()?
    };
    if invocations.is_empty() {
        anyhow::bail!("No invocations found");
    }
    let inv_id = match &addr.invocation_prefix {
        Some(p) => invocations
            .iter()
            .find(|m| m.invocation_id.starts_with(p.as_str()))
            .map(|m| m.invocation_id.clone())
            .ok_or_else(|| anyhow::anyhow!("No invocation matching prefix '{p}'"))?,
        None => invocations
            .last()
            .map(|m| m.invocation_id.clone())
            .ok_or_else(|| anyhow::anyhow!("No invocations available"))?,
    };
    let job_dir = store.base_dir.join(&inv_id).join(&addr.job_name);
    if !job_dir.exists() {
        anyhow::bail!(
            "No job named '{}' in invocation '{}'",
            addr.job_name,
            inv_id
        );
    }
    Ok(job_dir)
}

/// Write the coordinator PID file.
#[cfg(unix)]
fn write_pid_file(repo_hash: &str) -> Result<std::path::PathBuf> {
    let pid_path = coordinator_pid_path(repo_hash)?;
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;
    Ok(pid_path)
}

/// Spawn a detached coordinator child process to run background jobs.
///
/// The parent serializes `state + log_store_base` into a 0600-perms
/// tempfile, then spawns `daft __coordinator <state-file>` with
/// stdio routed to /dev/null and `DAFT_IS_COORDINATOR=1` injected via
/// the parent-side `Command::env(...)` (avoids edition-2024's
/// unsafe `std::env::set_var`). The parent returns `Ok(None)` immediately.
///
/// The child process reads + unlinks the state file, calls `setsid()` to
/// detach from the parent's session/TTY, then runs the jobs to completion.
/// See `run_coordinator`.
#[cfg(unix)]
pub fn spawn_coordinator(
    state: CoordinatorState,
    store: LogStore,
) -> Result<Option<Vec<JobResult>>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Resolve symlinks: when invoked via `git-worktree-checkout-branch` etc.,
    // `current_exe()` returns the symlink path; spawning that path would route
    // multicall dispatch to the symlink command (which then rejects
    // `__coordinator` as an unknown clap arg). Canonicalize to land on the
    // real `daft` binary so the spawned child dispatches correctly.
    let exe = std::env::current_exe()
        .context("Could not determine current executable")?
        .canonicalize()
        .context("Could not canonicalize executable path")?;

    let payload = CoordinatorPayload {
        state,
        log_store_base: store.base_dir.clone(),
    };
    let json = serde_json::to_vec(&payload).context("serialize coordinator state")?;

    // tempfile defaults to 0600 perms — no leak risk on shared hosts.
    // We `keep()` the path past Drop because the spawned child unlinks it.
    let tmp = tempfile::Builder::new()
        .prefix("daft-coord-")
        .suffix(".json")
        .tempfile()
        .context("create coordinator state tempfile")?;
    tmp.as_file()
        .write_all(&json)
        .context("write coordinator state")?;
    tmp.as_file()
        .sync_all()
        .context("sync coordinator state to disk")?;
    let (_file, state_path) = tmp.keep().context("persist coordinator state tempfile")?;

    // If `spawn()` fails the child never runs, so nothing will read or unlink
    // the state file — clean it up here rather than stranding a 0600 tempfile
    // until the next tmpfs sweep. The `keep()` above transferred ownership
    // away from `tempfile::Drop`, so the cleanup is on us.
    let spawn_result = Command::new(&exe)
        .arg("__coordinator")
        .arg(&state_path)
        .env("DAFT_IS_COORDINATOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();

    match spawn_result {
        Ok(_) => Ok(None),
        Err(e) => {
            std::fs::remove_file(&state_path).ok();
            Err(anyhow::Error::from(e).context(format!(
                "Could not spawn coordinator process from {}",
                exe.display()
            )))
        }
    }
}

/// Entry point for the spawned `daft __coordinator <state-file>` process.
///
/// Reads + unlinks the serialized [`CoordinatorPayload`], detaches via
/// `setsid()`, runs the job DAG, then `process::exit`s. Returns `Err` only
/// for fatal startup errors before the listener is up; runtime failures
/// inside `run_all_with_cancel` are handled internally and exit with code 1.
#[cfg(unix)]
pub fn run_coordinator(state_file: &std::path::Path) -> Result<()> {
    use std::process;

    let bytes = std::fs::read(state_file)
        .with_context(|| format!("read coordinator state file {}", state_file.display()))?;
    // Best-effort unlink. Even if it fails (e.g. tmpfs already swept), we
    // already have the bytes in memory; don't error out.
    let _ = std::fs::remove_file(state_file);
    let payload: CoordinatorPayload =
        serde_json::from_slice(&bytes).context("deserialize coordinator state")?;

    // Detach from the parent's session/controlling TTY. Tiny race window
    // exists between `Command::spawn` returning in the parent and this call
    // — the parent is exiting toward the user shell; SIGINT can't fire
    // until that shell prompt redraws.
    nix::unistd::setsid().ok();

    let store = LogStore::new(payload.log_store_base);
    let state = payload.state;

    // Write PID file (best-effort).
    let pid_path = write_pid_file(&state.repo_hash).ok();

    // Open the SQLite-backed job store and reconcile any rows left
    // `Running`/`Cancelling` by a previous coordinator that crashed before
    // it could record terminal states. Best-effort: if the store fails to
    // open we proceed without it — the legacy `meta.json` lifecycle still
    // works. Coordinator startup is the right place to wipe redb-era files
    // (`coordinator.redb`, `repo-policy.json`) — its stderr is allowed to
    // surface diagnostics; the completion / CLI-reader path is not.
    let job_store = match SqliteJobsStore::for_repo_base_with_wipe(&store.base_dir) {
        Ok(js) => {
            if let Err(e) = reconcile_active_jobs(&js, &state.repo_hash) {
                eprintln!("daft: coordinator reconciliation failed: {e}");
            }
            Some(js)
        }
        Err(e) => {
            eprintln!("daft: failed to open coordinator job store: {e}");
            None
        }
    };

    // Shared state for cancellation.
    let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
    let cancel_all = Arc::new(AtomicBool::new(false));
    let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));
    let shutdown = Arc::new(AtomicBool::new(false));

    let listener_handle = start_socket_listener(
        &state.repo_hash,
        store.base_dir.clone(),
        Arc::clone(&child_pids),
        Arc::clone(&cancel_all),
        Arc::clone(&cancelled_jobs),
        Arc::clone(&shutdown),
        job_store.clone(),
    )
    .ok();

    let exit_code = match state.run_all_with_cancel(
        &store,
        &child_pids,
        &cancel_all,
        &cancelled_jobs,
        job_store.as_ref(),
    ) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("daft: coordinator error: {e}");
            1
        }
    };

    // Tear down listener + PID file before exiting.
    shutdown.store(true, Ordering::Relaxed);
    if let Some((handle, socket_path)) = listener_handle {
        // Clean up the socket file to unblock the listener if it's blocked
        // in accept().
        std::fs::remove_file(&socket_path).ok();
        handle.join().ok();
    }
    if let Some(path) = pid_path {
        std::fs::remove_file(&path).ok();
    }

    process::exit(exit_code);
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::executor::JobSpec;
    use tempfile::TempDir;

    fn test_job(name: &str) -> JobSpec {
        JobSpec {
            name: name.to_string(),
            command: format!("echo {name}"),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_coordinator_state_new() {
        let state = CoordinatorState::new("test-repo", "inv-1");
        assert!(state.jobs.is_empty());
        assert_eq!(state.repo_hash, "test-repo");
    }

    #[test]
    fn test_coordinator_state_with_metadata() {
        let state = CoordinatorState::new("test-repo", "inv-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feature/tax-calc",
        );
        assert_eq!(state.trigger_command, "worktree-post-create");
        assert_eq!(state.hook_type, "worktree-post-create");
        assert_eq!(state.worktree, "feature/tax-calc");
    }

    #[test]
    fn test_coordinator_state_add_jobs() {
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(test_job("job-a"));
        state.add_job(test_job("job-b"));
        assert_eq!(state.jobs.len(), 2);
    }

    /// Regression test for #412: the spawn refactor depends on
    /// `CoordinatorPayload` round-tripping cleanly through serde JSON
    /// (parent serializes to tempfile -> spawned child deserializes).
    /// Asserts the structurally important fields survive — including the
    /// `Duration` adapter on `JobSpec.timeout`.
    #[test]
    fn coordinator_payload_round_trips_through_serde_json() {
        use std::path::PathBuf;
        use std::time::Duration;
        let mut state = CoordinatorState::new("repo-x", "inv-y").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feat/x",
        );
        state.add_job(JobSpec {
            name: "j".to_string(),
            command: "echo ok".to_string(),
            working_dir: std::env::temp_dir(),
            timeout: Duration::from_secs(120),
            background: true,
            needs: vec!["dep".to_string()],
            ..Default::default()
        });
        let payload = CoordinatorPayload {
            state,
            log_store_base: PathBuf::from("/tmp/daft-store"),
        };

        let bytes = serde_json::to_vec(&payload).expect("serialize payload");
        let back: CoordinatorPayload = serde_json::from_slice(&bytes).expect("deserialize payload");
        assert_eq!(back.state.repo_hash, "repo-x");
        assert_eq!(back.state.invocation_id, "inv-y");
        assert_eq!(back.state.worktree, "feat/x");
        assert_eq!(back.state.jobs.len(), 1);
        let job = &back.state.jobs[0];
        assert_eq!(job.command, "echo ok");
        assert_eq!(job.timeout, Duration::from_secs(120));
        assert_eq!(job.needs, vec!["dep".to_string()]);
        assert_eq!(back.log_store_base, PathBuf::from("/tmp/daft-store"));
    }

    /// Mirrors `run_coordinator`'s read+unlink half (we can't call the full
    /// function from a unit test because it ends with `process::exit`).
    /// Verifies the state file is gone after the child consumes it.
    #[test]
    fn coordinator_state_file_is_unlinked_after_read() {
        let tmp = TempDir::new().unwrap();
        let state_file = tmp.path().join("payload.json");
        let payload = CoordinatorPayload {
            state: CoordinatorState::new("repo-x", "inv-1"),
            log_store_base: tmp.path().join("store"),
        };
        std::fs::write(&state_file, serde_json::to_vec(&payload).unwrap()).unwrap();
        assert!(state_file.exists());

        let bytes = std::fs::read(&state_file).unwrap();
        std::fs::remove_file(&state_file).ok();
        let back: CoordinatorPayload = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(back.state.repo_hash, "repo-x");
        assert!(
            !state_file.exists(),
            "state file must be unlinked after the child reads it"
        );
    }

    #[test]
    fn test_coordinator_run_jobs_to_completion() {
        use crate::coordinator::ports::JobsStorePort;
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let job_store = SqliteJobsStore::for_repo_base(&store.base_dir).unwrap();
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(JobSpec {
            name: "echo-job".to_string(),
            command: "echo hello".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });

        let results = state
            .run_all_with_cancel(
                &store,
                &ChildPidMap::default(),
                &Arc::new(AtomicBool::new(false)),
                &CancelledJobs::default(),
                Some(&job_store),
            )
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].status.is_terminal());

        // Verify terminal row landed in SQLite (the meta.json sidecar is
        // not written post-cutover).
        let row = job_store
            .get_job("test-repo", "inv-1", "echo-job")
            .unwrap()
            .expect("job row persisted on completion");
        assert_eq!(row.status, "completed");
    }

    #[test]
    fn test_build_job_list_empty_store() {
        let tmp = TempDir::new().unwrap();
        let job_store = SqliteJobsStore::for_repo_base(tmp.path()).unwrap();
        let jobs = build_job_list(Some(&job_store), "rh");
        assert!(jobs.is_empty());
    }

    #[test]
    fn test_build_job_list_with_jobs() {
        let tmp = TempDir::new().unwrap();
        let job_store = SqliteJobsStore::for_repo_base(tmp.path()).unwrap();
        let row = JobRow {
            repo_hash: "rh".into(),
            invocation_id: "inv-1".into(),
            name: "build".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "/tmp/wt".into(),
            command: "cargo build".into(),
            working_dir: "/tmp/wt".into(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            finished_at: None,
            status: JobStatus::Completed.as_status_str().to_string(),
            exit_code: Some(0),
            pid: Some(1234),
            pgid: Some(1234),
            background: false,
            needs: vec![],
            tags: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
        };
        job_store.upsert_job(&row).unwrap();

        let jobs = build_job_list(Some(&job_store), "rh");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "build");
        assert_eq!(jobs[0].hook_type, "worktree-post-create");
        assert!(matches!(jobs[0].status, JobStatus::Completed));
        // Completed jobs should not have elapsed_secs.
        assert!(jobs[0].elapsed_secs.is_none());
        assert_eq!(jobs[0].exit_code, Some(0));
    }

    #[test]
    fn test_build_job_list_running_job_has_elapsed() {
        let tmp = TempDir::new().unwrap();
        let job_store = SqliteJobsStore::for_repo_base(tmp.path()).unwrap();
        let row = JobRow {
            repo_hash: "rh".into(),
            invocation_id: "inv-1".into(),
            name: "long-job".into(),
            hook_type: "worktree-post-create".into(),
            worktree: "/tmp/wt".into(),
            command: "sleep 100".into(),
            working_dir: "/tmp/wt".into(),
            env: HashMap::new(),
            started_at: chrono::Utc::now() - chrono::Duration::seconds(30),
            finished_at: None,
            status: JobStatus::Running.as_status_str().to_string(),
            exit_code: None,
            pid: Some(9999),
            pgid: Some(9999),
            background: false,
            needs: vec![],
            tags: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
        };
        job_store.upsert_job(&row).unwrap();

        let jobs = build_job_list(Some(&job_store), "rh");
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].status, JobStatus::Running));
        // Should have elapsed_secs >= 30 (approximately).
        assert!(jobs[0].elapsed_secs.unwrap() >= 29);
    }

    #[test]
    fn test_cancel_single_job_not_found() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));

        let response = cancel_single_job("nonexistent", &child_pids, &cancelled_jobs, &store);
        assert!(matches!(
            response,
            CoordinatorResponse::Error { message, .. } if message.contains("not found")
        ));
    }

    #[test]
    fn test_run_all_with_cancel_skips_when_cancelled() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(test_job("skipped-job"));

        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancel_all = Arc::new(AtomicBool::new(true)); // Already cancelled
        let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));

        let results = state
            .run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs, None)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Skipped);
    }

    #[test]
    fn test_run_all_populates_job_hook_type_and_worktree() {
        let tmp = TempDir::new().unwrap();
        use crate::coordinator::ports::JobsStorePort;
        let store = LogStore::new(tmp.path().to_path_buf());
        let job_store = SqliteJobsStore::for_repo_base(&store.base_dir).unwrap();
        let mut state = CoordinatorState::new("test-repo", "inv-pop-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feature/y",
        );
        state.add_job(JobSpec {
            name: "check-job".to_string(),
            command: "echo ok".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });

        state
            .run_all_with_cancel(
                &store,
                &ChildPidMap::default(),
                &Arc::new(AtomicBool::new(false)),
                &CancelledJobs::default(),
                Some(&job_store),
            )
            .unwrap();

        let row = job_store
            .get_job("test-repo", "inv-pop-1", "check-job")
            .unwrap()
            .expect("job row persisted on completion");
        assert_eq!(row.hook_type, "worktree-post-create");
        assert_eq!(row.worktree, "feature/y");
        assert!(row.background);
        assert!(row.finished_at.is_some());
    }

    // Reconciliation logic lives in `coordinator::domain::reconcile` and is
    // covered there by six unit tests against mock adapters
    // (`marks_dead_running_as_crashed`,
    // `leaves_alive_running_intact`,
    // `job_without_pgid_is_treated_as_crashed`,
    // `cancelling_status_is_also_reconciled`,
    // `terminal_statuses_are_not_touched`,
    // `other_repos_are_left_alone`). The thin wrapper in this module is
    // exercised end-to-end via the YAML integration scenarios.

    /// Filter predicates AND together; missing predicates wildcard. Tag
    /// matching is "contains" against the JobRow's `tags` vector.
    #[test]
    fn filter_matching_jobs_combines_predicates_and() {
        fn row(name: &str, hook: &str, wt: &str, inv: &str, tags: &[&str]) -> JobRow {
            JobRow {
                repo_hash: "rh".into(),
                invocation_id: inv.into(),
                name: name.into(),
                hook_type: hook.into(),
                worktree: wt.into(),
                command: "x".into(),
                working_dir: "/tmp".into(),
                env: HashMap::new(),
                started_at: chrono::Utc::now(),
                finished_at: None,
                status: JobStatus::Running.as_status_str().to_string(),
                exit_code: None,
                pid: Some(1),
                pgid: Some(1),
                background: true,
                needs: vec![],
                tags: tags.iter().map(|s| s.to_string()).collect(),
                retention_seconds: None,
                max_log_size_bytes: None,
            }
        }

        let rows = vec![
            row("a", "post-create", "feat/x", "abc123", &["slow"]),
            row("b", "post-create", "feat/y", "abc456", &["fast"]),
            row("c", "pre-remove", "feat/x", "def789", &["slow", "build"]),
        ];

        let now = chrono::Utc::now();

        // hook=post-create AND tag=slow → only `a`
        let args = CancelMatchingArgs {
            hook: Some("post-create"),
            worktree: None,
            tag: Some("slow"),
            invocation_prefix: None,
            older_than_secs: None,
        };
        let matched: Vec<_> = filter_matching_jobs(rows.clone(), &args, now)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(matched, vec!["a"]);

        // worktree=feat/x → `a` + `c`
        let args = CancelMatchingArgs {
            hook: None,
            worktree: Some("feat/x"),
            tag: None,
            invocation_prefix: None,
            older_than_secs: None,
        };
        let mut matched: Vec<_> = filter_matching_jobs(rows.clone(), &args, now)
            .into_iter()
            .map(|r| r.name)
            .collect();
        matched.sort();
        assert_eq!(matched, vec!["a", "c"]);

        // invocation_prefix=abc → `a` + `b`
        let args = CancelMatchingArgs {
            hook: None,
            worktree: None,
            tag: None,
            invocation_prefix: Some("abc"),
            older_than_secs: None,
        };
        let mut matched: Vec<_> = filter_matching_jobs(rows.clone(), &args, now)
            .into_iter()
            .map(|r| r.name)
            .collect();
        matched.sort();
        assert_eq!(matched, vec!["a", "b"]);

        // tag=build → only `c`
        let args = CancelMatchingArgs {
            hook: None,
            worktree: None,
            tag: Some("build"),
            invocation_prefix: None,
            older_than_secs: None,
        };
        let matched: Vec<_> = filter_matching_jobs(rows, &args, now)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(matched, vec!["c"]);
    }

    /// `older_than_secs` matches rows whose elapsed runtime (now − started_at)
    /// is at least the threshold. Rows that haven't existed long enough are
    /// filtered out.
    #[test]
    fn filter_matching_jobs_older_than_uses_elapsed_runtime() {
        let now = chrono::Utc::now();
        let old = JobRow {
            repo_hash: "rh".into(),
            invocation_id: "old".into(),
            name: "old".into(),
            hook_type: "h".into(),
            worktree: "w".into(),
            command: "x".into(),
            working_dir: "/tmp".into(),
            env: HashMap::new(),
            started_at: now - chrono::Duration::seconds(60),
            finished_at: None,
            status: JobStatus::Running.as_status_str().to_string(),
            exit_code: None,
            pid: None,
            pgid: None,
            background: true,
            needs: vec![],
            tags: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
        };
        let mut young = old.clone();
        young.name = "young".into();
        young.invocation_id = "young".into();
        young.started_at = now - chrono::Duration::seconds(5);

        let args = CancelMatchingArgs {
            hook: None,
            worktree: None,
            tag: None,
            invocation_prefix: None,
            older_than_secs: Some(30),
        };
        let matched: Vec<_> = filter_matching_jobs(vec![old, young], &args, now)
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert_eq!(matched, vec!["old"]);
    }

    /// End-to-end: a real coordinator run with `Some(job_store)` writes
    /// terminal JobRows containing the captured tags and child PGID.
    #[test]
    fn dual_write_persists_terminal_job_row_with_tags() {
        let tmp = TempDir::new().unwrap();
        let log_base = tmp.path().to_path_buf();
        let store = LogStore::new(log_base.clone());
        let job_store = SqliteJobsStore::for_repo_base(&log_base).unwrap();

        let mut state = CoordinatorState::new("rh", "inv-dual").with_metadata(
            "trigger",
            "worktree-post-create",
            "feat/x",
        );
        let mut job = test_job("dual-job");
        job.tags = vec!["fast".into(), "build".into()];
        state.add_job(job);

        state
            .run_all_with_cancel(
                &store,
                &ChildPidMap::default(),
                &Arc::new(AtomicBool::new(false)),
                &CancelledJobs::default(),
                Some(&job_store),
            )
            .unwrap();

        let row = job_store
            .get_job("rh", "inv-dual", "dual-job")
            .unwrap()
            .expect("terminal JobRow must be written");
        assert_eq!(row.status, JobStatus::Completed.as_status_str());
        assert_eq!(row.tags, vec!["fast", "build"]);
        assert!(row.pid.is_some(), "terminal write must record child pid");
        assert!(row.finished_at.is_some());
    }

    #[test]
    fn test_socket_listener_list_jobs() {
        use std::os::unix::net::UnixStream;

        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("test-listener.sock");
        let store_dir = tmp.path().join("store");
        std::fs::create_dir_all(&store_dir).unwrap();

        // Seed a job through the SQLite store so `ListJobs` sees it —
        // `build_job_list` now reads from SQLite, not from `meta.json`.
        let job_store = SqliteJobsStore::for_repo_base(&store_dir).unwrap();
        let row = JobRow {
            repo_hash: "test-repo".into(),
            invocation_id: "inv-1".into(),
            name: "test-job".into(),
            hook_type: "post-clone".into(),
            worktree: "/tmp/wt".into(),
            command: "echo test".into(),
            working_dir: "/tmp/wt".into(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            finished_at: None,
            status: JobStatus::Completed.as_status_str().to_string(),
            exit_code: Some(0),
            pid: Some(1234),
            pgid: Some(1234),
            background: false,
            needs: vec![],
            tags: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
        };
        job_store.upsert_job(&row).unwrap();

        // Manually bind the listener (bypassing coordinator_socket_path).
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancel_all = Arc::new(AtomicBool::new(false));
        let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        let job_store_for_thread = job_store.clone();

        let handle = std::thread::spawn(move || {
            let store = LogStore::new(store_dir);
            loop {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        handle_client_connection(
                            stream,
                            &store,
                            &child_pids,
                            &cancel_all,
                            &cancelled_jobs,
                            &shutdown_clone,
                            Some(&job_store_for_thread),
                            "test-repo",
                        );
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        // Client: connect and send a framed `RequestEnvelope { v: 1, ... }`.
        let mut stream = UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();

        let env = RequestEnvelope {
            v: PROTOCOL_VERSION,
            body: CoordinatorRequest::ListJobs,
        };
        let req_bytes = serde_json::to_vec(&env).unwrap();
        framing::write_frame(&mut stream, &req_bytes).unwrap();

        let resp_bytes = framing::read_frame(&mut stream).unwrap();
        let resp_env: ResponseEnvelope = serde_json::from_slice(&resp_bytes).unwrap();
        assert_eq!(resp_env.v, PROTOCOL_VERSION);
        match resp_env.body {
            CoordinatorResponse::Jobs(jobs) => {
                assert_eq!(jobs.len(), 1);
                assert_eq!(jobs[0].name, "test-job");
            }
            other => panic!("Expected Jobs response, got: {other:?}"),
        }

        // Shut down the listener.
        shutdown.store(true, Ordering::Relaxed);
        handle.join().ok();
    }

    #[test]
    fn test_send_response_round_trip() {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("test-send-resp.sock");

        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let response = CoordinatorResponse::Ack {
                message: "ok".to_string(),
            };
            send_response(&stream, &response);
        });

        let mut stream = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let bytes = framing::read_frame(&mut stream).unwrap();
        let env: ResponseEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(env.v, PROTOCOL_VERSION);
        assert!(matches!(
            env.body,
            CoordinatorResponse::Ack { message } if message == "ok"
        ));

        handle.join().unwrap();
    }

    fn make_ctx<'a>(
        inv: &'a str,
        child_pids: &ChildPidMap,
        cancel_all: &Arc<AtomicBool>,
        cancelled_jobs: &CancelledJobs,
        results: &Arc<Mutex<Vec<JobResult>>>,
        job_store: Option<&SqliteJobsStore>,
    ) -> JobInvocationContext<'a> {
        JobInvocationContext {
            repo_hash: "test-repo",
            invocation_id: inv,
            hook_type: "worktree-post-create",
            worktree: "feat/x",
            // SqliteJobsStore is Clone (Arc inside); cloning here lets the
            // caller hand us a borrow and keep its own reference.
            job_store: job_store.cloned(),
            results: Arc::clone(results),
            child_pids: Arc::clone(child_pids),
            cancel_all: Arc::clone(cancel_all),
            cancelled_jobs: Arc::clone(cancelled_jobs),
        }
    }

    #[allow(clippy::type_complexity)]
    fn make_test_state() -> (
        TempDir,
        LogStore,
        SqliteJobsStore,
        ChildPidMap,
        Arc<AtomicBool>,
        CancelledJobs,
        Arc<Mutex<Vec<JobResult>>>,
    ) {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().join("jobs").join("test-repo"));
        std::fs::create_dir_all(&store.base_dir).unwrap();
        let job_store = SqliteJobsStore::for_repo_base(&store.base_dir).unwrap();
        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancel_all = Arc::new(AtomicBool::new(false));
        let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));
        let results = Arc::new(Mutex::new(Vec::new()));
        (
            tmp,
            store,
            job_store,
            child_pids,
            cancel_all,
            cancelled_jobs,
            results,
        )
    }

    #[test]
    fn run_single_background_job_registers_and_deregisters_pid() {
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, results) =
            make_test_state();
        // Sleep 1.0 (not 0.4) gives the probe a wider window so the test
        // doesn't flake under heavy parallel-test load when fork-exec is slow
        // to schedule the registrar thread.
        let job = JobSpec {
            name: "sleep-job".to_string(),
            command: "sleep 1.0 && echo done".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        };
        let ctx = make_ctx(
            "00000000-0000-0000-0000-000000000001",
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
            &results,
            Some(&job_store),
        );

        let pids_probe = Arc::clone(&child_pids);
        let probe = std::thread::spawn(move || {
            // Poll until the registrar inserts, up to ~600ms. A fixed sleep
            // raced against fork-exec scheduling under load and produced
            // spurious failures in the full test sweep.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
            loop {
                let snap = pids_probe.lock().unwrap().clone();
                if snap.contains_key("sleep-job") {
                    return snap;
                }
                if std::time::Instant::now() >= deadline {
                    return snap;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });

        let _ = run_single_background_job(&job, &ctx, &store);

        let mid = probe.join().unwrap();
        assert!(
            mid.contains_key("sleep-job"),
            "PID should be registered mid-run"
        );
        assert!(
            !child_pids.lock().unwrap().contains_key("sleep-job"),
            "PID should be deregistered after completion"
        );
    }

    #[test]
    fn per_job_cancel_marks_status_cancelled_not_failed() {
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, results) =
            make_test_state();
        let job = JobSpec {
            name: "long-job".to_string(),
            command: "sleep 5".to_string(),
            working_dir: std::env::temp_dir(),
            timeout: std::time::Duration::from_secs(30),
            background: true,
            ..Default::default()
        };
        let inv_id = "00000000-0000-0000-0000-000000000002".to_string();
        let ctx = make_ctx(
            &inv_id,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
            &results,
            Some(&job_store),
        );

        let killer = {
            let pids = Arc::clone(&child_pids);
            let cancelled = Arc::clone(&cancelled_jobs);
            let store_for_killer = store.clone();
            std::thread::spawn(move || {
                // Wait for the registrar thread to record the PID before we
                // try to cancel by name. A fixed sleep raced against
                // fork-exec scheduling under heavy parallel-test load —
                // cancel_single_job would no-op because the map was still
                // empty.
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !pids.lock().unwrap().contains_key("long-job") {
                    if std::time::Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                // Route through the production cancellation path so that
                // regressions (e.g. dropping the `cancelled_jobs` insert or
                // skipping the SIGTERM) are caught here.
                let _ = cancel_single_job("long-job", &pids, &cancelled, &store_for_killer);
            })
        };

        let _ = run_single_background_job(&job, &ctx, &store);
        killer.join().unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let row = job_store
            .get_job("test-repo", &inv_id, "long-job")
            .unwrap()
            .expect("job row persisted via port");
        assert_eq!(
            row.status, "cancelled",
            "expected Cancelled, got {:?}",
            row.status
        );
    }

    #[test]
    fn silent_bg_output_deletes_log_on_success() {
        use crate::executor::BackgroundOutput;
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, results) =
            make_test_state();
        let job = JobSpec {
            name: "silent-ok".to_string(),
            command: "echo hello".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            background_output: Some(BackgroundOutput::Silent),
            ..Default::default()
        };
        let inv_id = "00000000-0000-0000-0000-000000000003".to_string();
        let ctx = make_ctx(
            &inv_id,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
            &results,
            Some(&job_store),
        );

        let _ = run_single_background_job(&job, &ctx, &store);

        let job_dir = store.base_dir.join(&inv_id).join("silent-ok");
        let log_path = LogStore::jsonl_path(&job_dir);
        assert!(
            !log_path.exists(),
            "silent + success should leave no log file"
        );
    }

    #[test]
    fn silent_bg_output_keeps_log_on_failure() {
        use crate::executor::BackgroundOutput;
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, results) =
            make_test_state();
        let job = JobSpec {
            name: "silent-fail".to_string(),
            command: "echo whoops; exit 1".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            background_output: Some(BackgroundOutput::Silent),
            ..Default::default()
        };
        let inv_id = "00000000-0000-0000-0000-000000000004".to_string();
        let ctx = make_ctx(
            &inv_id,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
            &results,
            Some(&job_store),
        );

        let _ = run_single_background_job(&job, &ctx, &store);

        let job_dir = store.base_dir.join(&inv_id).join("silent-fail");
        let log_path = LogStore::jsonl_path(&job_dir);
        assert!(
            log_path.exists(),
            "silent + failure should preserve log file"
        );
        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("whoops"));
    }

    #[test]
    fn non_silent_bg_output_always_writes_log() {
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, results) =
            make_test_state();
        let job = JobSpec {
            name: "loud-ok".to_string(),
            command: "echo loud".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            background_output: None,
            ..Default::default()
        };
        let inv_id = "00000000-0000-0000-0000-000000000005".to_string();
        let ctx = make_ctx(
            &inv_id,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
            &results,
            Some(&job_store),
        );

        let _ = run_single_background_job(&job, &ctx, &store);

        let job_dir = store.base_dir.join(&inv_id).join("loud-ok");
        let log_path = LogStore::jsonl_path(&job_dir);
        assert!(log_path.exists(), "non-silent should always retain log");
    }

    #[test]
    fn silent_bg_output_keeps_log_when_status_is_cancelled() {
        use crate::executor::BackgroundOutput;
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, results) =
            make_test_state();

        // Pre-insert into cancelled_jobs BEFORE running. The cmd will succeed
        // (exit 0), but the classifier will see was_cancelled_per_job and route
        // status to Cancelled. The log must survive.
        cancelled_jobs
            .lock()
            .unwrap()
            .insert("pre-cancelled".to_string());

        let job = JobSpec {
            name: "pre-cancelled".to_string(),
            command: "echo would-have-succeeded".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            background_output: Some(BackgroundOutput::Silent),
            ..Default::default()
        };
        let inv_id = "00000000-0000-0000-0000-000000000006".to_string();
        let ctx = make_ctx(
            &inv_id,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
            &results,
            Some(&job_store),
        );

        let _ = run_single_background_job(&job, &ctx, &store);

        let job_dir = store.base_dir.join(&inv_id).join("pre-cancelled");
        let log_path = LogStore::jsonl_path(&job_dir);
        assert!(
            log_path.exists(),
            "silent + cancelled (even when cmd succeeded) should preserve log"
        );

        use crate::coordinator::ports::JobsStorePort;
        let row = job_store
            .get_job("test-repo", &inv_id, "pre-cancelled")
            .unwrap()
            .expect("job row persisted via port");
        assert_eq!(
            row.status, "cancelled",
            "expected Cancelled, got {:?}",
            row.status
        );
    }

    #[test]
    fn bg_dependent_waits_for_dep_to_finish() {
        // Regression test for daft#454: B `needs: [A]` must not start until A
        // has terminated.
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        let mut state = CoordinatorState::new("test-repo", "inv-needs-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feat/x",
        );

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
            .run_all_with_cancel(
                &store,
                &child_pids,
                &cancel_all,
                &cancelled_jobs,
                Some(&job_store),
            )
            .unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let row_a = job_store
            .get_job("test-repo", "inv-needs-1", "dep-a")
            .unwrap()
            .expect("dep-a row");
        let row_b = job_store
            .get_job("test-repo", "inv-needs-1", "dep-b")
            .unwrap()
            .expect("dep-b row");

        let a_finished = row_a.finished_at.expect("a finished_at");
        let b_started = row_b.started_at;

        assert!(
            b_started >= a_finished,
            "dep-b started ({b_started}) before dep-a finished ({a_finished})"
        );
        assert_eq!(row_a.status, "completed");
        assert_eq!(row_b.status, "completed");
    }

    #[test]
    fn bg_dependent_skipped_when_dep_fails() {
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        // Use a marker path scoped to this test's TempDir so parallel test
        // runs cannot race on a global `/tmp` path.
        let marker = _tmp.path().join("dep-failed-side-effect");

        let mut state = CoordinatorState::new("test-repo", "inv-needs-fail-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feat/x",
        );

        state.add_job(JobSpec {
            name: "fails".to_string(),
            command: "exit 7".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });
        state.add_job(JobSpec {
            name: "dependent".to_string(),
            command: format!("touch {}", marker.display()),
            working_dir: std::env::temp_dir(),
            background: true,
            needs: vec!["fails".to_string()],
            ..Default::default()
        });

        state
            .run_all_with_cancel(
                &store,
                &child_pids,
                &cancel_all,
                &cancelled_jobs,
                Some(&job_store),
            )
            .unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let row_fails = job_store
            .get_job("test-repo", "inv-needs-fail-1", "fails")
            .unwrap()
            .expect("fails row");
        assert_eq!(row_fails.status, "failed");

        // The dependent's closure was NOT invoked (no spawn) — but the
        // coordinator synthesizes a JobRow after the DAG returns so the
        // job remains visible to `daft hooks jobs`. Status is `Skipped`
        // (the closest available variant). The job's command must NOT
        // have produced its side-effect.
        let row_dependent = job_store
            .get_job("test-repo", "inv-needs-fail-1", "dependent")
            .unwrap()
            .expect("dependent should have a synthesized dep-failed row");
        assert_eq!(
            row_dependent.status, "skipped",
            "dep-failed dependent should be Skipped, got {:?}",
            row_dependent.status
        );
        assert_eq!(row_dependent.needs, vec!["fails".to_string()]);
        assert!(
            !marker.exists(),
            "dependent ran its command despite dep failing"
        );
    }

    #[test]
    fn bg_dependent_skipped_when_dep_cancelled() {
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        // Marker path is scoped to this test's TempDir to avoid races with
        // parallel test runs.
        let marker = _tmp.path().join("dep-cancelled-side-effect");

        let mut state = CoordinatorState::new("test-repo", "inv-needs-cancel-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feat/x",
        );

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
            command: format!("touch {}", marker.display()),
            working_dir: std::env::temp_dir(),
            background: true,
            needs: vec!["long".to_string()],
            ..Default::default()
        });

        let pids = Arc::clone(&child_pids);
        let cancelled = Arc::clone(&cancelled_jobs);
        let store_for_killer = store.clone();
        let killer = std::thread::spawn(move || {
            // Wait until the registrar thread records "long"'s PID before
            // we try to cancel. A fixed 200ms sleep raced fork-exec
            // scheduling under heavy parallel-test load.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !pids.lock().unwrap().contains_key("long") {
                if std::time::Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let _ = cancel_single_job("long", &pids, &cancelled, &store_for_killer);
        });

        state
            .run_all_with_cancel(
                &store,
                &child_pids,
                &cancel_all,
                &cancelled_jobs,
                Some(&job_store),
            )
            .unwrap();
        killer.join().unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let row_long = job_store
            .get_job("test-repo", "inv-needs-cancel-1", "long")
            .unwrap()
            .expect("long row");
        assert_eq!(
            row_long.status, "cancelled",
            "long should be Cancelled, got {:?}",
            row_long.status
        );

        // After the DAG returns, the coordinator synthesizes a JobRow
        // for the dep-failed dependent so it remains visible to
        // `daft hooks jobs`. Status is `Skipped` (no `DepFailed` variant
        // exists in `JobStatus`). The job's command must NOT have run.
        let row_after = job_store
            .get_job("test-repo", "inv-needs-cancel-1", "after")
            .unwrap()
            .expect("after should have a synthesized dep-failed row");
        assert_eq!(
            row_after.status, "skipped",
            "after should be Skipped (dep cancelled), got {:?}",
            row_after.status
        );
        assert_eq!(row_after.needs, vec!["long".to_string()]);
        assert!(
            !marker.exists(),
            "after ran its command despite dep being cancelled"
        );
    }

    #[test]
    fn bg_cycle_in_needs_returns_error() {
        let (_tmp, store, _job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        let mut state = CoordinatorState::new("test-repo", "inv-cycle-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feat/x",
        );

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
            state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs, None);
        assert!(result.is_err(), "cycle should be reported as an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid background job DAG"),
            "error should mention invalid DAG, got: {msg}"
        );
    }

    #[test]
    fn bg_missing_dep_in_needs_returns_error() {
        // Defensive: the partitioner now strips foreground names from BG
        // `needs:` before the slice reaches the coordinator (see #556 /
        // `partition::partition_foreground_background`), so the only way
        // to reach this code path in production is a typoed name —
        // already rejected by the config validator. The behavior still
        // matters as a contract: if some caller does hand the coordinator
        // a dangling reference, surface it as a hard error rather than
        // silently dropping the job.
        let (_tmp, store, _job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        let mut state = CoordinatorState::new("test-repo", "inv-missing-1").with_metadata(
            "worktree-post-create",
            "worktree-post-create",
            "feat/x",
        );

        state.add_job(JobSpec {
            name: "only".to_string(),
            command: "echo only".to_string(),
            background: true,
            needs: vec!["does-not-exist".to_string()],
            ..Default::default()
        });

        let result =
            state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs, None);
        assert!(
            result.is_err(),
            "missing dep should be reported as an error"
        );
    }

    #[test]
    fn prefailed_job_recorded_as_skipped_without_running() {
        // Regression for #556: a BG job pre-marked as DepFailed by the
        // caller (because its FG dep failed) must not be executed, but
        // must still be persisted to SQLite so `daft hooks jobs` shows
        // it as `skipped` rather than silently vanishing.
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        let mut state = CoordinatorState::new("test-repo", "inv-prefail-1")
            .with_metadata("worktree-post-create", "worktree-post-create", "feat/x")
            .with_prefailed(vec!["doomed".to_string()]);

        // The command is `false` (always fails) so that if the cascade
        // misbehaves and the job actually runs, the test catches it via
        // the recorded status being `failed` rather than `skipped`.
        state.add_job(JobSpec {
            name: "doomed".to_string(),
            command: "false".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });

        state
            .run_all_with_cancel(
                &store,
                &child_pids,
                &cancel_all,
                &cancelled_jobs,
                Some(&job_store),
            )
            .unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let row = job_store
            .get_job("test-repo", "inv-prefail-1", "doomed")
            .unwrap()
            .expect("doomed row should exist");
        assert_eq!(
            row.status, "skipped",
            "prefailed BG job should be recorded as skipped, got {}",
            row.status
        );
        assert!(
            row.pid.is_none(),
            "prefailed BG job must not have spawned a child process (pid={:?})",
            row.pid
        );
    }

    #[test]
    fn prefailed_cascade_skips_transitive_bg_dependents() {
        // Regression for #556: a BG job that itself depends on a
        // prefailed BG job must also be recorded as skipped, matching
        // the cascade the wave loop applies for runtime failures.
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        let mut state = CoordinatorState::new("test-repo", "inv-prefail-2")
            .with_metadata("worktree-post-create", "worktree-post-create", "feat/x")
            .with_prefailed(vec!["doomed".to_string()]);

        state.add_job(JobSpec {
            name: "doomed".to_string(),
            command: "true".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });
        state.add_job(JobSpec {
            name: "downstream".to_string(),
            command: "true".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            needs: vec!["doomed".to_string()],
            ..Default::default()
        });

        state
            .run_all_with_cancel(
                &store,
                &child_pids,
                &cancel_all,
                &cancelled_jobs,
                Some(&job_store),
            )
            .unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let down = job_store
            .get_job("test-repo", "inv-prefail-2", "downstream")
            .unwrap()
            .expect("downstream row should exist");
        assert_eq!(down.status, "skipped");
        assert!(down.pid.is_none());
    }

    #[test]
    fn prefailed_does_not_block_unrelated_bg_jobs() {
        // A prefailed BG job must not poison BG jobs in a different
        // dependency subtree. Sibling BG jobs that don't depend on the
        // prefailed name run normally.
        let (_tmp, store, job_store, child_pids, cancel_all, cancelled_jobs, _results) =
            make_test_state();

        let mut state = CoordinatorState::new("test-repo", "inv-prefail-3")
            .with_metadata("worktree-post-create", "worktree-post-create", "feat/x")
            .with_prefailed(vec!["doomed".to_string()]);

        state.add_job(JobSpec {
            name: "doomed".to_string(),
            command: "true".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });
        state.add_job(JobSpec {
            name: "sibling".to_string(),
            command: "true".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });

        state
            .run_all_with_cancel(
                &store,
                &child_pids,
                &cancel_all,
                &cancelled_jobs,
                Some(&job_store),
            )
            .unwrap();

        use crate::coordinator::ports::JobsStorePort;
        let sib = job_store
            .get_job("test-repo", "inv-prefail-3", "sibling")
            .unwrap()
            .expect("sibling row should exist");
        assert_eq!(sib.status, "completed");
    }
}
