//! Width-normalization for child-process output shown in the live region.
//!
//! Every string a live-region `ProgressBar` displays must *render* exactly as
//! wide as `unicode-width` *measures* it. indicatif pads each drawn row to
//! exactly the terminal width (`draw_to_term`'s line filler) and computes rows
//! to move up / clear from the same measurement — so there is zero slack in
//! both directions. A glyph the terminal draws wider than measured wraps the
//! physical row and leaks a ghost copy of the region's top row on every redraw
//! tick; one drawn narrower under-fills the row, fuses the next line onto it,
//! and makes the redraw climb into scrollback (#751). Truncation cannot help:
//! the filler re-pads whatever space a clamp reserves. The only fix on daft's
//! side is normalizing the *content* so every width notion agrees.
//!
//! [`sanitize`] is that normalization, and it is a *live-render* concern:
//! applied at the seam where a buffered line becomes a bar `set_message`, never
//! at capture. Each job's output buffer holds the raw line — the source of
//! truth — and the live surfaces sanitize on the way to a bar: the rolling
//! window (`ThreadedJob::roll_window`), the succinct annotation
//! (`ThreadedJob::live_tail`), and the block hook renderer's rolling tail.
//! Permanent surfaces that `println` whole lines — receipt logs, the deferred
//! failure dump, the block renderer's scrollback echo — read the *raw* buffer
//! and keep full fidelity
//! (color, emoji, alignment); a `println`'d line wraps naturally and never
//! drives move-up math, so it cannot ghost. Don't route child text into a bar
//! around these seams; don't apply this to daft's own composed messages (they
//! carry intentional ANSI styling).
//!
//! Residual, terminal-dependent and unfixable from here: a base glyph some
//! terminals draw in emoji presentation by default even absent VS16 (e.g.
//! U+2764), and East-Asian-Ambiguous glyphs on terminals configured
//! `ambiguous = wide`, still measure narrower than they draw. There is no
//! client-side width notion that captures either.

use crate::output::format::strip_ansi;
use console::measure_text_width;

/// Normalize one captured child-output line for the live region.
///
/// - **ANSI escapes** are stripped (via [`strip_ansi`], whose scanner also
///   swallows OSC-8 hyperlink payloads — `console::strip_ansi_codes` leaks
///   them): a chatty command's color codes are merely unmeasured, but its
///   cursor moves and erases would rewrite the region.
/// - **`\r` rewrites** keep only the final segment — what the terminal would
///   have shown after a progress line repainted itself.
/// - **`\t` becomes one space** (the terminal advances to a tab stop the
///   width math cannot see); remaining control characters drop.
/// - **Variation selectors drop**: `✔\u{FE0F}` measures 1 column but VS16
///   forces emoji presentation, drawn 2 cells — the exact lefthook glyph from
///   the #751 field reports. Bare `✔` measures and draws 1 everywhere.
/// - **ZWJ/skin-tone/keycap sequences reduce to their first scalar**: the
///   parts measure individually (`👨‍👩‍👧` = 6) but modern terminals draw one
///   glyph (2), under-filling the row. The first scalar alone agrees with
///   both joining and non-joining terminals.
pub(crate) fn sanitize(raw: &str) -> String {
    let stripped = strip_ansi(raw.trim_end());
    let seg = stripped.rsplit('\r').next().unwrap_or(&stripped);
    let mut out = scrub(seg);
    out.truncate(out.trim_end().len());
    out
}

/// Normalize one **fixed row label** for the rail.
///
/// Labels are where arbitrary user text becomes rail furniture: a `-x`
/// command is reproduced exactly as typed (#812), and a shared file's path
/// is whatever the filesystem allows — which on Unix includes newlines.
///
/// This deliberately does **not** share [`sanitize`]'s `\r` rule. A label is
/// not a repainting progress line, so keeping only the last carriage-returned
/// segment would silently delete the front of it. Every whitespace run —
/// newlines included — collapses to a single space instead, so a pasted
/// multi-line snippet stays one row.
///
/// Getting this wrong costs more than looks: the region measures its shared
/// annotation column from the *rendered* label, so an embedded newline
/// mis-sizes every row's padding as well as splitting the bar's own line —
/// the #751 failure mode, reached through a different door.
pub(crate) fn sanitize_label(raw: &str) -> String {
    // Whitespace is flattened before scrubbing, not after: `scrub` drops
    // control characters outright, which would weld the text on either side
    // of a dropped newline into one word (`echo $f` + `done` → `$fdone`).
    let flattened: String = strip_ansi(raw)
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    scrub(&flattened)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The shared character scrub: tabs to spaces, control characters and
/// width-lying combining sequences dropped. Both entry points above run
/// their own line discipline first, then hand the result here.
fn scrub(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    let mut chars = seg.chars();
    while let Some(c) = chars.next() {
        match c {
            '\t' => out.push(' '),
            // Variation selectors (VS1–VS16), skin-tone modifiers, keycap.
            '\u{FE00}'..='\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{20E3}' => {}
            '\u{200D}' => {
                // A ZWJ *between two wide (emoji) scalars* joins them into one
                // drawn glyph — drop the joiner and the following scalar,
                // keeping the first scalar's honest width. After a narrow
                // scalar, or before one (complex-script shaping, `語\u{200D}A`),
                // the joiner is invisible and width-neutral: drop it alone and
                // keep the next character. (Known rare false positive: a ZWJ
                // between two CJK ideographs elides the second — accept it;
                // widening the gate re-opens the wide+ZWJ+text drop this
                // guards against.)
                let prev_wide = out.chars().next_back().is_some_and(is_wide);
                let next_wide = chars.clone().next().is_some_and(is_wide);
                if prev_wide && next_wide {
                    chars.next();
                }
            }
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Whether `c` measures two columns — the emoji-join signal used to tell a
/// composing ZWJ (between two wide scalars) from an invisible shaping joiner.
fn is_wide(c: char) -> bool {
    measure_text_width(c.encode_utf8(&mut [0u8; 4])) == 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_ascii_passes_unchanged() {
        assert_eq!(sanitize("Compiling daft v1.23.0"), "Compiling daft v1.23.0");
        assert_eq!(sanitize(""), "");
    }

    #[test]
    fn vs16_emoji_presentation_drops_to_text_presentation() {
        // The #751 field-report glyph: lefthook's `✔️` (U+2714 + VS16)
        // measures 1 column but draws 2, wrapping a filler-padded row.
        assert_eq!(
            sanitize("sync hooks: \u{2714}\u{FE0F} (pre-push, pre-commit)"),
            "sync hooks: \u{2714} (pre-push, pre-commit)"
        );
        assert_eq!(
            sanitize("\u{2714}\u{FE0F} unit tests (related) (33.07 seconds)"),
            "\u{2714} unit tests (related) (33.07 seconds)"
        );
        // VS15 (text presentation selector) is width-neutral; dropped for
        // symmetry.
        assert_eq!(sanitize("\u{2714}\u{FE0E} ok"), "\u{2714} ok");
        // Bare `✔` measures what it draws.
        assert_eq!(measure_text_width(&sanitize("\u{2714}\u{FE0F}")), 1);
    }

    #[test]
    fn cr_rewrites_keep_the_final_segment() {
        // A cargo/npm-style progress line: ANSI color + `\r` repaint. Only the
        // final segment survives, control-free — nothing that could return
        // the cursor or inject escapes into a rail row.
        assert_eq!(
            sanitize("\x1b[32mBuilding [==>  ] 40%\rBuilding [====] 100%\x1b[0m"),
            "Building [====] 100%"
        );
        assert_eq!(sanitize("gone\rkept"), "kept");
        // A trailing `\r` (or `\r\n` residue) is line termination, not a
        // rewrite — it must not blank the line.
        assert_eq!(sanitize("12%\r"), "12%");
    }

    #[test]
    fn controls_scrub_and_tabs_become_spaces() {
        assert_eq!(sanitize("tab\there"), "tab here");
        assert_eq!(sanitize("a\u{7}b\u{8}c"), "abc");
        // C1 controls are skipped by indicatif's width math but not by
        // terminals.
        assert_eq!(sanitize("a\u{85}b"), "ab");
        assert_eq!(sanitize("done  "), "done");
    }

    #[test]
    fn ansi_strips_including_osc_hyperlinks() {
        assert_eq!(sanitize("\x1b[38;5;208mwarn\x1b[0m"), "warn");
        assert_eq!(
            sanitize("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"),
            "link"
        );
        assert_eq!(sanitize("\x1b[2K"), "");
    }

    #[test]
    fn zwj_sequences_reduce_to_their_first_scalar() {
        // Measured 2 (first scalar) == drawn 2, whether the terminal joins
        // the sequence or not.
        assert_eq!(
            sanitize("\u{1F9D1}\u{200D}\u{1F4BB} tests"),
            "\u{1F9D1} tests"
        );
        assert_eq!(
            sanitize("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            "\u{1F468}"
        );
        // After a narrow scalar the joiner is shaping, not emoji composition:
        // drop the invisible joiner alone, keep the text.
        assert_eq!(sanitize("x\u{200D}y"), "xy");
        // A wide scalar followed by a ZWJ + *narrow* text is shaping too, not
        // an emoji join: the joiner drops but the text must survive. Keying the
        // elision on the previous glyph's width alone would eat the "A".
        assert_eq!(sanitize("\u{8A9E}\u{200D}A"), "\u{8A9E}A");
        // A ZWJ trailing with nothing after it drops alone (no scalar to eat).
        assert_eq!(sanitize("\u{1F468}\u{200D}"), "\u{1F468}");
    }

    #[test]
    fn skin_tone_and_keycap_marks_drop() {
        assert_eq!(
            sanitize("\u{1F44D}\u{1F3FD} approved"),
            "\u{1F44D} approved"
        );
        assert_eq!(sanitize("1\u{FE0F}\u{20E3} first"), "1 first");
    }

    #[test]
    fn wide_and_rail_glyphs_pass_unchanged() {
        // CJK measures 2 and draws 2 — already honest, untouched.
        assert_eq!(sanitize("日本語のログ"), "日本語のログ");
        // daft's own vocabulary must survive a round trip.
        assert_eq!(
            sanitize("\u{2713} \u{2502} \u{2514} \u{283c} \u{276f} \u{2726}"),
            "\u{2713} \u{2502} \u{2514} \u{283c} \u{276f} \u{2726}"
        );
    }

    #[test]
    fn an_ordinary_label_passes_through_untouched() {
        // Every label the rail carried before `-x`: commands, paths, branches.
        assert_eq!(sanitize_label("npm ci"), "npm ci");
        assert_eq!(sanitize_label(".env.local"), ".env.local");
        assert_eq!(sanitize_label("feat/rows-on-rail"), "feat/rows-on-rail");
        assert_eq!(sanitize_label(""), "");
    }

    #[test]
    fn a_multi_line_label_becomes_one_row() {
        // A pasted shell snippet is an ordinary thing to hand `-x`. Each
        // newline has to leave a word boundary behind: dropping it outright
        // would render `echo $f` + `done` as `$fdone`.
        assert_eq!(
            sanitize_label("for f in *; do\n  echo $f\ndone"),
            "for f in *; do echo $f done"
        );
        assert_eq!(sanitize_label("a\r\nb"), "a b");
        assert_eq!(sanitize_label("wrapped\\\n  --flag"), "wrapped\\ --flag");
    }

    #[test]
    fn a_label_never_loses_its_front_to_a_carriage_return() {
        // `sanitize` keeps only the last `\r` segment — right for a
        // repainting progress line, silent data loss for a label.
        assert_eq!(sanitize("kept\rlast"), "last");
        assert_eq!(sanitize_label("kept\rlast"), "kept last");
    }

    #[test]
    fn a_label_cannot_smuggle_escapes_or_width_lies_onto_the_rail() {
        assert_eq!(sanitize_label("\x1b[31mmake\x1b[0m test"), "make test");
        // Same VS16 lie as a captured line — a label is measured for the
        // shared annotation column, so it has to measure what it draws.
        assert_eq!(sanitize_label("deploy \u{2714}\u{FE0F}"), "deploy \u{2714}");
        assert_eq!(sanitize_label("tabs\tand   runs"), "tabs and runs");
        // Leading and trailing whitespace is padding, not content.
        assert_eq!(sanitize_label("  make  "), "make");
    }
}
