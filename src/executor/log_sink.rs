//! Job log sinks: a pluggable seam between the generic runner and
//! persistent log storage.
//!
//! The runner drives job execution and streams output via a presenter for
//! live display. A `LogSink`, if provided, also receives output chunks and
//! completion notifications so it can write `meta.json` + `output.log`
//! entries into a `LogStore`. Callers that don't need persistence pass
//! `None`.

use super::{JobResult, JobSpec};

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
