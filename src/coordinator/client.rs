//! Client for communicating with a running coordinator over Unix socket IPC.
//!
//! The protocol is length-prefixed framed JSON envelopes. Each frame is
//! a `u32` big-endian length followed by a UTF-8 JSON [`RequestEnvelope`]
//! (request) or [`ResponseEnvelope`] (response). Single-response requests
//! exchange one frame in each direction; streaming requests (e.g.
//! [`CoordinatorRequest::TailLogs`]) get many `StreamFrame` envelopes
//! terminated by exactly one `StreamEnd`, then the server closes.
//!
//! IPC is Unix-only. On non-Unix platforms `CoordinatorClient` exposes the
//! same API but `connect()` always returns `Ok(None)` — there is no
//! coordinator to talk to.

use super::JobInfo;
#[cfg(unix)]
use super::{
    CoordinatorRequest, CoordinatorResponse, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope,
    coordinator_socket_path, framing,
};
#[cfg(unix)]
use anyhow::Context;
use anyhow::Result;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::time::Duration;

/// Client for communicating with a running coordinator.
#[cfg(unix)]
pub struct CoordinatorClient {
    stream: UnixStream,
}

/// Stub on non-Unix platforms — the field is `Infallible`, so the type
/// can never be instantiated. `connect()` always returns `Ok(None)` and
/// the rest of the surface is reachable only through an instance, so it
/// is statically unreachable.
#[cfg(not(unix))]
pub struct CoordinatorClient(std::convert::Infallible);

#[cfg(not(unix))]
impl CoordinatorClient {
    /// No coordinator IPC on non-Unix platforms.
    pub fn connect(_repo_hash: &str) -> Result<Option<Self>> {
        Ok(None)
    }

    pub fn list_jobs(&mut self) -> Result<Vec<JobInfo>> {
        match self.0 {}
    }

    pub fn cancel_job(&mut self, _name: &str) -> Result<String> {
        match self.0 {}
    }

    pub fn cancel_all(&mut self) -> Result<String> {
        match self.0 {}
    }

    pub fn cancel_matching(
        &mut self,
        _hook: Option<&str>,
        _worktree: Option<&str>,
        _tag: Option<&str>,
        _invocation_prefix: Option<&str>,
        _older_than_secs: Option<u64>,
    ) -> Result<Vec<String>> {
        match self.0 {}
    }
}

#[cfg(unix)]
impl CoordinatorClient {
    /// Connect to the coordinator for the given repo.
    /// Returns `None` if no coordinator is running.
    pub fn connect(repo_hash: &str) -> Result<Option<Self>> {
        let socket_path = coordinator_socket_path(repo_hash)?;
        if !socket_path.exists() {
            return Ok(None);
        }

        match UnixStream::connect(&socket_path) {
            Ok(stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                Ok(Some(Self { stream }))
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                // Stale socket file -- clean it up.
                std::fs::remove_file(&socket_path).ok();
                Ok(None)
            }
            Err(e) => Err(e).context("Failed to connect to coordinator"),
        }
    }

    /// Create a client from an already-connected `UnixStream`.
    ///
    /// This is useful in tests where the socket is created directly
    /// rather than via `coordinator_socket_path()`.
    #[cfg(test)]
    fn from_stream(stream: UnixStream) -> Self {
        Self { stream }
    }

    /// Send a framed envelope and receive exactly one framed envelope back.
    /// For streaming responses, use [`Self::stream_request`] instead.
    pub fn send(&mut self, request: &CoordinatorRequest) -> Result<CoordinatorResponse> {
        self.write_request(request)?;
        self.read_response()
    }

    fn write_request(&mut self, request: &CoordinatorRequest) -> Result<()> {
        let envelope = RequestEnvelope {
            v: PROTOCOL_VERSION,
            body: request.clone(),
        };
        let bytes = serde_json::to_vec(&envelope)?;
        framing::write_frame(&mut self.stream, &bytes)?;
        Ok(())
    }

    fn read_response(&mut self) -> Result<CoordinatorResponse> {
        let bytes = framing::read_frame(&mut self.stream)?;
        let env: ResponseEnvelope =
            serde_json::from_slice(&bytes).context("decode coordinator response envelope")?;
        if env.v != PROTOCOL_VERSION {
            anyhow::bail!(
                "coordinator response wire-version {} is incompatible with client {}",
                env.v,
                PROTOCOL_VERSION
            );
        }
        Ok(env.body)
    }

    /// Send a streaming request. Returns an iterator over each
    /// [`CoordinatorResponse::StreamFrame`] payload; iteration ends after
    /// a `StreamEnd` envelope (or on stream close). Non-stream variants
    /// (`Error`, `Jobs`, etc.) are yielded once and then iteration ends.
    pub fn stream_request(
        &mut self,
        request: &CoordinatorRequest,
    ) -> Result<StreamingResponse<'_>> {
        self.write_request(request)?;
        // Streaming responses can take a long time; relax the read timeout
        // so a slow-producing tail doesn't error out.
        let _ = self.stream.set_read_timeout(None);
        Ok(StreamingResponse { client: self })
    }
}

/// Iterator over a streaming coordinator response. Each `next()` returns
/// the next frame's body or `None` once the stream ends (terminator
/// envelope received or socket closed).
#[cfg(unix)]
pub struct StreamingResponse<'a> {
    client: &'a mut CoordinatorClient,
}

#[cfg(unix)]
impl<'a> Iterator for StreamingResponse<'a> {
    type Item = Result<CoordinatorResponse>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.client.read_response() {
            Ok(CoordinatorResponse::StreamEnd) => None,
            Ok(resp) => Some(Ok(resp)),
            Err(e) => {
                // EOF from the server is the natural end of a stream;
                // surface other I/O errors so callers can act on them.
                if let Some(io_err) = e.downcast_ref::<std::io::Error>()
                    && io_err.kind() == std::io::ErrorKind::UnexpectedEof
                {
                    return None;
                }
                Some(Err(e))
            }
        }
    }
}

#[cfg(unix)]
impl CoordinatorClient {
    /// List all jobs from the coordinator.
    pub fn list_jobs(&mut self) -> Result<Vec<JobInfo>> {
        match self.send(&CoordinatorRequest::ListJobs)? {
            CoordinatorResponse::Jobs(jobs) => Ok(jobs),
            CoordinatorResponse::Error { message, .. } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    /// Cancel a specific job by name.
    pub fn cancel_job(&mut self, name: &str) -> Result<String> {
        match self.send(&CoordinatorRequest::CancelJob {
            name: name.to_string(),
        })? {
            CoordinatorResponse::Ack { message } => Ok(message),
            CoordinatorResponse::Error { message, .. } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    /// Cancel all running jobs.
    pub fn cancel_all(&mut self) -> Result<String> {
        match self.send(&CoordinatorRequest::CancelAll)? {
            CoordinatorResponse::Ack { message } => Ok(message),
            CoordinatorResponse::Error { message, .. } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    /// Cancel every active job matching the predicate set. Returns the
    /// names of the jobs that were signaled.
    pub fn cancel_matching(
        &mut self,
        hook: Option<&str>,
        worktree: Option<&str>,
        tag: Option<&str>,
        invocation_prefix: Option<&str>,
        older_than_secs: Option<u64>,
    ) -> Result<Vec<String>> {
        let req = CoordinatorRequest::CancelMatching {
            hook: hook.map(str::to_string),
            worktree: worktree.map(str::to_string),
            tag: tag.map(str::to_string),
            invocation_prefix: invocation_prefix.map(str::to_string),
            older_than_secs,
        };
        match self.send(&req)? {
            CoordinatorResponse::Cancelled { names, .. } => Ok(names),
            CoordinatorResponse::Error { message, .. } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    /// Open a streaming tail of a job's `output.jsonl` from the
    /// coordinator. Returns an iterator that yields one [`crate::coordinator::log_record::LogRecord`]
    /// per `StreamFrame` envelope until `StreamEnd` or socket close.
    pub fn tail_logs(
        &mut self,
        job: super::JobAddress,
        follow: bool,
        since_seq: Option<u64>,
    ) -> Result<StreamingResponse<'_>> {
        self.stream_request(&CoordinatorRequest::TailLogs {
            job,
            follow,
            since_seq,
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::coordinator::ErrorCode;
    use std::os::unix::net::{UnixListener, UnixStream};

    /// Helper: spin a one-shot listener that frames a single `ResponseEnvelope`
    /// back and returns the deserialized request the client sent.
    fn one_shot_server(
        socket_path: std::path::PathBuf,
        response: CoordinatorResponse,
    ) -> std::thread::JoinHandle<CoordinatorRequest> {
        let listener = UnixListener::bind(&socket_path).unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let req_bytes = framing::read_frame(&mut stream).unwrap();
            let env: RequestEnvelope = serde_json::from_slice(&req_bytes).unwrap();
            let response_env = ResponseEnvelope {
                v: PROTOCOL_VERSION,
                body: response,
            };
            let bytes = serde_json::to_vec(&response_env).unwrap();
            framing::write_frame(&mut stream, &bytes).unwrap();
            env.body
        })
    }

    #[test]
    fn ipc_round_trip_list_jobs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let handle = one_shot_server(socket_path.clone(), CoordinatorResponse::Jobs(vec![]));

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let jobs = client.list_jobs().unwrap();
        assert!(jobs.is_empty());

        let req = handle.join().unwrap();
        assert!(matches!(req, CoordinatorRequest::ListJobs));
    }

    #[test]
    fn ipc_round_trip_cancel_job() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let handle = one_shot_server(
            socket_path.clone(),
            CoordinatorResponse::Ack {
                message: "Cancelled job: build".to_string(),
            },
        );

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let result = client.cancel_job("build").unwrap();
        assert_eq!(result, "Cancelled job: build");

        let req = handle.join().unwrap();
        assert!(matches!(req, CoordinatorRequest::CancelJob { ref name } if name == "build"));
    }

    #[test]
    fn ipc_round_trip_error_response_is_surfaced() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let handle = one_shot_server(
            socket_path.clone(),
            CoordinatorResponse::Error {
                code: ErrorCode::JobNotFound,
                message: "Job not found: unknown".to_string(),
            },
        );

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let result = client.cancel_job("unknown");
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Job not found: unknown"));

        let _ = handle.join().unwrap();
    }

    #[test]
    fn ipc_round_trip_cancel_matching_returns_names() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let handle = one_shot_server(
            socket_path.clone(),
            CoordinatorResponse::Cancelled {
                count: 2,
                names: vec!["a".into(), "b".into()],
            },
        );

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let names = client
            .cancel_matching(Some("h"), None, None, None, None)
            .unwrap();
        assert_eq!(names, vec!["a", "b"]);

        let req = handle.join().unwrap();
        match req {
            CoordinatorRequest::CancelMatching { hook, .. } => {
                assert_eq!(hook.as_deref(), Some("h"))
            }
            other => panic!("unexpected request: {other:?}"),
        }
    }

    #[test]
    fn test_connect_no_socket_returns_none() {
        // Use a hash that won't have a real socket.
        // We override the state dir via the env var to use a temp dir.
        let _tmp = tempfile::TempDir::new().unwrap();
        // The connect method uses coordinator_socket_path which uses daft_state_dir().
        // In dev builds, we can set DAFT_STATE_DIR. In non-dev builds this
        // test still works because the socket path simply won't exist.
        let result = CoordinatorClient::connect("nonexistent-repo-hash-for-test");
        // This should return Ok(None) because the socket doesn't exist.
        // If daft_state_dir() fails (non-dev build without XDG), we just
        // skip the assertion.
        if let Ok(opt) = result {
            assert!(opt.is_none());
        }
    }

    #[test]
    fn test_request_serialization() {
        let request = CoordinatorRequest::ListJobs;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorRequest::ListJobs));

        let request = CoordinatorRequest::CancelJob {
            name: "build".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("build"));
        let deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            CoordinatorRequest::CancelJob { name } if name == "build"
        ));

        let request = CoordinatorRequest::CancelAll;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorRequest::CancelAll));

        let request = CoordinatorRequest::Shutdown;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorRequest::Shutdown));
    }

    #[test]
    fn test_response_serialization() {
        let response = CoordinatorResponse::Jobs(vec![]);
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorResponse::Jobs(jobs) if jobs.is_empty()));

        let response = CoordinatorResponse::Ack {
            message: "ok".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorResponse::Ack { message } if message == "ok"));

        let response = CoordinatorResponse::Error {
            code: ErrorCode::JobNotFound,
            message: "bad".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("JOB_NOT_FOUND"));
        let deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            CoordinatorResponse::Error { message, .. } if message == "bad"
        ));
    }

    #[test]
    fn test_job_info_serialization() {
        let info = JobInfo {
            name: "warm-build".to_string(),
            hook_type: "worktree-post-create".to_string(),
            worktree: "/tmp/wt".to_string(),
            status: crate::coordinator::log_store::JobStatus::Completed,
            elapsed_secs: Some(10),
            exit_code: Some(0),
        };
        let json = serde_json::to_string(&info).unwrap();
        let deserialized: JobInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, "warm-build");
        assert_eq!(deserialized.elapsed_secs, Some(10));
        assert_eq!(deserialized.exit_code, Some(0));
        assert!(matches!(
            deserialized.status,
            crate::coordinator::log_store::JobStatus::Completed
        ));
    }
}
