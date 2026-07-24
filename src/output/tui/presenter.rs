//! TUI presenter that forwards hook lifecycle events as [`DagEvent`]s.
//!
//! Instead of rendering progress directly (like [`CliPresenter`]), this
//! presenter sends events through an `mpsc::Sender<DagEvent>` so the ratatui
//! TUI renderer can display hook status inline with the sync/prune table.
//!
//! One `TuiPresenter` is created per `run_hook()` call, scoped to a single
//! branch and hook type.
//!
//! [`CliPresenter`]: crate::executor::cli_presenter::CliPresenter

use crate::core::worktree::sync_dag::{DagEvent, DagHookPhase, JobCompletionStatus};
use crate::executor::JobResult;
use crate::executor::presenter::JobPresenter;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A [`JobPresenter`] that sends [`DagEvent`]s through a channel.
///
/// The TUI renderer consumes these events to update hook status rows in the
/// inline table. Individual job-level events are no-ops — the TUI shows
/// phase-level status only.
pub struct TuiPresenter {
    /// Channel to the TUI renderer.
    sender: mpsc::Sender<DagEvent>,
    /// Which branch this presenter is scoped to.
    branch_name: String,
    /// Which hook phase this presenter is scoped to.
    hook_type: DagHookPhase,
    /// When the phase started, for computing duration.
    start: Mutex<Option<Instant>>,
    /// Whether any job has failed during this phase.
    has_failure: Mutex<bool>,
    /// Accumulated output from jobs (merged stdout+stderr via `on_job_output`).
    ///
    /// The generic runner streams both stdout and stderr lines through the same
    /// `on_job_output` callback, so this contains the merged output stream.
    output: Mutex<String>,
    /// Per-job output, in arrival order. With a recognized manager (#753)
    /// each entry is one manager job's own lines; on the plain path there is
    /// a single entry mirroring `output`.
    job_outputs: Mutex<Vec<(String, String)>>,
    /// The first job that failed — the failure report scopes to its lines
    /// instead of dumping every passing job's output.
    failing_job: Mutex<Option<String>>,
}

impl TuiPresenter {
    /// Create a new `TuiPresenter` wrapped in an `Arc`.
    ///
    /// Each presenter is scoped to a single branch + hook type combination,
    /// matching one `run_hook()` call in the TUI bridge.
    pub fn new(
        sender: mpsc::Sender<DagEvent>,
        branch_name: impl Into<String>,
        hook_type: impl Into<DagHookPhase>,
    ) -> Arc<Self> {
        Arc::new(Self {
            sender,
            branch_name: branch_name.into(),
            hook_type: hook_type.into(),
            start: Mutex::new(None),
            has_failure: Mutex::new(false),
            output: Mutex::new(String::new()),
            job_outputs: Mutex::new(Vec::new()),
            failing_job: Mutex::new(None),
        })
    }

    /// Return the accumulated output, or `None` if empty.
    fn take_output(&self) -> Option<String> {
        let output = self.output.lock().expect("TuiPresenter output poisoned");
        if output.is_empty() {
            None
        } else {
            Some(output.clone())
        }
    }

    /// The failure report's output: the failing job's own lines when a
    /// recognized manager named it (#753), the whole merged stream
    /// otherwise. Falls back to the full stream when the failing job's
    /// buffer is empty — a silent failure still deserves whatever context
    /// the stream has.
    fn take_failure_output(&self, failing_job: Option<&str>) -> Option<String> {
        if let Some(job) = failing_job {
            let jobs = self
                .job_outputs
                .lock()
                .expect("TuiPresenter job_outputs poisoned");
            if let Some((_, text)) = jobs.iter().find(|(name, _)| name == job)
                && !text.is_empty()
            {
                return Some(text.clone());
            }
        }
        self.take_output()
    }
}

impl JobPresenter for TuiPresenter {
    fn on_phase_start(&self, _phase_name: &str, _target: Option<&str>) {
        // The TUI already knows which branch a phase is acting on
        // (`self.branch_name`); the target hint is redundant here.
        *self.start.lock().expect("TuiPresenter start poisoned") = Some(Instant::now());

        let _ = self.sender.send(DagEvent::HookStarted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
        });
    }

    fn on_job_start(&self, name: &str, _description: Option<&str>, _command_preview: Option<&str>) {
        let _ = self.sender.send(DagEvent::JobStarted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
            job_name: name.to_string(),
        });
    }

    fn on_job_output(&self, name: &str, line: &str) {
        // Accumulate output for failure reporting — the merged stream and
        // the per-job cut (#753 scopes the report to the failing job).
        let mut output = self.output.lock().expect("TuiPresenter output poisoned");
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
        drop(output);

        let mut jobs = self
            .job_outputs
            .lock()
            .expect("TuiPresenter job_outputs poisoned");
        match jobs.iter_mut().rfind(|(job, _)| job == name) {
            Some((_, text)) => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(line);
            }
            None => jobs.push((name.to_string(), line.to_string())),
        }
    }

    fn on_job_success(&self, name: &str, duration: Duration) {
        let _ = self.sender.send(DagEvent::JobCompleted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
            job_name: name.to_string(),
            status: JobCompletionStatus::Succeeded,
            duration,
            skip_reason: None,
        });
    }

    fn on_job_failure(&self, name: &str, duration: Duration) {
        *self
            .has_failure
            .lock()
            .expect("TuiPresenter has_failure poisoned") = true;
        self.failing_job
            .lock()
            .expect("TuiPresenter failing_job poisoned")
            .get_or_insert_with(|| name.to_string());

        let _ = self.sender.send(DagEvent::JobCompleted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
            job_name: name.to_string(),
            status: JobCompletionStatus::Failed,
            duration,
            skip_reason: None,
        });
    }

    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        _show_duration: bool,
        _command_preview: Option<&str>,
    ) {
        let _ = self.sender.send(DagEvent::JobCompleted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
            job_name: name.to_string(),
            status: JobCompletionStatus::Skipped,
            duration,
            skip_reason: Some(reason.to_string()),
        });
    }

    fn on_job_cancelled(&self, name: &str, duration: Duration) {
        // Same shape as on_job_failure — emit a JobCompleted event with
        // Failed status. The cancellation distinction is surfaced at the
        // exec renderer layer, not here.
        self.on_job_failure(name, duration);
    }

    fn on_job_background(&self, _name: &str, _description: Option<&str>) {
        // No-op: TUI does not display background job dispatches.
    }

    fn on_message(&self, _msg: &str) {
        // No-op: TUI does not display informational messages inline.
    }

    fn on_phase_complete(&self, _total_duration: Duration) {
        let start = self.start.lock().expect("TuiPresenter start poisoned");
        let duration = start.map_or(Duration::ZERO, |s| s.elapsed());

        let has_failure = *self
            .has_failure
            .lock()
            .expect("TuiPresenter has_failure poisoned");

        let failing_job = self
            .failing_job
            .lock()
            .expect("TuiPresenter failing_job poisoned")
            .clone();
        let output = if has_failure {
            self.take_failure_output(failing_job.as_deref())
        } else {
            None
        };

        // The presenter only observes warn-mode failures (FailMode::Warn) because
        // abort-mode failures cause executor.execute() to bail!() before calling
        // on_phase_complete. The TuiBridge Err path handles abort-mode events
        // separately. Therefore warned == has_failure is correct here.
        let _ = self.sender.send(DagEvent::HookCompleted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
            success: !has_failure,
            warned: has_failure,
            duration,
            exit_code: None,
            output,
            failing_job,
        });
    }

    fn on_manager_engaged(&self, _scope: Option<&str>, manager: &str, version: Option<&str>) {
        let _ = self.sender.send(DagEvent::ManagerEngaged {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
            manager: manager.to_string(),
            version: version.map(str::to_string),
        });
    }

    fn take_results(&self) -> Vec<JobResult> {
        // TUI doesn't use per-job results — the TuiBridge handles outcomes
        // via the DagEvent channel.
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::HookType;

    #[test]
    fn tui_presenter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TuiPresenter>();
    }

    #[test]
    fn tui_presenter_new_returns_arc() {
        let (tx, _rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);
        // Should be usable as a trait object.
        let _: Arc<dyn JobPresenter> = presenter;
    }

    #[test]
    fn on_phase_start_sends_hook_started() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/login", HookType::PostCreate);

        presenter.on_phase_start("worktree-post-create", None);

        let event = rx.try_recv().expect("should receive HookStarted");
        match event {
            DagEvent::HookStarted {
                branch_name,
                hook_type,
            } => {
                assert_eq!(branch_name, "feat/login");
                assert_eq!(hook_type, HookType::PostCreate.into());
            }
            other => panic!("expected HookStarted, got {other:?}"),
        }

        // No more events.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn on_phase_complete_sends_hook_completed_success() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostClone);

        presenter.on_phase_start("post-clone", None);
        // Drain the HookStarted event.
        let _ = rx.recv().unwrap();

        presenter.on_phase_complete(Duration::from_secs(2));

        let event = rx.recv().expect("should receive HookCompleted");
        match event {
            DagEvent::HookCompleted {
                branch_name,
                hook_type,
                success,
                warned,
                duration,
                exit_code,
                output,
                failing_job,
            } => {
                assert_eq!(branch_name, "main");
                assert_eq!(hook_type, HookType::PostClone.into());
                assert!(success);
                assert!(!warned);
                assert!(failing_job.is_none());
                // Duration should be non-negative (close to zero in tests).
                assert!(duration.as_secs() < 5);
                assert!(exit_code.is_none());
                assert!(output.is_none());
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }

    #[test]
    fn on_phase_complete_sends_hook_completed_failure() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/broken", HookType::PreCreate);

        presenter.on_phase_start("worktree-pre-create", None);

        // Simulate a job failure.
        presenter.on_job_start("build", Some("Build project"), None);
        presenter.on_job_output("build", "error: compilation failed");
        presenter.on_job_failure("build", Duration::from_secs(1));

        presenter.on_phase_complete(Duration::from_secs(1));

        // Drain all events and find the HookCompleted.
        drop(presenter);
        let events: Vec<DagEvent> = rx.try_iter().collect();
        let hook_completed = events
            .iter()
            .find(|e| matches!(e, DagEvent::HookCompleted { .. }))
            .expect("should receive HookCompleted");
        match hook_completed {
            DagEvent::HookCompleted {
                success,
                warned,
                output,
                ..
            } => {
                assert!(!success);
                assert!(warned);
                assert_eq!(output.as_deref(), Some("error: compilation failed"));
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }

    #[test]
    fn job_output_accumulated_on_failure() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/x", HookType::PostCreate);

        presenter.on_phase_start("worktree-post-create", None);

        presenter.on_job_start("install", None, None);
        presenter.on_job_output("install", "fetching packages...");
        presenter.on_job_output("install", "error: network timeout");
        presenter.on_job_failure("install", Duration::from_secs(5));

        presenter.on_phase_complete(Duration::from_secs(5));

        // Drain all events and find the HookCompleted.
        drop(presenter);
        let events: Vec<DagEvent> = rx.try_iter().collect();
        let hook_completed = events
            .iter()
            .find(|e| matches!(e, DagEvent::HookCompleted { .. }))
            .expect("should receive HookCompleted");
        match hook_completed {
            DagEvent::HookCompleted { output, .. } => {
                let text = output.as_ref().expect("should have output on failure");
                assert!(text.contains("fetching packages..."));
                assert!(text.contains("error: network timeout"));
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }

    #[test]
    fn failure_output_scopes_to_the_failing_manager_job() {
        // A recognized manager (#753) routes per-job output through the
        // wrapper; the report must carry only the failing job's lines, not
        // every passing job's output — and name the job.
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/x", DagHookPhase::PrePush);

        presenter.on_phase_start("pre-push", None);
        presenter.on_job_start("fmt", None, None);
        presenter.on_job_output("fmt", "fmt clean");
        presenter.on_job_success("fmt", Duration::from_millis(300));
        presenter.on_job_start("unit tests (related)", None, None);
        presenter.on_job_output("unit tests (related)", "FAILED assertion at foo.rs:12");
        presenter.on_job_output("unit tests (related)", "exit status 1");
        presenter.on_job_failure("unit tests (related)", Duration::from_millis(500));
        presenter.on_phase_complete(Duration::from_secs(1));

        drop(presenter);
        let events: Vec<DagEvent> = rx.try_iter().collect();
        let completed = events
            .iter()
            .find(|e| matches!(e, DagEvent::HookCompleted { .. }))
            .expect("should receive HookCompleted");
        match completed {
            DagEvent::HookCompleted {
                output,
                failing_job,
                ..
            } => {
                assert_eq!(failing_job.as_deref(), Some("unit tests (related)"));
                let text = output.as_ref().expect("failure carries output");
                assert!(text.contains("FAILED assertion"), "{text}");
                assert!(
                    !text.contains("fmt clean"),
                    "passing jobs' output must not ride the report: {text}"
                );
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }

    #[test]
    fn silent_failing_job_falls_back_to_the_whole_stream() {
        // The failing job printed nothing: its empty buffer must not erase
        // the report — fall back to the merged stream for context.
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/x", DagHookPhase::PrePush);

        presenter.on_phase_start("pre-push", None);
        presenter.on_job_start("fmt", None, None);
        presenter.on_job_output("fmt", "fmt clean");
        presenter.on_job_success("fmt", Duration::from_millis(300));
        presenter.on_job_failure("mute", Duration::from_millis(100));
        presenter.on_phase_complete(Duration::from_secs(1));

        drop(presenter);
        let events: Vec<DagEvent> = rx.try_iter().collect();
        match events
            .iter()
            .find(|e| matches!(e, DagEvent::HookCompleted { .. }))
            .expect("should receive HookCompleted")
        {
            DagEvent::HookCompleted {
                output,
                failing_job,
                ..
            } => {
                assert_eq!(failing_job.as_deref(), Some("mute"));
                assert_eq!(output.as_deref(), Some("fmt clean"));
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }

    #[test]
    fn manager_engagement_forwards_as_a_dag_event() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/x", DagHookPhase::PrePush);

        presenter.on_manager_engaged(None, "lefthook", Some("2.1.10"));

        match rx.try_recv().expect("should receive ManagerEngaged") {
            DagEvent::ManagerEngaged {
                branch_name,
                manager,
                version,
                ..
            } => {
                assert_eq!(branch_name, "feat/x");
                assert_eq!(manager, "lefthook");
                assert_eq!(version.as_deref(), Some("2.1.10"));
            }
            other => panic!("expected ManagerEngaged, got {other:?}"),
        }
    }

    #[test]
    fn take_results_returns_empty() {
        let (tx, _rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostClone);

        let results = presenter.take_results();
        assert!(results.is_empty());
    }

    #[test]
    fn on_job_start_sends_job_started() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        presenter.on_job_start("build", Some("Build project"), None);

        let event = rx.try_recv().expect("should receive JobStarted");
        match event {
            DagEvent::JobStarted {
                branch_name,
                hook_type,
                job_name,
            } => {
                assert_eq!(branch_name, "main");
                assert_eq!(hook_type, HookType::PostCreate.into());
                assert_eq!(job_name, "build");
            }
            other => panic!("expected JobStarted, got {other:?}"),
        }
    }

    #[test]
    fn on_job_success_sends_job_completed() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        presenter.on_job_success("build", Duration::from_secs(1));

        let event = rx.try_recv().expect("should receive JobCompleted");
        match event {
            DagEvent::JobCompleted {
                job_name, status, ..
            } => {
                assert_eq!(job_name, "build");
                assert_eq!(status, JobCompletionStatus::Succeeded);
            }
            other => panic!("expected JobCompleted, got {other:?}"),
        }
    }

    #[test]
    fn on_job_failure_sends_event_and_sets_flag() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/x", HookType::PreRemove);

        presenter.on_job_failure("deploy", Duration::from_secs(2));

        let event = rx.try_recv().expect("should receive JobCompleted");
        match event {
            DagEvent::JobCompleted {
                job_name, status, ..
            } => {
                assert_eq!(job_name, "deploy");
                assert_eq!(status, JobCompletionStatus::Failed);
            }
            other => panic!("expected JobCompleted, got {other:?}"),
        }
    }

    #[test]
    fn on_job_cancelled_sends_failed_event() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "feat/x", HookType::PreRemove);

        presenter.on_job_cancelled("deploy", Duration::from_secs(2));

        let event = rx.try_recv().expect("should receive JobCompleted");
        match event {
            DagEvent::JobCompleted {
                job_name, status, ..
            } => {
                assert_eq!(job_name, "deploy");
                assert_eq!(status, JobCompletionStatus::Failed);
            }
            other => panic!("expected JobCompleted, got {other:?}"),
        }
    }

    #[test]
    fn on_job_skipped_sends_job_completed_with_reason() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        presenter.on_job_skipped("lint", "no files", Duration::ZERO, false, None);

        let event = rx.try_recv().expect("should receive JobCompleted");
        match event {
            DagEvent::JobCompleted {
                job_name,
                status,
                skip_reason,
                ..
            } => {
                assert_eq!(job_name, "lint");
                assert_eq!(status, JobCompletionStatus::Skipped);
                assert_eq!(skip_reason.as_deref(), Some("no files"));
            }
            other => panic!("expected JobCompleted, got {other:?}"),
        }
    }

    #[test]
    fn on_message_is_noop() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        presenter.on_message("hello world");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn output_not_included_on_success() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        presenter.on_phase_start("worktree-post-create", None);

        // Job produces output but succeeds.
        presenter.on_job_start("build", None, None);
        presenter.on_job_output("build", "compiled 42 files");
        presenter.on_job_success("build", Duration::from_secs(1));

        presenter.on_phase_complete(Duration::from_secs(1));

        // Drain all events and find the HookCompleted.
        drop(presenter);
        let events: Vec<DagEvent> = rx.try_iter().collect();
        let hook_completed = events
            .iter()
            .find(|e| matches!(e, DagEvent::HookCompleted { .. }))
            .expect("should receive HookCompleted");
        match hook_completed {
            DagEvent::HookCompleted {
                success, output, ..
            } => {
                assert!(success);
                // Output should be None on success — no need to show it.
                assert!(output.is_none());
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }

    #[test]
    fn phase_complete_without_start_uses_zero_duration() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostClone);

        // Calling on_phase_complete without on_phase_start should not panic.
        presenter.on_phase_complete(Duration::from_secs(1));

        let event = rx.recv().expect("should receive HookCompleted");
        match event {
            DagEvent::HookCompleted { duration, .. } => {
                assert_eq!(duration, Duration::ZERO);
            }
            other => panic!("expected HookCompleted, got {other:?}"),
        }
    }
}
