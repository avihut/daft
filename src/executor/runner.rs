//! High-level job runner that combines scheduling with presentation.
//!
//! [`run_jobs`] is the main entry point. It inspects the job list and
//! execution mode to dispatch to the appropriate strategy: sequential,
//! piped (stop-on-failure sequential), parallel, or DAG-ordered.

use super::command::{run_command, run_command_interactive, CommandResult};
use super::dag::DagGraph;
use super::presenter::JobPresenter;
use super::{ExecutionMode, JobResult, JobSpec, NodeStatus};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

// ─────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────

/// Execute a batch of jobs, routing to the appropriate scheduling strategy.
///
/// - If any job has non-empty `needs` -> build DAG, use parallel or sequential
///   based on mode
/// - If `Parallel` -> use DAG with no edges (all independent)
/// - If `Sequential` -> iterate in order, continue on failure
/// - If `Piped` -> iterate in order, stop on first failure
pub fn run_jobs(
    jobs: &[JobSpec],
    mode: ExecutionMode,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }

    let has_deps = jobs.iter().any(|j| !j.needs.is_empty());

    if has_deps {
        run_with_dag(jobs, mode, presenter, sink)
    } else {
        match mode {
            ExecutionMode::Parallel => run_parallel_flat(jobs, presenter, sink),
            ExecutionMode::Sequential => run_sequential(jobs, presenter, false, sink),
            ExecutionMode::Piped => run_sequential(jobs, presenter, true, sink),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Sequential execution
// ─────────────────────────────────────────────────────────────────────────

/// Run jobs one at a time in order.
///
/// When `stop_on_failure` is true (Piped mode), remaining jobs are marked
/// as `Skipped` after the first failure. Otherwise all jobs run regardless
/// of earlier failures.
fn run_sequential(
    jobs: &[JobSpec],
    presenter: &Arc<dyn JobPresenter>,
    stop_on_failure: bool,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let mut results = Vec::with_capacity(jobs.len());

    for (i, job) in jobs.iter().enumerate() {
        presenter.on_job_start(&job.name, job.description.as_deref(), Some(&job.command));
        if let Some(s) = sink {
            s.on_job_start(job);
        }
        let start = Instant::now();

        let cr = execute_single_job(job, presenter, sink)?;
        let duration = start.elapsed();
        let result = command_to_job_result(&job.name, &cr, duration);

        report_completion(job, &result, presenter);
        if let Some(s) = sink {
            s.on_job_complete(job, &result);
        }
        let failed = result.status == NodeStatus::Failed;
        results.push(result);

        if failed && stop_on_failure {
            // Mark remaining jobs as Skipped.
            for remaining in &jobs[i + 1..] {
                presenter.on_job_skipped(
                    &remaining.name,
                    "previous job failed",
                    Duration::ZERO,
                    false,
                );
                if let Some(s) = sink {
                    s.on_job_runner_skipped(remaining, "previous job failed");
                }
                results.push(JobResult {
                    name: remaining.name.clone(),
                    status: NodeStatus::Skipped,
                    duration: Duration::ZERO,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                });
            }
            return Ok(results);
        }
    }

    Ok(results)
}

// ─────────────────────────────────────────────────────────────────────────
// Parallel (no dependencies)
// ─────────────────────────────────────────────────────────────────────────

/// Run all jobs concurrently using a DAG with no edges.
fn run_parallel_flat(
    jobs: &[JobSpec],
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let nodes: Vec<(String, Vec<String>)> = jobs.iter().map(|j| (j.name.clone(), vec![])).collect();
    let graph = DagGraph::new(nodes)?;
    run_dag_execution(jobs, &graph, presenter, sink)
}

// ─────────────────────────────────────────────────────────────────────────
// DAG execution
// ─────────────────────────────────────────────────────────────────────────

/// Build a DAG from job specs and dispatch to parallel or sequential execution.
fn run_with_dag(
    jobs: &[JobSpec],
    mode: ExecutionMode,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let nodes: Vec<(String, Vec<String>)> = jobs
        .iter()
        .map(|j| (j.name.clone(), j.needs.clone()))
        .collect();
    let graph = DagGraph::new(nodes)?;

    match mode {
        ExecutionMode::Parallel => run_dag_execution(jobs, &graph, presenter, sink),
        _ => run_dag_sequential_exec(jobs, &graph, presenter, mode == ExecutionMode::Piped, sink),
    }
}

/// Parallel DAG execution using the thread-pool scheduler.
fn run_dag_execution(
    jobs: &[JobSpec],
    graph: &DagGraph,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let job_map = build_job_map(jobs);
    let max_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    // Shared storage for captured output keyed by node index.
    let captured: std::sync::Mutex<HashMap<usize, CapturedOutput>> =
        std::sync::Mutex::new(HashMap::new());
    let durations: std::sync::Mutex<HashMap<usize, Duration>> =
        std::sync::Mutex::new(HashMap::new());

    let sink_for_closure = sink.cloned();

    let statuses = graph.run_parallel(
        |idx, name| {
            let Some(job) = job_map.get(name) else {
                return NodeStatus::Failed;
            };

            presenter.on_job_start(name, job.description.as_deref(), Some(&job.command));
            if let Some(ref s) = sink_for_closure {
                s.on_job_start(job);
            }
            let start = Instant::now();

            let cr = execute_single_job(job, presenter, sink_for_closure.as_ref());
            let duration = start.elapsed();

            match cr {
                Ok(cr) => {
                    let result = command_to_job_result(name, &cr, duration);
                    report_completion(job, &result, presenter);
                    if let Some(ref s) = sink_for_closure {
                        s.on_job_complete(job, &result);
                    }

                    captured.lock().unwrap().insert(
                        idx,
                        CapturedOutput {
                            exit_code: cr.exit_code,
                            stdout: cr.stdout,
                            stderr: cr.stderr,
                        },
                    );
                    durations.lock().unwrap().insert(idx, duration);

                    result.status
                }
                Err(_) => {
                    presenter.on_job_failure(name, duration);
                    if let Some(ref s) = sink_for_closure {
                        let failed_result = JobResult {
                            name: job.name.clone(),
                            status: NodeStatus::Failed,
                            duration,
                            exit_code: None,
                            stdout: String::new(),
                            stderr: String::new(),
                        };
                        s.on_job_complete(job, &failed_result);
                    }
                    durations.lock().unwrap().insert(idx, duration);
                    NodeStatus::Failed
                }
            }
        },
        max_workers,
    );

    let captured = captured.into_inner().unwrap();
    let durations = durations.into_inner().unwrap();

    Ok(build_results_from_statuses(
        graph, &statuses, &captured, &durations, jobs, presenter, sink,
    ))
}

/// Sequential DAG execution (topological order, one at a time).
fn run_dag_sequential_exec(
    jobs: &[JobSpec],
    graph: &DagGraph,
    presenter: &Arc<dyn JobPresenter>,
    _stop_on_failure: bool,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<Vec<JobResult>> {
    let job_map = build_job_map(jobs);

    let captured: std::sync::Mutex<HashMap<usize, CapturedOutput>> =
        std::sync::Mutex::new(HashMap::new());
    let durations: std::sync::Mutex<HashMap<usize, Duration>> =
        std::sync::Mutex::new(HashMap::new());

    let sink_for_closure = sink.cloned();

    let statuses = graph.run_sequential(|idx, name| {
        let Some(job) = job_map.get(name) else {
            return NodeStatus::Failed;
        };

        presenter.on_job_start(name, job.description.as_deref(), Some(&job.command));
        if let Some(ref s) = sink_for_closure {
            s.on_job_start(job);
        }
        let start = Instant::now();

        let cr = execute_single_job(job, presenter, sink_for_closure.as_ref());
        let duration = start.elapsed();

        match cr {
            Ok(cr) => {
                let result = command_to_job_result(name, &cr, duration);
                report_completion(job, &result, presenter);
                if let Some(ref s) = sink_for_closure {
                    s.on_job_complete(job, &result);
                }

                captured.lock().unwrap().insert(
                    idx,
                    CapturedOutput {
                        exit_code: cr.exit_code,
                        stdout: cr.stdout,
                        stderr: cr.stderr,
                    },
                );
                durations.lock().unwrap().insert(idx, duration);

                result.status
            }
            Err(_) => {
                presenter.on_job_failure(name, duration);
                if let Some(ref s) = sink_for_closure {
                    let failed_result = JobResult {
                        name: job.name.clone(),
                        status: NodeStatus::Failed,
                        duration,
                        exit_code: None,
                        stdout: String::new(),
                        stderr: String::new(),
                    };
                    s.on_job_complete(job, &failed_result);
                }
                durations.lock().unwrap().insert(idx, duration);
                NodeStatus::Failed
            }
        }
    });

    let captured = captured.into_inner().unwrap();
    let durations = durations.into_inner().unwrap();

    Ok(build_results_from_statuses(
        graph, &statuses, &captured, &durations, jobs, presenter, sink,
    ))
}

// ─────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────

/// Captured stdout/stderr and exit code for a completed job.
struct CapturedOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Build a name -> JobSpec lookup map.
fn build_job_map(jobs: &[JobSpec]) -> HashMap<&str, &JobSpec> {
    jobs.iter().map(|j| (j.name.as_str(), j)).collect()
}

/// Execute a single job, choosing interactive or captured mode.
///
/// For non-interactive jobs, output lines are streamed to the presenter
/// in real time via a reader thread.
fn execute_single_job(
    job: &JobSpec,
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Result<CommandResult> {
    if job.interactive {
        run_command_interactive(&job.command, &job.env, &job.working_dir)
    } else {
        let (tx, rx) = mpsc::channel::<String>();

        // Spawn a reader thread that streams output to the presenter
        // and, if provided, the sink.
        let presenter_clone = Arc::clone(presenter);
        let sink_clone: Option<Arc<dyn crate::executor::log_sink::LogSink>> = sink.cloned();
        let job_name = job.name.clone();
        let job_for_sink = job.clone();
        let reader_handle = std::thread::spawn(move || {
            for line in rx {
                presenter_clone.on_job_output(&job_name, &line);
                if let Some(ref s) = sink_clone {
                    s.on_job_output(&job_for_sink, &line);
                }
            }
        });

        let result = run_command(
            &job.command,
            &job.env,
            &job.working_dir,
            job.timeout,
            Some(tx),
        );

        // Wait for the reader to drain all output before returning.
        reader_handle.join().ok();

        result
    }
}

/// Convert a `CommandResult` to a `JobResult`.
fn command_to_job_result(name: &str, cr: &CommandResult, duration: Duration) -> JobResult {
    JobResult {
        name: name.to_string(),
        status: if cr.success {
            NodeStatus::Succeeded
        } else {
            NodeStatus::Failed
        },
        duration,
        exit_code: cr.exit_code,
        stdout: cr.stdout.clone(),
        stderr: cr.stderr.clone(),
    }
}

/// Notify the presenter of job completion and emit failure messages.
fn report_completion(job: &JobSpec, result: &JobResult, presenter: &Arc<dyn JobPresenter>) {
    match result.status {
        NodeStatus::Succeeded => {
            presenter.on_job_success(&job.name, result.duration);
        }
        NodeStatus::Failed => {
            presenter.on_job_failure(&job.name, result.duration);
            if let Some(code) = result.exit_code {
                presenter.on_message(&format!("Job '{}' failed (exit code: {code})", job.name));
            } else {
                presenter.on_message(&format!("Job '{}' failed", job.name));
            }
            if let Some(ref fail_text) = job.fail_text {
                presenter.on_message(fail_text);
            }
        }
        _ => {}
    }
}

/// Build `Vec<JobResult>` from DAG statuses and captured data.
///
/// For nodes that were dep-failed (never executed), we emit a skipped
/// presenter event and produce a result with `DepFailed` status.
fn build_results_from_statuses(
    graph: &DagGraph,
    statuses: &[NodeStatus],
    captured: &HashMap<usize, CapturedOutput>,
    durations: &HashMap<usize, Duration>,
    jobs: &[JobSpec],
    presenter: &Arc<dyn JobPresenter>,
    sink: Option<&Arc<dyn crate::executor::log_sink::LogSink>>,
) -> Vec<JobResult> {
    let job_map: HashMap<&str, &JobSpec> = jobs.iter().map(|j| (j.name.as_str(), j)).collect();

    statuses
        .iter()
        .enumerate()
        .map(|(idx, &status)| {
            let name = &graph.names[idx];
            let duration = durations.get(&idx).copied().unwrap_or(Duration::ZERO);

            if status == NodeStatus::DepFailed {
                // Notify presenter about dep-failed jobs.
                if let Some(job) = job_map.get(name.as_str()) {
                    presenter.on_job_skipped(&job.name, "dependency failed", Duration::ZERO, false);
                    if let Some(s) = sink {
                        s.on_job_runner_skipped(job, "dependency failed");
                    }
                }
            }

            match captured.get(&idx) {
                Some(cap) => JobResult {
                    name: name.clone(),
                    status,
                    duration,
                    exit_code: cap.exit_code,
                    stdout: cap.stdout.clone(),
                    stderr: cap.stderr.clone(),
                },
                None => JobResult {
                    name: name.clone(),
                    status,
                    duration,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            }
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::presenter::NullPresenter;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A presenter that records events for verification in tests.
    struct RecordingPresenter {
        events: Mutex<Vec<String>>,
    }

    impl RecordingPresenter {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                events: Mutex::new(Vec::new()),
            })
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl JobPresenter for RecordingPresenter {
        fn on_phase_start(&self, phase_name: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("phase_start:{phase_name}"));
        }

        fn on_job_start(
            &self,
            name: &str,
            description: Option<&str>,
            _command_preview: Option<&str>,
        ) {
            let desc = description.unwrap_or("none");
            self.events
                .lock()
                .unwrap()
                .push(format!("job_start:{name}:{desc}"));
        }

        fn on_job_output(&self, name: &str, line: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("job_output:{name}:{line}"));
        }

        fn on_job_success(&self, name: &str, _duration: Duration) {
            self.events
                .lock()
                .unwrap()
                .push(format!("job_success:{name}"));
        }

        fn on_job_failure(&self, name: &str, _duration: Duration) {
            self.events
                .lock()
                .unwrap()
                .push(format!("job_failure:{name}"));
        }

        fn on_job_skipped(&self, name: &str, reason: &str, _duration: Duration, _show: bool) {
            self.events
                .lock()
                .unwrap()
                .push(format!("job_skipped:{name}:{reason}"));
        }

        fn on_job_background(&self, name: &str, _description: Option<&str>) {
            self.events
                .lock()
                .unwrap()
                .push(format!("job_background:{name}"));
        }

        fn on_message(&self, msg: &str) {
            self.events.lock().unwrap().push(format!("message:{msg}"));
        }

        fn on_phase_complete(&self, _total_duration: Duration) {
            self.events
                .lock()
                .unwrap()
                .push("phase_complete".to_string());
        }

        fn take_results(&self) -> Vec<JobResult> {
            Vec::new()
        }
    }

    fn tmp_dir() -> PathBuf {
        std::env::temp_dir()
    }

    fn make_job(name: &str, command: &str) -> JobSpec {
        JobSpec {
            name: name.into(),
            command: command.into(),
            working_dir: tmp_dir(),
            timeout: Duration::from_secs(10),
            ..Default::default()
        }
    }

    fn make_job_with_needs(name: &str, command: &str, needs: Vec<&str>) -> JobSpec {
        JobSpec {
            name: name.into(),
            command: command.into(),
            working_dir: tmp_dir(),
            needs: needs.into_iter().map(String::from).collect(),
            timeout: Duration::from_secs(10),
            ..Default::default()
        }
    }

    // ── Empty job list ─────────────────────────────────────────────────

    #[test]
    fn empty_jobs_returns_empty() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let results = run_jobs(&[], ExecutionMode::Sequential, &presenter, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn empty_jobs_parallel() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let results = run_jobs(&[], ExecutionMode::Parallel, &presenter, None).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn empty_jobs_piped() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let results = run_jobs(&[], ExecutionMode::Piped, &presenter, None).unwrap();
        assert!(results.is_empty());
    }

    // ── Single job ─────────────────────────────────────────────────────

    #[test]
    fn single_job_succeeds() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job("echo", "echo hello")];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "echo");
        assert_eq!(results[0].status, NodeStatus::Succeeded);
        assert_eq!(results[0].exit_code, Some(0));
        assert!(results[0].stdout.contains("hello"));
    }

    #[test]
    fn single_job_fails() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job("fail", "exit 1")];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Failed);
        assert_eq!(results[0].exit_code, Some(1));
    }

    #[test]
    fn single_job_captures_stderr() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job("stderr", "echo oops >&2")];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Succeeded);
        assert!(results[0].stderr.contains("oops"));
    }

    // ── Sequential mode ────────────────────────────────────────────────

    #[test]
    fn sequential_all_run_on_failure() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("first", "exit 1"),
            make_job("second", "echo ok"),
            make_job("third", "echo done"),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, NodeStatus::Failed);
        assert_eq!(results[1].status, NodeStatus::Succeeded);
        assert_eq!(results[2].status, NodeStatus::Succeeded);
    }

    #[test]
    fn sequential_preserves_order() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![
            make_job("a", "echo a"),
            make_job("b", "echo b"),
            make_job("c", "echo c"),
        ];
        run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let events = recorder.events();
        let starts: Vec<&String> = events
            .iter()
            .filter(|e| e.starts_with("job_start:"))
            .collect();
        assert_eq!(starts.len(), 3);
        assert!(starts[0].contains(":a:"));
        assert!(starts[1].contains(":b:"));
        assert!(starts[2].contains(":c:"));
    }

    // ── Piped mode ─────────────────────────────────────────────────────

    #[test]
    fn piped_stops_on_first_failure() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("first", "exit 1"),
            make_job("second", "echo should-not-run"),
            make_job("third", "echo should-not-run"),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Piped, &presenter, None).unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].status, NodeStatus::Failed);
        assert_eq!(results[1].status, NodeStatus::Skipped);
        assert_eq!(results[2].status, NodeStatus::Skipped);
    }

    #[test]
    fn piped_skipped_jobs_have_empty_output() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job("fail", "exit 1"), make_job("skip", "echo nope")];
        let results = run_jobs(&jobs, ExecutionMode::Piped, &presenter, None).unwrap();

        assert_eq!(results[1].status, NodeStatus::Skipped);
        assert!(results[1].stdout.is_empty());
        assert!(results[1].stderr.is_empty());
        assert!(results[1].exit_code.is_none());
    }

    #[test]
    fn piped_all_succeed_if_no_failures() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job("a", "echo a"), make_job("b", "echo b")];
        let results = run_jobs(&jobs, ExecutionMode::Piped, &presenter, None).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.status == NodeStatus::Succeeded));
    }

    #[test]
    fn piped_middle_failure_skips_rest() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "echo ok"),
            make_job("b", "exit 2"),
            make_job("c", "echo skip"),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Piped, &presenter, None).unwrap();

        assert_eq!(results[0].status, NodeStatus::Succeeded);
        assert_eq!(results[1].status, NodeStatus::Failed);
        assert_eq!(results[2].status, NodeStatus::Skipped);
    }

    // ── Parallel mode ──────────────────────────────────────────────────

    #[test]
    fn parallel_all_run() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "echo a"),
            make_job("b", "echo b"),
            make_job("c", "echo c"),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Parallel, &presenter, None).unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.status == NodeStatus::Succeeded));
    }

    #[test]
    fn parallel_failure_does_not_skip_others() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("ok1", "echo ok"),
            make_job("fail", "exit 1"),
            make_job("ok2", "echo ok"),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Parallel, &presenter, None).unwrap();

        assert_eq!(results.len(), 3);
        // Find results by name since parallel order is not deterministic.
        let ok1 = results.iter().find(|r| r.name == "ok1").unwrap();
        let fail = results.iter().find(|r| r.name == "fail").unwrap();
        let ok2 = results.iter().find(|r| r.name == "ok2").unwrap();
        assert_eq!(ok1.status, NodeStatus::Succeeded);
        assert_eq!(fail.status, NodeStatus::Failed);
        assert_eq!(ok2.status, NodeStatus::Succeeded);
    }

    // ── DAG mode ───────────────────────────────────────────────────────

    #[test]
    fn dag_dependencies_respected() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "echo a"),
            make_job_with_needs("b", "echo b", vec!["a"]),
            make_job_with_needs("c", "echo c", vec!["b"]),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Parallel, &presenter, None).unwrap();

        assert_eq!(results.len(), 3);
        assert!(results.iter().all(|r| r.status == NodeStatus::Succeeded));
    }

    #[test]
    fn dag_failure_cascades_to_dependents() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "exit 1"),
            make_job_with_needs("b", "echo b", vec!["a"]),
            make_job_with_needs("c", "echo c", vec!["b"]),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Parallel, &presenter, None).unwrap();

        assert_eq!(results.len(), 3);
        let a = results.iter().find(|r| r.name == "a").unwrap();
        let b = results.iter().find(|r| r.name == "b").unwrap();
        let c = results.iter().find(|r| r.name == "c").unwrap();
        assert_eq!(a.status, NodeStatus::Failed);
        assert_eq!(b.status, NodeStatus::DepFailed);
        assert_eq!(c.status, NodeStatus::DepFailed);
    }

    #[test]
    fn dag_diamond_all_succeed() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "echo a"),
            make_job_with_needs("b", "echo b", vec!["a"]),
            make_job_with_needs("c", "echo c", vec!["a"]),
            make_job_with_needs("d", "echo d", vec!["b", "c"]),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Parallel, &presenter, None).unwrap();

        assert_eq!(results.len(), 4);
        assert!(results.iter().all(|r| r.status == NodeStatus::Succeeded));
    }

    #[test]
    fn dag_diamond_one_branch_fails() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "echo a"),
            make_job_with_needs("b", "exit 1", vec!["a"]),
            make_job_with_needs("c", "echo c", vec!["a"]),
            make_job_with_needs("d", "echo d", vec!["b", "c"]),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Parallel, &presenter, None).unwrap();

        let a = results.iter().find(|r| r.name == "a").unwrap();
        let b = results.iter().find(|r| r.name == "b").unwrap();
        let c = results.iter().find(|r| r.name == "c").unwrap();
        let d = results.iter().find(|r| r.name == "d").unwrap();
        assert_eq!(a.status, NodeStatus::Succeeded);
        assert_eq!(b.status, NodeStatus::Failed);
        assert_eq!(c.status, NodeStatus::Succeeded);
        assert_eq!(d.status, NodeStatus::DepFailed);
    }

    #[test]
    fn dag_sequential_mode() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "echo a"),
            make_job_with_needs("b", "echo b", vec!["a"]),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.status == NodeStatus::Succeeded));
    }

    #[test]
    fn dag_sequential_failure_cascades() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![
            make_job("a", "exit 1"),
            make_job_with_needs("b", "echo b", vec!["a"]),
        ];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let a = results.iter().find(|r| r.name == "a").unwrap();
        let b = results.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(a.status, NodeStatus::Failed);
        assert_eq!(b.status, NodeStatus::DepFailed);
    }

    // ── Presenter events ───────────────────────────────────────────────

    #[test]
    fn presenter_receives_start_and_success_events() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![make_job("hello", "echo world")];
        run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let events = recorder.events();
        assert!(events.iter().any(|e| e.starts_with("job_start:hello")));
        assert!(events.iter().any(|e| e == "job_success:hello"));
    }

    #[test]
    fn presenter_receives_failure_events() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![make_job("broken", "exit 42")];
        run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let events = recorder.events();
        assert!(events.iter().any(|e| e == "job_failure:broken"));
        assert!(events
            .iter()
            .any(|e| e.contains("Job 'broken' failed (exit code: 42)")));
    }

    #[test]
    fn presenter_receives_fail_text() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![JobSpec {
            name: "hint".into(),
            command: "exit 1".into(),
            working_dir: tmp_dir(),
            fail_text: Some("Try running: npm install".into()),
            timeout: Duration::from_secs(10),
            ..Default::default()
        }];
        run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let events = recorder.events();
        assert!(events
            .iter()
            .any(|e| e == "message:Try running: npm install"));
    }

    #[test]
    fn presenter_receives_output_lines() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![make_job("multi", "echo line1; echo line2")];
        run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let events = recorder.events();
        let output_events: Vec<&String> = events
            .iter()
            .filter(|e| e.starts_with("job_output:multi:"))
            .collect();
        assert!(
            output_events.iter().any(|e| e.contains("line1")),
            "expected line1 in output events: {output_events:?}"
        );
        assert!(
            output_events.iter().any(|e| e.contains("line2")),
            "expected line2 in output events: {output_events:?}"
        );
    }

    #[test]
    fn presenter_receives_description() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![JobSpec {
            name: "install".into(),
            command: "echo ok".into(),
            working_dir: tmp_dir(),
            description: Some("Install dependencies".into()),
            timeout: Duration::from_secs(10),
            ..Default::default()
        }];
        run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        let events = recorder.events();
        assert!(events
            .iter()
            .any(|e| e == "job_start:install:Install dependencies"));
    }

    #[test]
    fn piped_presenter_receives_skipped_events() {
        let recorder = RecordingPresenter::new();
        let presenter: Arc<dyn JobPresenter> = recorder.clone();
        let jobs = vec![
            make_job("fail", "exit 1"),
            make_job("skip1", "echo nope"),
            make_job("skip2", "echo nope"),
        ];
        run_jobs(&jobs, ExecutionMode::Piped, &presenter, None).unwrap();

        let events = recorder.events();
        assert!(events.iter().any(|e| e.starts_with("job_skipped:skip1")));
        assert!(events.iter().any(|e| e.starts_with("job_skipped:skip2")));
    }

    // ── Environment variables ──────────────────────────────────────────

    #[test]
    fn job_env_vars_are_passed() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let mut env = HashMap::new();
        env.insert("MY_TEST_VAR".into(), "runner_test_value".into());
        let jobs = vec![JobSpec {
            name: "env".into(),
            command: "echo $MY_TEST_VAR".into(),
            working_dir: tmp_dir(),
            env,
            timeout: Duration::from_secs(10),
            ..Default::default()
        }];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results[0].status, NodeStatus::Succeeded);
        assert!(results[0].stdout.contains("runner_test_value"));
    }

    // ── Interactive jobs ───────────────────────────────────────────────

    #[test]
    fn interactive_job_succeeds() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![JobSpec {
            name: "interactive".into(),
            command: "true".into(),
            working_dir: tmp_dir(),
            interactive: true,
            timeout: Duration::from_secs(10),
            ..Default::default()
        }];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Succeeded);
        // Interactive jobs don't capture output.
        assert!(results[0].stdout.is_empty());
    }

    #[test]
    fn interactive_job_fails() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![JobSpec {
            name: "interactive-fail".into(),
            command: "exit 3".into(),
            working_dir: tmp_dir(),
            interactive: true,
            timeout: Duration::from_secs(10),
            ..Default::default()
        }];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();

        assert_eq!(results[0].status, NodeStatus::Failed);
        assert_eq!(results[0].exit_code, Some(3));
    }

    // ── command_to_job_result ──────────────────────────────────────────

    #[test]
    fn command_to_job_result_success() {
        let cr = CommandResult {
            success: true,
            exit_code: Some(0),
            stdout: "out".into(),
            stderr: String::new(),
        };
        let result = command_to_job_result("test", &cr, Duration::from_secs(1));
        assert_eq!(result.name, "test");
        assert_eq!(result.status, NodeStatus::Succeeded);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "out");
    }

    #[test]
    fn command_to_job_result_failure() {
        let cr = CommandResult {
            success: false,
            exit_code: Some(42),
            stdout: String::new(),
            stderr: "error\n".into(),
        };
        let result = command_to_job_result("test", &cr, Duration::from_millis(500));
        assert_eq!(result.status, NodeStatus::Failed);
        assert_eq!(result.exit_code, Some(42));
        assert_eq!(result.stderr, "error\n");
    }

    // ── Routing logic ──────────────────────────────────────────────────

    #[test]
    fn deps_trigger_dag_mode() {
        // When a job has `needs`, the runner should use DAG execution.
        // We verify this indirectly: if DAG catches a missing dep, it errors.
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job_with_needs("a", "echo a", vec!["nonexistent"])];
        let result = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None);
        assert!(result.is_err());
    }

    #[test]
    fn no_deps_uses_flat_execution() {
        // Without `needs`, sequential mode should not build a DAG.
        // A simple success confirms the flat path works.
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let jobs = vec![make_job("a", "echo ok"), make_job("b", "echo ok")];
        let results = run_jobs(&jobs, ExecutionMode::Sequential, &presenter, None).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.status == NodeStatus::Succeeded));
    }

    // ── LogSink ────────────────────────────────────────────────────────

    /// Minimal test sink that records which lifecycle methods were called.
    #[derive(Default)]
    struct RecordingLogSink {
        events: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingLogSink {
        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl crate::executor::log_sink::LogSink for RecordingLogSink {
        fn on_job_start(&self, spec: &JobSpec) {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{}", spec.name));
        }
        fn on_job_output(&self, spec: &JobSpec, line: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("output:{}:{}", spec.name, line));
        }
        fn on_job_complete(&self, spec: &JobSpec, result: &JobResult) {
            self.events
                .lock()
                .unwrap()
                .push(format!("complete:{}:{:?}", spec.name, result.status));
        }
        fn on_job_runner_skipped(&self, spec: &JobSpec, reason: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("runner_skipped:{}:{}", spec.name, reason));
        }
    }

    #[test]
    fn sink_receives_start_output_and_complete_for_successful_job() {
        let jobs = vec![make_job("hello", "echo hello")];
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let concrete = Arc::new(RecordingLogSink::default());
        let sink_arc: Arc<dyn crate::executor::log_sink::LogSink> = concrete.clone();

        let _ = run_jobs(
            &jobs,
            ExecutionMode::Sequential,
            &presenter,
            Some(&sink_arc),
        )
        .unwrap();

        let events = concrete.events();
        assert!(events.iter().any(|e| e == "start:hello"));
        assert!(events.iter().any(|e| e.starts_with("output:hello:")));
        assert!(events.iter().any(|e| e == "complete:hello:Succeeded"));
    }

    #[test]
    fn sink_receives_runner_skipped_in_piped_mode_after_failure() {
        let jobs = vec![make_job("bad", "false"), make_job("after", "echo never")];
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let concrete = Arc::new(RecordingLogSink::default());
        let sink_arc: Arc<dyn crate::executor::log_sink::LogSink> = concrete.clone();

        let _ = run_jobs(&jobs, ExecutionMode::Piped, &presenter, Some(&sink_arc)).unwrap();

        let events = concrete.events();
        assert!(events
            .iter()
            .any(|e| e == "runner_skipped:after:previous job failed"));
    }
}
