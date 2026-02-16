//! Terminal text styling utilities.
//!
//! Provides clean abstractions for ANSI terminal styling, keeping escape codes
//! isolated from application code.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Whether colors are enabled for stdout (cached on first call).
pub fn colors_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none())
}

/// Whether colors are enabled for stderr (cached on first call).
pub fn colors_enabled_stderr() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED
        .get_or_init(|| std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
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

/// ANSI escape code for cyan text.
pub const CYAN: &str = "\x1b[36m";

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

/// Wraps text in cyan styling (good for commands/code).
pub fn cyan(text: &str) -> String {
    format!("{CYAN}{text}{RESET}")
}

/// Formats a definition list item with a bold term.
/// Matches clap's command list formatting (2-space indent, 9-char term width).
pub fn def(term: &str, description: &str) -> String {
    // Pad the term to 9 chars, then add the styled version
    let padding = " ".repeat(9_usize.saturating_sub(term.len()));
    format!("  {BOLD}{term}{RESET}{padding}{description}")
}
