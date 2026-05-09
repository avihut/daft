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

use super::log_store::{JobMeta, JobStatus, LogStore};
#[cfg(unix)]
use super::{
    CoordinatorRequest, CoordinatorResponse, JobInfo, coordinator_pid_path, coordinator_socket_path,
};
use crate::executor::command::run_command;
use crate::executor::dag::DagGraph;
use crate::executor::{JobResult, JobSpec, NodeStatus};
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
        }
    }

    pub fn with_metadata(mut self, trigger_command: &str, hook_type: &str, worktree: &str) -> Self {
        self.trigger_command = trigger_command.to_string();
        self.hook_type = hook_type.to_string();
        self.worktree = worktree.to_string();
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
        let mut in_degree: Vec<usize> = (0..n).map(|i| graph.dependencies_of(i).len()).collect();
        let mut ready: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();

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
                let store_base = store.base_dir.clone();
                let hook_type = self.hook_type.clone();
                let worktree = self.worktree.clone();
                let results_clone = Arc::clone(&results);
                let child_pids_clone = Arc::clone(child_pids);
                let cancel_all_clone = Arc::clone(cancel_all);
                let cancelled_jobs_clone = Arc::clone(cancelled_jobs);

                let handle = std::thread::spawn(move || {
                    let local_store = LogStore::new(store_base);
                    let ctx = JobInvocationContext {
                        invocation_id: &inv_id,
                        hook_type: &hook_type,
                        worktree: &worktree,
                    };
                    run_single_background_job(
                        &job,
                        &ctx,
                        &local_store,
                        &results_clone,
                        &child_pids_clone,
                        &cancel_all_clone,
                        &cancelled_jobs_clone,
                    )
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

        // Synthesize meta + JobResult for jobs the scheduler marked DepFailed
        // (their closure was not invoked, so they would otherwise be invisible
        // to `daft hooks jobs`).
        for (idx, status) in statuses.iter().enumerate() {
            if *status != NodeStatus::DepFailed {
                continue;
            }
            let Some(job) = self.jobs.get(idx) else {
                continue;
            };
            if let Ok(job_dir) = store.create_job_dir(&self.invocation_id, &job.name) {
                let meta = JobMeta::skipped(
                    &job.name,
                    &self.hook_type,
                    &self.worktree,
                    &job.command,
                    job.background,
                    job.needs.clone(),
                );
                if let Err(e) = store.write_meta(&job_dir, &meta) {
                    eprintln!(
                        "daft: failed to write dep-failed meta for '{}': {e}",
                        job.name
                    );
                }
            } else {
                eprintln!(
                    "daft: failed to create dep-failed log dir for '{}'",
                    job.name
                );
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

/// Metadata propagated from the coordinator into each job execution.
struct JobInvocationContext<'a> {
    invocation_id: &'a str,
    hook_type: &'a str,
    worktree: &'a str,
}

/// Run a single background job: create log directory, write metadata,
/// stream output to a log file, execute the command, and update metadata
/// with the final status.
fn run_single_background_job(
    job: &JobSpec,
    ctx: &JobInvocationContext<'_>,
    store: &LogStore,
    results: &Arc<Mutex<Vec<JobResult>>>,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
) -> NodeStatus {
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
        pid: Some(std::process::id()),
        background: true,
        finished_at: None,
        needs: job.needs.clone(),
        retention_seconds,
        max_log_size_bytes,
        log_truncated: false,
        original_size_bytes: None,
    };
    if let Err(e) = store.write_meta(&job_dir, &meta) {
        eprintln!("daft: failed to write meta for '{}': {e}", job.name);
    }

    // 3. Set up an mpsc channel for output streaming.
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    // 4. Spawn a log writer thread that reads from the channel and writes
    //    to output.log.
    let log_path = LogStore::log_path(&job_dir);
    let log_path_for_writer = log_path.clone();
    let log_writer_handle = std::thread::spawn(move || {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path_for_writer);
        match file {
            Ok(mut f) => {
                for line in rx {
                    let _ = writeln!(f, "{line}");
                }
            }
            Err(e) => {
                // Drain the channel even if we cannot write.
                eprintln!("daft: failed to open log file: {e}");
                for _line in rx {}
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

    // Remove from the child PID map now that the command has finished.
    child_pids.lock().unwrap().remove(&job.name);

    // Wait for the log writer thread to finish.
    log_writer_handle.join().ok();

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

    // Silent mode: only retain the log file if the job did not succeed
    // (failed or cancelled). On success, the log is best-effort deleted.
    if is_silent && node_status == NodeStatus::Succeeded {
        let _ = std::fs::remove_file(&log_path);
    }

    // 8. Write updated meta with the final status, exit code, and finish time.
    meta.status = status;
    meta.exit_code = exit_code;
    meta.finished_at = Some(chrono::Utc::now());
    if let Err(e) = store.write_meta(&job_dir, &meta) {
        eprintln!("daft: failed to update meta for '{}': {e}", job.name);
    }

    // 9. On failure, print a one-line notification to stderr (best-effort,
    //    catches EPIPE). Suppressed for silent background_output.
    if node_status == NodeStatus::Failed && !is_silent {
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
                    );
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No pending connection; sleep briefly and retry.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => {
                    // Unexpected error; break the loop.
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
fn handle_client_connection(
    stream: std::os::unix::net::UnixStream,
    store: &LogStore,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
    cancelled_jobs: &CancelledJobs,
    shutdown: &Arc<AtomicBool>,
) {
    use std::io::{BufRead, BufReader};

    // Set a read timeout so a misbehaving client doesn't block the listener.
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();

    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.is_empty() {
        return;
    }

    let request: CoordinatorRequest = match serde_json::from_str(&line) {
        Ok(r) => r,
        Err(_) => {
            let resp = CoordinatorResponse::Error {
                message: "Invalid request".to_string(),
            };
            send_response(&stream, &resp);
            return;
        }
    };

    let response = match request {
        CoordinatorRequest::ListJobs => {
            let jobs = build_job_list(store);
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
    };

    send_response(&stream, &response);
}

/// Build a list of job info from the log store.
#[cfg(unix)]
fn build_job_list(store: &LogStore) -> Vec<JobInfo> {
    let job_dirs = match store.list_job_dirs() {
        Ok(dirs) => dirs,
        Err(_) => return vec![],
    };

    let now = chrono::Utc::now();
    let mut jobs = Vec::new();

    for dir in job_dirs {
        if let Ok(meta) = store.read_meta(&dir) {
            let elapsed_secs = if matches!(meta.status, JobStatus::Running) {
                let elapsed = now.signed_duration_since(meta.started_at);
                Some(elapsed.num_seconds().max(0) as u64)
            } else {
                None
            };

            jobs.push(JobInfo {
                name: meta.name,
                hook_type: meta.hook_type,
                worktree: meta.worktree,
                status: meta.status,
                elapsed_secs,
                exit_code: meta.exit_code,
            });
        }
    }

    jobs
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
            message: format!("Job not found or not running: {name}"),
        }
    }
}

/// Write a JSON response to the stream.
#[cfg(unix)]
fn send_response(stream: &std::os::unix::net::UnixStream, response: &CoordinatorResponse) {
    use std::io::Write;
    let mut msg = match serde_json::to_string(response) {
        Ok(m) => m,
        Err(_) => return,
    };
    msg.push('\n');
    let mut writer = stream;
    let _ = writer.write_all(msg.as_bytes());
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

    Command::new(&exe)
        .arg("__coordinator")
        .arg(&state_path)
        .env("DAFT_IS_COORDINATOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("Could not spawn coordinator process from {}", exe.display()))?;

    Ok(None)
}

/// Backwards-compatible alias for callers still using the old name.
/// Functionally identical to [`spawn_coordinator`]; kept thin so the
/// rename across `src/hooks/yaml_executor/mod.rs` and
/// `src/commands/hooks/jobs.rs` can land in a follow-up.
#[cfg(unix)]
pub fn fork_coordinator(
    state: CoordinatorState,
    store: LogStore,
) -> Result<Option<Vec<JobResult>>> {
    spawn_coordinator(state, store)
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
    )
    .ok();

    let exit_code =
        match state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs) {
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
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(JobSpec {
            name: "echo-job".to_string(),
            command: "echo hello".to_string(),
            working_dir: std::env::temp_dir(),
            background: true,
            ..Default::default()
        });

        let results = state.run_all(&store).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].status.is_terminal());

        // Verify log was written
        let meta = store
            .read_meta(&tmp.path().join("inv-1").join("echo-job"))
            .unwrap();
        assert!(matches!(
            meta.status,
            crate::coordinator::log_store::JobStatus::Completed
        ));
    }

    #[test]
    fn test_build_job_list_empty_store() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
        let jobs = build_job_list(&store);
        assert!(jobs.is_empty());
    }

    #[test]
    fn test_build_job_list_with_jobs() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());

        // Create a completed job.
        let dir = store.create_job_dir("inv-1", "build").unwrap();
        let meta = JobMeta {
            name: "build".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "/tmp/wt".to_string(),
            command: "cargo build".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: Some(1234),
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();

        let jobs = build_job_list(&store);
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
        let store = LogStore::new(tmp.path().to_path_buf());

        let dir = store.create_job_dir("inv-1", "long-job").unwrap();
        let meta = JobMeta {
            name: "long-job".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "/tmp/wt".to_string(),
            command: "sleep 100".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now() - chrono::Duration::seconds(30),
            status: JobStatus::Running,
            exit_code: None,
            pid: Some(9999),
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();

        let jobs = build_job_list(&store);
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
            CoordinatorResponse::Error { message } if message.contains("not found")
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
            .run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Skipped);
    }

    #[test]
    fn test_run_all_populates_job_hook_type_and_worktree() {
        let tmp = TempDir::new().unwrap();
        let store = LogStore::new(tmp.path().to_path_buf());
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

        state.run_all(&store).unwrap();

        let meta = store
            .read_meta(&tmp.path().join("inv-pop-1").join("check-job"))
            .unwrap();
        assert_eq!(meta.hook_type, "worktree-post-create");
        assert_eq!(meta.worktree, "feature/y");
        assert!(meta.background);
        assert!(meta.finished_at.is_some());
    }

    #[test]
    fn test_socket_listener_list_jobs() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("test-listener.sock");
        let store_dir = tmp.path().join("store");
        std::fs::create_dir_all(&store_dir).unwrap();

        let store = LogStore::new(store_dir.clone());

        // Create a job in the store so ListJobs returns something.
        let dir = store.create_job_dir("inv-1", "test-job").unwrap();
        let meta = JobMeta {
            name: "test-job".to_string(),
            hook_type: "post-clone".to_string(),
            worktree: "/tmp/wt".to_string(),
            command: "echo test".to_string(),
            working_dir: "/tmp/wt".to_string(),
            env: HashMap::new(),
            started_at: chrono::Utc::now(),
            status: JobStatus::Completed,
            exit_code: Some(0),
            pid: Some(1234),
            background: false,
            finished_at: None,
            needs: vec![],
            retention_seconds: None,
            max_log_size_bytes: None,
            log_truncated: false,
            original_size_bytes: None,
        };
        store.write_meta(&dir, &meta).unwrap();

        // Manually bind the listener (bypassing coordinator_socket_path).
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancel_all = Arc::new(AtomicBool::new(false));
        let cancelled_jobs: CancelledJobs = Arc::new(Mutex::new(HashSet::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

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
                        );
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        // Client: connect and send ListJobs.
        let stream = UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();

        let mut msg = serde_json::to_string(&CoordinatorRequest::ListJobs).unwrap();
        msg.push('\n');
        (&stream).write_all(msg.as_bytes()).unwrap();

        let mut reader = BufReader::new(&stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).unwrap();

        let response: CoordinatorResponse = serde_json::from_str(&response_line).unwrap();
        match response {
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
        use std::io::{BufRead, BufReader};

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

        let stream = std::os::unix::net::UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();

        let resp: CoordinatorResponse = serde_json::from_str(&line).unwrap();
        assert!(matches!(
            resp,
            CoordinatorResponse::Ack { message } if message == "ok"
        ));

        handle.join().unwrap();
    }

    fn make_ctx(inv: &str) -> JobInvocationContext<'_> {
        JobInvocationContext {
            invocation_id: inv,
            hook_type: "worktree-post-create",
            worktree: "feat/x",
        }
    }

    #[allow(clippy::type_complexity)]
    fn make_test_state() -> (
        TempDir,
        LogStore,
        ChildPidMap,
        Arc<AtomicBool>,
        CancelledJobs,
        Arc<Mutex<Vec<JobResult>>>,
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

        let _ = run_single_background_job(
            &job,
            &ctx,
            &store,
            &results,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
        );

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
            let store_for_killer = store.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(200));
                // Route through the production cancellation path so that
                // regressions (e.g. dropping the `cancelled_jobs` insert or
                // skipping the SIGTERM) are caught here.
                let _ = cancel_single_job("long-job", &pids, &cancelled, &store_for_killer);
            })
        };

        let _ = run_single_background_job(
            &job,
            &ctx,
            &store,
            &results,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
        );
        killer.join().unwrap();

        let job_dir = store.base_dir.join(&inv_id).join("long-job");
        let meta = store.read_meta(&job_dir).expect("meta should exist");
        assert!(
            matches!(meta.status, JobStatus::Cancelled),
            "expected Cancelled, got {:?}",
            meta.status
        );
    }

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

        let _ = run_single_background_job(
            &job,
            &ctx,
            &store,
            &results,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
        );

        let job_dir = store.base_dir.join(&inv_id).join("silent-ok");
        let log_path = LogStore::log_path(&job_dir);
        assert!(
            !log_path.exists(),
            "silent + success should leave no log file"
        );
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

        let _ = run_single_background_job(
            &job,
            &ctx,
            &store,
            &results,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
        );

        let job_dir = store.base_dir.join(&inv_id).join("silent-fail");
        let log_path = LogStore::log_path(&job_dir);
        assert!(
            log_path.exists(),
            "silent + failure should preserve log file"
        );
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

        let _ = run_single_background_job(
            &job,
            &ctx,
            &store,
            &results,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
        );

        let job_dir = store.base_dir.join(&inv_id).join("loud-ok");
        let log_path = LogStore::log_path(&job_dir);
        assert!(log_path.exists(), "non-silent should always retain log");
    }

    #[test]
    fn silent_bg_output_keeps_log_when_status_is_cancelled() {
        use crate::executor::BackgroundOutput;
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, results) = make_test_state();

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
        let ctx = make_ctx(&inv_id);

        let _ = run_single_background_job(
            &job,
            &ctx,
            &store,
            &results,
            &child_pids,
            &cancel_all,
            &cancelled_jobs,
        );

        let job_dir = store.base_dir.join(&inv_id).join("pre-cancelled");
        let log_path = LogStore::log_path(&job_dir);
        assert!(
            log_path.exists(),
            "silent + cancelled (even when cmd succeeded) should preserve log"
        );

        let meta = store.read_meta(&job_dir).expect("meta should exist");
        assert!(
            matches!(meta.status, JobStatus::Cancelled),
            "expected Cancelled, got {:?}",
            meta.status
        );
    }

    #[test]
    fn bg_dependent_waits_for_dep_to_finish() {
        // Regression test for daft#454: B `needs: [A]` must not start until A
        // has terminated.
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) = make_test_state();

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

    #[test]
    fn bg_dependent_skipped_when_dep_fails() {
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) = make_test_state();

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
            .run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs)
            .unwrap();

        let meta_fails = store
            .read_meta(&store.base_dir.join("inv-needs-fail-1").join("fails"))
            .expect("meta fails");
        assert!(matches!(meta_fails.status, JobStatus::Failed));

        // The dependent's closure was NOT invoked (no spawn) — but the
        // coordinator synthesizes a `meta.json` after the DAG returns so the
        // job remains visible to `daft hooks jobs`. Disk status is `Skipped`
        // (the closest available variant). The job's command must NOT have
        // produced its side-effect.
        let dep_dir = store.base_dir.join("inv-needs-fail-1").join("dependent");
        let meta_dependent = store
            .read_meta(&dep_dir)
            .expect("dependent should have a synthesized dep-failed meta");
        assert!(
            matches!(meta_dependent.status, JobStatus::Skipped),
            "dep-failed dependent should be Skipped on disk, got {:?}",
            meta_dependent.status
        );
        assert_eq!(meta_dependent.needs, vec!["fails".to_string()]);
        assert!(
            !marker.exists(),
            "dependent ran its command despite dep failing"
        );
    }

    #[test]
    fn bg_dependent_skipped_when_dep_cancelled() {
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) = make_test_state();

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

        // After the DAG returns, the coordinator synthesizes a meta record
        // for the dep-failed dependent so it remains visible to
        // `daft hooks jobs`. Disk status is `Skipped` (no `DepFailed` variant
        // exists in `JobStatus`). The job's command must NOT have run.
        let after_dir = store.base_dir.join("inv-needs-cancel-1").join("after");
        let meta_after = store
            .read_meta(&after_dir)
            .expect("after should have a synthesized dep-failed meta");
        assert!(
            matches!(meta_after.status, JobStatus::Skipped),
            "after should be Skipped on disk (dep cancelled), got {:?}",
            meta_after.status
        );
        assert_eq!(meta_after.needs, vec!["long".to_string()]);
        assert!(
            !marker.exists(),
            "after ran its command despite dep being cancelled"
        );
    }

    #[test]
    fn bg_cycle_in_needs_returns_error() {
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) = make_test_state();

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

        let result = state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs);
        assert!(result.is_err(), "cycle should be reported as an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("invalid background job DAG"),
            "error should mention invalid DAG, got: {msg}"
        );
    }

    #[test]
    fn bg_missing_dep_in_needs_returns_error() {
        let (_tmp, store, child_pids, cancel_all, cancelled_jobs, _results) = make_test_state();

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

        let result = state.run_all_with_cancel(&store, &child_pids, &cancel_all, &cancelled_jobs);
        assert!(
            result.is_err(),
            "missing dep should be reported as an error"
        );
    }
}
