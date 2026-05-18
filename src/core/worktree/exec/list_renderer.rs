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

/// Selects which outcomes get an output block dumped after the progress UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DumpMode {
    /// Default: only failed or cancelled worktrees are dumped.
    FailuresOnly,
    /// `--show-output`: every worktree is dumped, including successes.
    All,
}

/// Per-outcome output dump. Each rendered worktree gets:
///
/// ```text
/// <sigil> <branch>  ·  <cmd>  ·  <status>, <elapsed>, exit <code>
///   │ <captured line 1>
///   │ <captured line 2>
/// ```
///
/// Sigil/status come from outcome state (`✓ succeeded`, `✗ failed`,
/// `⊘ cancelled`). `mode` decides whether successful outcomes are included.
///
/// A blank line separates each block from whatever precedes it (the preceding
/// summary rows or a prior block) so the boundary is obvious. The vertical
/// gutter visually contains the captured output; the header has no gutter, so
/// absence-of-gutter on the next header is the section break.
pub fn render_output_dump<W: Sink>(
    sink: &mut W,
    report: &ExecReport,
    pipeline: &[CommandSpec],
    mode: DumpMode,
) -> std::io::Result<()> {
    for outcome in &report.outcomes {
        if matches!(mode, DumpMode::FailuresOnly) && outcome.succeeded() {
            continue;
        }
        writeln!(sink)?;
        render_outcome_block(sink, outcome, pipeline)?;
    }
    Ok(())
}

const GUTTER_PREFIX: &[u8] = "  \u{2502} ".as_bytes(); // "  │ "

fn render_outcome_block<W: Sink>(
    sink: &mut W,
    outcome: &WorktreeOutcome,
    pipeline: &[CommandSpec],
) -> std::io::Result<()> {
    let cmd_desc = pipeline
        .get(outcome.last_command_index)
        .map(|s| s.display())
        .unwrap_or_default();
    let (sigil, status) = if outcome.cancelled {
        ("\u{2298}", "cancelled")
    } else if outcome.succeeded() {
        ("\u{2713}", "succeeded")
    } else {
        ("\u{2717}", "failed")
    };
    let elapsed = format!("{:.1}s", outcome.elapsed.as_secs_f64());
    writeln!(
        sink,
        "{sigil} {}  \u{00b7}  {cmd_desc}  \u{00b7}  {status}, {elapsed}, exit {}",
        outcome.target.branch_name, outcome.exit_code,
    )?;
    write_gutter_lines(sink, &outcome.captured_output)
}

/// Prefix each line of `bytes` with the gutter and write it to `sink`. Splits
/// on `\n` to preserve the original line structure; a trailing line without a
/// newline still gets the gutter and a terminating newline. Empty input emits
/// nothing.
fn write_gutter_lines<W: Sink>(sink: &mut W, bytes: &[u8]) -> std::io::Result<()> {
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            sink.write_all(GUTTER_PREFIX)?;
            sink.write_all(&bytes[start..i])?;
            sink.write_all(b"\n")?;
            start = i + 1;
        }
    }
    if start < bytes.len() {
        sink.write_all(GUTTER_PREFIX)?;
        sink.write_all(&bytes[start..])?;
        sink.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::core::worktree::exec::ResolvedTarget;

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

    fn outcome(branch: &str, exit: i32, cancelled: bool, output: &[u8]) -> WorktreeOutcome {
        WorktreeOutcome {
            target: ResolvedTarget {
                worktree_path: format!("/r/{branch}").into(),
                branch_name: branch.into(),
            },
            last_command_index: 0,
            exit_code: exit,
            elapsed: Duration::from_millis(1200),
            captured_output: output.to_vec(),
            cancelled,
        }
    }

    fn dump(report: &ExecReport, pipeline: &[CommandSpec]) -> String {
        dump_with(report, pipeline, DumpMode::FailuresOnly)
    }

    fn dump_with(report: &ExecReport, pipeline: &[CommandSpec], mode: DumpMode) -> String {
        let mut out = Vec::new();
        render_output_dump(&mut out, report, pipeline, mode).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn failure_block_uses_gutter_and_status_label() {
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![outcome("feat/x", 101, false, b"line one\nline two\n")],
            orphan_branches_skipped: vec![],
        };
        let s = dump(&report, &pipeline);
        // Leading blank separates the block from the preceding summary rows.
        assert_eq!(
            s,
            "\n\u{2717} feat/x  \u{00b7}  mise dev  \u{00b7}  failed, 1.2s, exit 101\n  \
             \u{2502} line one\n  \u{2502} line two\n",
        );
    }

    #[test]
    fn cancelled_block_uses_distinct_sigil_and_label() {
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![outcome("feat/y", -1, true, b"interrupted\n")],
            orphan_branches_skipped: vec![],
        };
        let s = dump(&report, &pipeline);
        assert!(
            s.starts_with(
                "\n\u{2298} feat/y  \u{00b7}  mise dev  \u{00b7}  cancelled, 1.2s, exit -1\n"
            ),
            "expected leading-blank cancelled header, got: {s}"
        );
        assert!(s.contains("  \u{2502} interrupted\n"));
    }

    #[test]
    fn each_failure_block_starts_with_blank_line() {
        // Regression: prior renderer used a single horizontal bar per failure
        // with no separator, so back-to-back dumps ran together visually and
        // the first dump landed on the same line as the last summary row.
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![
                outcome("feat/a", -1, true, b"a-out\n"),
                outcome("feat/b", -1, true, b"b-out\n"),
            ],
            orphan_branches_skipped: vec![],
        };
        let s = dump(&report, &pipeline);
        // Every failure header is preceded by a blank line. The first one
        // separates the dump from the live summary rows; subsequent ones
        // separate consecutive failures.
        assert!(
            s.starts_with("\n\u{2298} feat/a"),
            "missing leading blank: {s:?}"
        );
        let between = "  \u{2502} a-out\n\n\u{2298} feat/b";
        assert!(
            s.contains(between),
            "missing blank-line separator between failures: {s}"
        );
    }

    #[test]
    fn successful_outcomes_are_skipped() {
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![
                outcome("ok", 0, false, b"ignored\n"),
                outcome("bad", 2, false, b"shown\n"),
            ],
            orphan_branches_skipped: vec![],
        };
        let s = dump(&report, &pipeline);
        assert!(!s.contains("ignored"), "successful output leaked: {s}");
        assert!(s.contains("  \u{2502} shown\n"));
        assert!(s.contains("\u{2717} bad"));
    }

    #[test]
    fn captured_output_without_trailing_newline_still_gets_gutter() {
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![outcome("bad", 1, false, b"no-newline-end")],
            orphan_branches_skipped: vec![],
        };
        let s = dump(&report, &pipeline);
        assert!(s.ends_with("  \u{2502} no-newline-end\n"), "got: {s:?}");
    }

    #[test]
    fn empty_captured_output_emits_only_header() {
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![outcome("bad", 1, false, b"")],
            orphan_branches_skipped: vec![],
        };
        let s = dump(&report, &pipeline);
        assert!(!s.contains('\u{2502}'), "unexpected gutter line: {s}");
        // One leading blank + one header line; no gutter lines.
        assert_eq!(s.lines().count(), 2, "expected blank + header only: {s:?}");
        assert!(s.starts_with('\n'));
    }

    #[test]
    fn all_mode_includes_successful_outcomes() {
        // With DumpMode::All, a successful outcome gets its own block — same
        // gutter format as failures but with the success sigil and label.
        let pipeline = vec![CommandSpec::Argv(vec!["echo".into(), "hi".into()])];
        let report = ExecReport {
            outcomes: vec![outcome("feat/ok", 0, false, b"hi\n")],
            orphan_branches_skipped: vec![],
        };
        let s = dump_with(&report, &pipeline, DumpMode::All);
        assert_eq!(
            s,
            "\n\u{2713} feat/ok  \u{00b7}  echo hi  \u{00b7}  succeeded, 1.2s, exit 0\n  \
             \u{2502} hi\n",
        );
    }

    #[test]
    fn all_mode_renders_success_failure_and_cancelled_in_order() {
        // Order of outcomes is preserved; each gets the right sigil/label.
        let pipeline = vec![CommandSpec::Argv(vec!["mise".into(), "dev".into()])];
        let report = ExecReport {
            outcomes: vec![
                outcome("feat/ok", 0, false, b"good\n"),
                outcome("feat/bad", 7, false, b"oops\n"),
                outcome("feat/cancel", -1, true, b"halt\n"),
            ],
            orphan_branches_skipped: vec![],
        };
        let s = dump_with(&report, &pipeline, DumpMode::All);
        let ok_pos = s.find("\u{2713} feat/ok").expect("missing success header");
        let bad_pos = s.find("\u{2717} feat/bad").expect("missing failure header");
        let cancel_pos = s
            .find("\u{2298} feat/cancel")
            .expect("missing cancelled header");
        assert!(
            ok_pos < bad_pos && bad_pos < cancel_pos,
            "blocks rendered out of order: {s}"
        );
        assert!(s.contains("succeeded, 1.2s, exit 0"));
        assert!(s.contains("failed, 1.2s, exit 7"));
        assert!(s.contains("cancelled, 1.2s, exit -1"));
        assert!(s.contains("  \u{2502} good\n"));
        assert!(s.contains("  \u{2502} oops\n"));
        assert!(s.contains("  \u{2502} halt\n"));
    }

    #[test]
    fn all_mode_with_empty_captured_output_emits_only_header() {
        // Parity with the FailuresOnly case: a success with no output still
        // gets its header block (leading blank + header line) and nothing else.
        let pipeline = vec![CommandSpec::Argv(vec!["true".into()])];
        let report = ExecReport {
            outcomes: vec![outcome("feat/quiet", 0, false, b"")],
            orphan_branches_skipped: vec![],
        };
        let s = dump_with(&report, &pipeline, DumpMode::All);
        assert!(!s.contains('\u{2502}'), "unexpected gutter line: {s}");
        assert_eq!(s.lines().count(), 2, "expected blank + header only: {s:?}");
        assert!(s.starts_with('\n'));
        assert!(s.contains("\u{2713} feat/quiet"));
    }
}
