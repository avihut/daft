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
    ///
    /// `target` names the entity the phase is acting on (e.g. the worktree
    /// being removed for `worktree-pre-remove`). Implementations may surface
    /// it in the phase header to disambiguate multi-source operations. `None`
    /// for project-scoped phases (`pre-merge`, `post-merge`, `post-clone`).
    fn on_phase_start(&self, phase_name: &str, target: Option<&str>);

    /// A job has started running.
    ///
    /// `command_preview` is the rendered shell command. When verbose mode is
    /// enabled, implementations should display it below the job name.
    fn on_job_start(&self, name: &str, description: Option<&str>, command_preview: Option<&str>);

    /// A running job produced an output line.
    fn on_job_output(&self, name: &str, line: &str);

    /// The events about to arrive are daft's own, not another tool's output
    /// to be parsed.
    ///
    /// Only the manager-output recognizer overrides this. It exists because
    /// `pre-push` is both the name of the synthetic row Path A emits around
    /// an opaque `git push` *and* a name a user may give a job inside their
    /// own `pre-push` hook. The recognizer holds the former, waiting to see
    /// whether the stream turns out to be a manager's; holding the latter
    /// would leave a real job's row missing. Path B knows the difference and
    /// says so, which is cheaper and more truthful than probing for it at
    /// every site that wraps a presenter.
    fn stand_down(&self) {}

    /// A job completed successfully.
    fn on_job_success(&self, name: &str, duration: Duration);

    /// A job failed.
    fn on_job_failure(&self, name: &str, duration: Duration);

    /// A job failed, carrying the child's exit code when one is known.
    ///
    /// `daft exec` surfaces `exit N` on the worker's rail row; hook renderers
    /// have no use for the code and inherit the default, which drops it and
    /// defers to [`Self::on_job_failure`]. A caller invokes exactly one of the
    /// two per failure.
    fn on_job_failure_with_exit(&self, name: &str, duration: Duration, _exit_code: Option<i32>) {
        self.on_job_failure(name, duration);
    }

    /// A job was skipped.
    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    );

    /// A job was cancelled by SIGINT while still running.
    fn on_job_cancelled(&self, name: &str, duration: Duration);

    /// A job was dispatched to run in the background.
    fn on_job_background(&self, name: &str, description: Option<&str>);

    /// A general informational message (not tied to a specific job).
    fn on_message(&self, msg: &str);

    /// Every job name this phase will render a row for, announced after
    /// `on_phase_start` and before any job runs. Width-aligned renderers
    /// size their name column from it once — receipt rows persist
    /// immediately and cannot re-pad when a wider-named job starts in a
    /// later `needs:` wave. Default: ignore.
    fn on_jobs_planned(&self, _names: &[String]) {}

    /// A hook manager was recognized on a job's output stream (#753): the
    /// jobs that follow are the manager's, routed as first-class events.
    /// `scope` is `None` on the pre-push gate path (the phase itself is the
    /// manager run) or the owning job's name when a manager runs inside a
    /// lifecycle job. Renderers may surface the fact (the rail folds it into
    /// the section header); everything else ignores it. Default: ignore.
    fn on_manager_engaged(&self, _scope: Option<&str>, _manager: &str, _version: Option<&str>) {}

    /// A recognized manager's output block for `name` flushed (#753). In
    /// lefthook's default (buffered) piped mode a job's block flushes at its
    /// completion, so this is a real-time "finished running" signal — ahead of
    /// the verdict, which the manager stamps only in its end-of-run summary.
    /// A live renderer stops the row's spinner and shows a neutral grey `✓`
    /// done-pending face; the summary later persists the confirmed `✓`/`✗`
    /// with the official duration. Display only — never a `JobResult`, never
    /// the verdict or exit policy. (Under `follow: true` the block header
    /// prints at job *start*; the row settles early there and the summary
    /// self-corrects — a rare, opt-in mode.) Default: ignore.
    fn on_manager_job_flushed(&self, _name: &str) {}

    /// A recognized manager running *inside* a lifecycle job reported one of
    /// its own jobs (#753). Children are presentation only: they never carry
    /// a `JobResult`, outcome policy stays the parent job's, and the child's
    /// raw lines still flow through `on_job_output` under the parent (its
    /// buffers, threads, and failure dumps are unchanged). Default: ignore.
    fn on_child_job_start(&self, _parent: &str, _name: &str) {}

    /// A manager child resolved successfully (its manager's summary said so,
    /// or the parent settled successfully with it still open).
    fn on_child_job_success(&self, _parent: &str, _name: &str, _duration: Duration) {}

    /// A manager child resolved failed.
    fn on_child_job_failure(&self, _parent: &str, _name: &str, _duration: Duration) {}

    /// A manager child settled by a cancelled parent — no row may be left
    /// spinning behind a `⊘` parent.
    fn on_child_job_cancelled(&self, _parent: &str, _name: &str, _duration: Duration) {}

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
    fn on_phase_start(&self, _phase_name: &str, _target: Option<&str>) {}
    fn on_job_start(
        &self,
        _name: &str,
        _description: Option<&str>,
        _command_preview: Option<&str>,
    ) {
    }
    fn on_job_output(&self, _name: &str, _line: &str) {}
    fn on_job_success(&self, _name: &str, _duration: Duration) {}
    fn on_job_failure(&self, _name: &str, _duration: Duration) {}
    fn on_job_skipped(
        &self,
        _name: &str,
        _reason: &str,
        _duration: Duration,
        _show_duration: bool,
        _command_preview: Option<&str>,
    ) {
    }
    fn on_job_cancelled(&self, _name: &str, _duration: Duration) {}
    fn on_job_background(&self, _name: &str, _description: Option<&str>) {}
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
        p.on_phase_start("test", None);
        p.on_job_start("job", Some("desc"), None);
        p.on_job_start("job", None, None);
        p.on_job_output("job", "line");
        p.on_job_success("job", Duration::from_secs(1));
        p.on_job_failure("job", Duration::from_secs(1));
        p.on_job_skipped("job", "reason", Duration::from_secs(0), false, None);
        p.on_job_cancelled("job", Duration::from_secs(1));
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
        p.on_phase_start("test", None);
        assert!(p.take_results().is_empty());
    }

    #[test]
    fn trait_object_from_null_presenter() {
        let presenter: Arc<dyn JobPresenter> = NullPresenter::arc();
        presenter.on_phase_start("phase", None);
        presenter.on_job_start("job", None, None);
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
            p.on_job_start("threaded-job", Some("from another thread"), None);
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
