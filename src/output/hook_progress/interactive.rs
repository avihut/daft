//! Rich (indicatif) renderer for interactive terminals.

use super::formatting::{DARK_GREY, ITALIC, ORANGE};
use super::{JobOutcome, JobResultEntry};
use crate::settings::HookOutputConfig;
use crate::styles;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::time::{Duration, Instant};

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
        for line in super::formatting::format_header_lines(hook_name, self.use_color) {
            self.mp.println(line).ok();
        }
    }

    pub fn start_job(&mut self, name: &str) {
        self.start_job_with_description(name, None);
    }

    pub fn start_job_with_description(&mut self, name: &str, description: Option<&str>) {
        let spinner = self.mp.add(ProgressBar::new_spinner());
        spinner.set_style(self.spinner_style.clone());

        let display_name = if self.use_color {
            format!("{ORANGE}{name}{}", styles::RESET)
        } else {
            name.to_string()
        };
        spinner.set_message(display_name);
        spinner.enable_steady_tick(Duration::from_millis(80));

        // Show description below the spinner if provided
        if let Some(desc) = description {
            let desc_bar = self.mp.insert_after(&spinner, ProgressBar::new_spinner());
            let desc_style =
                ProgressStyle::with_template(&format!("{}  {{msg}}", self.pipe_str)).unwrap();
            desc_bar.set_style(desc_style);
            let desc_msg = if self.use_color {
                format!("{DARK_GREY}{desc}{}", styles::RESET)
            } else {
                desc.to_string()
            };
            desc_bar.set_message(desc_msg);
        }

        // Separator and tail bars are created lazily in update_job_output as output arrives.
        self.jobs.insert(
            name.to_string(),
            JobState {
                spinner,
                separator: None,
                tail_lines: Vec::new(),
                output_buffer: Vec::new(),
                start_time: Instant::now(),
            },
        );
    }

    pub fn update_job_output(&mut self, name: &str, line: &str) {
        // Phase 1: buffer line, update timer, determine growth needs.
        // Clone ProgressBar anchors before releasing the jobs borrow so
        // Phase 2/3 can call self.mp without a conflicting borrow.
        // ProgressBar is Arc-based; cloning is cheap.
        let (needs_sep, needs_tail, spinner_clone, last_anchor_clone) = {
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

            // Nothing to display when quiet or tail_lines == 0.
            if self.config.quiet || self.config.tail_lines == 0 {
                return;
            }

            let max = self.config.tail_lines as usize;
            let needs_new_bar = state.tail_lines.len() < max;
            let needs_sep = needs_new_bar && state.separator.is_none();

            let spinner_clone = state.spinner.clone();
            let last_anchor_clone = state
                .tail_lines
                .last()
                .or(state.separator.as_ref())
                .unwrap_or(&state.spinner)
                .clone();

            (needs_sep, needs_new_bar, spinner_clone, last_anchor_clone)
        };

        // Phase 2: lazily create the separator (inserted after the spinner).
        // Only happens on the very first output line of a job.
        let new_sep = if needs_sep {
            let sep = self
                .mp
                .insert_after(&spinner_clone, ProgressBar::new_spinner());
            let sep_style = ProgressStyle::with_template(&self.pipe_str).unwrap();
            sep.set_style(sep_style);
            sep.set_message(String::new());
            Some(sep)
        } else {
            None
        };

        // Phase 3: lazily create one new tail bar per output line until capped.
        // Insert after the separator if it was just created, otherwise after the last anchor.
        let new_tail = if needs_tail {
            let anchor = new_sep.as_ref().unwrap_or(&last_anchor_clone);
            let pb = self.mp.insert_after(anchor, ProgressBar::new_spinner());
            pb.set_style(self.tail_style.clone());
            pb.set_message(String::new());
            Some(pb)
        } else {
            None
        };

        // Phase 4: attach new bars to state, then update the rolling display.
        let Some(state) = self.jobs.get_mut(name) else {
            return;
        };

        if let Some(sep) = new_sep {
            state.separator = Some(sep);
        }
        if let Some(pb) = new_tail {
            state.tail_lines.push(pb);
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

    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
    ) {
        use super::formatting::YELLOW;

        // Remove job state and clear its bars
        if let Some(state) = self.jobs.remove(name) {
            if let Some(ref sep) = state.separator {
                sep.finish_and_clear();
            }
            for pb in &state.tail_lines {
                pb.finish_and_clear();
            }
            state.spinner.finish_and_clear();
        }

        // Always print skip info as a single inline line (no blank line after)
        let msg = if self.use_color {
            format!(
                "{}  {ORANGE}{name}{} {DARK_GREY}(skip){} {YELLOW}{reason}{}",
                self.pipe_str,
                styles::RESET,
                styles::RESET,
                styles::RESET
            )
        } else {
            format!("{}  {name} (skip) {reason}", self.pipe_str)
        };
        self.mp.println(msg).ok();

        // Skipped jobs are added to finished_jobs for the summary
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Skipped {
                reason: reason.to_string(),
                show_duration,
            },
            duration,
        });
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
            outcome: if success {
                JobOutcome::Success
            } else {
                JobOutcome::Failed
            },
            duration,
        });
    }

    pub fn print_summary(&self, total_duration: Duration) {
        for line in super::formatting::format_summary_lines(
            &self.finished_jobs,
            total_duration,
            self.use_color,
        ) {
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
