//! Plain text renderer for non-TTY environments (CI, pipes).
//!
//! Prints progress messages as simple lines to stderr without spinners
//! or ANSI escape sequences.

use super::{JobOutcome, JobResultEntry};
use std::time::Duration;

#[derive(Default)]
pub struct PlainHookRenderer {
    output_lines: Vec<String>,
    finished_jobs: Vec<JobResultEntry>,
    jobs_with_output: std::collections::HashSet<String>,
    verbose: bool,
    compact_finalization: bool,
    name_column_width: usize,
    previews: std::collections::HashMap<String, String>,
}

impl PlainHookRenderer {
    pub fn new() -> Self {
        Self {
            name_column_width: super::formatting::DEFAULT_NAME_COLUMN_WIDTH,
            ..Self::default()
        }
    }

    pub fn with_verbose(verbose: bool) -> Self {
        Self {
            verbose,
            name_column_width: super::formatting::DEFAULT_NAME_COLUMN_WIDTH,
            ..Self::default()
        }
    }

    pub fn set_name_column_width(&mut self, width: usize) {
        self.name_column_width = width;
    }

    /// Toggle the compact-finalization branch used by `daft exec` and friends.
    ///
    /// When enabled, `finish_job_success` / `finish_job_failure` emit a single
    /// compact row (e.g. `  ✓  master  (1.8s)`) instead of the multi-line
    /// heading + output dump. The per-job `JobResultEntry` is still recorded
    /// in both modes so callers relying on `take_finished_jobs()` keep working.
    pub fn set_compact_finalization(&mut self, on: bool) {
        self.compact_finalization = on;
    }

    pub fn print_header(&self, hook_name: &str, target: Option<&str>) {
        for line in super::formatting::format_header_lines(hook_name, target, false) {
            eprintln!("{line}");
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
        let msg = format!("\u{2503}  {name} \u{276f}");
        eprintln!("{msg}");
        self.output_lines.push(msg);
        if let Some(desc) = description {
            let desc_msg = format!("\u{2503}    {desc}");
            eprintln!("{desc_msg}");
            self.output_lines.push(desc_msg);
        }
        if self.verbose
            && let Some(cmd) = command_preview
        {
            let cmd_msg = format!("\u{2503}    {cmd}");
            eprintln!("{cmd_msg}");
            self.output_lines.push(cmd_msg);
        }
        if let Some(cmd) = command_preview {
            self.previews.insert(name.to_string(), cmd.to_string());
        }
    }

    pub fn update_job_output(&mut self, name: &str, line: &str) {
        self.jobs_with_output.insert(name.to_string());
        eprintln!("\u{2503}  {line}");
        self.output_lines.push(line.to_string());
    }

    fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
        if self.compact_finalization {
            let preview = self.previews.remove(name);
            let state = if success {
                super::formatting::RowState::Success { duration }
            } else {
                super::formatting::RowState::Failure { duration }
            };
            eprintln!(
                "{}",
                super::formatting::format_compact_row(
                    name,
                    preview.as_deref(),
                    state,
                    self.name_column_width,
                    false,
                )
            );
        } else if !self.jobs_with_output.contains(name) {
            eprintln!("\u{2503}  No output");
        }
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

    pub fn finish_job_success(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, true, duration);
    }

    pub fn finish_job_failure(&mut self, name: &str, duration: Duration) {
        self.finish_job(name, false, duration);
    }

    pub fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        // Non-compact branch intentionally emits nothing: cancellation is only
        // reachable from exec paths, which always enable compact_finalization.
        if self.compact_finalization {
            let preview = self.previews.remove(name);
            eprintln!(
                "{}",
                super::formatting::format_compact_row(
                    name,
                    preview.as_deref(),
                    super::formatting::RowState::Cancelled { duration },
                    self.name_column_width,
                    false,
                )
            );
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
        let stored = self.previews.remove(name);
        let preview = command_preview.or(stored.as_deref());
        if self.compact_finalization {
            eprintln!(
                "{}",
                super::formatting::format_compact_row(
                    name,
                    preview,
                    super::formatting::RowState::Skipped,
                    self.name_column_width,
                    false,
                )
            );
        } else {
            eprintln!("\u{2503}  {name} (skip) {reason}");
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

    pub fn print_summary(&self, total_duration: Duration) {
        for line in
            super::formatting::format_summary_lines(&self.finished_jobs, total_duration, false)
        {
            eprintln!("{line}");
        }
    }

    /// Add a pre-built result entry (e.g., for background jobs).
    pub fn push_finished_job(&mut self, entry: JobResultEntry) {
        self.finished_jobs.push(entry);
    }

    /// Show a background job dispatch in plain output.
    pub fn show_background_job(&self, name: &str, description: Option<&str>) {
        let desc = description.map(|d| format!(" -- {d}")).unwrap_or_default();
        eprintln!("  {name} (background){desc}");
    }

    pub fn take_finished_jobs(&mut self) -> Vec<JobResultEntry> {
        std::mem::take(&mut self.finished_jobs)
    }

    pub fn println(&self, msg: &str) {
        eprintln!("{msg}");
    }
}
