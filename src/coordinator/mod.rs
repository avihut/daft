pub mod client;
pub mod log_store;
pub mod process;

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
}

/// Response from coordinator to CLI.
#[derive(Debug, Serialize, Deserialize)]
pub enum CoordinatorResponse {
    /// List of job statuses.
    Jobs(Vec<JobInfo>),
    /// Acknowledgement with optional message.
    Ack { message: String },
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
