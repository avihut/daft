//! Keyboard input handling for the collect picker TUI.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::state::{CollectPickerState, FocusPanel, FooterButton};

/// Result of processing a key event.
pub enum InputResult {
    /// Continue the event loop.
    Continue,
    /// User requested cancel (Esc/q).
    Cancel,
    /// User activated Submit from the footer.
    Submit,
}

/// Poll for a key event (blocks up to `timeout`).
pub fn poll_key(timeout: Duration) -> Option<KeyEvent> {
    if event::poll(timeout).ok()? {
        if let Event::Key(key) = event::read().ok()? {
            return Some(key);
        }
    }
    None
}

/// Handle a key event and update state. Returns what the main loop should do.
pub fn handle_key(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    // Ctrl+C always cancels
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return InputResult::Cancel;
    }

    match state.focus {
        FocusPanel::TabBar => handle_tab_bar(key, state),
        FocusPanel::WorktreeList => handle_worktree_list(key, state),
        FocusPanel::Preview => handle_preview(key, state),
        FocusPanel::Footer => handle_footer(key, state),
    }
}

fn handle_tab_bar(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(),
        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        _ => {}
    }
    InputResult::Continue
}

fn handle_worktree_list(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
        KeyCode::Up | KeyCode::Char('k') => state.move_up(),
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(),
        KeyCode::Char(' ') | KeyCode::Enter => state.toggle_selection(),
        KeyCode::Char('m') => state.toggle_materialized(),
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        KeyCode::Tab => state.toggle_panel(),
        _ => {}
    }
    InputResult::Continue
}

fn handle_preview(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
        KeyCode::Up | KeyCode::Char('k') => state.move_up(),
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(),
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        KeyCode::Tab => state.toggle_panel(),
        _ => {}
    }
    InputResult::Continue
}

fn handle_footer(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => state.move_up(),
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
            state.footer_next();
        }
        KeyCode::Tab => state.toggle_panel(),
        KeyCode::Char('q') | KeyCode::Esc => return InputResult::Cancel,
        KeyCode::Enter | KeyCode::Char(' ') => {
            state.activate_footer();
            match state.footer_cursor {
                FooterButton::Submit if state.submitted => return InputResult::Submit,
                FooterButton::Cancel if state.cancelled => return InputResult::Cancel,
                _ => {}
            }
        }
        _ => {}
    }
    InputResult::Continue
}
