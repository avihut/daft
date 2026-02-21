//! Hook progress renderer using indicatif for spinners and rolling output.

mod formatting;
mod interactive;
mod plain;

pub use interactive::HookProgressRenderer;
pub use plain::PlainHookRenderer;

use crate::settings::HookOutputConfig;
use crate::styles;
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
            HookRenderer::Plain(PlainHookRenderer::new())
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
            HookRenderer::Progress(r) => r.start_job(name),
            HookRenderer::Plain(r) => r.start_job(name),
        }
    }

    pub fn start_job_with_description(&mut self, name: &str, description: Option<&str>) {
        match self {
            HookRenderer::Progress(r) => r.start_job_with_description(name, description),
            HookRenderer::Plain(r) => r.start_job_with_description(name, description),
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
    ) {
        match self {
            HookRenderer::Progress(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration);
            }
            HookRenderer::Plain(r) => r.finish_job_skipped(name, reason, duration, show_duration),
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

// ─────────────────────────────────────────────────────────────────────────
// Standalone functions (for YAML executor path)
// ─────────────────────────────────────────────────────────────────────────

/// Print the hook execution header to stderr.
///
/// Displays a dark-grey framed box with the hook name, version, and hook type.
/// Suppressed when `DAFT_TESTING` env var is set (keeps test output clean).
pub fn print_hook_header(hook_name: &str) {
    if formatting::output_suppressed() {
        return;
    }
    for line in formatting::format_header_lines(hook_name, styles::colors_enabled_stderr()) {
        eprintln!("{line}");
    }
}

/// Print the summary section after all hook jobs have completed.
/// Suppressed when `DAFT_TESTING` env var is set (keeps test output clean).
pub fn print_hook_summary(job_results: &[JobResultEntry], total_duration: Duration) {
    if formatting::output_suppressed() {
        return;
    }
    for line in formatting::format_summary_lines(
        job_results,
        total_duration,
        styles::colors_enabled_stderr(),
    ) {
        eprintln!("{line}");
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
        renderer.start_job("test-job");
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
        renderer.start_job("test-job");

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
        renderer.start_job("test-job");
        renderer.update_job_output("test-job", "should not show");
        renderer.finish_job_success("test-job", Duration::from_secs(1));
    }

    #[test]
    fn test_finish_job_failure() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("failing-job");
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
        renderer.start_job("test-job");
        renderer.update_job_output("test-job", "line 1");
        renderer.update_job_output("test-job", "line 2");
        renderer.finish_job_success("test-job", Duration::from_secs(2));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn test_plain_renderer_failure() {
        let mut renderer = PlainHookRenderer::new();
        renderer.start_job("fail-job");
        renderer.finish_job_failure("fail-job", Duration::from_secs(3));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
    }

    #[test]
    fn test_summary_tracking() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job-a");
        renderer.finish_job_success("job-a", Duration::from_millis(150));
        renderer.start_job("job-b");
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
        renderer.start_job("job-a");
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
        renderer.start_job("job-a");
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
    fn test_dynamic_window_starts_empty() {
        // Before any output, no tail bars should be allocated
        let config = HookOutputConfig {
            tail_lines: 6,
            ..Default::default()
        };
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job");
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
        renderer.start_job("job");

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
        renderer.start_job("job");

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
        renderer.start_job("silent-job");
        // finish without any output
        renderer.finish_job_success("silent-job", Duration::from_secs(1));
        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
    }
}
