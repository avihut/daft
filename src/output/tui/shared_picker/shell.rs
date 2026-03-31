//! Terminal setup and teardown helpers for the shared picker TUI.

use crossterm::{
    cursor,
    event::DisableMouseCapture,
    execute,
    terminal::{self, LeaveAlternateScreen},
};
use std::io;

/// Restore the terminal to its normal state.
pub fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show
    );
}
