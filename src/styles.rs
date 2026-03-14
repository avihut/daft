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

/// ANSI escape code for dark gray text (bright black).
pub const DARK_GRAY: &str = "\x1b[90m";

/// Symbol for the current worktree indicator.
pub const CURRENT_WORKTREE_SYMBOL: &str = ">";

/// Symbol for the default branch indicator.
pub const DEFAULT_BRANCH_SYMBOL: &str = "\u{25c9}";

/// Wraps text in bold styling.
pub fn bold(text: &str) -> String {
    format!("{BOLD}{text}{RESET}")
}

/// Wraps text in dim styling.
pub fn dim(text: &str) -> String {
    format!("{DIM}{text}{RESET}")
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

/// Wraps text in dark gray styling (bright black).
pub fn dark_gray(text: &str) -> String {
    format!("{DARK_GRAY}{text}{RESET}")
}

/// Formats a definition list item with a bold term.
/// Matches clap's command list formatting (2-space indent, 9-char term width).
pub fn def(term: &str, description: &str) -> String {
    // Pad the term to 9 chars, then add the styled version
    let padding = " ".repeat(9_usize.saturating_sub(term.len()));
    format!("  {BOLD}{term}{RESET}{padding}{description}")
}
