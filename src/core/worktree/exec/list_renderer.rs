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
    let wt_label = if target_count == 1 {
        "worktree"
    } else {
        "worktrees"
    };
    let cmd_label = if pipeline.len() == 1 {
        "command"
    } else {
        "commands"
    };
    let summary = format!(
        "{} {} · {} {}",
        target_count,
        wt_label,
        pipeline.len(),
        cmd_label,
    );
    const TOTAL_WIDTH: usize = 60;
    const PREFIX_DASHES: usize = 8;
    let summary_cols = summary.chars().count() + 2;
    let suffix_dashes = TOTAL_WIDTH.saturating_sub(PREFIX_DASHES + summary_cols);
    writeln!(
        sink,
        "{} {summary} {}",
        "─".repeat(PREFIX_DASHES),
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
