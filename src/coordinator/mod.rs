pub mod clean_policy;
pub mod client;
pub mod framing;
pub mod log_record;
pub mod log_store;
pub mod process;
pub mod store;
pub mod types;

pub use types::JobAddress;

use serde::{Deserialize, Serialize};

/// Current wire-protocol version. Servers tag every framed envelope with
/// this value; mismatches surface as a `SchemaMismatch` error rather than
/// silent JSON parse failures.
pub const PROTOCOL_VERSION: u16 = 1;

/// Wire envelope for requests. Encoded as `{"v":1,"req":"<name>","payload":{...}}`
/// thanks to `#[serde(flatten)]` on the body.
#[derive(Debug, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub v: u16,
    #[serde(flatten)]
    pub body: CoordinatorRequest,
}

/// Wire envelope for responses. Encoded as `{"v":1,"kind":"<variant>","payload":...}`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub v: u16,
    #[serde(flatten)]
    pub body: CoordinatorResponse,
}

/// Request from CLI to coordinator.
///
/// Variants are serde-tagged `#[serde(tag = "req", content = "payload")]`
/// so the wire shape lives inside [`RequestEnvelope`] without colliding
/// with the `v` field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "req", content = "payload")]
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
    /// Streaming tail of a job's structured log file. Server emits one
    /// `StreamFrame` envelope per `LogRecord` then a single `StreamEnd`
    /// when the source closes (job finished, or `follow=false` reached
    /// EOF). Implementation lives in
    /// `coordinator::process::handle_tail_logs`.
    TailLogs {
        job: types::JobAddress,
        /// `true` blocks at EOF waiting for more records (until job exits
        /// or coordinator shuts down). `false` reads through current EOF
        /// then sends `StreamEnd`.
        follow: bool,
        /// Skip records with `seq < since_seq`. Lets clients resume after
        /// a disconnect without replaying records they already saw.
        #[serde(default)]
        since_seq: Option<u64>,
    },
}

/// Stable error codes carried inside [`CoordinatorResponse::Error`]. Fixed
/// strings so older clients can still recognize them after the enum grows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    JobNotFound,
    NoMatch,
    KillFailed,
    SchemaMismatch,
    Internal,
}

/// Response from coordinator to CLI. Tagged `#[serde(tag = "kind", content = "payload")]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum CoordinatorResponse {
    /// List of job statuses.
    Jobs(Vec<JobInfo>),
    /// Acknowledgement with optional message.
    Ack { message: String },
    /// Result of a `CancelMatching` — count of jobs signaled + their names.
    Cancelled { count: usize, names: Vec<String> },
    /// Error response with a typed code + human-readable message.
    Error { code: ErrorCode, message: String },
    /// One frame of a streaming response. Body is a serialized
    /// [`crate::coordinator::log_record::LogRecord`] (or any other
    /// streaming payload added later).
    StreamFrame(serde_json::Value),
    /// End-of-stream sentinel. Server closes the connection after sending.
    StreamEnd,
}

/// Summary info about a background job.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
