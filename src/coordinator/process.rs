//! Coordinator process for running background hook jobs.
//!
//! The coordinator is a single forked child process that runs background
//! hook jobs as threads. When background jobs exist, the parent daft
//! process forks once. The parent prints a summary and exits. The child
//! (coordinator) runs the background jobs, writes logs, and exits when done.
//!
//! A Unix socket listener runs in a separate thread alongside job execution,
//! handling IPC requests from CLI commands (`daft hooks jobs`).

use super::log_store::{JobMeta, JobStatus, LogStore};
use super::{
    coordinator_pid_path, coordinator_socket_path, CoordinatorRequest, CoordinatorResponse, JobInfo,
};
use crate::executor::command::run_command;
use crate::executor::{JobResult, JobSpec, NodeStatus};
use anyhow::Result;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Shared state for tracking running child processes.
/// Maps job name to the child process PID, allowing cancellation.
type ChildPidMap = Arc<Mutex<HashMap<String, u32>>>;

/// State for a coordinator process managing background jobs.
pub struct CoordinatorState {
    pub repo_hash: String,
    pub invocation_id: String,
    pub jobs: Vec<JobSpec>,
}

impl CoordinatorState {
    pub fn new(repo_hash: &str, invocation_id: &str) -> Self {
        Self {
            repo_hash: repo_hash.to_string(),
            invocation_id: invocation_id.to_string(),
            jobs: Vec::new(),
        }
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
        )
    }

    /// Run all background jobs with cancellation support.
    ///
    /// `child_pids` is shared with the socket listener so it can send
    /// SIGTERM to running child processes. `cancel_all` is a global flag
    /// that, when set, causes new jobs to skip and running jobs to be killed.
    fn run_all_with_cancel(
        &self,
        store: &LogStore,
        child_pids: &ChildPidMap,
        cancel_all: &Arc<AtomicBool>,
    ) -> Result<Vec<JobResult>> {
        let mut handles = Vec::new();
        let results = Arc::new(Mutex::new(Vec::new()));

        for job in &self.jobs {
            let job = job.clone();
            let inv_id = self.invocation_id.clone();
            let store_base = store.base_dir.clone();
            let results = Arc::clone(&results);
            let child_pids = Arc::clone(child_pids);
            let cancel_all = Arc::clone(cancel_all);

            let handle = std::thread::spawn(move || {
                let local_store = LogStore::new(store_base);
                run_single_background_job(
                    &job,
                    &inv_id,
                    &local_store,
                    &results,
                    &child_pids,
                    &cancel_all,
                );
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().ok();
        }

        let results = match Arc::try_unwrap(results) {
            Ok(mutex) => mutex.into_inner().unwrap_or_default(),
            Err(arc) => arc.lock().unwrap().clone(),
        };

        Ok(results)
    }
}

/// Run a single background job: create log directory, write metadata,
/// stream output to a log file, execute the command, and update metadata
/// with the final status.
fn run_single_background_job(
    job: &JobSpec,
    invocation_id: &str,
    store: &LogStore,
    results: &Arc<Mutex<Vec<JobResult>>>,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
) {
    let start = Instant::now();

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
        return;
    }

    // 1. Create the job log directory.
    let job_dir = match store.create_job_dir(invocation_id, &job.name) {
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
            return;
        }
    };

    // 2. Write initial meta with Running status.
    let mut meta = JobMeta {
        name: job.name.clone(),
        hook_type: String::new(),
        worktree: String::new(),
        command: job.command.clone(),
        working_dir: job.working_dir.display().to_string(),
        env: job.env.clone(),
        started_at: chrono::Utc::now(),
        status: JobStatus::Running,
        exit_code: None,
        pid: Some(std::process::id()),
        background: true,
        finished_at: None,
    };
    if let Err(e) = store.write_meta(&job_dir, &meta) {
        eprintln!("daft: failed to write meta for '{}': {e}", job.name);
    }

    // 3. Set up an mpsc channel for output streaming.
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    // 4. Spawn a log writer thread that reads from the channel and writes
    //    to output.log.
    let log_path = LogStore::log_path(&job_dir);
    let log_writer_handle = std::thread::spawn(move || {
        let file = OpenOptions::new().create(true).append(true).open(&log_path);
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

    // 5. Call run_command() to execute the shell command.
    let cmd_result = run_command(
        &job.command,
        &job.env,
        &job.working_dir,
        job.timeout,
        Some(tx),
    );

    // Remove from the child PID map now that the command has finished.
    child_pids.lock().unwrap().remove(&job.name);

    // 6. Wait for the log writer thread to finish.
    log_writer_handle.join().ok();

    let duration = start.elapsed();

    // 7. Determine final status, considering cancellation.
    let was_cancelled = cancel_all.load(Ordering::Relaxed);

    let (status, node_status, exit_code) = if was_cancelled {
        (JobStatus::Cancelled, NodeStatus::Skipped, None)
    } else {
        match &cmd_result {
            Ok(cr) if cr.success => (JobStatus::Completed, NodeStatus::Succeeded, cr.exit_code),
            Ok(cr) => (JobStatus::Failed, NodeStatus::Failed, cr.exit_code),
            Err(_) => (JobStatus::Failed, NodeStatus::Failed, None),
        }
    };

    meta.status = status;
    meta.exit_code = exit_code;
    if let Err(e) = store.write_meta(&job_dir, &meta) {
        eprintln!("daft: failed to update meta for '{}': {e}", job.name);
    }

    // 8. On failure, print a one-line notification to stderr (best-effort,
    //    catches EPIPE).
    if node_status == NodeStatus::Failed {
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

    // 9. Push the JobResult to the shared results vec.
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
}

/// Start a Unix socket listener that handles IPC requests from CLI clients.
///
/// The listener runs in a separate thread and processes requests until:
/// - A `Shutdown` request is received
/// - The `shutdown` flag is set (e.g., when all jobs complete)
///
/// Returns a `JoinHandle` for the listener thread.
fn start_socket_listener(
    repo_hash: &str,
    store_base: std::path::PathBuf,
    child_pids: ChildPidMap,
    cancel_all: Arc<AtomicBool>,
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
                    handle_client_connection(stream, &store, &child_pids, &cancel_all, &shutdown);
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
fn handle_client_connection(
    stream: std::os::unix::net::UnixStream,
    store: &LogStore,
    child_pids: &ChildPidMap,
    cancel_all: &Arc<AtomicBool>,
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
        CoordinatorRequest::CancelJob { name } => cancel_single_job(&name, child_pids, store),
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
                // SAFETY: Sending SIGTERM to a child process we own.
                unsafe {
                    libc::kill(*pid as libc::pid_t, libc::SIGTERM);
                }
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

            // Kill all running children.
            let pids: Vec<u32> = child_pids.lock().unwrap().values().copied().collect();
            for pid in pids {
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGTERM);
                }
            }

            CoordinatorResponse::Ack {
                message: "Coordinator shutting down".to_string(),
            }
        }
    };

    send_response(&stream, &response);
}

/// Build a list of job info from the log store.
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

/// Cancel a single job by name.
fn cancel_single_job(
    name: &str,
    child_pids: &ChildPidMap,
    _store: &LogStore,
) -> CoordinatorResponse {
    let pids = child_pids.lock().unwrap();
    if let Some(&pid) = pids.get(name) {
        // SAFETY: Sending SIGTERM to a child process we own.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
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
fn write_pid_file(repo_hash: &str) -> Result<std::path::PathBuf> {
    let pid_path = coordinator_pid_path(repo_hash)?;
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;
    Ok(pid_path)
}

/// Fork the current process into a coordinator that runs background jobs.
///
/// The parent process returns `Ok(None)` immediately.
/// The child process runs all jobs, then exits via `process::exit(0)`.
///
/// # Safety
/// Uses `libc::fork()`. Call after all foreground work is complete.
#[cfg(unix)]
pub fn fork_coordinator(
    state: CoordinatorState,
    store: LogStore,
) -> Result<Option<Vec<JobResult>>> {
    use std::process;

    // SAFETY: fork() is called after all foreground work is complete.
    // The child process runs background jobs as threads and exits.
    let pid = unsafe { libc::fork() };

    match pid {
        -1 => anyhow::bail!("fork() failed: {}", std::io::Error::last_os_error()),
        0 => {
            // Child: become the coordinator

            // SAFETY: setsid() creates a new session so the coordinator
            // survives the parent's terminal exit.
            unsafe {
                libc::setsid();
            }

            // Set the guard env var to prevent recursive background spawning.
            // SAFETY: This is called in the forked child before spawning any
            // threads, so there is no concurrent access to the environment.
            unsafe {
                std::env::set_var("DAFT_IS_COORDINATOR", "1");
            }

            // Write PID file (best-effort).
            let pid_path = write_pid_file(&state.repo_hash).ok();

            // Shared state for cancellation.
            let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
            let cancel_all = Arc::new(AtomicBool::new(false));
            let shutdown = Arc::new(AtomicBool::new(false));

            // Start the socket listener thread.
            let listener_handle = start_socket_listener(
                &state.repo_hash,
                store.base_dir.clone(),
                Arc::clone(&child_pids),
                Arc::clone(&cancel_all),
                Arc::clone(&shutdown),
            )
            .ok();

            // Run all jobs (blocking).
            let _results = state.run_all_with_cancel(&store, &child_pids, &cancel_all)?;

            // Signal the listener to stop and wait for it.
            shutdown.store(true, Ordering::Relaxed);
            if let Some((handle, socket_path)) = listener_handle {
                // Clean up the socket file to unblock the listener if it's
                // in accept().
                std::fs::remove_file(&socket_path).ok();
                handle.join().ok();
            }

            // Clean up PID file.
            if let Some(path) = pid_path {
                std::fs::remove_file(&path).ok();
            }

            process::exit(0);
        }
        _child_pid => {
            // Parent: return immediately
            Ok(None)
        }
    }
}

#[cfg(test)]
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
    fn test_coordinator_state_add_jobs() {
        let mut state = CoordinatorState::new("test-repo", "inv-1");
        state.add_job(test_job("job-a"));
        state.add_job(test_job("job-b"));
        assert_eq!(state.jobs.len(), 2);
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

        let response = cancel_single_job("nonexistent", &child_pids, &store);
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

        let results = state
            .run_all_with_cancel(&store, &child_pids, &cancel_all)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Skipped);
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
        };
        store.write_meta(&dir, &meta).unwrap();

        // Manually bind the listener (bypassing coordinator_socket_path).
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        listener.set_nonblocking(true).unwrap();

        let child_pids: ChildPidMap = Arc::new(Mutex::new(HashMap::new()));
        let cancel_all = Arc::new(AtomicBool::new(false));
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
}
