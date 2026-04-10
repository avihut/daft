//! Shared pager helper for commands that produce long scrollable output.
//!
//! The pager respects the user's `$PAGER` environment variable and falls back
//! to `less` when unset. If the pager cannot be spawned, text is written
//! directly to stdout so no output is ever lost.

use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

/// Display `text` through a pager when stdout is a terminal.
///
/// Respects `$PAGER` (falling back to `less`). Passes `-R` so ANSI color
/// escapes are rendered. When stdout is not a TTY, or when the pager cannot
/// be spawned, `text` is written directly to stdout as a fallback.
pub fn display_with_pager(text: &str) {
    if !io::stdout().is_terminal() {
        print_fallback(text);
        return;
    }

    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());

    let result = Command::new("sh")
        .args(["-c", &format!("{pager} -R")])
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()
        });

    if result.is_err() {
        print_fallback(text);
    }
}

fn print_fallback(text: &str) {
    let _ = io::stdout().write_all(text.as_bytes());
    let _ = io::stdout().flush();
}
