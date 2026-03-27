//! Coordinator process for running background hook jobs.
//!
//! The coordinator is a single forked child process that runs background
//! hook jobs as threads. When background jobs exist, the parent daft
//! process forks once. The parent prints a summary and exits. The child
//! (coordinator) runs the background jobs, writes logs, and exits when done.

use super::log_store::{JobMeta, JobStatus, LogStore};
use crate::executor::command::run_command;
use crate::executor::{JobResult, JobSpec, NodeStatus};
use anyhow::Result;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::Instant;

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
        let mut handles = Vec::new();
        let results = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        for job in &self.jobs {
            let job = job.clone();
            let inv_id = self.invocation_id.clone();
            let store_base = store.base_dir.clone();
            let results = std::sync::Arc::clone(&results);

            let handle = std::thread::spawn(move || {
                let local_store = LogStore::new(store_base);
                run_single_background_job(&job, &inv_id, &local_store, &results);
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().ok();
        }

        let results = match std::sync::Arc::try_unwrap(results) {
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
    results: &std::sync::Arc<std::sync::Mutex<Vec<JobResult>>>,
) {
    let start = Instant::now();

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

    // 6. Wait for the log writer thread to finish.
    log_writer_handle.join().ok();

    let duration = start.elapsed();

    // 7. Update meta with final status.
    let (status, node_status, exit_code) = match &cmd_result {
        Ok(cr) if cr.success => (JobStatus::Completed, NodeStatus::Succeeded, cr.exit_code),
        Ok(cr) => (JobStatus::Failed, NodeStatus::Failed, cr.exit_code),
        Err(_) => (JobStatus::Failed, NodeStatus::Failed, None),
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

            let _results = state.run_all(&store)?;
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
}
