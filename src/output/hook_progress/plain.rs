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
}

impl PlainHookRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn print_header(&self, hook_name: &str) {
        for line in super::formatting::format_header_lines(hook_name, false) {
            eprintln!("{line}");
        }
    }

    pub fn start_job(&mut self, name: &str) {
        self.start_job_with_description(name, None);
    }

    pub fn start_job_with_description(&mut self, name: &str, description: Option<&str>) {
        let msg = format!("\u{2503}  {name} \u{276f}");
        eprintln!("{msg}");
        self.output_lines.push(msg);
        if let Some(desc) = description {
            let desc_msg = format!("\u{2503}    {desc}");
            eprintln!("{desc_msg}");
            self.output_lines.push(desc_msg);
        }
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

    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
    ) {
        eprintln!("\u{2503}  {name} (skip) {reason}");
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

    pub fn take_finished_jobs(&mut self) -> Vec<JobResultEntry> {
        std::mem::take(&mut self.finished_jobs)
    }

    pub fn println(&self, msg: &str) {
        eprintln!("{msg}");
    }
}
