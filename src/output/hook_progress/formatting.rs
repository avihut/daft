//! Shared formatting helpers for hook progress rendering.
//!
//! Color constants, duration formatting, and header/summary box drawing
//! used by both the interactive and plain renderers.

use crate::styles;
use crate::VERSION;
use std::time::Duration;

// ANSI color codes for hook output (256-color palette)
pub(super) const ORANGE: &str = "\x1b[38;5;208m";
pub(super) const YELLOW: &str = "\x1b[38;5;220m";
pub(super) const GREY: &str = "\x1b[38;5;245m";
pub(super) const BRIGHT_WHITE: &str = "\x1b[97m";
pub(super) const DARK_GREY: &str = "\x1b[38;5;240m";
pub(super) const ITALIC: &str = "\x1b[3m";

/// Check if hook visual output should be suppressed (e.g. during tests).
///
/// Returns true when running unit tests (`cfg!(test)`) or when `DAFT_TESTING`
/// env var is set (for integration tests that invoke the binary as a subprocess).
pub(super) fn output_suppressed() -> bool {
    cfg!(test) || std::env::var("DAFT_TESTING").is_ok()
}

/// Generate the hook header lines (dark-grey framed box).
pub(super) fn format_header_lines(hook_name: &str, use_color: bool) -> Vec<String> {
    let content_width =
        " daft hooks v".len() + VERSION.len() + "  hook: ".len() + hook_name.len() + " ".len();
    let border_h = "\u{2500}".repeat(content_width);

    if use_color {
        vec![
            format!("{GREY}\u{250c}{border_h}\u{2510}{}", styles::RESET),
            format!(
                "{GREY}\u{2502} {ORANGE}daft hooks {GREY}v{VERSION}  hook: {}{BRIGHT_WHITE}{hook_name}{}{GREY} \u{2502}{}",
                styles::BOLD, styles::RESET, styles::RESET
            ),
            format!("{GREY}\u{2514}{border_h}\u{2518}{}", styles::RESET),
        ]
    } else {
        vec![
            format!("\u{250c}{border_h}\u{2510}"),
            format!("\u{2502} daft hooks v{VERSION}  hook: {hook_name} \u{2502}"),
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
pub(super) fn format_duration(d: Duration) -> String {
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

/// Render a finalized per-job row for compact-finalization mode.
///
/// Matches `crate::core::worktree::exec::list_renderer::render_outcome`'s
/// visible shape: two-space indent, sigil, double space, 24-char left-padded
/// name, single space, parenthesized duration. Colored variant adds ANSI
/// escapes consistent with the summary formatting.
#[allow(dead_code)] // Wired up by Task 3 (compact-finalization branching in renderers).
pub(super) fn format_compact_row(
    name: &str,
    success: bool,
    duration: Duration,
    use_color: bool,
) -> String {
    let sigil = if success { "\u{2713}" } else { "\u{2717}" };
    let elapsed = format_duration(duration);
    if use_color {
        let color = if success { styles::GREEN } else { styles::RED };
        format!(
            "  {}{sigil}  {:<24}{} {GREY}({elapsed}){}",
            color,
            name,
            styles::RESET,
            styles::RESET
        )
    } else {
        format!("  {sigil}  {:<24} ({elapsed})", name)
    }
}

#[cfg(test)]
mod compact_row_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn compact_row_success_plain() {
        let row = format_compact_row("master", true, Duration::from_millis(1800), false);
        // Matches list_renderer::render_outcome's visible format:
        //   "  ✓  master                    (1.8s)"
        assert!(row.contains("\u{2713}"), "expected ✓, got: {row:?}");
        assert!(row.contains("master"), "missing name: {row:?}");
        assert!(row.contains("(1.8s)"), "missing elapsed: {row:?}");
    }

    #[test]
    fn compact_row_failure_plain() {
        let row = format_compact_row("feat/dirty", false, Duration::from_millis(1200), false);
        assert!(row.contains("\u{2717}"), "expected ✗, got: {row:?}");
        assert!(row.contains("feat/dirty"));
        assert!(row.contains("(1.2s)"));
    }

    #[test]
    fn compact_row_has_leading_indent() {
        let row = format_compact_row("x", true, Duration::from_secs(1), false);
        assert!(
            row.starts_with("  "),
            "expected 2-space leading indent, got: {row:?}"
        );
    }

    #[test]
    fn compact_row_color_wraps_sigil_and_name() {
        let row = format_compact_row("x", true, Duration::from_secs(1), true);
        assert!(
            row.starts_with(&format!("  {}", crate::styles::GREEN)),
            "colored success row should start with 2-space indent + GREEN, got: {row:?}"
        );
        // A RESET must appear before GREY to close the sigil+name color region.
        let reset_idx = row.find(crate::styles::RESET).expect("must contain RESET");
        let grey_idx = row.find(GREY).expect("must contain GREY");
        assert!(
            reset_idx < grey_idx,
            "RESET must close the color span before GREY duration; got: {row:?}"
        );
        assert!(
            row.ends_with(crate::styles::RESET),
            "row must end with RESET, got: {row:?}"
        );
    }

    #[test]
    fn compact_row_failure_color_uses_red() {
        let row = format_compact_row("x", false, Duration::from_secs(1), true);
        assert!(
            row.starts_with(&format!("  {}", crate::styles::RED)),
            "colored failure row should start with 2-space indent + RED, got: {row:?}"
        );
    }
}
