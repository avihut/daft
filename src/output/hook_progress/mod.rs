//! Hook progress renderer using indicatif for spinners and rolling output.

mod formatting;
mod interactive;
mod plain;

pub(crate) use formatting::DEFAULT_NAME_COLUMN_WIDTH;
pub use interactive::HookProgressRenderer;
pub use plain::PlainHookRenderer;

use crate::settings::HookOutputConfig;
use std::time::Duration;

/// Outcome of a completed job.
#[derive(Debug, Clone)]
pub enum JobOutcome {
    Success,
    Failed,
    Skipped { reason: String, show_duration: bool },
}

/// Entry recording a completed job for the summary.
#[derive(Debug, Clone)]
pub struct JobResultEntry {
    pub name: String,
    pub outcome: JobOutcome,
    pub duration: Duration,
}

// ─────────────────────────────────────────────────────────────────────────
// Unified renderer enum
// ─────────────────────────────────────────────────────────────────────────

/// Unified hook renderer with TTY and non-TTY variants.
///
/// Use [`HookRenderer::auto`] to automatically select the appropriate
/// renderer based on whether stderr is a terminal.
pub enum HookRenderer {
    /// Rich spinner-based output for interactive terminals.
    Progress(Box<HookProgressRenderer>),
    /// Plain text output for CI, pipes, and non-TTY environments.
    Plain(PlainHookRenderer),
}

impl HookRenderer {
    /// Auto-detect: use rich renderer if stderr is a TTY, plain otherwise.
    /// Returns a hidden renderer when `DAFT_TESTING` is set to keep test output clean.
    pub fn auto(config: &HookOutputConfig) -> Self {
        if formatting::output_suppressed() {
            return HookRenderer::Progress(Box::new(HookProgressRenderer::new_hidden(config)));
        }
        use std::io::IsTerminal;
        if std::io::stderr().is_terminal() {
            HookRenderer::Progress(Box::new(HookProgressRenderer::new(config)))
        } else {
            let mut plain = PlainHookRenderer::with_verbose(config.verbose);
            plain.set_compact_finalization(config.compact_finalization);
            HookRenderer::Plain(plain)
        }
    }

    #[cfg(test)]
    pub fn new_hidden(config: &HookOutputConfig) -> Self {
        HookRenderer::Progress(Box::new(HookProgressRenderer::new_hidden(config)))
    }

    pub fn print_header(&self, hook_name: &str) {
        match self {
            HookRenderer::Progress(r) => r.print_header(hook_name),
            HookRenderer::Plain(r) => r.print_header(hook_name),
        }
    }

    pub fn start_job(&mut self, name: &str) {
        match self {
            HookRenderer::Progress(r) => r.start_job(name, None),
            HookRenderer::Plain(r) => r.start_job(name, None),
        }
    }

    pub fn start_job_with_description(
        &mut self,
        name: &str,
        description: Option<&str>,
        command_preview: Option<&str>,
    ) {
        match self {
            HookRenderer::Progress(r) => {
                r.start_job_with_description(name, description, command_preview);
            }
            HookRenderer::Plain(r) => {
                r.start_job_with_description(name, description, command_preview);
            }
        }
    }

    pub fn update_job_output(&mut self, name: &str, line: &str) {
        match self {
            HookRenderer::Progress(r) => r.update_job_output(name, line),
            HookRenderer::Plain(r) => r.update_job_output(name, line),
        }
    }

    pub fn finish_job_success(&mut self, name: &str, duration: Duration) {
        match self {
            HookRenderer::Progress(r) => r.finish_job_success(name, duration),
            HookRenderer::Plain(r) => r.finish_job_success(name, duration),
        }
    }

    pub fn finish_job_failure(&mut self, name: &str, duration: Duration) {
        match self {
            HookRenderer::Progress(r) => r.finish_job_failure(name, duration),
            HookRenderer::Plain(r) => r.finish_job_failure(name, duration),
        }
    }

    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        match self {
            HookRenderer::Progress(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
            }
            HookRenderer::Plain(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
            }
        }
    }

    pub fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        match self {
            HookRenderer::Progress(r) => r.finish_job_cancelled(name, duration),
            HookRenderer::Plain(r) => r.finish_job_cancelled(name, duration),
        }
    }

    pub fn set_name_column_width(&mut self, width: usize) {
        match self {
            HookRenderer::Progress(r) => r.set_name_column_width(width),
            HookRenderer::Plain(r) => r.set_name_column_width(width),
        }
    }

    pub fn print_summary(&self, total_duration: Duration) {
        match self {
            HookRenderer::Progress(r) => r.print_summary(total_duration),
            HookRenderer::Plain(r) => r.print_summary(total_duration),
        }
    }

    pub fn take_finished_jobs(&mut self) -> Vec<JobResultEntry> {
        match self {
            HookRenderer::Progress(r) => r.take_finished_jobs(),
            HookRenderer::Plain(r) => r.take_finished_jobs(),
        }
    }

    pub fn println(&self, msg: &str) {
        match self {
            HookRenderer::Progress(r) => r.println(msg),
            HookRenderer::Plain(r) => r.println(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_renderer_creation() {
        let config = HookOutputConfig::default();
        let _renderer = HookProgressRenderer::new_hidden(&config);
    }

    #[test]
    fn test_start_and_finish_job() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("test-job", None);
        renderer.finish_job_success("test-job", Duration::from_secs(2));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Success));
    }

    #[test]
    fn test_update_job_output_rolling_window() {
        let config = HookOutputConfig {
            tail_lines: 3,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("test-job", None);

        for i in 0..10 {
            renderer.update_job_output("test-job", &format!("line {i}"));
        }

        let output = renderer.get_buffered_output("test-job");
        assert_eq!(output.len(), 10);

        renderer.finish_job_success("test-job", Duration::from_secs(1));
    }

    #[test]
    fn test_quiet_mode_no_tail_lines() {
        let config = HookOutputConfig {
            quiet: true,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("test-job", None);
        renderer.update_job_output("test-job", "should not show");
        renderer.finish_job_success("test-job", Duration::from_secs(1));
    }

    #[test]
    fn test_finish_job_failure() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("failing-job", None);
        renderer.update_job_output("failing-job", "error output");
        renderer.finish_job_failure("failing-job", Duration::from_secs(3));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }

    #[test]
    fn test_format_duration_milliseconds() {
        assert_eq!(
            formatting::format_duration(Duration::from_millis(112)),
            "112ms"
        );
        assert_eq!(
            formatting::format_duration(Duration::from_millis(98)),
            "98ms"
        );
        assert_eq!(formatting::format_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(
            formatting::format_duration(Duration::from_millis(999)),
            "999ms"
        );
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(
            formatting::format_duration(Duration::from_secs_f64(2.3)),
            "2.3s"
        );
        assert_eq!(
            formatting::format_duration(Duration::from_secs_f64(1.0)),
            "1.0s"
        );
        assert_eq!(
            formatting::format_duration(Duration::from_secs_f64(59.9)),
            "59.9s"
        );
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(
            formatting::format_duration(Duration::from_secs(65)),
            "1m 5s"
        );
        assert_eq!(
            formatting::format_duration(Duration::from_secs(120)),
            "2m 0s"
        );
    }

    #[test]
    fn test_plain_renderer_lifecycle() {
        let mut renderer = PlainHookRenderer::new();
        renderer.start_job("test-job", None);
        renderer.update_job_output("test-job", "line 1");
        renderer.update_job_output("test-job", "line 2");
        renderer.finish_job_success("test-job", Duration::from_secs(2));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn test_plain_renderer_failure() {
        let mut renderer = PlainHookRenderer::new();
        renderer.start_job("fail-job", None);
        renderer.finish_job_failure("fail-job", Duration::from_secs(3));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }

    #[test]
    fn plain_compact_finalization_records_success() {
        let mut renderer = PlainHookRenderer::new();
        renderer.set_compact_finalization(true);
        renderer.start_job("master", None);
        renderer.update_job_output("master", "build output");
        renderer.finish_job_success("master", Duration::from_millis(1800));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "master");
        assert!(matches!(jobs[0].outcome, JobOutcome::Success));
    }

    #[test]
    fn plain_compact_finalization_records_failure() {
        let mut renderer = PlainHookRenderer::new();
        renderer.set_compact_finalization(true);
        renderer.start_job("feat/dirty", None);
        renderer.finish_job_failure("feat/dirty", Duration::from_millis(1200));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }

    #[test]
    fn plain_compact_finalization_propagates_via_auto() {
        // Verify the HookRenderer::auto bridge: when config.compact_finalization
        // is true and stderr is not a TTY, the Plain variant should be set up
        // with compact_finalization enabled. Because auto() returns a hidden
        // Progress renderer under cfg(test), we exercise the bridge path
        // directly by mirroring its logic on a PlainHookRenderer.
        let config = HookOutputConfig {
            compact_finalization: true,
            ..Default::default()
        };
        let mut plain = PlainHookRenderer::with_verbose(config.verbose);
        plain.set_compact_finalization(config.compact_finalization);
        plain.start_job("branch", None);
        plain.finish_job_success("branch", Duration::from_millis(500));
        let jobs = plain.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Success));
    }

    #[test]
    fn test_summary_tracking() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job-a", None);
        renderer.finish_job_success("job-a", Duration::from_millis(150));
        renderer.start_job("job-b", None);
        renderer.finish_job_failure("job-b", Duration::from_secs(2));

        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "job-a");
        assert!(matches!(jobs[0].outcome, JobOutcome::Success));
        assert_eq!(jobs[1].name, "job-b");
        assert!(matches!(jobs[1].outcome, JobOutcome::Failed));
    }

    #[test]
    fn test_take_finished_jobs() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job-a", None);
        renderer.finish_job_success("job-a", Duration::from_millis(100));

        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        // After take, should be empty
        let jobs2 = renderer.take_finished_jobs();
        assert!(jobs2.is_empty());
    }

    #[test]
    fn test_print_header() {
        let config = HookOutputConfig::default();
        let renderer = HookProgressRenderer::new_hidden(&config);
        // Just verify it doesn't panic
        renderer.print_header("post-clone");
    }

    #[test]
    fn test_print_summary() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job-a", None);
        renderer.finish_job_success("job-a", Duration::from_millis(150));
        // Just verify it doesn't panic
        renderer.print_summary(Duration::from_secs(1));
    }

    #[test]
    fn test_format_header_lines_plain() {
        let lines = formatting::format_header_lines("post-create", false);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with('\u{250c}'));
        assert!(lines[1].contains("daft hooks"));
        assert!(lines[1].contains("post-create"));
        assert!(lines[2].starts_with('\u{2514}'));
    }

    #[test]
    fn test_format_summary_lines_empty() {
        let lines = formatting::format_summary_lines(&[], Duration::from_secs(1), false);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_format_summary_lines_with_jobs() {
        let jobs = vec![
            JobResultEntry {
                name: "job-a".to_string(),
                outcome: JobOutcome::Success,
                duration: Duration::from_millis(150),
            },
            JobResultEntry {
                name: "job-b".to_string(),
                outcome: JobOutcome::Failed,
                duration: Duration::from_secs(2),
            },
        ];
        let lines = formatting::format_summary_lines(&jobs, Duration::from_secs(3), false);
        // 2 blank + separator + summary + 2 jobs = 6
        assert_eq!(lines.len(), 6);
        assert!(lines[3].contains("summary:"));
        assert!(lines[4].contains("job-a"));
        assert!(lines[5].contains("job-b"));
    }

    #[test]
    fn active_job_has_trailing_spacer_for_vertical_separation() {
        // Parallel job blocks need a blank spacer line at the bottom so they
        // render with breathing room between them while active. The trailer
        // must exist immediately after start_job (before any output) and must
        // be cleared when the job finishes.
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job", None);
        assert!(
            renderer.has_trailer("job"),
            "active jobs must have a trailing spacer bar"
        );
        renderer.finish_job_success("job", Duration::from_secs(1));
        assert!(
            !renderer.has_trailer("job"),
            "trailer must be cleared when job finishes"
        );
    }

    #[test]
    fn test_dynamic_window_starts_empty() {
        // Before any output, no tail bars should be allocated
        let config = HookOutputConfig {
            tail_lines: 6,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job", None);
        // No output sent — tail line count must be 0
        assert_eq!(renderer.get_tail_line_count("job"), 0);
        renderer.finish_job_success("job", Duration::from_secs(1));
    }

    #[test]
    fn test_dynamic_window_grows_with_output() {
        // Each output line adds one tail bar until max is reached
        let config = HookOutputConfig {
            tail_lines: 6,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job", None);

        for i in 1..=6 {
            renderer.update_job_output("job", &format!("line {i}"));
            assert_eq!(
                renderer.get_tail_line_count("job"),
                i,
                "expected {i} tail bars after {i} output lines"
            );
        }

        renderer.finish_job_success("job", Duration::from_secs(1));
    }

    #[test]
    fn test_dynamic_window_caps_at_max() {
        // After max lines, no new tail bars are added — window rolls instead
        let config = HookOutputConfig {
            tail_lines: 3,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job", None);

        for i in 0..10 {
            renderer.update_job_output("job", &format!("line {i}"));
        }

        // Tail bar count must not exceed config max
        assert_eq!(renderer.get_tail_line_count("job"), 3);
        // But buffer holds all lines
        assert_eq!(renderer.get_buffered_output("job").len(), 10);

        renderer.finish_job_success("job", Duration::from_secs(1));
    }

    #[test]
    fn test_dynamic_window_zero_output_no_separator() {
        // A job with no output should not create a separator or any tail bars
        let config = HookOutputConfig {
            tail_lines: 6,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("silent-job", None);
        // finish without any output
        renderer.finish_job_success("silent-job", Duration::from_secs(1));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn compact_finalization_records_success_without_panicking() {
        let config = HookOutputConfig {
            compact_finalization: true,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("master", None);
        renderer.update_job_output("master", "some build output");
        renderer.finish_job_success("master", Duration::from_millis(1800));

        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "master");
        assert!(matches!(jobs[0].outcome, JobOutcome::Success));
    }

    #[test]
    fn compact_finalization_records_failure_without_panicking() {
        let config = HookOutputConfig {
            compact_finalization: true,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("feat/dirty", None);
        renderer.update_job_output("feat/dirty", "panicked!");
        renderer.finish_job_failure("feat/dirty", Duration::from_millis(1200));

        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }

    #[test]
    fn compact_finalization_records_cancelled_without_panicking() {
        let config = HookOutputConfig {
            compact_finalization: true,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job_with_description("cancelled-job", None, Some("sleep 10"));
        renderer.finish_job_cancelled("cancelled-job", Duration::from_secs(2));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }

    #[test]
    fn timer_promotes_on_wall_clock_even_without_output() {
        // Regression for daft exec spinners that never showed an elapsed
        // timer when the job was long-running but silent (e.g. cargo
        // compiling a single crate for tens of seconds with no new stdout).
        // The promotion must fire on a wall-clock deadline driven from
        // `start_job`, not from `update_job_output`.
        let config = HookOutputConfig {
            timer_delay_secs: 0,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("silent-job", None);

        // Promotion is performed by a detached thread; poll briefly rather
        // than relying on a timing-fragile fixed sleep.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !renderer.timer_promoted("silent-job") && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            renderer.timer_promoted("silent-job"),
            "spinner timer must be promoted on a wall-clock deadline even when the job emits no output"
        );

        renderer.finish_job_success("silent-job", Duration::from_secs(1));
    }

    #[test]
    fn plain_compact_finalization_cancelled_records_outcome() {
        let mut renderer = PlainHookRenderer::new();
        renderer.set_compact_finalization(true);
        renderer.start_job_with_description("cancelled-job", None, Some("sleep 10"));
        renderer.finish_job_cancelled("cancelled-job", Duration::from_secs(2));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }
}
