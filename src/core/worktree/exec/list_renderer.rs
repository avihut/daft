//! List-mode renderer for `daft exec` multi-target runs.
//!
//! Prints a header once, then one line per worktree as each
//! completes. Not interactive; no in-place terminal repainting.

use super::{CommandSpec, ExecReport, WorktreeOutcome};

/// Abstraction that lets tests capture output to a string rather than
/// stdout. Production callers pass `&mut std::io::stdout()`.
pub trait Sink: std::io::Write {}
impl<T: std::io::Write> Sink for T {}

pub fn render_header<W: Sink>(
    sink: &mut W,
    target_count: usize,
    pipeline: &[CommandSpec],
) -> std::io::Result<()> {
    writeln!(sink, "{}", format_header_line(target_count, pipeline.len()))
}

const HEADER_WIDTH: usize = 60;
const HEADER_MIN_SIDE_DASHES: usize = 4;

/// Format the scope-summary divider with the summary horizontally centered
/// inside the divider. Width is fixed; padding on either side of the summary
/// is balanced (right side absorbs the remainder when the gap is odd).
pub(super) fn format_header_line(target_count: usize, command_count: usize) -> String {
    let wt_label = if target_count == 1 {
        "worktree"
    } else {
        "worktrees"
    };
    let cmd_label = if command_count == 1 {
        "command"
    } else {
        "commands"
    };
    let summary = format!("{target_count} {wt_label} · {command_count} {cmd_label}");
    let summary_cols = summary.chars().count() + 2; // include surrounding spaces
    let total_dashes = HEADER_WIDTH
        .saturating_sub(summary_cols)
        .max(HEADER_MIN_SIDE_DASHES * 2);
    let prefix_dashes = total_dashes / 2;
    let suffix_dashes = total_dashes - prefix_dashes;
    format!(
        "{} {summary} {}",
        "─".repeat(prefix_dashes),
        "─".repeat(suffix_dashes),
    )
}

pub fn render_outcome<W: Sink>(
    sink: &mut W,
    outcome: &WorktreeOutcome,
    pipeline: &[CommandSpec],
) -> std::io::Result<()> {
    let sigil = if outcome.cancelled {
        "⊘"
    } else if outcome.succeeded() {
        "✓"
    } else {
        "✗"
    };
    let elapsed = format!("{:.1}s", outcome.elapsed.as_secs_f64());
    if outcome.succeeded() {
        writeln!(
            sink,
            "  {sigil}  {:<24} ({elapsed})",
            outcome.target.branch_name
        )?;
    } else {
        let cmd_desc = pipeline
            .get(outcome.last_command_index)
            .map(|s| s.display())
            .unwrap_or_default();
        writeln!(
            sink,
            "  {sigil}  {:<24} ({elapsed})   {cmd_desc} → exit {}",
            outcome.target.branch_name, outcome.exit_code
        )?;
    }
    Ok(())
}

pub fn render_failed_output_dump<W: Sink>(
    sink: &mut W,
    report: &ExecReport,
    pipeline: &[CommandSpec],
) -> std::io::Result<()> {
    for outcome in &report.outcomes {
        if outcome.succeeded() {
            continue;
        }
        let cmd_desc = pipeline
            .get(outcome.last_command_index)
            .map(|s| s.display())
            .unwrap_or_default();
        writeln!(
            sink,
            "─── {} ── {cmd_desc} → exit {} ────────────────────────────",
            outcome.target.branch_name, outcome.exit_code
        )?;
        sink.write_all(&outcome.captured_output)?;
        if !outcome.captured_output.ends_with(b"\n") {
            writeln!(sink)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split_dashes(line: &str, summary: &str) -> (usize, usize) {
        let needle = format!(" {summary} ");
        let idx = line.find(&needle).expect("summary not found in header");
        let prefix = line[..idx].chars().count();
        let suffix = line[idx + needle.len()..].chars().count();
        (prefix, suffix)
    }

    #[test]
    fn header_summary_is_horizontally_centered() {
        // Regression: previously the header used 8 fixed leading dashes,
        // which left the summary visibly left-of-center. After the fix,
        // prefix and suffix dash counts must differ by at most 1.
        let line = format_header_line(4, 1);
        let (prefix, suffix) = split_dashes(&line, "4 worktrees · 1 command");
        assert!(
            prefix.abs_diff(suffix) <= 1,
            "expected centered divider, got prefix={prefix} suffix={suffix}: {line}"
        );
    }

    #[test]
    fn header_total_width_matches_constant() {
        let line = format_header_line(2, 3);
        assert_eq!(
            line.chars().count(),
            HEADER_WIDTH,
            "header line must occupy {HEADER_WIDTH} columns"
        );
    }

    #[test]
    fn header_handles_summary_overflowing_total_width() {
        // If the summary itself is wider than HEADER_WIDTH, fall back to
        // the minimum side dashes rather than producing a malformed line.
        let line = format_header_line(12345678, 87654321);
        let (prefix, suffix) = split_dashes(&line, "12345678 worktrees · 87654321 commands");
        assert!(prefix >= HEADER_MIN_SIDE_DASHES);
        assert!(suffix >= HEADER_MIN_SIDE_DASHES);
    }
}
