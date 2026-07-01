//! Shared formatting helpers for hook progress rendering.
//!
//! Color constants, duration formatting, and header/summary box drawing
//! used by both the interactive and plain renderers.

use crate::VERSION;
use crate::styles;
use std::time::Duration;

// ANSI color codes for hook output (256-color palette). The scaffolding
// greys and the attention yellow are shared with the plan-execute timeline
// (`crate::output::palette`) so the two surfaces compose seamlessly (#651).
pub(super) const ORANGE: &str = "\x1b[38;5;208m";
pub(super) use crate::output::palette::{DARK_GREY, GREY, YELLOW};
pub(super) const BRIGHT_WHITE: &str = "\x1b[97m";
pub(super) const BLUE: &str = "\x1b[38;5;75m";
pub(super) const ITALIC: &str = "\x1b[3m";

/// Default name-column width used when no target list is available to compute
/// the actual maximum. Matches the legacy `list_renderer::render_outcome` format.
pub(crate) const DEFAULT_NAME_COLUMN_WIDTH: usize = 24;

/// Check if hook visual output should be suppressed (e.g. during tests).
///
/// Delegates to the shared predicate in `crate::output::palette` so the hook
/// renderer and the timeline suppress under exactly the same conditions.
pub(super) fn output_suppressed() -> bool {
    crate::output::palette::testing_suppressed()
}

/// Generate the hook header lines (dark-grey framed box).
///
/// `target` names the entity the hook is acting on (e.g. the worktree/branch
/// being removed for `worktree-pre-remove`). When provided, the title gains an
/// ` on: <target>` segment so multi-source operations don't leave the user
/// guessing which worktree the hooks are touching. `None` for project-scoped
/// hooks (`pre-merge`, `post-merge`, `post-clone`).
pub(super) fn format_header_lines(
    hook_name: &str,
    target: Option<&str>,
    use_color: bool,
) -> Vec<String> {
    let target_segment = target.map(|t| format!("  on: {t}")).unwrap_or_default();
    let content_width = " daft hooks v".len()
        + VERSION.len()
        + "  ".len()
        + hook_name.len()
        + target_segment.len()
        + " ".len();
    let border_h = "\u{2500}".repeat(content_width);

    if use_color {
        let target_part = target
            .map(|t| format!("  {GREY}on: {BRIGHT_WHITE}{t}{}", styles::RESET))
            .unwrap_or_default();
        vec![
            format!("{GREY}\u{250c}{border_h}\u{2510}{}", styles::RESET),
            format!(
                "{GREY}\u{2502} {ORANGE}daft hooks {GREY}v{VERSION}  {}{BRIGHT_WHITE}{hook_name}{}{target_part}{GREY} \u{2502}{}",
                styles::BOLD,
                styles::RESET,
                styles::RESET
            ),
            format!("{GREY}\u{2514}{border_h}\u{2518}{}", styles::RESET),
        ]
    } else {
        vec![
            format!("\u{250c}{border_h}\u{2510}"),
            format!("\u{2502} daft hooks v{VERSION}  {hook_name}{target_segment} \u{2502}"),
            format!("\u{2514}{border_h}\u{2518}"),
        ]
    }
}

/// Generate the summary lines (separator + totals + per-job results).
pub(super) fn format_summary_lines(
    jobs: &[super::JobResultEntry],
    total_duration: Duration,
    use_color: bool,
) -> Vec<String> {
    use super::JobOutcome;

    if jobs.is_empty() {
        return Vec::new();
    }

    let total_str = format_duration(total_duration);
    let mut lines = vec![String::new(), String::new()]; // two blank lines before separator

    if use_color {
        lines.push(format!("{GREY}{}{}", "\u{2500}".repeat(40), styles::RESET));
        lines.push(format!(
            "{ORANGE}summary: {GREY}(done in {total_str}){}",
            styles::RESET
        ));
        for job in jobs {
            match &job.outcome {
                JobOutcome::Success => {
                    let dur = format_duration(job.duration);
                    lines.push(format!(
                        "{}  \u{2714} {}{} {GREY}({dur}){}",
                        styles::GREEN,
                        job.name,
                        styles::RESET,
                        styles::RESET
                    ));
                }
                JobOutcome::Failed => {
                    let dur = format_duration(job.duration);
                    lines.push(format!(
                        "{}  \u{2718} {}{} {GREY}({dur}){}",
                        styles::RED,
                        job.name,
                        styles::RESET,
                        styles::RESET
                    ));
                }
                JobOutcome::Skipped { show_duration, .. } => {
                    if *show_duration {
                        let dur = format_duration(job.duration);
                        lines.push(format!(
                            "{YELLOW}  \u{2298} {}{} {GREY}({dur}){}",
                            job.name,
                            styles::RESET,
                            styles::RESET
                        ));
                    } else {
                        lines.push(format!("{YELLOW}  \u{2298} {}{}", job.name, styles::RESET));
                    }
                }
                JobOutcome::Background { description } => {
                    let desc = description
                        .as_deref()
                        .map(|d| format!(" {GREY}\u{2014} {d}{}", styles::RESET))
                        .unwrap_or_default();
                    lines.push(format!(
                        "{BLUE}  \u{21BB} {} {GREY}(background){}{desc}",
                        job.name,
                        styles::RESET
                    ));
                }
            }
        }
    } else {
        lines.push("\u{2500}".repeat(40));
        lines.push(format!("summary: (done in {total_str})"));
        for job in jobs {
            match &job.outcome {
                JobOutcome::Success => {
                    let dur = format_duration(job.duration);
                    lines.push(format!("  \u{2714} {} ({dur})", job.name));
                }
                JobOutcome::Failed => {
                    let dur = format_duration(job.duration);
                    lines.push(format!("  \u{2718} {} ({dur})", job.name));
                }
                JobOutcome::Skipped { show_duration, .. } => {
                    if *show_duration {
                        let dur = format_duration(job.duration);
                        lines.push(format!("  \u{2298} {} ({dur})", job.name));
                    } else {
                        lines.push(format!("  \u{2298} {}", job.name));
                    }
                }
                JobOutcome::Background { description } => {
                    let desc = description
                        .as_deref()
                        .map(|d| format!(" \u{2014} {d}"))
                        .unwrap_or_default();
                    lines.push(format!("  \u{21BB} {} (background){desc}", job.name));
                }
            }
        }
    }

    lines
}

/// Format a duration to the most appropriate scale.
///
/// - Under 1 second: milliseconds (e.g., "112ms")
/// - 1-60 seconds: seconds with one decimal (e.g., "2.3s")
/// - Over 60 seconds: minutes and seconds (e.g., "1m 5s")
///
/// `pub(crate)` so the plan-execute timeline renders durations in exactly
/// the same vocabulary as the hook rows it composes with.
pub(crate) fn format_duration(d: Duration) -> String {
    let millis = d.as_millis();
    if millis < 1000 {
        format!("{millis}ms")
    } else {
        let secs = d.as_secs_f64();
        if secs < 60.0 {
            format!("{secs:.1}s")
        } else {
            let mins = d.as_secs() / 60;
            let remaining = d.as_secs() % 60;
            format!("{mins}m {remaining}s")
        }
    }
}

/// Lifecycle state of a finalized row (one per pipeline step).
#[derive(Debug, Clone, Copy)]
pub(super) enum RowState {
    Success { duration: Duration },
    Failure { duration: Duration },
    Cancelled { duration: Duration },
    Skipped,
}

/// Render a finalized per-step row for compact-finalization mode.
///
/// Shape (monospace):
/// ```text
///   <glyph>  <name padded to name_width>  ❯ <preview>  <right>
/// ```
/// When `command_preview` is `None`, the `❯ <preview>` segment is omitted.
/// `<right>` is the state-specific suffix: `(1.5s)` for success/failure,
/// `cancelled after 1.2s` for cancelled, `skipped` for skipped.
pub(super) fn format_compact_row(
    name: &str,
    command_preview: Option<&str>,
    state: RowState,
    name_width: usize,
    use_color: bool,
) -> String {
    let (sigil, color_code) = match state {
        RowState::Success { .. } => ("\u{2713}", styles::GREEN),
        RowState::Failure { .. } => ("\u{2717}", styles::RED),
        RowState::Cancelled { .. } => ("\u{2298}", YELLOW),
        RowState::Skipped => ("\u{25cb}", DARK_GREY),
    };
    let right = match state {
        RowState::Success { duration } | RowState::Failure { duration } => {
            format!("({})", format_duration(duration))
        }
        RowState::Cancelled { duration } => {
            format!("cancelled after {}", format_duration(duration))
        }
        RowState::Skipped => "skipped".to_string(),
    };

    let name_part = format!("{:<w$}", name, w = name_width);
    let preview_segment = command_preview
        .map(|p| format!("  \u{276f} {p}"))
        .unwrap_or_default();

    if use_color {
        format!(
            "  {color_code}{sigil}  {name_part}{}{preview_segment}  {GREY}{right}{}",
            styles::RESET,
            styles::RESET,
        )
    } else {
        format!("  {sigil}  {name_part}{preview_segment}  {right}")
    }
}

#[cfg(test)]
mod compact_row_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn success_row_with_preview_plain() {
        let row = format_compact_row(
            "master",
            Some("mise dev"),
            RowState::Success {
                duration: Duration::from_millis(1900),
            },
            12,
            false,
        );
        assert!(row.contains("\u{2713}"), "expected ✓, got: {row:?}");
        assert!(row.contains("master"), "missing branch: {row:?}");
        assert!(
            row.contains("\u{276f} mise dev"),
            "missing preview: {row:?}"
        );
        assert!(row.contains("(1.9s)"), "missing elapsed: {row:?}");
    }

    #[test]
    fn failure_row_with_preview_plain() {
        let row = format_compact_row(
            "feat/dirty",
            Some("cargo build"),
            RowState::Failure {
                duration: Duration::from_millis(1200),
            },
            12,
            false,
        );
        assert!(row.contains("\u{2717}"), "expected ✗, got: {row:?}");
        assert!(row.contains("feat/dirty"));
        assert!(row.contains("\u{276f} cargo build"));
        assert!(row.contains("(1.2s)"));
    }

    #[test]
    fn cancelled_row_with_preview_plain() {
        let row = format_compact_row(
            "master",
            Some("mise dev"),
            RowState::Cancelled {
                duration: Duration::from_millis(1200),
            },
            12,
            false,
        );
        assert!(row.contains("\u{2298}"), "expected ⊘, got: {row:?}");
        assert!(row.contains("master"));
        assert!(row.contains("\u{276f} mise dev"));
        assert!(
            row.contains("cancelled after 1.2s"),
            "missing cancelled suffix: {row:?}"
        );
    }

    #[test]
    fn skipped_row_with_preview_plain() {
        let row = format_compact_row(
            "daft-330/feat/merge",
            Some("mise fmt"),
            RowState::Skipped,
            20,
            false,
        );
        assert!(row.contains("\u{25cb}"), "expected ○, got: {row:?}");
        assert!(row.contains("daft-330/feat/merge"));
        assert!(row.contains("\u{276f} mise fmt"));
        assert!(
            row.ends_with("skipped"),
            "expected 'skipped' suffix: {row:?}"
        );
    }

    #[test]
    fn name_is_padded_to_requested_width() {
        let row = format_compact_row(
            "a",
            Some("cmd"),
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            10,
            false,
        );
        assert!(
            row.contains("a         "),
            "branch must be left-padded to 10 chars, got: {row:?}"
        );
    }

    #[test]
    fn preview_none_omits_arrow_segment_plain() {
        let row = format_compact_row(
            "master",
            None,
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            10,
            false,
        );
        assert!(
            !row.contains("\u{276f}"),
            "no preview ⇒ no arrow, got: {row:?}"
        );
        assert!(row.contains("master"));
        assert!(row.contains("(1.0s)"));
    }

    #[test]
    fn row_has_leading_indent() {
        let row = format_compact_row(
            "x",
            None,
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            4,
            false,
        );
        assert!(
            row.starts_with("  "),
            "expected 2-space leading indent, got: {row:?}"
        );
    }

    #[test]
    fn colored_success_uses_green_sigil() {
        let row = format_compact_row(
            "x",
            Some("cmd"),
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            4,
            true,
        );
        assert!(
            row.contains(crate::styles::GREEN),
            "colored success row should include GREEN, got: {row:?}"
        );
    }

    #[test]
    fn colored_cancelled_uses_yellow_sigil() {
        let row = format_compact_row(
            "x",
            Some("cmd"),
            RowState::Cancelled {
                duration: Duration::from_secs(1),
            },
            4,
            true,
        );
        assert!(
            row.contains(YELLOW),
            "colored cancelled row should include YELLOW, got: {row:?}"
        );
    }

    #[test]
    fn colored_skipped_uses_dark_grey() {
        let row = format_compact_row("x", Some("cmd"), RowState::Skipped, 4, true);
        assert!(
            row.contains(DARK_GREY),
            "colored skipped row should include DARK_GREY, got: {row:?}"
        );
    }

    #[test]
    fn colored_failure_uses_red_sigil() {
        let row = format_compact_row(
            "x",
            Some("cmd"),
            RowState::Failure {
                duration: Duration::from_secs(1),
            },
            4,
            true,
        );
        assert!(
            row.contains(crate::styles::RED),
            "colored failure row should include RED, got: {row:?}"
        );
    }

    #[test]
    fn colored_row_bounds_color_spans_correctly() {
        let row = format_compact_row(
            "x",
            Some("cmd"),
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            4,
            true,
        );
        assert!(
            row.ends_with(crate::styles::RESET),
            "row must terminate with RESET to prevent color bleed: {row:?}"
        );
        let first_reset = row.find(crate::styles::RESET).expect("must contain RESET");
        let grey_idx = row.find(GREY).expect("must contain GREY");
        assert!(
            first_reset < grey_idx,
            "RESET must precede GREY to close the sigil color region: {row:?}"
        );
    }
}
