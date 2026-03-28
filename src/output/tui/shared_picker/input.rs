//! Keyboard input handling for the shared picker TUI.
//!
//! The shell handles navigation keys (jk, hl, Tab, Esc->footer, PgUp/PgDn).
//! Unhandled keys are delegated to the active `PickerMode`.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::state::{FocusPanel, PickerState};
use super::{LoopAction, PickerMode};

/// Poll for a key event (blocks up to `timeout`).
pub fn poll_key(timeout: Duration) -> Option<KeyEvent> {
    if event::poll(timeout).ok()? {
        if let Event::Key(key) = event::read().ok()? {
            return Some(key);
        }
    }
    None
}

/// Handle a key event, routing navigation to the shell and actions to the mode.
pub fn handle_key(key: KeyEvent, state: &mut PickerState, mode: &mut dyn PickerMode) -> LoopAction {
    // Ctrl+C always exits
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return LoopAction::Exit;
    }

    // Let the mode intercept keys before shell navigation handling.
    if mode.pre_handle_key(key.code, state) {
        return LoopAction::Continue;
    }

    match state.focus {
        FocusPanel::TabBar => handle_tab_bar(key, state, mode),
        FocusPanel::WorktreeList => handle_worktree_list(key, state, mode),
        FocusPanel::Preview => handle_preview(key, state, mode),
        FocusPanel::Footer => mode.handle_footer_key(key.code, state),
    }
}

fn extra(mode: &dyn PickerMode) -> usize {
    mode.extra_tab_labels().len()
}

fn handle_tab_bar(key: KeyEvent, state: &mut PickerState, mode: &mut dyn PickerMode) -> LoopAction {
    match key.code {
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(extra(mode)),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(extra(mode)),
        KeyCode::Down | KeyCode::Char('j') => {
            if state.is_virtual_tab() {
                state.focus = FocusPanel::Footer;
            } else {
                let all = mode.all_entries_traversable(state.current_tab());
                state.move_down(all);
            }
        }
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        // On virtual tab, delegate Enter/Space/a to mode
        KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('a') if state.is_virtual_tab() => {
            return mode.handle_list_key(KeyCode::Char('a'), state);
        }
        _ => {}
    }
    LoopAction::Continue
}

fn handle_worktree_list(
    key: KeyEvent,
    state: &mut PickerState,
    mode: &mut dyn PickerMode,
) -> LoopAction {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            let all = mode.all_entries_traversable(state.current_tab());
            state.move_down(all);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let all = mode.all_entries_traversable(state.current_tab());
            state.move_up(all);
        }
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(extra(mode)),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(extra(mode)),
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        KeyCode::Tab | KeyCode::BackTab => state.toggle_panel(),
        _ => return mode.handle_list_key(key.code, state),
    }
    LoopAction::Continue
}

fn handle_preview(key: KeyEvent, state: &mut PickerState, mode: &dyn PickerMode) -> LoopAction {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => {
            let all = mode.all_entries_traversable(state.current_tab());
            state.move_down(all);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let all = mode.all_entries_traversable(state.current_tab());
            state.move_up(all);
        }
        KeyCode::PageDown => state.page_down(),
        KeyCode::PageUp => state.page_up(),
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(extra(mode)),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(extra(mode)),
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        KeyCode::Tab | KeyCode::BackTab => state.toggle_panel(),
        _ => {}
    }
    LoopAction::Continue
}
