//! Job log sinks: a pluggable seam between the generic runner and
//! persistent log storage.
//!
//! The runner drives job execution and streams output via a presenter for
//! live display. A `LogSink`, if provided, also receives output chunks and
//! completion notifications so it can write `meta.json` + `output.log`
//! entries into a `LogStore`. Callers that don't need persistence pass
//! `None`.

use super::{JobResult, JobSpec};
use crate::coordinator::log_store::{JobMeta, JobStatus, LogStore};
use crate::executor::NodeStatus;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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

    fn node_to_job_status(status: NodeStatus) -> JobStatus {
        match status {
            NodeStatus::Succeeded => JobStatus::Completed,
            NodeStatus::Failed => JobStatus::Failed,
            NodeStatus::Skipped | NodeStatus::DepFailed => JobStatus::Skipped,
            NodeStatus::Pending | NodeStatus::Running => {
                unreachable!("on_job_complete called with non-terminal NodeStatus: {status:?}")
            }
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
            status: Self::node_to_job_status(result.status),
            exit_code: result.exit_code,
            pid: None,
            background: false,
            finished_at: Some(chrono::Utc::now()),
            needs: spec.needs.clone(),
        };

        if let Err(e) = self
            .store
            .write_job_record(&self.invocation_id, &meta, &buf.output)
        {
            eprintln!("daft: failed to write job record for '{}': {e}", spec.name);
        }
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
            needs: spec.needs.clone(),
        };

        if let Err(e) = self
            .store
            .write_job_record(&self.invocation_id, &meta, reason.as_bytes())
        {
            eprintln!("daft: failed to write job record for '{}': {e}", spec.name);
        }
    }
}

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

        let mut spec = make_spec("pnpm-install", false);
        spec.needs = vec!["db-migrate".to_string()];
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
        assert_eq!(loaded.needs, vec!["db-migrate".to_string()]);

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
        assert!(
            !job_dir.exists(),
            "no record should be written for in-flight job"
        );
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
