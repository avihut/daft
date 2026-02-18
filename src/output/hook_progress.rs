//! Hook progress renderer using indicatif for spinners and rolling output.

use crate::settings::HookOutputConfig;
use crate::styles;
use crate::VERSION;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// ANSI color codes for hook output (256-color palette)
const ORANGE: &str = "\x1b[38;5;208m";
const GREY: &str = "\x1b[38;5;245m";
const BRIGHT_WHITE: &str = "\x1b[97m";
const DARK_GREY: &str = "\x1b[38;5;240m";
const ITALIC: &str = "\x1b[3m";

/// Check if hook visual output should be suppressed (e.g. during tests).
///
/// Returns true when running unit tests (`cfg!(test)`) or when `DAFT_TESTING`
/// env var is set (for integration tests that invoke the binary as a subprocess).
fn output_suppressed() -> bool {
    cfg!(test) || std::env::var("DAFT_TESTING").is_ok()
}

/// Entry recording a completed job for the summary.
#[derive(Debug, Clone)]
pub struct JobResultEntry {
    pub name: String,
    pub success: bool,
    pub duration: Duration,
}

// ─────────────────────────────────────────────────────────────────────────
// Shared formatting helpers (single source of truth for header/summary)
// ─────────────────────────────────────────────────────────────────────────

/// Generate the hook header lines (dark-grey framed box).
fn format_header_lines(hook_name: &str, use_color: bool) -> Vec<String> {
    let content_width =
        " daft hooks v".len() + VERSION.len() + "  hook: ".len() + hook_name.len() + " ".len();
    let border_h = "\u{2500}".repeat(content_width);

    if use_color {
        vec![
            format!("{GREY}\u{250c}{border_h}\u{2510}{}", styles::RESET),
            format!(
                "{GREY}\u{2502} {ORANGE}daft hooks {GREY}v{VERSION}  hook: {}{BRIGHT_WHITE}{hook_name}{}{GREY} \u{2502}{}",
                styles::BOLD, styles::RESET, styles::RESET
            ),
            format!("{GREY}\u{2514}{border_h}\u{2518}{}", styles::RESET),
        ]
    } else {
        vec![
            format!("\u{250c}{border_h}\u{2510}"),
            format!("\u{2502} daft hooks v{VERSION}  hook: {hook_name} \u{2502}"),
            format!("\u{2514}{border_h}\u{2518}"),
        ]
    }
}

/// Generate the summary lines (separator + totals + per-job results).
fn format_summary_lines(
    jobs: &[JobResultEntry],
    total_duration: Duration,
    use_color: bool,
) -> Vec<String> {
    if jobs.is_empty() {
        return Vec::new();
    }

    let total_str = format_duration(total_duration);
    let mut lines = vec![String::new(), String::new()]; // two blank lines before separator

    if use_color {
        lines.push(format!("{GREY}{}{}", "\u{2500}".repeat(40), styles::RESET));
        lines.push(format!(
            "{ORANGE}summary: {GREY}(done in {total_str}){}",
            styles::RESET
        ));
        for job in jobs {
            let (marker, color) = if job.success {
                ("\u{2714}", styles::GREEN)
            } else {
                ("\u{2718}", styles::RED)
            };
            let dur = format_duration(job.duration);
            lines.push(format!(
                "{color}  {marker} {}{} {GREY}({dur}){}",
                job.name,
                styles::RESET,
                styles::RESET
            ));
        }
    } else {
        lines.push("\u{2500}".repeat(40));
        lines.push(format!("summary: (done in {total_str})"));
        for job in jobs {
            let marker = if job.success { "\u{2714}" } else { "\u{2718}" };
            let dur = format_duration(job.duration);
            lines.push(format!("  {marker} {} ({dur})", job.name));
        }
    }

    lines
}

// ─────────────────────────────────────────────────────────────────────────
// Rich (indicatif) renderer
// ─────────────────────────────────────────────────────────────────────────

struct JobState {
    spinner: ProgressBar,
    separator: Option<ProgressBar>,
    tail_lines: Vec<ProgressBar>,
    output_buffer: Vec<String>,
    start_time: Instant,
}

pub struct HookProgressRenderer {
    mp: MultiProgress,
    jobs: HashMap<String, JobState>,
    config: HookOutputConfig,
    finished_jobs: Vec<JobResultEntry>,
    use_color: bool,
    pipe_str: String,
    arrow_str: String,
    spinner_style: ProgressStyle,
    spinner_style_with_timer: ProgressStyle,
    tail_style: ProgressStyle,
}

impl HookProgressRenderer {
    pub fn new(config: &HookOutputConfig) -> Self {
        Self::create(
            config,
            MultiProgress::new(),
            styles::colors_enabled_stderr(),
        )
    }

    pub fn new_hidden(config: &HookOutputConfig) -> Self {
        Self::create(
            config,
            MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden()),
            false,
        )
    }

    fn create(config: &HookOutputConfig, mp: MultiProgress, use_color: bool) -> Self {
        let pipe_str = if use_color {
            format!("{ORANGE}\u{2503}{}", styles::RESET)
        } else {
            "\u{2503}".to_string()
        };

        let arrow = if use_color {
            format!("{ORANGE}\u{276f}{}", styles::RESET)
        } else {
            "\u{276f}".to_string()
        };

        let spinner_style = ProgressStyle::with_template(&format!(
            "{pipe_str}  {{spinner}} {{msg}} {arrow}"
        ))
        .unwrap()
        .tick_chars(
            "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
        );

        let spinner_style_with_timer = ProgressStyle::with_template(&format!(
            "{pipe_str}  {{spinner}} {{msg}} {arrow} [{{elapsed_precise}}]"
        ))
        .unwrap()
        .tick_chars(
            "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
        );

        let tail_style = ProgressStyle::with_template(&format!("{pipe_str}  {{msg}}")).unwrap();

        Self {
            mp,
            jobs: HashMap::new(),
            config: config.clone(),
            finished_jobs: Vec::new(),
            use_color,
            pipe_str,
            arrow_str: arrow,
            spinner_style,
            spinner_style_with_timer,
            tail_style,
        }
    }

    pub fn print_header(&self, hook_name: &str) {
        for line in format_header_lines(hook_name, self.use_color) {
            self.mp.println(line).ok();
        }
    }

    pub fn start_job(&mut self, name: &str) {
        let spinner = self.mp.add(ProgressBar::new_spinner());
        spinner.set_style(self.spinner_style.clone());

        let display_name = if self.use_color {
            format!("{ORANGE}{name}{}", styles::RESET)
        } else {
            name.to_string()
        };
        spinner.set_message(display_name);
        spinner.enable_steady_tick(Duration::from_millis(80));

        // Empty separator line between job header and output
        let separator = if !self.config.quiet && self.config.tail_lines > 0 {
            let sep = self.mp.insert_after(&spinner, ProgressBar::new_spinner());
            let sep_style = ProgressStyle::with_template(&self.pipe_str).unwrap();
            sep.set_style(sep_style);
            sep.set_message(String::new());
            Some(sep)
        } else {
            None
        };

        let anchor = separator.as_ref().unwrap_or(&spinner);

        let tail_lines = if self.config.quiet || self.config.tail_lines == 0 {
            Vec::new()
        } else {
            (0..self.config.tail_lines)
                .map(|_| {
                    let pb = self.mp.insert_after(anchor, ProgressBar::new_spinner());
                    pb.set_style(self.tail_style.clone());
                    pb.set_message(String::new());
                    pb
                })
                .collect()
        };

        self.jobs.insert(
            name.to_string(),
            JobState {
                spinner,
                separator,
                tail_lines,
                output_buffer: Vec::new(),
                start_time: Instant::now(),
            },
        );
    }

    pub fn update_job_output(&mut self, name: &str, line: &str) {
        let Some(state) = self.jobs.get_mut(name) else {
            return;
        };

        state.output_buffer.push(line.to_string());

        if state.start_time.elapsed()
            >= Duration::from_secs(u64::from(self.config.timer_delay_secs))
        {
            state
                .spinner
                .set_style(self.spinner_style_with_timer.clone());
        }

        if !state.tail_lines.is_empty() {
            let buf_len = state.output_buffer.len();
            let tail_count = state.tail_lines.len();
            let start = buf_len.saturating_sub(tail_count);

            for (i, tail_pb) in state.tail_lines.iter().enumerate() {
                let buf_idx = start + i;
                if buf_idx < buf_len {
                    tail_pb.set_message(state.output_buffer[buf_idx].clone());
                } else {
                    tail_pb.set_message(String::new());
                }
            }
        }
    }

    pub fn finish_job_success(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, true, duration);
    }

    pub fn finish_job_failure(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, false, duration);
    }

    fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
        let Some(state) = self.jobs.remove(name) else {
            return;
        };

        // Clear ALL bars from the draw area. Using finish_and_clear (not
        // finish_with_message) avoids "zombie" bars that would flush on
        // MultiProgress drop — potentially after the summary has already
        // been printed to stderr.
        if let Some(ref sep) = state.separator {
            sep.finish_and_clear();
        }
        for pb in &state.tail_lines {
            pb.finish_and_clear();
        }
        state.spinner.finish_and_clear();

        // Print heading as a permanent line. Because the spinner is already
        // cleared, mp.println() inserts this above remaining *active*
        // spinners only — i.e. after all previously finished jobs' output.
        let finished_name = if self.use_color {
            format!("{ORANGE}{name}{}", styles::RESET)
        } else {
            name.to_string()
        };
        self.mp
            .println(format!(
                "{}  {finished_name} {}",
                self.pipe_str, self.arrow_str
            ))
            .ok();

        // Print full output as permanent lines below the heading
        let has_output = !state.output_buffer.is_empty();
        if !self.config.quiet && has_output {
            for line in &state.output_buffer {
                self.mp.println(format!("{}  {line}", self.pipe_str)).ok();
            }
        }

        if !self.config.quiet && !has_output {
            let msg = if self.use_color {
                format!(
                    "{}  {DARK_GREY}{ITALIC}No output{}",
                    self.pipe_str,
                    styles::RESET
                )
            } else {
                format!("{}  No output", self.pipe_str)
            };
            self.mp.println(msg).ok();
        }

        // Empty line after each job's section
        self.mp.println(String::new()).ok();

        // Record for summary
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            success,
            duration,
        });
    }

    pub fn print_summary(&self, total_duration: Duration) {
        for line in format_summary_lines(&self.finished_jobs, total_duration, self.use_color) {
            self.mp.println(line).ok();
        }
    }

    /// Extract finished job results (for use in callers that need them).
    pub fn take_finished_jobs(&mut self) -> Vec<JobResultEntry> {
        std::mem::take(&mut self.finished_jobs)
    }

    #[cfg(test)]
    pub fn get_buffered_output(&self, name: &str) -> &[String] {
        self.jobs
            .get(name)
            .map(|s| s.output_buffer.as_slice())
            .unwrap_or(&[])
    }

    #[cfg(test)]
    pub fn get_tail_line_count(&self, name: &str) -> usize {
        self.jobs.get(name).map(|s| s.tail_lines.len()).unwrap_or(0)
    }

    pub fn println(&self, msg: &str) {
        self.mp.println(msg).ok();
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Plain text renderer (CI, pipes, non-TTY)
// ─────────────────────────────────────────────────────────────────────────

/// Plain text renderer for non-TTY environments (CI, pipes).
///
/// Prints progress messages as simple lines to stderr without spinners
/// or ANSI escape sequences.
#[derive(Default)]
pub struct PlainHookRenderer {
    output_lines: Vec<String>,
    finished_jobs: Vec<JobResultEntry>,
    jobs_with_output: std::collections::HashSet<String>,
}

impl PlainHookRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn print_header(&self, hook_name: &str) {
        for line in format_header_lines(hook_name, false) {
            eprintln!("{line}");
        }
    }

    pub fn start_job(&mut self, name: &str) {
        let msg = format!("\u{2503}  {name} \u{276f}");
        eprintln!("{msg}");
        self.output_lines.push(msg);
    }

    pub fn update_job_output(&mut self, name: &str, line: &str) {
        self.jobs_with_output.insert(name.to_string());
        eprintln!("\u{2503}  {line}");
        self.output_lines.push(line.to_string());
    }

    fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
        if !self.jobs_with_output.contains(name) {
            eprintln!("\u{2503}  No output");
        }
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            success,
            duration,
        });
    }

    pub fn finish_job_success(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, true, duration);
    }

    pub fn finish_job_failure(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, false, duration);
    }

    pub fn print_summary(&self, total_duration: Duration) {
        for line in format_summary_lines(&self.finished_jobs, total_duration, false) {
            eprintln!("{line}");
        }
    }

    pub fn take_finished_jobs(&mut self) -> Vec<JobResultEntry> {
        std::mem::take(&mut self.finished_jobs)
    }

    pub fn println(&self, msg: &str) {
        eprintln!("{msg}");
    }
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
        if output_suppressed() {
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
    if output_suppressed() {
        return;
    }
    for line in format_header_lines(hook_name, styles::colors_enabled_stderr()) {
        eprintln!("{line}");
    }
}

/// Print the summary section after all hook jobs have completed.
/// Suppressed when `DAFT_TESTING` env var is set (keeps test output clean).
pub fn print_hook_summary(job_results: &[JobResultEntry], total_duration: Duration) {
    if output_suppressed() {
        return;
    }
    for line in format_summary_lines(job_results, total_duration, styles::colors_enabled_stderr()) {
        eprintln!("{line}");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Duration formatting
// ─────────────────────────────────────────────────────────────────────────

/// Format a duration to the most appropriate scale.
///
/// - Under 1 second: milliseconds (e.g., "112ms")
/// - 1-60 seconds: seconds with one decimal (e.g., "2.3s")
/// - Over 60 seconds: minutes and seconds (e.g., "1m 5s")
fn format_duration(d: Duration) -> String {
    let millis = d.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        let secs = d.as_secs_f64();
        if secs < 60.0 {
            format!("{secs:.1}s")
        } else {
            let mins = d.as_secs() / 60;
            let remaining = d.as_secs() % 60;
            format!("{mins}m {remaining}s")
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
        renderer.start_job("test-job");
        renderer.finish_job_success("test-job", Duration::from_secs(2));
        assert_eq!(renderer.finished_jobs.len(), 1);
        assert!(renderer.finished_jobs[0].success);
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
        assert_eq!(renderer.finished_jobs.len(), 1);
        assert!(!renderer.finished_jobs[0].success);
    }

    #[test]
    fn test_format_duration_milliseconds() {
        assert_eq!(format_duration(Duration::from_millis(112)), "112ms");
        assert_eq!(format_duration(Duration::from_millis(98)), "98ms");
        assert_eq!(format_duration(Duration::from_millis(0)), "0ms");
        assert_eq!(format_duration(Duration::from_millis(999)), "999ms");
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(Duration::from_secs_f64(2.3)), "2.3s");
        assert_eq!(format_duration(Duration::from_secs_f64(1.0)), "1.0s");
        assert_eq!(format_duration(Duration::from_secs_f64(59.9)), "59.9s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(Duration::from_secs(65)), "1m 5s");
        assert_eq!(format_duration(Duration::from_secs(120)), "2m 0s");
    }

    #[test]
    fn test_plain_renderer_lifecycle() {
        let mut renderer = PlainHookRenderer::new();
        renderer.start_job("test-job");
        renderer.update_job_output("test-job", "line 1");
        renderer.update_job_output("test-job", "line 2");
        renderer.finish_job_success("test-job", Duration::from_secs(2));
        assert!(renderer.output_lines.iter().any(|l| l.contains("test-job")));
        assert!(renderer.output_lines.iter().any(|l| l.contains("line 1")));
        assert_eq!(renderer.finished_jobs.len(), 1);
    }

    #[test]
    fn test_plain_renderer_failure() {
        let mut renderer = PlainHookRenderer::new();
        renderer.start_job("fail-job");
        renderer.finish_job_failure("fail-job", Duration::from_secs(3));
        assert_eq!(renderer.finished_jobs.len(), 1);
        assert!(!renderer.finished_jobs[0].success);
    }

    #[test]
    fn test_summary_tracking() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job-a");
        renderer.finish_job_success("job-a", Duration::from_millis(150));
        renderer.start_job("job-b");
        renderer.finish_job_failure("job-b", Duration::from_secs(2));

        assert_eq!(renderer.finished_jobs.len(), 2);
        assert_eq!(renderer.finished_jobs[0].name, "job-a");
        assert!(renderer.finished_jobs[0].success);
        assert_eq!(renderer.finished_jobs[1].name, "job-b");
        assert!(!renderer.finished_jobs[1].success);
    }

    #[test]
    fn test_take_finished_jobs() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("job-a");
        renderer.finish_job_success("job-a", Duration::from_millis(100));

        let jobs = renderer.take_finished_jobs();
        assert_eq!(jobs.len(), 1);
        assert!(renderer.finished_jobs.is_empty());
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
        let lines = format_header_lines("post-create", false);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with('\u{250c}'));
        assert!(lines[1].contains("daft hooks"));
        assert!(lines[1].contains("post-create"));
        assert!(lines[2].starts_with('\u{2514}'));
    }

    #[test]
    fn test_format_summary_lines_empty() {
        let lines = format_summary_lines(&[], Duration::from_secs(1), false);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_format_summary_lines_with_jobs() {
        let jobs = vec![
            JobResultEntry {
                name: "job-a".to_string(),
                success: true,
                duration: Duration::from_millis(150),
            },
            JobResultEntry {
                name: "job-b".to_string(),
                success: false,
                duration: Duration::from_secs(2),
            },
        ];
        let lines = format_summary_lines(&jobs, Duration::from_secs(3), false);
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
        assert_eq!(renderer.finished_jobs.len(), 1);
    }
}
