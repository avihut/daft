//! Presentation trait for job execution progress.
//!
//! The [`JobPresenter`] trait decouples execution from display, allowing
//! different renderers (CLI spinners, TUI, tests) to observe the same events.

use super::JobResult;
use std::sync::Arc;
use std::time::Duration;

/// Trait for observing job execution lifecycle events.
///
/// Implementations must be `Send + Sync` so presenters can be shared across
/// threads. Methods take `&self` (not `&mut self`) — implementations should
/// use interior mutability (e.g., `Mutex`) when state updates are needed.
pub trait JobPresenter: Send + Sync {
    /// A new execution phase is starting (e.g., "post-clone", "sync").
    fn on_phase_start(&self, phase_name: &str);

    /// A job has started running.
    fn on_job_start(&self, name: &str, description: Option<&str>);

    /// A running job produced an output line.
    fn on_job_output(&self, name: &str, line: &str);

    /// A job completed successfully.
    fn on_job_success(&self, name: &str, duration: Duration);

    /// A job failed.
    fn on_job_failure(&self, name: &str, duration: Duration);

    /// A job was skipped.
    fn on_job_skipped(&self, name: &str, reason: &str, duration: Duration, show_duration: bool);

    /// A general informational message (not tied to a specific job).
    fn on_message(&self, msg: &str);

    /// A phase has completed. Display the summary.
    fn on_phase_complete(&self, total_duration: Duration);

    /// Drain and return all accumulated job results.
    fn take_results(&self) -> Vec<JobResult>;
}

// ─────────────────────────────────────────────────────────────────────────
// NullPresenter — no-op implementation for tests
// ─────────────────────────────────────────────────────────────────────────

/// A no-op presenter that silently discards all events.
///
/// Useful in tests where presentation output is not needed.
pub struct NullPresenter;

impl NullPresenter {
    /// Create a new `NullPresenter` wrapped in an `Arc`.
    pub fn arc() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl JobPresenter for NullPresenter {
    fn on_phase_start(&self, _phase_name: &str) {}
    fn on_job_start(&self, _name: &str, _description: Option<&str>) {}
    fn on_job_output(&self, _name: &str, _line: &str) {}
    fn on_job_success(&self, _name: &str, _duration: Duration) {}
    fn on_job_failure(&self, _name: &str, _duration: Duration) {}
    fn on_job_skipped(
        &self,
        _name: &str,
        _reason: &str,
        _duration: Duration,
        _show_duration: bool,
    ) {
    }
    fn on_message(&self, _msg: &str) {}
    fn on_phase_complete(&self, _total_duration: Duration) {}
    fn take_results(&self) -> Vec<JobResult> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::NodeStatus;

    #[test]
    fn null_presenter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NullPresenter>();
    }

    #[test]
    fn null_presenter_arc_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<NullPresenter>>();
    }

    #[test]
    fn null_presenter_methods_are_no_ops() {
        let p = NullPresenter;
        p.on_phase_start("test");
        p.on_job_start("job", Some("desc"));
        p.on_job_start("job", None);
        p.on_job_output("job", "line");
        p.on_job_success("job", Duration::from_secs(1));
        p.on_job_failure("job", Duration::from_secs(1));
        p.on_job_skipped("job", "reason", Duration::from_secs(0), false);
        p.on_message("hello");
        p.on_phase_complete(Duration::from_secs(5));
    }

    #[test]
    fn null_presenter_take_results_returns_empty() {
        let p = NullPresenter;
        let results = p.take_results();
        assert!(results.is_empty());
    }

    #[test]
    fn null_presenter_arc_constructor() {
        let p = NullPresenter::arc();
        p.on_phase_start("test");
        assert!(p.take_results().is_empty());
    }

    #[test]
    fn trait_object_from_null_presenter() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        presenter.on_phase_start("phase");
        presenter.on_job_start("job", None);
        presenter.on_job_output("job", "output");
        presenter.on_job_success("job", Duration::from_secs(1));
        presenter.on_phase_complete(Duration::from_secs(2));
        assert!(presenter.take_results().is_empty());
    }

    /// Verify that the trait can be used as a shared reference across threads.
    #[test]
    fn presenter_usable_across_threads() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        let p = Arc::clone(&presenter);

        let handle = std::thread::spawn(move || {
            p.on_job_start("threaded-job", Some("from another thread"));
            p.on_job_success("threaded-job", Duration::from_millis(100));
        });

        handle.join().unwrap();
        // Main thread can still use presenter.
        presenter.on_phase_complete(Duration::from_secs(1));
    }

    /// Verify NodeStatus is accessible from presenter test module (re-export check).
    #[test]
    fn node_status_accessible() {
        assert!(NodeStatus::Succeeded.is_terminal());
    }
}
