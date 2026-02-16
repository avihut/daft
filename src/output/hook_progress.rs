//! Hook progress renderer using indicatif for spinners and rolling output.

use crate::settings::HookOutputConfig;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::time::{Duration, Instant};

struct JobState {
    spinner: ProgressBar,
    tail_lines: Vec<ProgressBar>,
    output_buffer: Vec<String>,
    start_time: Instant,
}

pub struct HookProgressRenderer {
    mp: MultiProgress,
    jobs: HashMap<String, JobState>,
    config: HookOutputConfig,
    spinner_style: ProgressStyle,
    spinner_style_with_timer: ProgressStyle,
    tail_style: ProgressStyle,
}

impl HookProgressRenderer {
    pub fn new(config: &HookOutputConfig) -> Self {
        Self::create(config, MultiProgress::new())
    }

    #[cfg(test)]
    pub fn new_hidden(config: &HookOutputConfig) -> Self {
        Self::create(
            config,
            MultiProgress::with_draw_target(indicatif::ProgressDrawTarget::hidden()),
        )
    }

    fn create(config: &HookOutputConfig, mp: MultiProgress) -> Self {
        let spinner_style = ProgressStyle::with_template("  {spinner} {msg}")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

        let spinner_style_with_timer =
            ProgressStyle::with_template("  {spinner} {msg} [{elapsed_precise}]")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

        let tail_style = ProgressStyle::with_template("  │   {msg}").unwrap();

        Self {
            mp,
            jobs: HashMap::new(),
            config: config.clone(),
            spinner_style,
            spinner_style_with_timer,
            tail_style,
        }
    }

    pub fn start_job(&mut self, name: &str) {
        let spinner = self.mp.add(ProgressBar::new_spinner());
        spinner.set_style(self.spinner_style.clone());
        spinner.set_message(name.to_string());
        spinner.enable_steady_tick(Duration::from_millis(80));

        let tail_lines = if self.config.quiet || self.config.tail_lines == 0 {
            Vec::new()
        } else {
            (0..self.config.tail_lines)
                .map(|_| {
                    let pb = self.mp.insert_after(&spinner, ProgressBar::new_spinner());
                    pb.set_style(self.tail_style.clone());
                    pb.set_message("");
                    pb
                })
                .collect()
        };

        self.jobs.insert(
            name.to_string(),
            JobState {
                spinner,
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

        for pb in &state.tail_lines {
            pb.finish_and_clear();
        }

        if !state.output_buffer.is_empty() && !self.config.quiet {
            for line in &state.output_buffer {
                self.mp.println(format!("  │   {line}")).ok();
            }
        }

        let duration_str = format_duration(duration);
        let marker = if success { "✓" } else { "✗" };

        state
            .spinner
            .set_style(ProgressStyle::with_template("  {msg}").unwrap());
        state
            .spinner
            .finish_with_message(format!("{marker} {name} ({duration_str})"));
    }

    #[cfg(test)]
    pub fn get_buffered_output(&self, name: &str) -> &[String] {
        self.jobs
            .get(name)
            .map(|s| s.output_buffer.as_slice())
            .unwrap_or(&[])
    }

    pub fn println(&self, msg: &str) {
        self.mp.println(msg).ok();
    }
}

fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let mins = secs as u64 / 60;
        let remaining = secs as u64 % 60;
        format!("{mins}m {remaining}s")
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
        renderer.finish_job_success("test-job", std::time::Duration::from_secs(2));
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

        renderer.finish_job_success("test-job", std::time::Duration::from_secs(1));
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
        renderer.finish_job_success("test-job", std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_finish_job_failure() {
        let config = HookOutputConfig::default();
        let mut renderer = HookProgressRenderer::new_hidden(&config);
        renderer.start_job("failing-job");
        renderer.update_job_output("failing-job", "error output");
        renderer.finish_job_failure("failing-job", std::time::Duration::from_secs(3));
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(
            format_duration(std::time::Duration::from_secs_f64(2.3)),
            "2.3s"
        );
        assert_eq!(format_duration(std::time::Duration::from_secs(65)), "1m 5s");
        assert_eq!(
            format_duration(std::time::Duration::from_secs_f64(0.5)),
            "0.5s"
        );
    }
}
