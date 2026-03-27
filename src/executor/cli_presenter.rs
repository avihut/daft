//! CLI presenter wrapping [`HookRenderer`] for thread-safe progress output.
//!
//! Adapts the existing `HookRenderer` (which uses `&mut self`) into the
//! [`JobPresenter`] trait (which uses `&self` with interior mutability).

use super::presenter::JobPresenter;
use super::{JobResult, NodeStatus};
use crate::output::hook_progress::{HookRenderer, JobOutcome, JobResultEntry};
use crate::settings::HookOutputConfig;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Thread-safe CLI presenter backed by [`HookRenderer`].
///
/// Wraps a `HookRenderer` in a `Mutex` so it can be shared across threads
/// via `Arc<CliPresenter>`. All trait methods lock the mutex briefly.
pub struct CliPresenter {
    renderer: Mutex<HookRenderer>,
}

impl CliPresenter {
    /// Create a presenter that auto-detects TTY vs plain output.
    pub fn auto(config: &HookOutputConfig) -> Arc<Self> {
        Arc::new(Self {
            renderer: Mutex::new(HookRenderer::auto(config)),
        })
    }

    /// Create from an existing `HookRenderer` (useful for tests).
    #[cfg(test)]
    pub fn from_renderer(renderer: HookRenderer) -> Arc<Self> {
        Arc::new(Self {
            renderer: Mutex::new(renderer),
        })
    }

    /// Convert a `JobResultEntry` (from `HookRenderer`) into our generic `JobResult`.
    fn entry_to_job_result(entry: JobResultEntry) -> JobResult {
        let status = match &entry.outcome {
            JobOutcome::Success => NodeStatus::Succeeded,
            JobOutcome::Failed => NodeStatus::Failed,
            JobOutcome::Skipped { .. } => NodeStatus::Skipped,
            JobOutcome::Background { .. } => NodeStatus::Pending,
        };

        JobResult {
            name: entry.name,
            status,
            duration: entry.duration,
            // HookRenderer does not track exit codes or output.
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

impl JobPresenter for CliPresenter {
    fn on_phase_start(&self, phase_name: &str) {
        let r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.print_header(phase_name);
    }

    fn on_job_start(&self, name: &str, description: Option<&str>, command_preview: Option<&str>) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.start_job_with_description(name, description, command_preview);
    }

    fn on_job_output(&self, name: &str, line: &str) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.update_job_output(name, line);
    }

    fn on_job_success(&self, name: &str, duration: Duration) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.finish_job_success(name, duration);
    }

    fn on_job_failure(&self, name: &str, duration: Duration) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.finish_job_failure(name, duration);
    }

    fn on_job_skipped(&self, name: &str, reason: &str, duration: Duration, show_duration: bool) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.finish_job_skipped(name, reason, duration, show_duration);
    }

    fn on_job_background(&self, name: &str, description: Option<&str>) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.show_background_job(name, description);
        r.record_background_job(name, description);
    }

    fn on_message(&self, msg: &str) {
        let r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.println(msg);
    }

    fn on_phase_complete(&self, total_duration: Duration) {
        let r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.print_summary(total_duration);
    }

    fn take_results(&self) -> Vec<JobResult> {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.take_finished_jobs()
            .into_iter()
            .map(Self::entry_to_job_result)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_presenter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CliPresenter>();
    }

    #[test]
    fn auto_returns_arc() {
        let config = HookOutputConfig::default();
        let presenter = CliPresenter::auto(&config);
        // Should be usable as a trait object.
        let _: Arc<dyn JobPresenter> = presenter;
    }

    #[test]
    fn from_renderer_creates_presenter() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);
        let _: Arc<dyn JobPresenter> = presenter;
    }

    #[test]
    fn lifecycle_through_trait() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter: Arc<dyn JobPresenter> = CliPresenter::from_renderer(renderer);

        presenter.on_phase_start("post-clone");
        presenter.on_job_start("install", Some("Install dependencies"), None);
        presenter.on_job_output("install", "fetching packages...");
        presenter.on_job_success("install", Duration::from_secs(2));
        presenter.on_phase_complete(Duration::from_secs(3));

        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "install");
        assert_eq!(results[0].status, NodeStatus::Succeeded);
    }

    #[test]
    fn failure_maps_to_failed_status() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);

        presenter.on_job_start("build", None, None);
        presenter.on_job_failure("build", Duration::from_secs(1));

        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Failed);
    }

    #[test]
    fn skipped_maps_to_skipped_status() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);

        presenter.on_job_start("lint", None, None);
        presenter.on_job_skipped("lint", "no files changed", Duration::from_millis(10), false);

        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, NodeStatus::Skipped);
    }

    #[test]
    fn take_results_drains() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);

        presenter.on_job_start("a", None, None);
        presenter.on_job_success("a", Duration::from_secs(1));

        let first = presenter.take_results();
        assert_eq!(first.len(), 1);

        let second = presenter.take_results();
        assert!(second.is_empty());
    }

    #[test]
    fn entry_to_job_result_success() {
        let entry = JobResultEntry {
            name: "job".into(),
            outcome: JobOutcome::Success,
            duration: Duration::from_secs(1),
        };
        let result = CliPresenter::entry_to_job_result(entry);
        assert_eq!(result.status, NodeStatus::Succeeded);
        assert!(result.exit_code.is_none());
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn entry_to_job_result_failed() {
        let entry = JobResultEntry {
            name: "job".into(),
            outcome: JobOutcome::Failed,
            duration: Duration::from_secs(2),
        };
        let result = CliPresenter::entry_to_job_result(entry);
        assert_eq!(result.status, NodeStatus::Failed);
    }

    #[test]
    fn entry_to_job_result_skipped() {
        let entry = JobResultEntry {
            name: "job".into(),
            outcome: JobOutcome::Skipped {
                reason: "condition".into(),
                show_duration: true,
            },
            duration: Duration::from_millis(50),
        };
        let result = CliPresenter::entry_to_job_result(entry);
        assert_eq!(result.status, NodeStatus::Skipped);
    }

    #[test]
    fn on_message_does_not_panic() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter = CliPresenter::from_renderer(renderer);
        presenter.on_message("informational message");
    }

    #[test]
    fn usable_across_threads() {
        let config = HookOutputConfig::default();
        let renderer = HookRenderer::new_hidden(&config);
        let presenter: Arc<dyn JobPresenter> = CliPresenter::from_renderer(renderer);
        let p = Arc::clone(&presenter);

        let handle = std::thread::spawn(move || {
            p.on_job_start("threaded", None, None);
            p.on_job_success("threaded", Duration::from_millis(50));
        });

        handle.join().unwrap();
        let results = presenter.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "threaded");
    }
}
