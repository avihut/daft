//! Default / verbose / very-verbose pretty reporter.
//!
//! Internal branching on `verbosity` keeps formatting differences local —
//! three top-level structs would be near-duplicate code. The verbosity
//! ladder:
//!
//! | level         | per-step lines | check icons (pass) | captured (pass) | captured (fail)        | expanded command |
//! |---------------|----------------|--------------------|-----------------|------------------------|------------------|
//! | `Default`     | yes            | no                 | no              | first 20 lines         | no               |
//! | `Verbose`     | yes            | yes                | yes (20 lines)  | first 20 lines         | yes              |
//! | `VeryVerbose` | yes            | yes                | yes (untrunc.)  | full, no line cap      | yes              |

use std::io::{self, Write};

use term_styles as styles;

use super::{Reporter, RunSummary, ScenarioStatus, StepReport, Verbosity};
use crate::manual_test::schema::{Scenario, Step};

const CAPTURE_LINE_CAP: usize = 20;

pub struct PrettyReporter {
    verbosity: Verbosity,
}

impl PrettyReporter {
    pub fn new(verbosity: Verbosity) -> Self {
        Self { verbosity }
    }

    fn show_expanded_command(&self) -> bool {
        matches!(self.verbosity, Verbosity::Verbose | Verbosity::VeryVerbose)
    }

    fn show_pass_check_icons(&self) -> bool {
        matches!(self.verbosity, Verbosity::Verbose | Verbosity::VeryVerbose)
    }

    fn show_pass_capture(&self) -> bool {
        matches!(self.verbosity, Verbosity::Verbose | Verbosity::VeryVerbose)
    }

    fn capture_cap(&self) -> Option<usize> {
        match self.verbosity {
            Verbosity::VeryVerbose => None,
            _ => Some(CAPTURE_LINE_CAP),
        }
    }
}

impl Reporter for PrettyReporter {
    fn scenario_header(&self, out: &mut dyn Write, scenario: &Scenario) -> io::Result<()> {
        // Leading blank separates this scenario block from the previous one
        // (cleanup note + footer attach tight to the scenario above; the
        // breathing room lives here, owned by the next scenario's header).
        writeln!(out)?;
        writeln!(out, "{}", styles::cyan(&scenario.name))?;
        if !scenario.source_path.as_os_str().is_empty() {
            writeln!(
                out,
                "{}",
                styles::dim(&format!("  at {}", scenario.source_path.display()))
            )?;
        }
        Ok(())
    }

    fn step_start(
        &self,
        out: &mut dyn Write,
        idx: usize,
        total: usize,
        step: &Step,
    ) -> io::Result<()> {
        write!(
            out,
            "{} {} ... ",
            styles::blue(&format!("[{}/{}]", idx + 1, total)),
            &step.name
        )
    }

    fn step_pass(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()> {
        let check_count = report.assertions.len();
        if check_count > 0 {
            writeln!(
                out,
                "{} {}",
                styles::green("ok"),
                styles::dim(&format!("({check_count} checks)"))
            )?;
        } else {
            writeln!(out, "{}", styles::green("ok"))?;
        }

        if self.show_expanded_command() {
            if let Some(cmd) = report.expanded_command {
                writeln!(out, "  {}", styles::cyan(&format!("$ {cmd}")))?;
            }
        }
        if self.show_pass_check_icons() {
            for a in report.assertions {
                writeln!(out, "  {} {}", styles::green("✓"), styles::dim(&a.label))?;
            }
        }
        if self.show_pass_capture() {
            write_captured(out, report, self.capture_cap())?;
        }
        Ok(())
    }

    fn step_fail(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()> {
        let fail_count = report.assertions.iter().filter(|a| !a.passed).count();
        writeln!(
            out,
            "{} {}",
            styles::red("FAIL"),
            styles::dim(&format!("({fail_count} failed)"))
        )?;
        if self.show_expanded_command() {
            if let Some(cmd) = report.expanded_command {
                writeln!(out, "  {}", styles::cyan(&format!("$ {cmd}")))?;
            }
        }
        for a in report.assertions.iter().filter(|a| !a.passed) {
            writeln!(out, "  {} {}", styles::red("x"), a.label)?;
            if let Some(detail) = &a.detail {
                for line in detail.lines() {
                    writeln!(out, "    {}", styles::dim(line))?;
                }
            }
        }
        write_captured(out, report, self.capture_cap())?;
        Ok(())
    }

    fn scenario_footer(
        &self,
        out: &mut dyn Write,
        scenario: &Scenario,
        status: ScenarioStatus,
    ) -> io::Result<()> {
        // No trailing blank: the cleanup_note (or next scenario_header)
        // attaches directly so the footer reads as part of its scenario block.
        match status {
            ScenarioStatus::Pass => writeln!(out, "{} {}", styles::green("✓"), &scenario.name),
            ScenarioStatus::Fail => writeln!(out, "{} {}", styles::red("✗"), &scenario.name),
        }
    }

    fn cleanup_note(&self, out: &mut dyn Write, msg: &str) -> io::Result<()> {
        writeln!(out, "{}", styles::dim(msg))
    }

    fn run_summary(&self, out: &mut dyn Write, summary: &RunSummary<'_>) -> io::Result<()> {
        write_run_summary(out, summary)
    }
}

/// Write the captured stdout/stderr block, honoring an optional line cap.
fn write_captured(
    out: &mut dyn Write,
    report: &StepReport<'_>,
    cap: Option<usize>,
) -> io::Result<()> {
    let combined = combine_captured(report.stdout, report.stderr);
    if combined.is_empty() {
        return Ok(());
    }
    writeln!(out, "  {}", styles::dim("--- captured output ---"))?;
    let mut total_lines = 0usize;
    let line_iter = combined.lines();
    let limit = cap.unwrap_or(usize::MAX);
    for line in line_iter.take(limit) {
        writeln!(out, "  {}", styles::dim(line))?;
        total_lines += 1;
    }
    if let Some(cap) = cap {
        let actual = combined.lines().count();
        if actual > cap {
            writeln!(
                out,
                "  {}",
                styles::dim(&format!(
                    "... {} more lines truncated (re-run with -vv for full output)",
                    actual - total_lines
                ))
            )?;
        }
    }
    Ok(())
}

fn combine_captured(stdout: Option<&str>, stderr: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(s) = stdout {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    if let Some(s) = stderr {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    parts.join("\n")
}

/// Format the end-of-run summary block.
///
/// Layout (matches the issue mockup):
///   <blank>
///   Failed scenarios:
///   <per-failure block>
///   <blank>
///   Scenarios:  X passed, Y failed   (Z total)
///   Steps:      X passed, Y failed   (Z total)
///   Duration:   MM:SS (parallel jobs: N)
///   <blank>
///   Reproduce:
///     mise run test:manual -- --ci <token>
///     ...
fn write_run_summary(out: &mut dyn Write, s: &RunSummary<'_>) -> io::Result<()> {
    writeln!(out)?;

    if !s.failed.is_empty() {
        writeln!(out, "{}", styles::red("Failed scenarios:"))?;
        writeln!(out)?;
        for f in &s.failed {
            writeln!(
                out,
                "  {} {}   {}",
                styles::red("✗"),
                f.name,
                styles::dim(&f.source.display().to_string())
            )?;
            if let Some(failing) = f.failing_step {
                writeln!(
                    out,
                    "      step {}/{}  {}",
                    failing.index + 1,
                    failing.total,
                    failing.step_name
                )?;
                for a in &failing.failed_assertions {
                    writeln!(out, "      {} {}", styles::red("✗"), a.label)?;
                    if let Some(detail) = &a.detail {
                        for line in detail.lines() {
                            writeln!(out, "        {line}")?;
                        }
                    }
                }
                if !failing.captured_output.is_empty() {
                    writeln!(out, "      {}", styles::dim("--- captured output ---"))?;
                    for line in failing.captured_output.lines() {
                        writeln!(out, "      {line}")?;
                    }
                }
            }
            writeln!(out)?;
        }
    }

    if !s.errors.is_empty() {
        writeln!(out, "{}", styles::red("Errors:"))?;
        for e in &s.errors {
            writeln!(out, "  {} {}: {}", styles::red("ERROR"), e.name, e.error)?;
        }
        writeln!(out)?;
    }

    writeln!(
        out,
        "Scenarios:  {} passed, {} failed   ({} total)",
        styles::green(&s.scenarios_passed.to_string()),
        render_failed_count(s.scenarios_failed),
        s.scenarios_total
    )?;
    writeln!(
        out,
        "Steps:      {} passed, {} failed   ({} total)",
        styles::green(&s.steps_passed.to_string()),
        render_failed_count(s.steps_failed),
        s.steps_total
    )?;
    let parallel_suffix = match s.parallel_jobs {
        Some(n) if n > 1 => format!(" (parallel jobs: {n})"),
        _ => String::new(),
    };
    writeln!(
        out,
        "Duration:   {}{parallel_suffix}",
        format_duration(s.duration)
    )?;

    if !s.failed.is_empty() {
        writeln!(out)?;
        writeln!(out, "Reproduce:")?;
        for f in &s.failed {
            writeln!(out, "  mise run test:manual -- --ci {}", f.reproduce_token)?;
        }
    }
    writeln!(out)?;

    Ok(())
}

fn render_failed_count(n: usize) -> String {
    if n > 0 {
        styles::red(&n.to_string())
    } else {
        "0".to_string()
    }
}

fn format_duration(d: std::time::Duration) -> String {
    let total = d.as_secs();
    let minutes = total / 60;
    let seconds = total % 60;
    format!("{minutes:02}:{seconds:02}")
}
