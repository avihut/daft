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

/// Whether a prompt is parked in `term.read_key()` right now, plus its
/// cancel message. `ctrlc` accepts exactly one handler per process, so
/// when a command with a long-lived handler (sync's two-stage cancel)
/// installed first, this module's `set_handler` below is a silent no-op
/// — and an escalate-a-flag handler cannot unblock a thread stuck in a
/// blocking terminal read; the process would hang until a stray
/// keypress. Such handlers call [`exit_if_prompt_active`] first, which
/// takes over the prompt's exact cancel contract.
static PROMPT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PROMPT_CANCEL_MSG: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// RAII marker for the interactive read window; `Drop` disarms so every
/// return path (selection, read error) clears the takeover state.
struct PromptActiveGuard;

impl PromptActiveGuard {
    fn arm(cancel_message: Option<String>) -> Self {
        if let Ok(mut msg) = PROMPT_CANCEL_MSG.lock() {
            *msg = cancel_message;
        }
        PROMPT_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}

impl Drop for PromptActiveGuard {
    fn drop(&mut self) {
        PROMPT_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut msg) = PROMPT_CANCEL_MSG.lock() {
            *msg = None;
        }
    }
}

/// Cancel-exit for a signalled prompt: newline, the active prompt's
/// cancel message (if any), then exit 130 — the interrupted-by-signal
/// convention. With ctrlc's `termination` feature this also covers
/// SIGTERM, and a killed prompt must not read as success to calling
/// scripts.
///
/// `pub(crate)`: the timeline's [`suspend_for_prompt`] arms this as the
/// interrupt behavior for the whole prompt window, not just the key read.
///
/// [`suspend_for_prompt`]: crate::output::timeline::TimelineHandle::suspend_for_prompt
pub(crate) fn exit_for_cancelled_prompt() -> ! {
    use std::io::IsTerminal;
    // A prompt can fire under the timeline's live region, whose EchoCtlGuard
    // turned `^C` echo off for the region's lifetime. process::exit skips
    // the guard's drop, so restore by hand — exactly like the region's own
    // interrupt arm does with its saved copy.
    crate::output::term_guard::restore_active_termios();
    // swap(false), not load: when two exit paths race (the dispatcher thread
    // and the in-band takeover in `read_from_terminal`), only the first
    // prints the cancel line.
    let was_active = PROMPT_ACTIVE.swap(false, std::sync::atomic::Ordering::SeqCst);
    let msg = PROMPT_CANCEL_MSG
        .lock()
        .ok()
        .and_then(|m| m.clone())
        .filter(|_| was_active);
    let use_color = std::io::stderr().is_terminal() && std::env::var("NO_COLOR").is_err();
    eprintln!();
    if let Some(msg) = msg {
        if use_color {
            eprintln!("\x1b[2m{msg}\x1b[0m");
        } else {
            eprintln!("{msg}");
        }
    }
    std::process::exit(130);
}

/// For process-global ctrlc handlers owned by commands (sync): if an
/// interactive prompt is currently reading the terminal, print its
/// cancel message and exit 130 — never returns in that case. The
/// prompt's own `set_handler` lost the one-per-process race to the
/// command's handler, so the command must honor the prompt contract on
/// its behalf; without this, escalating a cancel flag leaves the main
/// thread wedged in `term.read_key()` until a stray keypress.
pub fn exit_if_prompt_active() {
    if PROMPT_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
        exit_for_cancelled_prompt();
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

    // Arm the process-wide takeover state (#672): store this prompt's cancel
    // message and mark a prompt parked in read_key, so a command that already
    // owns the ctrlc handler (sync's two-stage cancel) honors the cancel
    // contract via `exit_if_prompt_active` — and so `exit_for_cancelled_prompt`
    // has the message to print no matter which handler fires. Drop disarms on
    // every return path.
    let _guard = PromptActiveGuard::arm(config.cancel_message.clone());
    // Route this prompt's own Ctrl+C through the process-global dispatcher
    // (`crate::interrupt`, #651) rather than a bare `ctrlc::set_handler`.
    // Swap-and-restore, not set-and-clear: a prompt can fire while the
    // plan-execute timeline's planning face is live (remove's consolidation
    // prompts run before the plan commits), and clearing the slot on resolve
    // would strand that region without its Ctrl-C collapse. Both
    // handler-ownership paths converge on `exit_for_cancelled_prompt`
    // (cancel message, exit 130), matching the timeline region's own Ctrl+C.
    let outer = crate::interrupt::swap_behavior(|| exit_for_cancelled_prompt());

    let result = loop {
        let key = match term.read_key() {
            Ok(k) => k,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::Interrupted {
                    // Ctrl-C: console restored its raw-mode termios and
                    // re-raised SIGINT *before* handing back this error, so
                    // the dispatcher thread is about to run whatever sits in
                    // the slot. Racing it to `restore_behavior` below would
                    // hand the outer behavior (the region's collapse, or the
                    // bare default — neither prints this prompt's cancel
                    // line) the exit that belongs here. Take the slot back
                    // and run the cancel exit on this thread; `None` means
                    // the dispatcher already owns the exit — park and let
                    // it finish.
                    if let Some(behavior) = crate::interrupt::take_behavior() {
                        behavior();
                    }
                    loop {
                        std::thread::park();
                    }
                }
                break PromptResult::Cancelled;
            }
        };

        match key {
            Key::Char(c) => {
                let lower = c.to_ascii_lowercase();
                if valid_keys.contains(&lower) {
                    break PromptResult::Selected(lower);
                }
                // Ignore unrecognized characters
            }
            Key::Enter | Key::Escape => {
                break PromptResult::Selected(default_key(config));
            }
            _ => {} // Ignore arrow keys, etc.
        }
    };
    crate::interrupt::restore_behavior(outer);
    result
}

fn default_key(config: &PromptConfig) -> char {
    config
        .options
        .iter()
        .find(|o| o.is_default)
        .map(|o| o.key)
        .unwrap_or('n')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The takeover state must track the interactive read window
    /// exactly: armed with the prompt's message while reading, fully
    /// cleared on every exit path (Drop), and `exit_if_prompt_active`
    /// must be a no-op the rest of the time — sync's ctrlc handler
    /// calls it on every signal.
    #[test]
    fn prompt_active_guard_arms_and_disarms() {
        use std::sync::atomic::Ordering;

        assert!(!PROMPT_ACTIVE.load(Ordering::SeqCst));
        {
            let _g = PromptActiveGuard::arm(Some("Prune cancelled.".into()));
            assert!(PROMPT_ACTIVE.load(Ordering::SeqCst));
            assert_eq!(
                PROMPT_CANCEL_MSG.lock().unwrap().as_deref(),
                Some("Prune cancelled.")
            );
        }
        assert!(!PROMPT_ACTIVE.load(Ordering::SeqCst));
        assert!(PROMPT_CANCEL_MSG.lock().unwrap().is_none());
        // With no prompt active this must return instead of exiting.
        exit_if_prompt_active();
    }
}
