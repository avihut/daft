//! Terminal text styling utilities.
//!
//! Provides clean abstractions for ANSI terminal styling, keeping escape codes
//! isolated from application code.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Whether colors are enabled for stdout (cached on first call).
///
/// Checks (in order):
/// 1. `NO_COLOR` set → disabled
/// 2. `CLICOLOR_FORCE` set and non-zero → enabled (even when piped)
/// 3. stdout is a TTY → enabled
pub fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if is_env_force_color() {
            return true;
        }
        std::io::stdout().is_terminal()
    })
}

/// Whether colors are enabled for stderr (cached on first call).
///
/// Same precedence as [`colors_enabled`] but checks stderr TTY status.
pub fn colors_enabled_stderr() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        if is_env_force_color() {
            return true;
        }
        std::io::stderr().is_terminal()
    })
}

/// Check if `CLICOLOR_FORCE` is set to a non-zero value.
fn is_env_force_color() -> bool {
    std::env::var("CLICOLOR_FORCE")
        .map(|v| v != "0")
        .unwrap_or(false)
}

/// ANSI escape code for bold text.
pub const BOLD: &str = "\x1b[1m";

/// ANSI escape code to reset all styling.
pub const RESET: &str = "\x1b[0m";

/// ANSI escape code for dim/faint text.
pub const DIM: &str = "\x1b[2m";

/// ANSI escape code for green text.
pub const GREEN: &str = "\x1b[32m";

/// ANSI escape code for yellow text.
pub const YELLOW: &str = "\x1b[33m";

/// ANSI escape code for red text.
pub const RED: &str = "\x1b[31m";

/// ANSI escape code for bright blue text.
pub const BLUE: &str = "\x1b[94m";

/// ANSI escape code for cyan text.
pub const CYAN: &str = "\x1b[36m";

/// Accent/brand color index for the 256-color palette.
/// Use with ratatui: `Color::Indexed(ACCENT_COLOR_INDEX)`.
pub const ACCENT_COLOR_INDEX: u8 = 208;

/// ANSI escape code for orange text (256-color, matches [`ACCENT_COLOR_INDEX`]).
pub const ORANGE: &str = "\x1b[38;5;208m";

/// ANSI escape code for bright purple text (ratatui equivalent: `Color::LightMagenta`).
pub const BRIGHT_PURPLE: &str = "\x1b[95m";

/// ANSI escape code for dark gray text (bright black).
pub const DARK_GRAY: &str = "\x1b[90m";

/// 256-color palette index for the current-row background highlight.
/// Use with ratatui: `Color::Indexed(CURRENT_ROW_BG_INDEX)`.
pub const CURRENT_ROW_BG_INDEX: u8 = 235;

/// ANSI escape code for the current-row background (256-color, matches
/// [`CURRENT_ROW_BG_INDEX`]).
pub const CURRENT_ROW_BG: &str = "\x1b[48;5;235m";

/// Symbol for the current worktree indicator.
pub const CURRENT_WORKTREE_SYMBOL: &str = ">";

/// Symbol for the default branch indicator.
pub const DEFAULT_BRANCH_SYMBOL: &str = "\u{2726}";

/// Symbol for sandbox (detached HEAD) worktrees.
pub const SANDBOX_SYMBOL: &str = "\u{25cb}";

/// Symbols for a git operation paused in a worktree, shown in the annotation
/// column's state slot. One glyph per operation, so the shape alone says which
/// — the `status` column spells it out for anyone who wants words.
///
/// A worktree can only be in one of these at a time, and each is a state the
/// user has to finish or abort, so they share a slot and a color.
pub mod op_symbols {
    /// Rebase — a cycle, for commits being replayed.
    pub const REBASE: &str = "\u{27f2}";
    /// `git am` — a mailbox apply.
    pub const AM: &str = "\u{2709}";
    /// Merge — two histories joining.
    pub const MERGE: &str = "\u{21c4}";
    /// Cherry-pick — a commit lifted onto this branch.
    pub const CHERRY_PICK: &str = "\u{2937}";
    /// Revert — an undo.
    pub const REVERT: &str = "\u{238c}";
    /// Bisect — a search halving the range.
    pub const BISECT: &str = "\u{25d0}";
}

/// Wraps text in bold styling.
pub fn bold(text: &str) -> String {
    format!("{BOLD}{text}{RESET}")
}

/// ANSI escape code for underlined text.
pub const UNDERLINE: &str = "\x1b[4m";

/// Wraps text in dim styling.
pub fn dim(text: &str) -> String {
    format!("{DIM}{text}{RESET}")
}

/// Wraps text in dim + underlined styling.
pub fn dim_underline(text: &str) -> String {
    format!("{DIM}{UNDERLINE}{text}{RESET}")
}

/// Wraps text in an OSC 8 terminal hyperlink to `url` (ST-terminated).
/// Supporting terminals (iTerm2, Kitty, WezTerm, recent GNOME/Windows
/// Terminal) make the text clickable; others ignore the sequence and render
/// the text unchanged. The caller is responsible for `url` being
/// control-character-free — a stray ESC/BEL would terminate the payload
/// early (daft's forge cache sanitizes URLs at the persistence boundary).
pub fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

/// Paints one fully-rendered table line with the current-row background —
/// the blocking-table counterpart of a ratatui row styled with
/// `Color::Indexed(CURRENT_ROW_BG_INDEX)`.
///
/// Styled spans produced by this crate end in [`RESET`], which clears
/// backgrounds along with everything else, so the background is re-asserted
/// after every reset. That keeps the highlight continuous across styled
/// cells and the padding between them.
pub fn paint_current_row(line: &str) -> String {
    let reassert = format!("{RESET}{CURRENT_ROW_BG}");
    format!("{CURRENT_ROW_BG}{}{RESET}", line.replace(RESET, &reassert))
}

/// Wraps text in bold + red styling. Used for primary failure markers
/// (`✗`, `❯`, `FAIL`, failed-scenario footers) per the test runner reporter's
/// design language (`xtask/src/manual_test/reporter/CLAUDE.md`).
pub fn bold_red(text: &str) -> String {
    format!("{BOLD}{RED}{text}{RESET}")
}

/// Wraps text in bold + green styling. Used for primary pass markers
/// (`✓` icons, passed-scenario footers).
pub fn bold_green(text: &str) -> String {
    format!("{BOLD}{GREEN}{text}{RESET}")
}

/// Wraps text in bold + cyan styling. Used for section-heading primary
/// content (scenario name at the top of a scenario block).
pub fn bold_cyan(text: &str) -> String {
    format!("{BOLD}{CYAN}{text}{RESET}")
}

/// Wraps text in green styling.
pub fn green(text: &str) -> String {
    format!("{GREEN}{text}{RESET}")
}

/// Wraps text in yellow styling.
pub fn yellow(text: &str) -> String {
    format!("{YELLOW}{text}{RESET}")
}

/// Wraps text in red styling.
pub fn red(text: &str) -> String {
    format!("{RED}{text}{RESET}")
}

/// Wraps text in blue styling.
pub fn blue(text: &str) -> String {
    format!("{BLUE}{text}{RESET}")
}

/// Wraps text in cyan styling (good for commands/code).
pub fn cyan(text: &str) -> String {
    format!("{CYAN}{text}{RESET}")
}

/// Wraps text in orange styling (brand color).
pub fn orange(text: &str) -> String {
    format!("{ORANGE}{text}{RESET}")
}

/// Wraps text in bright purple styling.
pub fn bright_purple(text: &str) -> String {
    format!("{BRIGHT_PURPLE}{text}{RESET}")
}

/// Wraps text in bold + bright purple styling. Used by the test runner
/// reporter for the step-name Level-2 anchor at `-vv` (`pretty.rs`).
pub fn bold_bright_purple(text: &str) -> String {
    format!("{BOLD}{BRIGHT_PURPLE}{text}{RESET}")
}

/// Wraps text in dark gray styling (bright black).
pub fn dark_gray(text: &str) -> String {
    format!("{DARK_GRAY}{text}{RESET}")
}

/// Formats a name with the default branch marker (`✦`) prepended when `is_default` is true.
///
/// Used by sync/prune non-TUI output to visually mark the default branch,
/// matching the annotation column in `list`.
pub fn format_with_default_marker(name: &str, is_default: bool) -> String {
    if is_default {
        if colors_enabled() {
            format!("{} {name}", bright_purple(DEFAULT_BRANCH_SYMBOL))
        } else {
            format!("{DEFAULT_BRANCH_SYMBOL} {name}")
        }
    } else {
        name.to_string()
    }
}

/// Formats a definition list item with a bold term.
/// Matches clap's command list formatting (2-space indent, 9-char term width).
pub fn def(term: &str, description: &str) -> String {
    // Pad the term to 9 chars, then add the styled version
    let padding = " ".repeat(9_usize.saturating_sub(term.len()));
    format!("  {BOLD}{term}{RESET}{padding}{description}")
}

// ── Syntax highlighting palette ──────────────────────────────────────────

/// Semantic color roles for syntax highlighting.
///
/// Maps abstract roles to ANSI escape sequences so all syntax highlighters
/// in the CLI share a consistent palette. Each field is a raw ANSI code
/// (e.g., `"\x1b[36m"`); callers must append [`RESET`] after the styled span.
///
/// Use [`SYNTAX`] for the shared palette instance.
pub struct SyntaxPalette {
    /// Structural delimiters and keywords: `{{ }}`, top-level YAML keys.
    pub keyword: &'static str,
    /// Data references and names: variable names, YAML sub-keys.
    pub identifier: &'static str,
    /// Value-producing tokens: quoted strings, filter/function names.
    pub string: &'static str,
    /// Constant values: booleans, numbers, null.
    pub literal: &'static str,
    /// Low-emphasis structural markers: list dashes, pipe operators.
    pub punctuation: &'static str,
    /// Emphasis modifier for headings/special names (bold, not a color).
    pub heading: &'static str,
}

/// Shared syntax highlighting palette.
pub const SYNTAX: SyntaxPalette = SyntaxPalette {
    keyword: YELLOW,
    identifier: CYAN,
    string: GREEN,
    literal: YELLOW,
    punctuation: DIM,
    heading: BOLD,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_current_row_wraps_a_plain_line() {
        let painted = paint_current_row("  alpha  1  ~/src/alpha");
        assert!(painted.starts_with(CURRENT_ROW_BG));
        assert!(painted.ends_with(RESET));
        assert!(painted.contains("alpha"));
    }

    #[test]
    fn paint_current_row_reasserts_background_after_each_reset() {
        // A line with dim and cyan spans, as blocking tables emit them.
        let line = format!("{} {} {}", cyan(">"), "alpha", dim("~/src/alpha"));
        let painted = paint_current_row(&line);
        // Every RESET inside the line is followed by a background re-assert,
        // so the highlight never drops mid-line...
        let reassert = format!("{RESET}{CURRENT_ROW_BG}");
        assert_eq!(
            painted.matches(&reassert).count(),
            2,
            "one re-assert per styled span: {painted:?}"
        );
        // ...and the line still terminates with a bare reset.
        assert!(painted.ends_with(RESET));
        assert!(!painted.ends_with(&reassert));
    }

    #[test]
    fn current_row_bg_escape_matches_the_palette_index() {
        assert!(CURRENT_ROW_BG.contains(&format!("48;5;{CURRENT_ROW_BG_INDEX}")));
    }
}
