//! Interactive single-keypress prompt for terminal input.
//!
//! Provides a reusable prompt that reads a single character without waiting
//! for Enter. Uses `console::Term` for cross-platform raw terminal handling.

/// A selectable option in the prompt.
pub struct PromptOption {
    /// Single character key (case-insensitive).
    pub key: char,
    /// Label printed after the prompt when selected.
    pub label: &'static str,
    /// Whether this is the default (selected by Enter/Esc).
    pub is_default: bool,
}

/// Configuration for the prompt.
pub struct PromptConfig {
    /// Available options.
    pub options: Vec<PromptOption>,
    /// Message printed when the user cancels with Ctrl+C.
    /// If None, just prints a newline and exits.
    pub cancel_message: Option<String>,
}

/// Result of a prompt interaction.
pub enum PromptResult {
    /// User selected an option (returns the key char, lowercased).
    Selected(char),
    /// User cancelled (Ctrl+C, Ctrl+D, or EOF).
    Cancelled,
}

/// Read a single keypress from an interactive terminal.
///
/// Switches the terminal to raw mode, reads one key, restores terminal
/// state, and returns the result. Enter and Esc select the default option.
/// Unrecognized keys are ignored.
///
/// For piped stdin (non-interactive, e.g. tests), reads a full line
/// and matches the first character.
pub fn single_key_select(config: &PromptConfig) -> PromptResult {
    if std::env::var("DAFT_TESTING").is_ok() {
        return read_from_pipe(config);
    }

    if !std::io::stdin().is_terminal() {
        return PromptResult::Cancelled;
    }

    read_from_terminal(config)
}

use std::io::IsTerminal;

/// Pipe/test fallback: read a line and match first char.
fn read_from_pipe(config: &PromptConfig) -> PromptResult {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => PromptResult::Cancelled,
        Ok(_) => {
            let first = line.trim().to_lowercase();
            let default_key = default_key(config);

            if first.is_empty() {
                return PromptResult::Selected(default_key);
            }

            for opt in &config.options {
                if first.starts_with(opt.key) || first == opt.label.to_lowercase() {
                    return PromptResult::Selected(opt.key);
                }
            }

            PromptResult::Selected(default_key)
        }
    }
}

/// Interactive terminal: read a single key using console::Term.
fn read_from_terminal(config: &PromptConfig) -> PromptResult {
    use console::{Key, Term};

    let term = Term::stderr();
    let valid_keys: Vec<char> = config
        .options
        .iter()
        .map(|o| o.key.to_ascii_lowercase())
        .collect();

    // Install Ctrl+C handler for clean exit with cancel message.
    // console::Term doesn't suppress ^C echo on its own.
    let cancel_msg = config.cancel_message.clone();
    let _ = ctrlc::set_handler(move || {
        let use_color = std::io::stderr().is_terminal() && std::env::var("NO_COLOR").is_err();
        eprintln!();
        if let Some(ref msg) = cancel_msg {
            if use_color {
                eprintln!("\x1b[2m{msg}\x1b[0m");
            } else {
                eprintln!("{msg}");
            }
        }
        std::process::exit(0);
    });

    loop {
        let key = match term.read_key() {
            Ok(k) => k,
            Err(_) => return PromptResult::Cancelled,
        };

        match key {
            Key::Char(c) => {
                let lower = c.to_ascii_lowercase();
                if valid_keys.contains(&lower) {
                    return PromptResult::Selected(lower);
                }
                // Ignore unrecognized characters
            }
            Key::Enter | Key::Escape => {
                return PromptResult::Selected(default_key(config));
            }
            _ => {} // Ignore arrow keys, etc.
        }
    }
}

fn default_key(config: &PromptConfig) -> char {
    config
        .options
        .iter()
        .find(|o| o.is_default)
        .map(|o| o.key)
        .unwrap_or('n')
}
