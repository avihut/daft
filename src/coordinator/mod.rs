pub mod clean_policy;
pub mod client;
pub mod framing;
pub mod log_record;
pub mod log_store;
pub mod process;
pub mod store;

use serde::{Deserialize, Serialize};

/// Request from CLI to coordinator.
#[derive(Debug, Serialize, Deserialize)]
pub enum CoordinatorRequest {
    /// List all jobs and their current status.
    ListJobs,
    /// Cancel a specific job by name.
    CancelJob { name: String },
    /// Cancel all running jobs.
    CancelAll,
    /// Graceful shutdown of the coordinator.
    Shutdown,
    /// Cancel every active job matching the supplied predicates.
    ///
    /// All present predicates AND together. The coordinator filters its
    /// redb-recorded `Running`/`Cancelling` rows and signals SIGTERM to each
    /// match's process group. Use `None` for predicates you don't want to
    /// constrain. At least one predicate is required (validated client-side
    /// in `Cancel` subcommand handler).
    CancelMatching {
        hook: Option<String>,
        worktree: Option<String>,
        tag: Option<String>,
        invocation_prefix: Option<String>,
        older_than_secs: Option<u64>,
    },
}

/// Response from coordinator to CLI.
#[derive(Debug, Serialize, Deserialize)]
pub enum CoordinatorResponse {
    /// List of job statuses.
    Jobs(Vec<JobInfo>),
    /// Acknowledgement with optional message.
    Ack { message: String },
    /// Result of a `CancelMatching` — count of jobs signaled + their names.
    Cancelled { count: usize, names: Vec<String> },
    /// Error response.
    Error { message: String },
}

/// Summary info about a background job.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobInfo {
    pub name: String,
    pub hook_type: String,
    pub worktree: String,
    pub status: log_store::JobStatus,
    pub elapsed_secs: Option<u64>,
    pub exit_code: Option<i32>,
}

/// Returns the socket path for a coordinator.
pub fn coordinator_socket_path(repo_hash: &str) -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::daft_state_dir()?.join(format!("coordinator-{repo_hash}.sock")))
}

/// Returns the PID file path for a coordinator.
pub fn coordinator_pid_path(repo_hash: &str) -> anyhow::Result<std::path::PathBuf> {
    Ok(crate::daft_state_dir()?.join(format!("coordinator-{repo_hash}.pid")))
}
