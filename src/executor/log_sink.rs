//! Job log sinks: a pluggable seam between the generic runner and
//! persistent log storage.
//!
//! The runner drives job execution and streams output via a presenter for
//! live display. A `LogSink`, if provided, also receives output chunks and
//! completion notifications so it can write the per-job `output.jsonl`
//! record stream into a `LogStore`. Callers that don't need persistence
//! pass `None`.

use super::{JobResult, JobSpec};
use crate::coordinator::adapters::SqliteJobsStore;
use crate::coordinator::log_record::{LogRecord, OutputKind, StatusEvent, record_from};
use crate::coordinator::log_store::{JobMeta, JobStatus, LogStore};
use crate::coordinator::ports::JobsStorePort;
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

    /// Called for every output line the runner reads from the child
    /// process. `kind` discriminates stdout vs stderr so structured log
    /// capture (`output.jsonl`) can record each line's source stream.
    fn on_job_output(&self, spec: &JobSpec, kind: OutputKind, line: &str);

    /// Called exactly once per job, after the command terminates.
    fn on_job_complete(&self, spec: &JobSpec, result: &JobResult);

    /// Called when a job is skipped by the runner (e.g., piped mode after
    /// a prior failure, or dep-failed in a DAG). The reason describes why.
    fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str);
}

/// A `LogSink` that buffers output per job and writes the
/// `output.jsonl` log file plus the SQLite `jobs` row atomically at
/// `on_job_complete`.
///
/// If a job is in flight when the sink is dropped (e.g., the main process
/// crashes mid-run), its buffered output is discarded and no record is
/// written. This matches the "atomic at completion" design choice in
/// docs/superpowers/specs/2026-04-11-universal-hook-logging.md §1.
pub struct BufferingLogSink {
    store: Arc<LogStore>,
    /// SQLite source-of-truth for job metadata. `None` when the per-repo
    /// store can't be opened — the sink still writes `output.jsonl` so
    /// log viewers work, but the row is absent from `daft hooks jobs ls`
    /// until the next successful open.
    job_store: Option<SqliteJobsStore>,
    repo_hash: String,
    invocation_id: String,
    hook_type: String,
    worktree: String,
    buffers: Mutex<HashMap<String, JobBuffer>>,
}

struct JobBuffer {
    started_at: chrono::DateTime<chrono::Utc>,
    /// Structured log records — written to `output.jsonl` at `on_job_complete`.
    records: Vec<LogRecord>,
    /// Per-job sequence counter. Advances on every `on_job_output` call
    /// whether or not the record is retained (so sampling produces visible
    /// gaps consumers can detect).
    next_seq: u64,
    /// Sampled-down stream rate. `None` = emit every record.
    sampling_every_nth: Option<u32>,
}

impl BufferingLogSink {
    pub fn new(
        store: Arc<LogStore>,
        repo_hash: String,
        invocation_id: String,
        hook_type: String,
        worktree: String,
    ) -> Self {
        let job_store = match SqliteJobsStore::for_repo_base(&store.base_dir) {
            Ok(js) => Some(js),
            Err(e) => {
                eprintln!(
                    "daft: warning: opening coordinator store at {} failed: {e} \
                     (foreground job records will not be persisted to the store)",
                    store.base_dir.display()
                );
                None
            }
        };
        Self {
            store,
            job_store,
            repo_hash,
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

    /// Persist a `JobMeta` into the SQLite store as a `JobRow`. Best
    /// effort: errors go to stderr and don't abort the surrounding write.
    fn persist_job_row(&self, meta: &JobMeta, tags: &[String]) {
        let Some(js) = self.job_store.as_ref() else {
            return;
        };
        let row = crate::store::models::JobRow {
            repo_hash: self.repo_hash.clone(),
            invocation_id: self.invocation_id.clone(),
            name: meta.name.clone(),
            hook_type: meta.hook_type.clone(),
            worktree: meta.worktree.clone(),
            command: meta.command.clone(),
            working_dir: meta.working_dir.clone(),
            env: meta.env.clone(),
            started_at: meta.started_at,
            finished_at: meta.finished_at,
            status: meta.status.as_status_str().to_string(),
            exit_code: meta.exit_code,
            pid: meta.pid,
            // Foreground jobs run inline — no separate process group leader.
            pgid: meta.pid,
            background: meta.background,
            needs: meta.needs.clone(),
            tags: tags.to_vec(),
            retention_seconds: meta.retention_seconds,
            max_log_size_bytes: meta.max_log_size_bytes,
        };
        if let Err(e) = js.upsert_job(&row) {
            eprintln!(
                "daft: failed to persist job '{}' to the coordinator store: {e}",
                meta.name
            );
        }
    }
}

impl LogSink for BufferingLogSink {
    fn on_job_start(&self, spec: &JobSpec) {
        let sampling = spec
            .log_config
            .as_ref()
            .and_then(|lc| lc.sampling_every_nth);
        let mut buffers = self.buffers.lock().unwrap();
        buffers.insert(
            spec.name.clone(),
            JobBuffer {
                started_at: chrono::Utc::now(),
                records: Vec::new(),
                next_seq: 0,
                sampling_every_nth: sampling,
            },
        );
    }

    fn on_job_output(&self, spec: &JobSpec, kind: OutputKind, line: &str) {
        let mut buffers = self.buffers.lock().unwrap();
        let Some(buf) = buffers.get_mut(&spec.name) else {
            return;
        };
        let seq = buf.next_seq;
        buf.next_seq += 1;
        // Sampling drops the record but `seq` still advances so consumers
        // can detect gaps. `Status` records (only written on completion)
        // are never sampled.
        if let Some(n) = buf.sampling_every_nth
            && n > 1
            && !seq.is_multiple_of(n as u64)
        {
            return;
        }
        buf.records.push(record_from(seq, kind, line));
    }

    fn on_job_complete(&self, spec: &JobSpec, result: &JobResult) {
        let buf = {
            let mut buffers = self.buffers.lock().unwrap();
            buffers.remove(&spec.name)
        };
        let Some(buf) = buf else { return };

        let retention_seconds = spec
            .log_config
            .as_ref()
            .and_then(|lc| lc.retention.as_deref())
            .and_then(|s| crate::coordinator::clean_policy::parse_duration_str(s).ok())
            .map(|n| n as i64);
        let max_log_size_bytes = spec
            .log_config
            .as_ref()
            .and_then(|lc| lc.max_log_size.as_deref())
            .and_then(|s| crate::coordinator::clean_policy::parse_size(s).ok());

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
            retention_seconds,
            max_log_size_bytes,
        };

        // Append a terminal Status record so JSONL readers see lifecycle
        // signals alongside stdout/stderr.
        let mut records = buf.records;
        records.push(LogRecord::status(
            buf.next_seq,
            StatusEvent::Finished {
                exit_code: result.exit_code,
            },
        ));

        if let Err(e) = self
            .store
            .write_job_record_jsonl(&self.invocation_id, &meta, &records)
        {
            eprintln!("daft: failed to write job record for '{}': {e}", spec.name);
        }
        // SQLite source-of-truth write happens after the log file is on
        // disk so a successful `daft hooks jobs logs` lookup can rely on
        // both being present.
        self.persist_job_row(&meta, &spec.tags);
    }

    fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str) {
        // Remove any buffered state for defensive cleanup.
        {
            let mut buffers = self.buffers.lock().unwrap();
            buffers.remove(&spec.name);
        }

        let meta = JobMeta::skipped(
            &spec.name,
            &self.hook_type,
            &self.worktree,
            &spec.command,
            false,
            spec.needs.clone(),
        );

        // Skipped jobs still get a JSONL log: a single Stdout record carrying
        // the reason so consumers don't have to special-case missing files.
        let records = vec![LogRecord::stdout(0, reason)];

        if let Err(e) = self
            .store
            .write_job_record_jsonl(&self.invocation_id, &meta, &records)
        {
            eprintln!("daft: failed to write job record for '{}': {e}", spec.name);
        }
        self.persist_job_row(&meta, &spec.tags);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinator::adapters::SqliteJobsStore;
    use crate::coordinator::log_store::LogStore;
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
    fn buffering_sink_writes_row_and_log_on_complete() {
        use crate::coordinator::ports::JobsStorePort;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "test-repo".to_string(),
            "inv1".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let mut spec = make_spec("pnpm-install", false);
        spec.needs = vec!["db-migrate".to_string()];
        sink.on_job_start(&spec);
        sink.on_job_output(&spec, OutputKind::Stdout, "installing...");
        sink.on_job_output(&spec, OutputKind::Stdout, "done");
        sink.on_job_complete(&spec, &make_result("pnpm-install", NodeStatus::Succeeded));

        // SQLite is the source of truth for job metadata post-cutover.
        let job_store = SqliteJobsStore::for_repo_base(&store.base_dir).unwrap();
        let row = job_store
            .get_job("test-repo", "inv1", "pnpm-install")
            .unwrap()
            .expect("row persisted on complete");
        assert_eq!(row.status, "completed");
        assert_eq!(row.hook_type, "worktree-post-create");
        assert_eq!(row.worktree, "feature/x");
        assert!(!row.background);
        assert!(row.finished_at.is_some());
        assert_eq!(row.needs, vec!["db-migrate".to_string()]);

        let job_dir = tmp.path().join("inv1").join("pnpm-install");
        let log_text = std::fs::read_to_string(LogStore::jsonl_path(&job_dir)).unwrap();
        assert!(log_text.contains(r#""data":"installing...""#));
        assert!(log_text.contains(r#""data":"done""#));
        // Terminal Status record present.
        assert!(log_text.contains(r#""kind":"status""#));
    }

    #[test]
    fn buffering_sink_records_failed_jobs() {
        use crate::coordinator::ports::JobsStorePort;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "test-repo".to_string(),
            "inv2".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let spec = make_spec("broken", false);
        sink.on_job_start(&spec);
        sink.on_job_output(&spec, OutputKind::Stderr, "error: oops");
        let mut result = make_result("broken", NodeStatus::Failed);
        result.exit_code = Some(2);
        sink.on_job_complete(&spec, &result);

        let job_store = SqliteJobsStore::for_repo_base(&store.base_dir).unwrap();
        let row = job_store
            .get_job("test-repo", "inv2", "broken")
            .unwrap()
            .expect("row persisted on complete");
        assert_eq!(row.status, "failed");
        assert_eq!(row.exit_code, Some(2));
    }

    #[test]
    fn buffering_sink_drops_in_flight_on_drop() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));
        {
            let sink = BufferingLogSink::new(
                Arc::clone(&store),
                "test-repo".to_string(),
                "inv3".to_string(),
                "worktree-post-create".to_string(),
                "feature/x".to_string(),
            );
            let spec = make_spec("never-finishes", false);
            sink.on_job_start(&spec);
            sink.on_job_output(&spec, OutputKind::Stdout, "working...");
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
        use crate::coordinator::ports::JobsStorePort;
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "test-repo".to_string(),
            "inv4".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let spec = make_spec("after-the-failure", false);
        sink.on_job_runner_skipped(&spec, "previous job failed");

        let job_store = SqliteJobsStore::for_repo_base(&store.base_dir).unwrap();
        let row = job_store
            .get_job("test-repo", "inv4", "after-the-failure")
            .unwrap()
            .expect("skipped row persisted");
        assert_eq!(row.status, "skipped");

        let job_dir = tmp.path().join("inv4").join("after-the-failure");
        let log_text = std::fs::read_to_string(LogStore::jsonl_path(&job_dir)).unwrap();
        assert!(log_text.contains(r#""data":"previous job failed""#));
    }

    /// With `sampling_every_nth = 10`, exactly one of every 10 `Stdout`
    /// records survives, and the terminal `Status` is always present.
    /// The `seq` field advances on every dropped record so consumers
    /// see the gap.
    #[test]
    fn sampling_every_nth_drops_records_but_advances_seq() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "test-repo".to_string(),
            "inv-sample".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let mut spec = make_spec("noisy", false);
        spec.log_config = Some(crate::executor::LogConfig {
            sampling_every_nth: Some(10),
            ..Default::default()
        });

        sink.on_job_start(&spec);
        for i in 0..100 {
            sink.on_job_output(&spec, OutputKind::Stdout, &format!("line {i}"));
        }
        sink.on_job_complete(&spec, &make_result("noisy", NodeStatus::Succeeded));

        let job_dir = tmp.path().join("inv-sample").join("noisy");
        let log_text = std::fs::read_to_string(LogStore::jsonl_path(&job_dir)).unwrap();

        // Count Stdout records: should be 10 (seqs 0, 10, 20, …, 90).
        let stdout_count = log_text
            .lines()
            .filter(|l| l.contains(r#""kind":"stdout""#))
            .count();
        assert_eq!(stdout_count, 10, "expected exactly 10 sampled records");

        // Seqs of surviving records: 0, 10, 20, ..., 90.
        for retained in [0u64, 10, 20, 50, 90] {
            assert!(
                log_text.contains(&format!(r#""seq":{retained}"#)),
                "missing retained seq {retained}"
            );
        }
        // Sampled-out records DO NOT appear (e.g. seq 5).
        assert!(!log_text.contains(r#""seq":5,"#));

        // Terminal Status record uses the final seq (100 — the next
        // unused seq after 100 lines).
        assert!(log_text.contains(r#""seq":100"#));
        assert!(log_text.contains(r#""kind":"status""#));
    }

    /// With no `sampling_every_nth`, every record is emitted.
    #[test]
    fn no_sampling_emits_every_record() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(LogStore::new(tmp.path().to_path_buf()));

        let sink = BufferingLogSink::new(
            Arc::clone(&store),
            "test-repo".to_string(),
            "inv-all".to_string(),
            "worktree-post-create".to_string(),
            "feature/x".to_string(),
        );

        let spec = make_spec("dense", false);
        sink.on_job_start(&spec);
        for i in 0..5 {
            sink.on_job_output(&spec, OutputKind::Stdout, &format!("line {i}"));
        }
        sink.on_job_complete(&spec, &make_result("dense", NodeStatus::Succeeded));

        let job_dir = tmp.path().join("inv-all").join("dense");
        let log_text = std::fs::read_to_string(LogStore::jsonl_path(&job_dir)).unwrap();
        let stdout_count = log_text
            .lines()
            .filter(|l| l.contains(r#""kind":"stdout""#))
            .count();
        assert_eq!(stdout_count, 5);
    }
}
