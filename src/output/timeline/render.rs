//! Pure line formatting for the rail timeline.
//!
//! Visual grammar (locked in #651): a dim rail (`┌ │ └`) threads the command
//! into one connected object; state lives in the leading glyph; payload stays
//! plain; durations are dim, parenthesized, and only shown at ≥ 1s. Colors
//! map onto the house budget — green success (never bold), bold-red failure,
//! yellow attention, cyan activity, greys for scaffolding — and dim is never
//! combined with a color on the same span.

use crate::output::hook_progress::format_duration;
use crate::output::palette::{BLUE, DARK_GREY, GREY, YELLOW};
use crate::styles;
use std::time::Duration;

/// Resolved visual state of a persisted row.
pub(super) enum RowFace {
    /// `✓` green — the step completed.
    Done { duration: Option<Duration> },
    /// `✗` bold red — the step failed; label stays imperative.
    Failed,
    /// `○` dark grey — resolved without running, expected (label replaced).
    SkippedExpected,
    /// `↓` yellow — resolved without running, attention-worthy.
    SkippedAttention,
    /// `○` dark grey — never reached because an earlier step failed.
    NotReached,
    /// `↻` blue — handed to the background coordinator (hook jobs); renders
    /// a fixed dim `background` annotation.
    Background,
}

/// Minimum duration worth printing on a row.
pub(super) const DURATION_THRESHOLD: Duration = Duration::from_secs(1);

pub(super) fn paint(code: &str, text: &str, use_color: bool) -> String {
    if use_color {
        format!("{code}{text}{}", styles::RESET)
    } else {
        text.to_string()
    }
}

/// `┌  Starting daft-652/x ← master`
pub(super) fn header(text: &str, annotation: Option<&str>, use_color: bool) -> String {
    let corner = paint(GREY, "\u{250c}", use_color);
    match annotation {
        Some(a) => format!("{corner}  {text} {a}"),
        None => format!("{corner}  {text}"),
    }
}

/// The rail spacer line: a lone dim `│`.
pub(super) fn spacer(use_color: bool) -> String {
    paint(GREY, "\u{2502}", use_color)
}

/// `└  Ready in 6.3s`
pub(super) fn footer(text: &str, use_color: bool) -> String {
    let corner = paint(GREY, "\u{2514}", use_color);
    format!("{corner}  {text}")
}

/// `├─ feat/a` — dim structural anchor for a row group, branching off the
/// rail toward its label (a `│` spacer always precedes it). The stroke
/// swallows the first gap space, so the label keeps the glyph-column rhythm.
pub(super) fn group(label: &str, use_color: bool) -> String {
    let rail = paint(GREY, "\u{251c}\u{2500}", use_color);
    format!("{rail} {}", paint(DARK_GREY, label, use_color))
}

/// Tuck a rendered row inside the rail: `│  <row>`. Section members (group
/// spans, hook jobs) render this way so the rail stays continuous and the
/// anchor's `├─` visibly carries its children.
pub(super) fn gutter(row: &str, use_color: bool) -> String {
    format!("{}  {row}", paint(GREY, "\u{2502}", use_color))
}

/// `○  no remote branch` — a non-step annotation.
pub(super) fn note(text: &str, use_color: bool) -> String {
    paint(DARK_GREY, &format!("\u{25cb}  {text}"), use_color)
}

/// A pending step row: `○  Create worktree   ../feat/x`, all dark grey.
pub(super) fn pending_row(
    label: &str,
    annotation: Option<&str>,
    label_width: usize,
    use_color: bool,
) -> String {
    let body = row_body(label, annotation, label_width);
    paint(DARK_GREY, &format!("\u{25cb}  {body}"), use_color)
}

/// The message part of the active row (the glyph slot is the bar template's
/// cyan spinner; this is everything after it).
pub(super) fn active_message(label: &str, annotation: Option<&str>, label_width: usize) -> String {
    row_body(label, annotation, label_width)
}

/// A persisted (final) step row.
pub(super) fn final_row(
    face: &RowFace,
    label: &str,
    annotation: Option<&str>,
    label_width: usize,
    use_color: bool,
) -> String {
    match face {
        RowFace::Done { duration } => {
            let glyph = paint(styles::GREEN, "\u{2713}", use_color);
            let body = row_body(label, annotation, label_width);
            match duration.filter(|d| *d >= DURATION_THRESHOLD) {
                Some(d) => {
                    let dur = paint(GREY, &format!("({})", format_duration(d)), use_color);
                    format!("{glyph}  {body}  {dur}")
                }
                None => format!("{glyph}  {body}"),
            }
        }
        RowFace::Failed => {
            let glyph = if use_color {
                format!("{}{}\u{2717}{}", styles::BOLD, styles::RED, styles::RESET)
            } else {
                "\u{2717}".to_string()
            };
            format!("{glyph}  {}", row_body(label, annotation, label_width))
        }
        RowFace::SkippedExpected => {
            let body = row_body(label, annotation, label_width);
            paint(DARK_GREY, &format!("\u{25cb}  {body}"), use_color)
        }
        RowFace::SkippedAttention => {
            let glyph = paint(YELLOW, "\u{2193}", use_color);
            format!("{glyph}  {}", row_body(label, annotation, label_width))
        }
        RowFace::NotReached => {
            let body = row_body(label, Some("(not run)"), label_width);
            paint(DARK_GREY, &format!("\u{25cb}  {body}"), use_color)
        }
        RowFace::Background => {
            let glyph = paint(BLUE, "\u{21bb}", use_color);
            let body = row_body(
                label,
                Some(&paint(DARK_GREY, "background", use_color)),
                label_width,
            );
            format!("{glyph}  {body}")
        }
    }
}

/// `<label padded>  <annotation>` — annotation column only when present.
fn row_body(label: &str, annotation: Option<&str>, label_width: usize) -> String {
    match annotation {
        Some(a) if !a.is_empty() => format!("{label:<label_width$}  {a}"),
        _ => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_carries_annotation() {
        let line = header("Starting daft-652/x", Some("\u{2190} master"), false);
        assert_eq!(line, "\u{250c}  Starting daft-652/x \u{2190} master");
    }

    #[test]
    fn pending_row_is_plain_without_color() {
        let line = pending_row("Create worktree", Some("../feat/x"), 16, false);
        assert_eq!(line, "\u{25cb}  Create worktree   ../feat/x");
    }

    #[test]
    fn done_row_shows_duration_at_threshold() {
        let face = RowFace::Done {
            duration: Some(Duration::from_millis(1900)),
        };
        let line = final_row(&face, "Pushed", Some("\u{2192} origin/x"), 6, false);
        assert!(line.starts_with("\u{2713}  Pushed"), "got: {line}");
        assert!(line.ends_with("(1.9s)"), "got: {line}");
    }

    #[test]
    fn done_row_hides_subsecond_duration() {
        let face = RowFace::Done {
            duration: Some(Duration::from_millis(120)),
        };
        let line = final_row(&face, "Created branch", None, 14, false);
        assert_eq!(line, "\u{2713}  Created branch");
    }

    #[test]
    fn failed_row_keeps_imperative_label_with_detail() {
        let line = final_row(
            &RowFace::Failed,
            "Push",
            Some("pre-push hook rejected"),
            4,
            false,
        );
        assert_eq!(line, "\u{2717}  Push  pre-push hook rejected");
    }

    #[test]
    fn not_reached_row_is_marked() {
        let line = final_row(&RowFace::NotReached, "Delete branch", None, 13, false);
        assert_eq!(line, "\u{25cb}  Delete branch  (not run)");
    }

    #[test]
    fn group_anchor_branches_off_the_rail() {
        // The stroke replaces the first gap space: the label stays in the
        // same column as every other row's body.
        let line = group("post-create hooks", false);
        assert_eq!(line, "\u{251c}\u{2500} post-create hooks");
    }

    #[test]
    fn gutter_tucks_a_row_inside_the_rail() {
        let row = final_row(&RowFace::Done { duration: None }, ".env", None, 4, false);
        assert_eq!(gutter(&row, false), "\u{2502}  \u{2713}  .env");
    }

    #[test]
    fn gutter_rail_glyph_is_grey_body_untouched() {
        let line = gutter("\u{2713}  .env", true);
        assert!(line.starts_with(GREY), "got: {line}");
        assert!(line.ends_with("\u{2713}  .env"), "got: {line}");
    }

    #[test]
    fn background_row_carries_fixed_annotation() {
        let line = final_row(&RowFace::Background, "check-todos", None, 11, false);
        assert_eq!(line, "\u{21bb}  check-todos  background");
    }

    #[test]
    fn background_row_colors_glyph_blue_and_annotation_dim() {
        let line = final_row(&RowFace::Background, "check-todos", None, 11, true);
        assert!(line.starts_with(BLUE), "got: {line}");
        assert!(line.contains(DARK_GREY), "got: {line}");
        assert!(!line.contains(styles::GREEN), "got: {line}");
    }

    #[test]
    fn color_state_lives_on_the_glyph_only() {
        let face = RowFace::Done { duration: None };
        let line = final_row(&face, "Created worktree", Some("../x"), 16, true);
        // Green glyph, reset, then plain body — the label carries no color.
        assert!(line.starts_with(&format!("{}\u{2713}{}", styles::GREEN, styles::RESET)));
        assert!(line.contains("Created worktree"));
        // No green anywhere after the glyph span.
        let after_glyph = &line[format!("{}\u{2713}{}", styles::GREEN, styles::RESET).len()..];
        assert!(!after_glyph.contains(styles::GREEN));
    }

    #[test]
    fn attention_skip_is_yellow_glyph_plain_body() {
        let line = final_row(
            &RowFace::SkippedAttention,
            "post-create hooks",
            Some("skipped \u{2014} repo not trusted"),
            17,
            true,
        );
        assert!(line.starts_with(YELLOW));
        assert!(line.contains("repo not trusted"));
    }
}
