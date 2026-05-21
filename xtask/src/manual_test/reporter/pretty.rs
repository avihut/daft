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
use std::time::Duration;

use term_styles as styles;

use super::{Reporter, RunSummary, ScenarioStatus, StepReport, Verbosity};
use crate::manual_test::schema::{Scenario, Step};

const CAPTURE_LINE_CAP: usize = 20;

/// Scenarios slower than this earn a `(slow)` annotation on their footer.
/// Picked empirically — most scenarios finish in <1s, the slowest tend to be
/// the multi-clone fixtures around 3–4s, and anything above 5s is a real
/// outlier worth surfacing.
const SLOW_THRESHOLD: Duration = Duration::from_secs(5);

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
        // §2 (Hierarchy): scenario name is a primary heading — bold + named color.
        writeln!(out, "{}", styles::bold_cyan(&scenario.name))?;
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
        // §1 budget: blue is not a daft color slot. §2: counters are
        // scaffolding (tertiary), dim.
        write!(
            out,
            "{} {} ... ",
            styles::dim(&format!("[{}/{}]", idx + 1, total)),
            &step.name
        )
    }

    fn step_pass(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()> {
        let check_count = report.assertions.len();
        // §4 (pass-quiet/fail-loud): pass marker is minimal — lowercase `ok` in
        // plain green (not bold). The bold/loud stacking is reserved for FAIL.
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
                // §1: cyan is reserved for section heading. `$` lines are
                // content-level metadata — dim per §2 tertiary.
                writeln!(out, "  {}", styles::dim(&format!("$ {cmd}")))?;
            }
        }
        if self.show_pass_check_icons() {
            for a in report.assertions {
                // §3 iconography: ✓ is bold green at every level.
                writeln!(
                    out,
                    "  {} {}",
                    styles::bold_green("✓"),
                    styles::dim(&a.label)
                )?;
            }
        }
        if self.show_pass_capture() {
            write_captured(out, report, self.capture_cap())?;
        }
        Ok(())
    }

    fn step_fail(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()> {
        let fail_count = report.assertions.iter().filter(|a| !a.passed).count();
        // §4 (pass-quiet/fail-loud): FAIL stacks signals — bold + red + uppercase.
        writeln!(
            out,
            "{} {}",
            styles::bold_red("FAIL"),
            styles::dim(&format!("({fail_count} failed)"))
        )?;
        if self.show_expanded_command() {
            if let Some(cmd) = report.expanded_command {
                // §1: cyan is reserved for section heading. `$` lines are
                // content-level metadata — dim per §2 tertiary.
                writeln!(out, "  {}", styles::dim(&format!("$ {cmd}")))?;
            }
        }
        for a in report.assertions.iter().filter(|a| !a.passed) {
            writeln!(out, "  {} {}", styles::bold_red("✗"), a.label)?;
            if let Some(detail) = &a.detail {
                // §1 + §2: assertion detail lines under a failed assertion are
                // the failure payload — secondary (default fg), not tertiary
                // (dim). `dim` + a colored diff label collapses to muddy
                // grey-X on most terminals.
                for line in detail.lines() {
                    writeln!(out, "    {line}")?;
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
        duration: Duration,
    ) -> io::Result<()> {
        // No trailing blank: the cleanup_note (or next scenario_header)
        // attaches directly so the footer reads as part of its scenario block.
        //
        // §2/§4: icon AND scenario name share the same semantic at the same
        // granularity (this scenario passed/failed) — so the entire prefix is
        // bold + the outcome color, giving one strong horizontal anchor per
        // scenario. The dim duration suffix stays separate (tertiary metadata).
        let icon_and_name = match status {
            ScenarioStatus::Pass => styles::bold_green(&format!("✓ {}", &scenario.name)),
            ScenarioStatus::Fail => styles::bold_red(&format!("✗ {}", &scenario.name)),
        };
        let suffix = scenario_duration_suffix(duration);
        writeln!(out, "{}{}", icon_and_name, suffix)
    }

    fn cleanup_note(&self, out: &mut dyn Write, msg: &str) -> io::Result<()> {
        writeln!(out, "{}", styles::dim(msg))
    }

    fn run_summary(&self, out: &mut dyn Write, summary: &RunSummary<'_>) -> io::Result<()> {
        write_run_summary(out, summary)
    }
}

/// Write captured stdout and stderr as two labeled blocks, honoring an
/// optional per-stream line cap. Keeping the streams separate matches how
/// other test runners (Vitest, pytest) surface failure detail — the reader
/// can immediately tell whether the noise came from the program's normal
/// output or its error stream.
fn write_captured(
    out: &mut dyn Write,
    report: &StepReport<'_>,
    cap: Option<usize>,
) -> io::Result<()> {
    write_captured_section(out, "stdout", report.stdout, cap)?;
    write_captured_section(out, "stderr", report.stderr, cap)?;
    Ok(())
}

/// Write a single captured stream as a `--- {label} ---` block with the given
/// line cap. No-op when the stream is missing or empty after trimming.
fn write_captured_section(
    out: &mut dyn Write,
    label: &str,
    content: Option<&str>,
    cap: Option<usize>,
) -> io::Result<()> {
    let trimmed = match content {
        Some(s) => s.trim(),
        None => return Ok(()),
    };
    if trimmed.is_empty() {
        return Ok(());
    }
    writeln!(out, "  {}", styles::dim(&format!("--- {label} ---")))?;
    let limit = cap.unwrap_or(usize::MAX);
    let mut printed = 0usize;
    for line in trimmed.lines().take(limit) {
        writeln!(out, "  {}", styles::dim(line))?;
        printed += 1;
    }
    if let Some(cap) = cap {
        let actual = trimmed.lines().count();
        if actual > cap {
            writeln!(
                out,
                "  {}",
                styles::dim(&format!(
                    "... {} more lines truncated (re-run with -vv for full output)",
                    actual - printed
                ))
            )?;
        }
    }
    Ok(())
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
        writeln!(out, "{}", failed_scenarios_banner(s.failed.len()))?;
        writeln!(out)?;
        for (idx, f) in s.failed.iter().enumerate() {
            // §2: `1) ✗ name` is the primary anchor for the failure entry —
            // bold red across the icon+name span keeps it as one strong line.
            // The path uses `display_path` (scenarios-dir-relative) so it
            // doesn't outweigh the scenario name with absolute-path bytes.
            writeln!(
                out,
                "  {}) {}   {}{}",
                idx + 1,
                styles::bold_red(&format!("✗ {}", f.name)),
                styles::dim(&f.display_path),
                scenario_duration_suffix(f.duration),
            )?;
            if let Some(failing) = f.failing_step {
                let citation = step_citation(f.source, failing.line);
                // §3 iconography: ❯ is bold red. §2: focal step name is the
                // primary content of the sub-block — bold default-fg (the red
                // already belongs to the marker; the name is the data).
                writeln!(
                    out,
                    "      {} {} {}{}",
                    styles::bold_red("❯"),
                    styles::dim(&format!("step {}/{}", failing.index + 1, failing.total)),
                    styles::bold(&failing.step_name),
                    citation,
                )?;
                for a in &failing.failed_assertions {
                    writeln!(out, "      {} {}", styles::bold_red("✗"), a.label)?;
                    if let Some(detail) = &a.detail {
                        for line in detail.lines() {
                            writeln!(out, "        {line}")?;
                        }
                    }
                }
                write_summary_capture(out, "stdout", &failing.captured_stdout)?;
                write_summary_capture(out, "stderr", &failing.captured_stderr)?;
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

/// Trailing footer suffix for scenario duration. Returns an empty string for
/// `Duration::ZERO` (interactive runner sentinel — see `Reporter::scenario_footer`),
/// otherwise `"  142ms"` (or `"  2.3s  (slow)"` over [`SLOW_THRESHOLD`]).
///
/// The leading double-space is intentional: it separates the duration from
/// the scenario name without competing with the leading icon's color span.
pub(super) fn scenario_duration_suffix(d: Duration) -> String {
    if d.is_zero() {
        return String::new();
    }
    let core = format_short_duration(d);
    if d >= SLOW_THRESHOLD {
        format!("  {core}  {}", styles::yellow("(slow)"))
    } else {
        format!("  {}", styles::dim(&core))
    }
}

/// Short-form duration formatter used in footers and the failures block.
///
/// - `< 1s` → `"142ms"`
/// - `< 60s` → `"2.3s"`
/// - `>= 60s` → `"1m04s"`
///
/// Tied to the `(slow)` annotation rule above — keep the breakpoints visually
/// distinct so the eye can group fast/slow/very-slow without reading the units.
pub(super) fn format_short_duration(d: Duration) -> String {
    let total_ms = d.as_millis();
    if total_ms < 1_000 {
        format!("{total_ms}ms")
    } else if d.as_secs() < 60 {
        format!("{:.1}s", d.as_secs_f64())
    } else {
        let total = d.as_secs();
        let minutes = total / 60;
        let seconds = total % 60;
        format!("{minutes}m{seconds:02}s")
    }
}

/// Render `   at file.yml:N` next to the failing-step line, using just the
/// source file's basename — the full path appears on the line above. Returns
/// an empty string when no line number is available (synthetic scenarios in
/// tests, or pre-Phase-3 cached `FailingStep` records).
fn step_citation(source: &std::path::Path, line: Option<usize>) -> String {
    let line = match line {
        Some(n) => n,
        None => return String::new(),
    };
    let basename = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.display().to_string());
    format!("   {}", styles::dim(&format!("at {basename}:{line}")))
}

/// Render the `⎯⎯⎯ Failed Scenarios (N) ⎯⎯⎯` section banner above the
/// failures block. Fixed-width (no terminal-width probing) so golden tests
/// stay deterministic across environments.
///
/// §1 + §2 (CLAUDE.md): label is primary content (bold red); rule chars are
/// decoration (dim). Coloring the whole banner red would have the decoration
/// competing with the data.
fn failed_scenarios_banner(failure_count: usize) -> String {
    const RULE: &str = "⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯";
    format!(
        "{} {} {}",
        styles::dim(RULE),
        styles::bold_red(&format!("Failed Scenarios ({failure_count})")),
        styles::dim(RULE),
    )
}

/// Render a single captured stream inside the failed-scenarios summary block.
/// Indentation is six spaces (matching the surrounding step-detail block); the
/// label uses dim styling to stay subordinate to the assertion lines above it.
fn write_summary_capture(out: &mut dyn Write, label: &str, content: &str) -> io::Result<()> {
    if content.is_empty() {
        return Ok(());
    }
    writeln!(out, "      {}", styles::dim(&format!("--- {label} ---")))?;
    for line in content.lines() {
        writeln!(out, "      {line}")?;
    }
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
