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

use crate::core::worktree::sync_dag::DagEvent;
use crate::executor::presenter::JobPresenter;
use crate::executor::JobResult;
use crate::hooks::HookType;
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
    hook_type: HookType,
    /// When the phase started, for computing duration.
    start: Mutex<Option<Instant>>,
    /// Whether any job has failed during this phase.
    has_failure: Mutex<bool>,
    /// Accumulated output from jobs (merged stdout+stderr via `on_job_output`).
    ///
    /// The generic runner streams both stdout and stderr lines through the same
    /// `on_job_output` callback, so this contains the merged output stream.
    output: Mutex<String>,
}

impl TuiPresenter {
    /// Create a new `TuiPresenter` wrapped in an `Arc`.
    ///
    /// Each presenter is scoped to a single branch + hook type combination,
    /// matching one `run_hook()` call in the TUI bridge.
    pub fn new(
        sender: mpsc::Sender<DagEvent>,
        branch_name: impl Into<String>,
        hook_type: HookType,
    ) -> Arc<Self> {
        Arc::new(Self {
            sender,
            branch_name: branch_name.into(),
            hook_type,
            start: Mutex::new(None),
            has_failure: Mutex::new(false),
            output: Mutex::new(String::new()),
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
}

impl JobPresenter for TuiPresenter {
    fn on_phase_start(&self, _phase_name: &str) {
        *self.start.lock().expect("TuiPresenter start poisoned") = Some(Instant::now());

        let _ = self.sender.send(DagEvent::HookStarted {
            branch_name: self.branch_name.clone(),
            hook_type: self.hook_type,
        });
    }

    fn on_job_start(&self, _name: &str, _description: Option<&str>) {
        // No-op: TUI shows phase-level status only.
    }

    fn on_job_output(&self, _name: &str, line: &str) {
        // Accumulate output for failure reporting.
        let mut output = self.output.lock().expect("TuiPresenter output poisoned");
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(line);
    }

    fn on_job_success(&self, _name: &str, _duration: Duration) {
        // No-op: TUI shows phase-level status only.
    }

    fn on_job_failure(&self, _name: &str, _duration: Duration) {
        *self
            .has_failure
            .lock()
            .expect("TuiPresenter has_failure poisoned") = true;
    }

    fn on_job_skipped(
        &self,
        _name: &str,
        _reason: &str,
        _duration: Duration,
        _show_duration: bool,
    ) {
        // No-op: TUI shows phase-level status only.
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

        let output = if has_failure {
            self.take_output()
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

        presenter.on_phase_start("worktree-post-create");

        let event = rx.try_recv().expect("should receive HookStarted");
        match event {
            DagEvent::HookStarted {
                branch_name,
                hook_type,
            } => {
                assert_eq!(branch_name, "feat/login");
                assert_eq!(hook_type, HookType::PostCreate);
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

        presenter.on_phase_start("post-clone");
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
            } => {
                assert_eq!(branch_name, "main");
                assert_eq!(hook_type, HookType::PostClone);
                assert!(success);
                assert!(!warned);
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

        presenter.on_phase_start("worktree-pre-create");
        let _ = rx.recv().unwrap(); // Drain HookStarted.

        // Simulate a job failure.
        presenter.on_job_start("build", Some("Build project"));
        presenter.on_job_output("build", "error: compilation failed");
        presenter.on_job_failure("build", Duration::from_secs(1));

        presenter.on_phase_complete(Duration::from_secs(1));

        let event = rx.recv().expect("should receive HookCompleted");
        match event {
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

        presenter.on_phase_start("worktree-post-create");
        let _ = rx.recv().unwrap(); // Drain HookStarted.

        presenter.on_job_start("install", None);
        presenter.on_job_output("install", "fetching packages...");
        presenter.on_job_output("install", "error: network timeout");
        presenter.on_job_failure("install", Duration::from_secs(5));

        presenter.on_phase_complete(Duration::from_secs(5));

        let event = rx.recv().expect("should receive HookCompleted");
        match event {
            DagEvent::HookCompleted { output, .. } => {
                let text = output.expect("should have output on failure");
                assert!(text.contains("fetching packages..."));
                assert!(text.contains("error: network timeout"));
            }
            other => panic!("expected HookCompleted, got {other:?}"),
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
    fn job_level_events_are_noop() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        // These should not send any events.
        presenter.on_job_start("build", Some("Build project"));
        presenter.on_job_success("build", Duration::from_secs(1));
        presenter.on_job_skipped("lint", "no files", Duration::ZERO, false);
        presenter.on_message("hello world");

        // Channel should be empty.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn output_not_included_on_success() {
        let (tx, rx) = mpsc::channel();
        let presenter = TuiPresenter::new(tx, "main", HookType::PostCreate);

        presenter.on_phase_start("worktree-post-create");
        let _ = rx.recv().unwrap(); // Drain HookStarted.

        // Job produces output but succeeds.
        presenter.on_job_start("build", None);
        presenter.on_job_output("build", "compiled 42 files");
        presenter.on_job_success("build", Duration::from_secs(1));

        presenter.on_phase_complete(Duration::from_secs(1));

        let event = rx.recv().expect("should receive HookCompleted");
        match event {
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
