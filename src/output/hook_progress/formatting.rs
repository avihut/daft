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
