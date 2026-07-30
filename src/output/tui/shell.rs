//! Terminal teardown, shared by every full-screen TUI daft draws.
//!
//! Hoisted out of `shared_picker/` when the settings screen needed it: the
//! cleanup sequence has to be one place, because every takeover screen has to
//! reach it from three paths — normal exit, error, and the panic hook. daft
//! ships `panic = "abort"`, so a Drop guard does not run and the hook is the
//! only thing that restores a terminal left in raw mode.

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
