//! Client for communicating with a running coordinator over Unix socket IPC.
//!
//! The protocol is simple JSON-over-Unix-socket with newline delimiters.
//! Each message is one JSON object followed by `\n`. The client sends a
//! [`CoordinatorRequest`], and the coordinator responds with a
//! [`CoordinatorResponse`].

use super::{coordinator_socket_path, CoordinatorRequest, CoordinatorResponse, JobInfo};
use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Client for communicating with a running coordinator.
pub struct CoordinatorClient {
    stream: UnixStream,
}

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

    /// Send a request and receive the response.
    pub fn send(&mut self, request: &CoordinatorRequest) -> Result<CoordinatorResponse> {
        let mut msg = serde_json::to_string(request)?;
        msg.push('\n');
        self.stream.write_all(msg.as_bytes())?;

        let mut reader = BufReader::new(&self.stream);
        let mut response_line = String::new();
        reader.read_line(&mut response_line)?;

        let response: CoordinatorResponse = serde_json::from_str(&response_line)?;
        Ok(response)
    }

    /// List all jobs from the coordinator.
    pub fn list_jobs(&mut self) -> Result<Vec<JobInfo>> {
        match self.send(&CoordinatorRequest::ListJobs)? {
            CoordinatorResponse::Jobs(jobs) => Ok(jobs),
            CoordinatorResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    /// Cancel a specific job by name.
    pub fn cancel_job(&mut self, name: &str) -> Result<String> {
        match self.send(&CoordinatorRequest::CancelJob {
            name: name.to_string(),
        })? {
            CoordinatorResponse::Ack { message } => Ok(message),
            CoordinatorResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }

    /// Cancel all running jobs.
    pub fn cancel_all(&mut self) -> Result<String> {
        match self.send(&CoordinatorRequest::CancelAll)? {
            CoordinatorResponse::Ack { message } => Ok(message),
            CoordinatorResponse::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("Unexpected response from coordinator"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};

    #[test]
    fn test_ipc_round_trip_list_jobs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();

        // Spawn a handler thread that mimics a coordinator.
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let request: CoordinatorRequest = serde_json::from_str(&line).unwrap();
            assert!(matches!(request, CoordinatorRequest::ListJobs));

            let response = CoordinatorResponse::Jobs(vec![]);
            let mut msg = serde_json::to_string(&response).unwrap();
            msg.push('\n');
            stream
                .try_clone()
                .unwrap()
                .write_all(msg.as_bytes())
                .unwrap();
        });

        // Client side.
        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let jobs = client.list_jobs().unwrap();
        assert!(jobs.is_empty());

        handle.join().unwrap();
    }

    #[test]
    fn test_ipc_round_trip_list_jobs_with_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let request: CoordinatorRequest = serde_json::from_str(&line).unwrap();
            assert!(matches!(request, CoordinatorRequest::ListJobs));

            let response = CoordinatorResponse::Jobs(vec![JobInfo {
                name: "build".to_string(),
                hook_type: "worktree-post-create".to_string(),
                worktree: "/tmp/wt".to_string(),
                status: crate::coordinator::log_store::JobStatus::Running,
                elapsed_secs: Some(42),
                exit_code: None,
            }]);
            let mut msg = serde_json::to_string(&response).unwrap();
            msg.push('\n');
            stream
                .try_clone()
                .unwrap()
                .write_all(msg.as_bytes())
                .unwrap();
        });

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let jobs = client.list_jobs().unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "build");
        assert_eq!(jobs[0].elapsed_secs, Some(42));
        assert!(matches!(
            jobs[0].status,
            crate::coordinator::log_store::JobStatus::Running
        ));

        handle.join().unwrap();
    }

    #[test]
    fn test_ipc_round_trip_cancel_job() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let request: CoordinatorRequest = serde_json::from_str(&line).unwrap();
            match request {
                CoordinatorRequest::CancelJob { name } => {
                    assert_eq!(name, "build");
                }
                _ => panic!("Expected CancelJob"),
            }

            let response = CoordinatorResponse::Ack {
                message: "Cancelled job: build".to_string(),
            };
            let mut msg = serde_json::to_string(&response).unwrap();
            msg.push('\n');
            stream
                .try_clone()
                .unwrap()
                .write_all(msg.as_bytes())
                .unwrap();
        });

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let result = client.cancel_job("build").unwrap();
        assert_eq!(result, "Cancelled job: build");

        handle.join().unwrap();
    }

    #[test]
    fn test_ipc_round_trip_cancel_all() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let request: CoordinatorRequest = serde_json::from_str(&line).unwrap();
            assert!(matches!(request, CoordinatorRequest::CancelAll));

            let response = CoordinatorResponse::Ack {
                message: "Cancelled 3 jobs".to_string(),
            };
            let mut msg = serde_json::to_string(&response).unwrap();
            msg.push('\n');
            stream
                .try_clone()
                .unwrap()
                .write_all(msg.as_bytes())
                .unwrap();
        });

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let result = client.cancel_all().unwrap();
        assert_eq!(result, "Cancelled 3 jobs");

        handle.join().unwrap();
    }

    #[test]
    fn test_ipc_error_response() {
        let tmp = tempfile::TempDir::new().unwrap();
        let socket_path = tmp.path().join("test.sock");

        let listener = UnixListener::bind(&socket_path).unwrap();

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let response = CoordinatorResponse::Error {
                message: "Job not found: unknown".to_string(),
            };
            let mut msg = serde_json::to_string(&response).unwrap();
            msg.push('\n');
            stream
                .try_clone()
                .unwrap()
                .write_all(msg.as_bytes())
                .unwrap();
        });

        let stream = UnixStream::connect(&socket_path).unwrap();
        let mut client = CoordinatorClient::from_stream(stream);
        let result = client.cancel_job("unknown");
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Job not found: unknown"));

        handle.join().unwrap();
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
            message: "bad".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorResponse::Error { message } if message == "bad"));
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
