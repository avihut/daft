//! Terminal text styling utilities.
//!
//! Provides clean abstractions for ANSI terminal styling, keeping escape codes
//! isolated from application code.

/// ANSI escape code for bold text.
pub const BOLD: &str = "\x1b[1m";

/// ANSI escape code to reset all styling.
pub const RESET: &str = "\x1b[0m";

/// Wraps text in bold styling.
pub fn bold(text: &str) -> String {
    format!("{BOLD}{text}{RESET}")
}

/// Formats a definition list item with a bold term.
/// Matches clap's command list formatting (2-space indent, 9-char term width).
pub fn def(term: &str, description: &str) -> String {
    // Pad the term to 9 chars, then add the styled version
    let padding = " ".repeat(9_usize.saturating_sub(term.len()));
    format!("  {BOLD}{term}{RESET}{padding}{description}")
}
