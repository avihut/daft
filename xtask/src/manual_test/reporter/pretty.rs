//! Reporter for all four verbosity levels.
//!
//! Internal branching on `verbosity` keeps formatting differences local —
//! four top-level structs would be near-duplicate code. The verbosity ladder
//! lives in `reporter/CLAUDE.md` §6 (authoritative); summarized here for the
//! grep-from-code path:
//!
//! | level         | pass footer | fail footer | header + per-step | check icons | capture on fail | capture on pass | expanded cmd |
//! |---------------|-------------|-------------|-------------------|-------------|-----------------|-----------------|--------------|
//! | `Quiet`       | no          | yes         | no                | no          | no              | no              | no           |
//! | `Default`     | yes         | yes         | no                | no          | no              | no              | no           |
//! | `Verbose`     | yes         | yes         | yes               | no          | first 20 lines  | no              | no           |
//! | `VeryVerbose` | yes         | yes         | yes               | yes         | uncapped        | uncapped        | yes          |
//!
//! Failures always surface — the end-of-run failures block emits at every
//! level when there are failures, so `Quiet` / `Default` defer per-step
//! detail to the summary rather than dropping it.

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

    /// CLAUDE.md §6: scenario header (cyan name + dim path) ships from `-v`
    /// upward. `Default` / `Quiet` collapse to one footer line per scenario.
    fn show_scenario_header(&self) -> bool {
        matches!(self.verbosity, Verbosity::Verbose | Verbosity::VeryVerbose)
    }

    /// CLAUDE.md §6: per-step lines (`[N/M] name ... ok | FAIL`, fail-step
    /// assertion details + inline capture) ship from `-v` upward. The
    /// end-of-run failures block still carries the full failure detail at
    /// every level — `Default` / `Quiet` defer it, they don't drop it.
    fn show_per_step_lines(&self) -> bool {
        matches!(self.verbosity, Verbosity::Verbose | Verbosity::VeryVerbose)
    }

    /// CLAUDE.md §6: pass footer is everything but `Quiet`. The fail footer
    /// is unconditional (see `scenario_footer`).
    fn show_pass_footer(&self) -> bool {
        !matches!(self.verbosity, Verbosity::Quiet)
    }

    /// CLAUDE.md §6: `$ expanded-command` is `-vv`-only. The expansion is
    /// rarely the thing the reader needs; it's noise outside firehose mode.
    fn show_expanded_command(&self) -> bool {
        matches!(self.verbosity, Verbosity::VeryVerbose)
    }

    /// CLAUDE.md §6: per-check `✓` icons on a passing step are `-vv`-only —
    /// at `-v` the step-level `ok (N checks)` is enough.
    fn show_pass_check_icons(&self) -> bool {
        matches!(self.verbosity, Verbosity::VeryVerbose)
    }

    /// CLAUDE.md §6: inline capture on a passing step is `-vv`-only.
    /// On-fail capture is gated by `show_per_step_lines` (it lives inside
    /// `step_fail`), and so emits at `-v` upward.
    fn show_pass_capture(&self) -> bool {
        matches!(self.verbosity, Verbosity::VeryVerbose)
    }

    /// CLAUDE.md §6: capture line cap. `-v` caps at 20 lines; `-vv` uncapped.
    /// Only consulted when capture is actually being emitted (per
    /// `show_per_step_lines` for fail, `show_pass_capture` for pass).
    fn capture_cap(&self) -> Option<usize> {
        match self.verbosity {
            Verbosity::VeryVerbose => None,
            _ => Some(CAPTURE_LINE_CAP),
        }
    }

    /// CLAUDE.md §6 `-vv` callout: at `-vv` step blocks are separated by a
    /// blank line — Layer 3 + 4 content otherwise floods consecutive steps
    /// into a wall of text. At `-v` step blocks stack densely (Layer 2
    /// only, nothing in between to separate).
    fn show_block_spacing(&self) -> bool {
        matches!(self.verbosity, Verbosity::VeryVerbose)
    }
}

impl Reporter for PrettyReporter {
    fn scenario_header(&self, out: &mut dyn Write, scenario: &Scenario) -> io::Result<()> {
        // §6: scenario header ships from `-v` upward.
        if !self.show_scenario_header() {
            return Ok(());
        }
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
        // §6 `-vv`: blank between scenario header (Layer 1 metadata) and the
        // first step block (Layer 2). At `-v` step blocks attach tight.
        if self.show_block_spacing() {
            writeln!(out)?;
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
        // §6: per-step lines ship from `-v` upward.
        if !self.show_per_step_lines() {
            return Ok(());
        }
        // §6 `-vv` callout: blank line between step blocks at `-vv`. Owned by
        // step_start (not the prior step_pass/step_fail) so the first step
        // — whose leading blank comes from scenario_header — doesn't double.
        if idx > 0 && self.show_block_spacing() {
            writeln!(out)?;
        }
        // §6 indent ladder: step opening line indents to col 2 (Layer 2
        // sits one indent under Layer 1 scenario). Backported to `-v`.
        // §1 budget: `[N/M]` counter is scaffolding, dim.
        // §1 + §6: step name uses plain cyan — same structural color as the
        // bold-cyan scenario header above, with the bold/plain weight
        // marking the Level 1 vs Level 2 shift. Applies at both `-v` and
        // `-vv` (color alone is enough; bold would compete with the
        // scenario header).
        write!(
            out,
            "  {} {} ... ",
            styles::dim(&format!("[{}/{}]", idx + 1, total)),
            styles::cyan(&step.name),
        )
    }

    fn step_pass(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()> {
        // §6: per-step lines (including this `ok` outcome line) ship from
        // `-v` upward.
        if !self.show_per_step_lines() {
            return Ok(());
        }
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
                write_expanded_command(out, cmd)?;
            }
        }
        if self.show_pass_check_icons() {
            for a in report.assertions {
                // §6 indent ladder: ✓ verification is Layer 3, col 6.
                // §3 + §4: `✓` is plain green (not bold) at every level.
                // §2: assertion labels are secondary (default fg, not dim).
                writeln!(out, "      {} {}", styles::green("✓"), &a.label)?;
            }
        }
        if self.show_pass_capture() {
            write_captured(out, report, self.capture_cap())?;
        }
        Ok(())
    }

    fn step_fail(&self, out: &mut dyn Write, report: &StepReport<'_>) -> io::Result<()> {
        // §6: per-step lines (including the `FAIL` outcome + inline assertion
        // detail + on-fail capture) ship from `-v` upward. At `Default` /
        // `Quiet` the user sees just the scenario footer inline; the full
        // failure detail lands in the end-of-run failures block.
        if !self.show_per_step_lines() {
            return Ok(());
        }
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
                write_expanded_command(out, cmd)?;
            }
        }
        for a in report.assertions.iter().filter(|a| !a.passed) {
            // §6 indent ladder: ✗ assertion at col 6 (Layer 3); detail at
            // col 8 (2 inside the ✗ line — same sub-element relationship
            // detail has had since the ladder was at 2-space increments).
            writeln!(out, "      {} {}", styles::bold_red("✗"), a.label)?;
            if let Some(detail) = &a.detail {
                // §1 + §2: assertion detail lines under a failed assertion are
                // the failure payload — secondary (default fg), not tertiary
                // (dim). `dim` + a colored diff label collapses to muddy
                // grey-X on most terminals.
                for line in detail.lines() {
                    writeln!(out, "        {line}")?;
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
        // §6: pass footer is everything but `-q`; fail footer is unconditional.
        // The asymmetry is the §4 pass-quiet/fail-loud principle landing at
        // the verbosity-flag level — `-q` runs read as "silent until red."
        if matches!(status, ScenarioStatus::Pass) && !self.show_pass_footer() {
            return Ok(());
        }
        // No trailing blank: the cleanup_note (or next scenario_header)
        // attaches directly so the footer reads as part of its scenario block.
        //
        // §4 (pass-quiet, fail-loud): the styling is asymmetric.
        //   Pass: `✓` in plain green + name in default fg + dim duration.
        //     Bold green on every passing footer in a 252-scenario green run
        //     turns the whole stream into chrome — the eye stops being able
        //     to skim past it. Plain green on the tiny `✓` glyph is enough.
        //   Fail: whole `✗ name` span bold red so a single red line jumps
        //     off a wall of quiet pass lines.
        let prefix = match status {
            ScenarioStatus::Pass => {
                format!("{} {}", styles::green("✓"), &scenario.name)
            }
            ScenarioStatus::Fail => styles::bold_red(&format!("✗ {}", &scenario.name)),
        };
        let suffix = scenario_duration_suffix(duration);
        writeln!(out, "{}{}", prefix, suffix)
    }

    fn cleanup_note(&self, out: &mut dyn Write, msg: &str) -> io::Result<()> {
        writeln!(out, "{}", styles::dim(msg))
    }

    fn run_summary(&self, out: &mut dyn Write, summary: &RunSummary<'_>) -> io::Result<()> {
        write_run_summary(out, summary)
    }
}

/// Write the expanded `$ command` block at the Layer-3 indent (col 6).
///
/// §1: command body is blue (step-action color). Multi-line `run:` YAML
/// fields produce multi-line expanded commands; each continuation line
/// gets a `>` shell-prompt prefix at the same indent so the visual frame
/// holds across line wraps and the reader can tell at a glance that the
/// whole block is one command, not a sequence of step actions.
fn write_expanded_command(out: &mut dyn Write, cmd: &str) -> io::Result<()> {
    for (i, line) in cmd.lines().enumerate() {
        let prefix = if i == 0 { '$' } else { '>' };
        writeln!(out, "      {}", styles::blue(&format!("{prefix} {line}")))?;
    }
    Ok(())
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
    // §6 indent ladder + `-vv` callout: stream label at col 6 (Layer 4
    // header), body at col 10. Both dim — the step-color above plus the
    // indent provide the visual frame; color on the capture would compete
    // with the step-identity signal. No `--- {label} ---` decoration; the
    // indent does the framing now.
    writeln!(out, "      {}", styles::dim(label))?;
    let limit = cap.unwrap_or(usize::MAX);
    let mut printed = 0usize;
    for line in trimmed.lines().take(limit) {
        writeln!(out, "          {}", styles::dim(line))?;
        printed += 1;
    }
    if let Some(cap) = cap {
        let actual = trimmed.lines().count();
        if actual > cap {
            writeln!(
                out,
                "          {}",
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
            // The duration is the only suffix; the location pointer moves to
            // its own line below.
            writeln!(
                out,
                "  {}) {}{}",
                idx + 1,
                styles::bold_red(&format!("✗ {}", f.name)),
                scenario_duration_suffix(f.duration),
            )?;
            // §1/§2: location pointer is the answer to "where did this fail"
            // — secondary (default fg), on its own line. Most terminals
            // recognize `path:line` as click-to-open; keeping it unstyled
            // and on its own line maximizes that affordance.
            let line_num = f.failing_step.and_then(|fs| fs.line);
            writeln!(
                out,
                "      {}",
                failure_location_line(&f.display_path, line_num),
            )?;
            if let Some(failing) = f.failing_step {
                // §3 iconography: ❯ is bold red. §2: focal step name is the
                // primary content of the sub-block — bold default-fg (the red
                // already belongs to the marker; the name is the data).
                writeln!(
                    out,
                    "      {} {} {}",
                    styles::bold_red("❯"),
                    styles::dim(&format!("step {}/{}", failing.index + 1, failing.total)),
                    styles::bold(&failing.step_name),
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

/// Render the failure-block location pointer: the scenarios-relative path,
/// optionally followed by `:line` when the failing step's source line is
/// known. Emitted unstyled (default fg) so most terminals' click-to-open
/// `path:line` heuristics fire on it. The caller owns indentation.
fn failure_location_line(display_path: &str, line: Option<usize>) -> String {
    match line {
        Some(n) => format!("{display_path}:{n}"),
        None => display_path.to_string(),
    }
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
