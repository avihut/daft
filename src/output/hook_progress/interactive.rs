//! Rich (indicatif) renderer for interactive terminals.

use super::formatting::{BLUE, DARK_GREY, ITALIC, ORANGE};
use super::{JobOutcome, JobResultEntry};
use crate::settings::HookOutputConfig;
use crate::styles;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct JobState {
    spinner: ProgressBar,
    separator: Option<ProgressBar>,
    tail_lines: Vec<ProgressBar>,
    trailer: Option<ProgressBar>,
    output_buffer: Vec<String>,
    command_preview: Option<String>,
    /// Set to true once the spinner template has been swapped to include
    /// the elapsed-time suffix. Driven by a per-job background thread so
    /// the swap fires on a wall-clock deadline regardless of whether the
    /// job has emitted any output. Only consumed by tests; the production
    /// path observes the swap via the spinner re-rendering.
    #[cfg_attr(not(test), allow(dead_code))]
    timer_promoted: Arc<AtomicBool>,
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
    trailer_style: ProgressStyle,
    name_column_width: usize,
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
            "{pipe_str}  {{spinner}} {{msg}}"
        ))
        .unwrap()
        .tick_chars(
            "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
        );

        let spinner_style_with_timer = ProgressStyle::with_template(&format!(
            "{pipe_str}  {{spinner}} {{msg}} [{{elapsed_precise}}]"
        ))
        .unwrap()
        .tick_chars(
            "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
        );

        let tail_style = ProgressStyle::with_template(&format!("{pipe_str}  {{msg}}")).unwrap();

        // Single space (not empty) so indicatif's line-count accounting stays
        // aligned with actual terminal lines — an empty template can desync
        // the internal "drawn lines" counter and leave stale bars visible
        // when other bars are finish-and-cleared (notably during Ctrl-C).
        let trailer_style = ProgressStyle::with_template(" ").unwrap();

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
            trailer_style,
            name_column_width: super::formatting::DEFAULT_NAME_COLUMN_WIDTH,
        }
    }

    /// Override the branch-name column width used in compact finalization rows.
    /// Default is `DEFAULT_NAME_COLUMN_WIDTH` (matches `list_renderer::render_outcome`).
    pub fn set_name_column_width(&mut self, width: usize) {
        self.name_column_width = width;
    }

    pub fn print_header(&self, hook_name: &str) {
        for line in super::formatting::format_header_lines(hook_name, self.use_color) {
            self.mp.println(line).ok();
        }
    }

    pub fn start_job(&mut self, name: &str, command_preview: Option<&str>) {
        self.start_job_with_description(name, None, command_preview);
    }

    pub fn start_job_with_description(
        &mut self,
        name: &str,
        description: Option<&str>,
        command_preview: Option<&str>,
    ) {
        let spinner = self.mp.add(ProgressBar::new_spinner());
        spinner.set_style(self.spinner_style.clone());

        let display_name = match command_preview {
            Some(cmd) if self.use_color => format!(
                "{ORANGE}{name}{}  {arrow} {cmd}",
                styles::RESET,
                arrow = self.arrow_str,
            ),
            Some(cmd) => format!("{name}  \u{276f} {cmd}"),
            None if self.use_color => format!(
                "{ORANGE}{name}{}  {arrow}",
                styles::RESET,
                arrow = self.arrow_str,
            ),
            None => format!("{name}  \u{276f}"),
        };
        spinner.set_message(display_name);
        spinner.enable_steady_tick(Duration::from_millis(80));

        // Show description below the spinner if provided
        let mut last_bar = spinner.clone();
        if let Some(desc) = description {
            let desc_bar = self.mp.insert_after(&last_bar, ProgressBar::new_spinner());
            let desc_style =
                ProgressStyle::with_template(&format!("{}  {{msg}}", self.pipe_str)).unwrap();
            desc_bar.set_style(desc_style);
            let desc_msg = if self.use_color {
                format!("{DARK_GREY}{desc}{}", styles::RESET)
            } else {
                desc.to_string()
            };
            desc_bar.set_message(desc_msg);
            last_bar = desc_bar;
        }

        // Trailer is a blank spacer bar that sits at the bottom of this job's
        // block so parallel jobs running concurrently render with visual breathing
        // room between them. It's inserted now (before any tails exist) so later
        // insert_after(separator/tail) calls place tails between the separator
        // and this trailer.
        let trailer = self.mp.insert_after(&last_bar, ProgressBar::new_spinner());
        trailer.set_style(self.trailer_style.clone());
        trailer.set_message(String::new());

        // Spawn a one-shot promoter that swaps the spinner template to include
        // the elapsed-time suffix once the configured delay elapses. This must
        // run on a wall-clock deadline rather than being gated on output
        // arrival, because long-running silent jobs (e.g. `cargo build` stuck
        // on `Compiling …` for tens of seconds) would otherwise never get a
        // visible timer.
        let timer_promoted = Arc::new(AtomicBool::new(false));
        let delay = Duration::from_secs(u64::from(self.config.timer_delay_secs));
        let promoted_for_thread = Arc::clone(&timer_promoted);
        let spinner_for_thread = spinner.clone();
        let timer_style = self.spinner_style_with_timer.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            spinner_for_thread.set_style(timer_style);
            promoted_for_thread.store(true, Ordering::SeqCst);
        });

        // Separator and tail bars are created lazily in update_job_output as output arrives.
        self.jobs.insert(
            name.to_string(),
            JobState {
                spinner,
                separator: None,
                tail_lines: Vec::new(),
                trailer: Some(trailer),
                output_buffer: Vec::new(),
                command_preview: command_preview.map(str::to_string),
                timer_promoted,
            },
        );
    }

    pub fn update_job_output(&mut self, name: &str, line: &str) {
        // Phase 1: buffer line, determine growth needs. Clone ProgressBar
        // anchors before releasing the jobs borrow so Phase 2/3 can call
        // self.mp without a conflicting borrow. ProgressBar is Arc-based;
        // cloning is cheap.
        // The spinner-style swap to include `{elapsed_precise}` is handled
        // by the per-job promoter thread spawned in
        // `start_job_with_description` — driving it from this function would
        // miss long-running silent jobs that produce no further output.
        let (needs_sep, needs_tail, spinner_clone, last_anchor_clone) = {
            let Some(state) = self.jobs.get_mut(name) else {
                return;
            };

            state.output_buffer.push(line.to_string());

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

    pub fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        let Some(state) = self.jobs.remove(name) else {
            return;
        };

        self.remove_job_bars(&state);

        // Non-compact branch intentionally emits nothing: cancellation is only
        // reachable from exec paths, which always enable compact_finalization.
        if self.config.compact_finalization {
            let preview = state.command_preview.as_deref();
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    preview,
                    super::formatting::RowState::Cancelled { duration },
                    self.name_column_width,
                    self.use_color,
                ))
                .ok();
        }

        // JobOutcome has no Cancelled variant; record as Failed so callers
        // that inspect finished_jobs treat a cancelled step as non-success.
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Failed,
            duration,
        });
    }

    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        use super::formatting::YELLOW;

        let stored_preview = if let Some(state) = self.jobs.remove(name) {
            self.remove_job_bars(&state);
            state.command_preview
        } else {
            None
        };

        if self.config.compact_finalization {
            let preview = command_preview.or(stored_preview.as_deref());
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    preview,
                    super::formatting::RowState::Skipped,
                    self.name_column_width,
                    self.use_color,
                ))
                .ok();
        } else {
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
        }

        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Skipped {
                reason: reason.to_string(),
                show_duration,
            },
            duration,
        });
    }

    /// Remove the job's bars from MultiProgress instead of using
    /// `finish_and_clear`. The latter transitions the bar to `DoneHidden`
    /// and interacts with indicatif's zombie-line accounting: on drop,
    /// the bar's `mark_zombie` can feed non-zero line counts into
    /// `LineAdjust::Keep`, leaving the last-drawn spinner line stuck in
    /// scrollback above subsequent `mp.println` output. `mp.remove`
    /// hides the bar's draw target and unlinks it from the ordering,
    /// so the next `mp.println` does an atomic redraw that cleanly
    /// clears the old bar lines.
    fn remove_job_bars(&self, state: &JobState) {
        if let Some(ref sep) = state.separator {
            self.mp.remove(sep);
        }
        for pb in &state.tail_lines {
            self.mp.remove(pb);
        }
        if let Some(ref trailer) = state.trailer {
            self.mp.remove(trailer);
        }
        self.mp.remove(&state.spinner);
    }

    fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
        let Some(state) = self.jobs.remove(name) else {
            return;
        };

        self.remove_job_bars(&state);

        if self.config.compact_finalization {
            let preview = state.command_preview.as_deref();
            let row_state = if success {
                super::formatting::RowState::Success { duration }
            } else {
                super::formatting::RowState::Failure { duration }
            };
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    preview,
                    row_state,
                    self.name_column_width,
                    self.use_color,
                ))
                .ok();
        } else {
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
        }

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

    /// Add a pre-built result entry (e.g., for background jobs).
    pub fn push_finished_job(&mut self, entry: JobResultEntry) {
        self.finished_jobs.push(entry);
    }

    /// Show a background job dispatch in the live progress area.
    ///
    /// Uses `mp.println()` for permanent output (same as `finish_job`),
    /// so lines survive MultiProgress redraws and appear reliably.
    pub fn show_background_job(&self, name: &str, description: Option<&str>) {
        let cyan = "\x1b[38;5;80m";

        let blue_pipe = if self.use_color {
            format!("{BLUE}\u{2503}{}", styles::RESET)
        } else {
            "\u{2503}".to_string()
        };

        let name_line = if self.use_color {
            format!(
                "{blue_pipe}  {BLUE}{name}{} {cyan}(background){}",
                styles::RESET,
                styles::RESET
            )
        } else {
            format!("{blue_pipe}  {name} (background)")
        };
        self.mp.println(name_line).ok();

        if let Some(desc) = description {
            let desc_line = if self.use_color {
                format!("{blue_pipe}  {DARK_GREY}{desc}{}", styles::RESET)
            } else {
                format!("{blue_pipe}  {desc}")
            };
            self.mp.println(desc_line).ok();
        }

        self.mp.println(String::new()).ok();
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

    #[cfg(test)]
    pub fn has_trailer(&self, name: &str) -> bool {
        self.jobs
            .get(name)
            .map(|s| s.trailer.is_some())
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub fn timer_promoted(&self, name: &str) -> bool {
        self.jobs
            .get(name)
            .map(|s| s.timer_promoted.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    pub fn println(&self, msg: &str) {
        self.mp.println(msg).ok();
    }
}
