//! Pure line formatting for the rail timeline.
//!
//! Visual grammar (locked in #651): a dim rail (`┌ │ └`) threads the command
//! into one connected object; state lives in the leading glyph; labels speak
//! daft's vocabulary plain (bold for section headings); subjects wear
//! identity inks constant across states — cyan for the network, manila for
//! paths, violet for shared files, blue for background work; durations are
//! dim, parenthesized, and only shown at ≥ 1s. Outcome colors — green
//! success (never bold), bold-red failure, yellow attention — stay on the
//! glyph for spine steps and additionally flood the name on hook-job rows
//! (the verbose block's scheme, so both hook presentations agree). Dim is
//! never combined with a color on the same span.

use super::plan::{SubjectInk, SubjectInks};
use crate::output::hook_progress::format_duration;
use crate::output::palette::{BLUE, DARK_GREY, GREY, MANILA, VIOLET, YELLOW};
use crate::styles;
use std::time::Duration;

/// Resolved visual state of a persisted spine row.
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
}

/// Resolved visual state of a hook-job receipt row. Unlike spine rows, the
/// outcome color floods the job name — the verbose block's scheme
/// (`hook_progress::formatting`), kept so the succinct and full hook
/// presentations speak one color language.
pub(super) enum HookJobFace {
    /// `✓` — green glyph and name.
    Done { duration: Option<Duration> },
    /// `✗` — bold-red glyph, red name.
    Failed,
    /// `↓` — yellow glyph and name; the reason stays plain.
    SkippedAttention,
    /// `↻` — blue glyph and name, fixed dim `background` annotation.
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

/// ANSI code for a subject ink; `None` renders as-is.
fn ink_code(ink: SubjectInk) -> Option<&'static str> {
    match ink {
        SubjectInk::Plain => None,
        SubjectInk::Remote => Some(styles::CYAN),
        SubjectInk::Path => Some(MANILA),
        SubjectInk::Shared => Some(VIOLET),
    }
}

fn paint_ink(code: Option<&str>, text: &str, use_color: bool) -> String {
    match code {
        Some(code) if use_color => format!("{code}{text}{}", styles::RESET),
        _ => text.to_string(),
    }
}

/// Inks for rows whose parts are all daft's own words.
pub(super) const PLAIN_INKS: SubjectInks = SubjectInks {
    label: SubjectInk::Plain,
    annotation: SubjectInk::Plain,
};

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

/// `├─ feat/a` — section heading anchor, branching off the rail toward its
/// bold label (a `│` spacer always precedes it). The stroke swallows the
/// first gap space, so the label keeps the glyph-column rhythm. An
/// annotation (the verbose hook anchor's `worktree-post-create · daft v…`)
/// trails the label a tier down, in the scaffolding grey.
pub(super) fn group(label: &str, annotation: Option<&str>, use_color: bool) -> String {
    let rail = paint(GREY, "\u{251c}\u{2500}", use_color);
    let label = if use_color {
        format!("{}{label}{}", styles::BOLD, styles::RESET)
    } else {
        label.to_string()
    };
    match annotation.filter(|a| !a.is_empty()) {
        Some(a) => format!("{rail} {label}  {}", paint(GREY, a, use_color)),
        None => format!("{rail} {label}"),
    }
}

/// Tuck a rendered row inside the rail: `│  <row>`. Section members (group
/// spans, hook jobs) render this way so the rail stays continuous and the
/// anchor's `├─` visibly carries its children.
pub(super) fn gutter(row: &str, use_color: bool) -> String {
    format!("{}  {row}", paint(GREY, "\u{2502}", use_color))
}

/// `○  no remote branch` — a non-step annotation, recessed a tier.
pub(super) fn note(text: &str, use_color: bool) -> String {
    paint(GREY, &format!("\u{25cb}  {text}"), use_color)
}

/// A pending step row: dim `○` glyph, plain (readable) label, annotation a
/// tier below unless it wears an identity ink — the committed plan must be
/// glanceable, not a grey slab.
pub(super) fn pending_row(
    label: &str,
    annotation: Option<&str>,
    label_width: usize,
    inks: SubjectInks,
    use_color: bool,
) -> String {
    let glyph = paint(DARK_GREY, "\u{25cb}", use_color);
    let body = row_body(label, annotation, label_width, inks, Some(GREY), use_color);
    format!("{glyph}  {body}")
}

/// The message part of the active row (the glyph slot is the bar template's
/// cyan spinner; this is everything after it).
pub(super) fn active_message(
    label: &str,
    annotation: Option<&str>,
    label_width: usize,
    inks: SubjectInks,
    use_color: bool,
) -> String {
    row_body(label, annotation, label_width, inks, None, use_color)
}

/// A persisted (final) spine row. Identity inks apply to the label always
/// and to the annotation only on the Done face — failure details and skip
/// reasons are composed text and render plain; expected skips and
/// not-reached rows dim wholesale (nothing happened; no ink survives dim).
pub(super) fn final_row(
    face: &RowFace,
    label: &str,
    annotation: Option<&str>,
    label_width: usize,
    inks: SubjectInks,
    use_color: bool,
) -> String {
    let reason_inks = SubjectInks {
        label: inks.label,
        annotation: SubjectInk::Plain,
    };
    match face {
        RowFace::Done { duration } => {
            let glyph = paint(styles::GREEN, "\u{2713}", use_color);
            let dur = duration
                .filter(|d| *d >= DURATION_THRESHOLD)
                .map(|d| paint(GREY, &format!("({})", format_duration(d)), use_color));
            match (annotation.filter(|a| !a.is_empty()), dur) {
                // Both: the annotation owns its column, duration trails it.
                (Some(_), Some(d)) => {
                    let body = row_body(label, annotation, label_width, inks, None, use_color);
                    format!("{glyph}  {body}  {d}")
                }
                // A lone duration seats in the annotation column — section
                // receipts share one duration column instead of each
                // hugging its own name. Pre-painted grey; ink must not
                // re-wrap it.
                (None, Some(d)) => {
                    let body = row_body(label, Some(&d), label_width, reason_inks, None, use_color);
                    format!("{glyph}  {body}")
                }
                _ => {
                    let body = row_body(label, annotation, label_width, inks, None, use_color);
                    format!("{glyph}  {body}")
                }
            }
        }
        RowFace::Failed => {
            let glyph = if use_color {
                format!("{}{}\u{2717}{}", styles::BOLD, styles::RED, styles::RESET)
            } else {
                "\u{2717}".to_string()
            };
            let body = row_body(label, annotation, label_width, reason_inks, None, use_color);
            format!("{glyph}  {body}")
        }
        RowFace::SkippedExpected => {
            let body = row_body(label, annotation, label_width, PLAIN_INKS, None, false);
            paint(DARK_GREY, &format!("\u{25cb}  {body}"), use_color)
        }
        RowFace::SkippedAttention => {
            let glyph = paint(YELLOW, "\u{2193}", use_color);
            let body = row_body(label, annotation, label_width, reason_inks, None, use_color);
            format!("{glyph}  {body}")
        }
        RowFace::NotReached => {
            let body = row_body(
                label,
                Some("(not run)"),
                label_width,
                PLAIN_INKS,
                None,
                false,
            );
            paint(DARK_GREY, &format!("\u{25cb}  {body}"), use_color)
        }
    }
}

/// A hook-job receipt row: the outcome color floods glyph and name together.
pub(super) fn hook_job_row(
    face: &HookJobFace,
    name: &str,
    annotation: Option<&str>,
    name_width: usize,
    use_color: bool,
) -> String {
    let flooded = |code: &str, glyph: &str, ann_code: Option<&str>, ann: Option<&str>| {
        let glyph = paint(code, glyph, use_color);
        let name_part = match ann.filter(|a| !a.is_empty()) {
            Some(a) => format!(
                "{}  {}",
                paint(code, &format!("{name:<name_width$}"), use_color),
                paint_ink(ann_code, a, use_color)
            ),
            None => paint(code, name, use_color),
        };
        format!("{glyph}  {name_part}")
    };
    match face {
        HookJobFace::Done { duration } => {
            let dur = duration
                .filter(|d| *d >= DURATION_THRESHOLD)
                .map(|d| paint(GREY, &format!("({})", format_duration(d)), use_color));
            flooded(styles::GREEN, "\u{2713}", None, dur.as_deref())
        }
        HookJobFace::Failed => {
            let glyph = if use_color {
                format!("{}{}\u{2717}{}", styles::BOLD, styles::RED, styles::RESET)
            } else {
                "\u{2717}".to_string()
            };
            let name_part = match annotation.filter(|a| !a.is_empty()) {
                Some(a) => format!(
                    "{}  {a}",
                    paint(styles::RED, &format!("{name:<name_width$}"), use_color)
                ),
                None => paint(styles::RED, name, use_color),
            };
            format!("{glyph}  {name_part}")
        }
        HookJobFace::SkippedAttention => flooded(YELLOW, "\u{2193}", None, annotation),
        HookJobFace::Background => flooded(
            BLUE,
            "\u{21bb}",
            Some(DARK_GREY),
            Some(annotation.unwrap_or("background")),
        ),
    }
}

/// `<label padded>  <annotation>` — annotation column only when present.
/// Padding is applied before painting so ANSI never skews the width; the
/// annotation falls back to `ann_fallback` (e.g. the pending tier's grey)
/// when it carries no identity ink.
fn row_body(
    label: &str,
    annotation: Option<&str>,
    label_width: usize,
    inks: SubjectInks,
    ann_fallback: Option<&'static str>,
    use_color: bool,
) -> String {
    let label_code = ink_code(inks.label);
    match annotation.filter(|a| !a.is_empty()) {
        Some(a) => {
            let padded = format!("{label:<label_width$}");
            let ann_code = ink_code(inks.annotation).or(ann_fallback);
            format!(
                "{}  {}",
                paint_ink(label_code, &padded, use_color),
                paint_ink(ann_code, a, use_color)
            )
        }
        None => paint_ink(label_code, label, use_color),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REMOTE_ANN: SubjectInks = SubjectInks {
        label: SubjectInk::Plain,
        annotation: SubjectInk::Remote,
    };
    const PATH_ANN: SubjectInks = SubjectInks {
        label: SubjectInk::Plain,
        annotation: SubjectInk::Path,
    };
    const SHARED_LABEL: SubjectInks = SubjectInks {
        label: SubjectInk::Shared,
        annotation: SubjectInk::Plain,
    };

    #[test]
    fn header_carries_annotation() {
        let line = header("Starting daft-652/x", Some("\u{2190} master"), false);
        assert_eq!(line, "\u{250c}  Starting daft-652/x \u{2190} master");
    }

    #[test]
    fn pending_row_is_plain_without_color() {
        let line = pending_row("Create worktree", Some("../feat/x"), 16, PLAIN_INKS, false);
        assert_eq!(line, "\u{25cb}  Create worktree   ../feat/x");
    }

    #[test]
    fn pending_glyph_dims_but_the_label_stays_plain() {
        // The committed plan must be readable: only the ○ wears the pending
        // grey; the label is default ink; the untyped annotation recedes one
        // tier (mid grey, not the glyph's dark grey).
        let line = pending_row("Check out branch", Some("detail"), 16, PLAIN_INKS, true);
        assert!(
            line.starts_with(&format!("{DARK_GREY}\u{25cb}{}", styles::RESET)),
            "got: {line:?}"
        );
        assert!(
            line.contains(&format!("{}  Check out branch", styles::RESET)),
            "label must carry no ink: {line:?}"
        );
        assert!(
            line.contains(&format!("{GREY}detail{}", styles::RESET)),
            "untyped annotation sits a tier below: {line:?}"
        );
    }

    #[test]
    fn identity_inks_survive_the_pending_state() {
        // `→ origin/x` is cyan whether the push has happened or not.
        let line = pending_row("Push", Some("\u{2192} origin/x"), 4, REMOTE_ANN, true);
        assert!(
            line.contains(&format!(
                "{}\u{2192} origin/x{}",
                styles::CYAN,
                styles::RESET
            )),
            "got: {line:?}"
        );
    }

    #[test]
    fn done_row_shows_duration_at_threshold() {
        let face = RowFace::Done {
            duration: Some(Duration::from_millis(1900)),
        };
        let line = final_row(
            &face,
            "Pushed",
            Some("\u{2192} origin/x"),
            6,
            PLAIN_INKS,
            false,
        );
        assert!(line.starts_with("\u{2713}  Pushed"), "got: {line}");
        assert!(line.ends_with("(1.9s)"), "got: {line}");
    }

    #[test]
    fn remote_subject_is_cyan_on_the_done_row() {
        let face = RowFace::Done { duration: None };
        let line = final_row(
            &face,
            "Created branch",
            Some("\u{2190} origin/master"),
            14,
            REMOTE_ANN,
            true,
        );
        assert!(
            line.contains(&format!(
                "{}\u{2190} origin/master{}",
                styles::CYAN,
                styles::RESET
            )),
            "got: {line:?}"
        );
    }

    #[test]
    fn path_subject_is_manila() {
        let face = RowFace::Done { duration: None };
        let line = final_row(&face, "Created worktree", Some("../x"), 16, PATH_ANN, true);
        assert!(
            line.contains(&format!("{MANILA}../x{}", styles::RESET)),
            "got: {line:?}"
        );
    }

    #[test]
    fn shared_label_is_violet_in_every_live_state() {
        let pending = pending_row(".env", None, 4, SHARED_LABEL, true);
        let done = final_row(
            &RowFace::Done { duration: None },
            ".env",
            None,
            4,
            SHARED_LABEL,
            true,
        );
        for line in [pending, done] {
            assert!(
                line.contains(&format!("{VIOLET}.env{}", styles::RESET)),
                "got: {line:?}"
            );
        }
    }

    #[test]
    fn lone_duration_seats_in_the_annotation_column() {
        let face = RowFace::Done {
            duration: Some(Duration::from_millis(2100)),
        };
        let line = final_row(&face, "prepare-db", None, 12, PLAIN_INKS, false);
        assert_eq!(line, "\u{2713}  prepare-db    (2.1s)");
    }

    #[test]
    fn lone_duration_is_not_rewrapped_by_the_subject_ink() {
        // `✓ Checked out branch  (1.2s)` on a remote-typed stage: the
        // pre-painted grey duration must not be nested inside cyan.
        let face = RowFace::Done {
            duration: Some(Duration::from_millis(1200)),
        };
        let line = final_row(&face, "Checked out branch", None, 18, REMOTE_ANN, true);
        assert!(!line.contains(styles::CYAN), "got: {line:?}");
        assert!(line.contains(GREY), "duration stays grey: {line:?}");
    }

    #[test]
    fn done_row_hides_subsecond_duration() {
        let face = RowFace::Done {
            duration: Some(Duration::from_millis(120)),
        };
        let line = final_row(&face, "Created branch", None, 14, PLAIN_INKS, false);
        assert_eq!(line, "\u{2713}  Created branch");
    }

    #[test]
    fn failed_row_keeps_imperative_label_and_plain_detail() {
        // The detail is composed text — the stage's remote ink must not
        // paint it.
        let line = final_row(
            &RowFace::Failed,
            "Push",
            Some("pre-push hook rejected"),
            4,
            REMOTE_ANN,
            true,
        );
        assert!(!line.contains(styles::CYAN), "got: {line:?}");
        assert!(line.contains("pre-push hook rejected"));
        let plain = final_row(
            &RowFace::Failed,
            "Push",
            Some("pre-push hook rejected"),
            4,
            PLAIN_INKS,
            false,
        );
        assert_eq!(plain, "\u{2717}  Push  pre-push hook rejected");
    }

    #[test]
    fn not_reached_row_dims_wholesale() {
        let line = final_row(
            &RowFace::NotReached,
            "Delete branch",
            None,
            13,
            PLAIN_INKS,
            false,
        );
        assert_eq!(line, "\u{25cb}  Delete branch  (not run)");
        let colored = final_row(
            &RowFace::NotReached,
            "Delete branch",
            None,
            13,
            REMOTE_ANN,
            true,
        );
        assert!(colored.starts_with(DARK_GREY), "got: {colored:?}");
        assert!(!colored.contains(styles::CYAN), "no ink survives dim");
    }

    #[test]
    fn expected_skip_dims_wholesale_over_identity_inks() {
        let line = final_row(
            &RowFace::SkippedExpected,
            ".env",
            Some("already linked"),
            4,
            SHARED_LABEL,
            true,
        );
        assert!(line.starts_with(DARK_GREY), "got: {line:?}");
        assert!(!line.contains(VIOLET), "no ink survives dim: {line:?}");
    }

    #[test]
    fn group_anchor_branches_off_the_rail() {
        // The stroke replaces the first gap space: the label stays in the
        // same column as every other row's body.
        let line = group("post-create hooks", None, false);
        assert_eq!(line, "\u{251c}\u{2500} post-create hooks");
    }

    #[test]
    fn group_anchor_label_is_a_bold_heading() {
        let line = group("post-create hooks", None, true);
        assert!(
            line.contains(&format!(
                "{}post-create hooks{}",
                styles::BOLD,
                styles::RESET
            )),
            "got: {line:?}"
        );
        assert!(!line.contains(DARK_GREY), "headings are not scaffolding");
    }

    #[test]
    fn group_anchor_annotation_trails_grey() {
        let plain = group("post-create hooks", Some("worktree-post-create"), false);
        assert_eq!(
            plain,
            "\u{251c}\u{2500} post-create hooks  worktree-post-create"
        );
        let line = group("post-create hooks", Some("worktree-post-create"), true);
        assert!(
            line.contains(&format!("{GREY}worktree-post-create{}", styles::RESET)),
            "annotation sits in the scaffolding grey: {line:?}"
        );
        assert!(
            line.contains(&format!(
                "{}post-create hooks{}",
                styles::BOLD,
                styles::RESET
            )),
            "label stays a bold heading: {line:?}"
        );
    }

    #[test]
    fn group_anchor_empty_annotation_is_none() {
        assert_eq!(
            group("shared files", Some(""), false),
            "\u{251c}\u{2500} shared files"
        );
    }

    #[test]
    fn note_recedes_one_tier_not_two() {
        let line = note("no remote branch", true);
        assert!(line.starts_with(GREY), "got: {line:?}");
        assert!(!line.contains(DARK_GREY), "got: {line:?}");
    }

    #[test]
    fn gutter_tucks_a_row_inside_the_rail() {
        let row = final_row(
            &RowFace::Done { duration: None },
            ".env",
            None,
            4,
            PLAIN_INKS,
            false,
        );
        assert_eq!(gutter(&row, false), "\u{2502}  \u{2713}  .env");
    }

    #[test]
    fn gutter_rail_glyph_is_grey_body_untouched() {
        let line = gutter("\u{2713}  .env", true);
        assert!(line.starts_with(GREY), "got: {line}");
        assert!(line.ends_with("\u{2713}  .env"), "got: {line}");
    }

    #[test]
    fn attention_skip_is_yellow_glyph_plain_body() {
        let line = final_row(
            &RowFace::SkippedAttention,
            "post-create hooks",
            Some("skipped \u{2014} repo not trusted"),
            17,
            PLAIN_INKS,
            true,
        );
        assert!(line.starts_with(YELLOW));
        assert!(line.contains("repo not trusted"));
    }

    // ── hook-job rows: the outcome floods the name ───────────────────────

    #[test]
    fn hook_success_floods_the_name_green() {
        let face = HookJobFace::Done {
            duration: Some(Duration::from_millis(2100)),
        };
        let line = hook_job_row(&face, "build", None, 5, true);
        assert!(
            line.contains(&format!("{}build{}", styles::GREEN, styles::RESET)),
            "got: {line:?}"
        );
        assert!(line.contains("(2.1s)"));
        let plain = hook_job_row(&face, "build", None, 5, false);
        assert_eq!(plain, "\u{2713}  build  (2.1s)");
    }

    #[test]
    fn hook_failure_floods_the_name_red() {
        let line = hook_job_row(&HookJobFace::Failed, "build", None, 5, true);
        assert!(
            line.contains(&format!("{}build{}", styles::RED, styles::RESET)),
            "got: {line:?}"
        );
    }

    #[test]
    fn hook_skip_floods_the_name_yellow_reason_plain() {
        let line = hook_job_row(
            &HookJobFace::SkippedAttention,
            "lint",
            Some("skipped \u{2014} requested (--skip-hooks)"),
            4,
            true,
        );
        assert!(
            line.contains(&format!("{YELLOW}lint{}", styles::RESET)),
            "got: {line:?}"
        );
        assert!(
            line.contains(&format!("{}  skipped", styles::RESET)),
            "reason stays plain: {line:?}"
        );
    }

    #[test]
    fn hook_background_is_blue_name_dim_annotation() {
        let plain = hook_job_row(&HookJobFace::Background, "check-todos", None, 11, false);
        assert_eq!(plain, "\u{21bb}  check-todos  background");
        let line = hook_job_row(&HookJobFace::Background, "check-todos", None, 11, true);
        assert!(line.starts_with(BLUE), "got: {line:?}");
        assert!(
            line.contains(&format!("{BLUE}check-todos{}", styles::RESET)),
            "got: {line:?}"
        );
        assert!(
            line.contains(&format!("{DARK_GREY}background{}", styles::RESET)),
            "got: {line:?}"
        );
    }
}
