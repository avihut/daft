//! Key dispatch. Turns a keystroke into a state change, and nothing else.
//!
//! Separated from the event loop so every binding is testable without a
//! terminal: the loop's job is to read events and draw, and this decides what
//! they mean.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::state::ScreenState;

/// What the event loop should do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Redraw and keep going.
    Continue,
    /// Leave the screen.
    Quit,
}

/// Apply a keystroke.
///
/// `page` is how many rows fit, so the paging keys move by a screenful of the
/// terminal the user actually has.
pub fn handle_key(state: &mut ScreenState, key: KeyEvent, page: usize) -> Action {
    // Key *release* events arrive on Windows and on terminals with the
    // kitty protocol; acting on both halves double-moves the cursor.
    if key.kind == KeyEventKind::Release {
        return Action::Continue;
    }

    // Anything typed while the filter prompt is open is filter text, except
    // the two keys that close it. Checking this first is what lets a setting
    // be found by typing "sync" without `s` flipping the write scope.
    if state.is_prompt_open()
        && let Some(action) = filter_key(state, key)
    {
        return action;
    }

    // A fresh keystroke supersedes whatever the last one reported.
    state.status = None;

    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Action::Quit,
        (KeyCode::Char('q'), _) => Action::Quit,
        // Esc unwinds one layer at a time: a narrowed list first, the screen
        // second. Quitting straight out of a filtered view would throw away
        // context the user is still using.
        (KeyCode::Esc, _) => {
            if state.is_filtering() {
                state.clear_filter();
                Action::Continue
            } else {
                Action::Quit
            }
        }

        (KeyCode::Down, _) | (KeyCode::Char('j'), _) => {
            state.move_down();
            Action::Continue
        }
        (KeyCode::Up, _) | (KeyCode::Char('k'), _) => {
            state.move_up();
            Action::Continue
        }
        // h/l cross between the panes rather than walking anything: the rail
        // is to the left of the list, so left goes to it and right comes back.
        (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
            if state.rail_visible {
                state.focus_rail();
            }
            Action::Continue
        }
        (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
            state.focus_list();
            Action::Continue
        }
        (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
            state.toggle_focus();
            Action::Continue
        }

        (KeyCode::Char('g'), _) | (KeyCode::Home, _) => {
            state.move_to_top();
            Action::Continue
        }
        (KeyCode::Char('G'), _) | (KeyCode::End, _) => {
            state.move_to_bottom();
            Action::Continue
        }
        (KeyCode::PageDown, _) | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
            state.page(1, page);
            Action::Continue
        }
        (KeyCode::PageUp, _) | (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
            state.page(-1, page);
            Action::Continue
        }

        (KeyCode::Char('/'), _) => {
            state.start_filter();
            Action::Continue
        }
        (KeyCode::Char('s'), _) => {
            state.cycle_write_scope();
            Action::Continue
        }

        _ => Action::Continue,
    }
}

/// Keys while the filter prompt is open. `None` means "not mine".
fn filter_key(state: &mut ScreenState, key: KeyEvent) -> Option<Action> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(Action::Quit),
        (KeyCode::Esc, _) => {
            state.clear_filter();
            Some(Action::Continue)
        }
        (KeyCode::Enter, _) => {
            // Keep the text and hand the arrow keys back to the list.
            state.commit_filter();
            Some(Action::Continue)
        }
        (KeyCode::Backspace, _) => {
            state.filter_pop();
            Some(Action::Continue)
        }
        // Arrows still navigate while filtering — narrowing and then picking
        // without leaving the prompt is the whole point of an incremental
        // search.
        (KeyCode::Down, _) => {
            state.move_down();
            Some(Action::Continue)
        }
        (KeyCode::Up, _) => {
            state.move_up();
            Some(Action::Continue)
        }
        (KeyCode::Char(ch), m) if m.is_empty() || m == KeyModifiers::SHIFT => {
            state.filter_push(ch);
            Some(Action::Continue)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::resolve::{Snapshot, resolve_all};
    use crate::commands::config::screen::state::{Focus, Mode};
    use crate::git::ConfigScope;

    fn state() -> ScreenState {
        let config = resolve_all(&Snapshot {
            in_repo: true,
            ..Default::default()
        });
        ScreenState::new(config, true, Some("daft".to_string()))
    }

    fn press(state: &mut ScreenState, code: KeyCode) -> Action {
        handle_key(state, KeyEvent::new(code, KeyModifiers::NONE), 10)
    }

    fn ctrl(state: &mut ScreenState, ch: char) -> Action {
        handle_key(
            state,
            KeyEvent::new(KeyCode::Char(ch), KeyModifiers::CONTROL),
            10,
        )
    }

    #[test]
    fn hjkl_and_the_arrows_do_the_same_things() {
        let mut vim = state();
        let mut arrows = state();

        for _ in 0..5 {
            press(&mut vim, KeyCode::Char('j'));
            press(&mut arrows, KeyCode::Down);
        }
        assert_eq!(vim.cursor(), arrows.cursor());

        press(&mut vim, KeyCode::Char('h'));
        press(&mut arrows, KeyCode::Left);
        assert_eq!(vim.focus, arrows.focus);
        assert_eq!(vim.focus, Focus::Rail);

        press(&mut vim, KeyCode::Char('l'));
        press(&mut arrows, KeyCode::Right);
        assert_eq!(vim.focus, Focus::List);
        assert_eq!(arrows.focus, Focus::List);
    }

    #[test]
    fn q_and_esc_and_ctrl_c_all_leave() {
        assert_eq!(press(&mut state(), KeyCode::Char('q')), Action::Quit);
        assert_eq!(press(&mut state(), KeyCode::Esc), Action::Quit);
        assert_eq!(ctrl(&mut state(), 'c'), Action::Quit);
    }

    #[test]
    fn a_key_release_is_ignored() {
        let mut state = state();
        let start = state.cursor();
        let mut event = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        event.kind = KeyEventKind::Release;
        handle_key(&mut state, event, 10);
        assert_eq!(
            state.cursor(),
            start,
            "acting on press and release double-moves"
        );
    }

    #[test]
    fn typing_in_the_filter_does_not_trigger_the_command_keys() {
        let mut state = state();
        let scope = state.write_scope;

        press(&mut state, KeyCode::Char('/'));
        for ch in "sync".chars() {
            press(&mut state, KeyCode::Char(ch));
        }

        assert_eq!(state.filter.as_deref(), Some("sync"));
        assert_eq!(
            state.write_scope, scope,
            "the s in 'sync' must not flip the write scope"
        );
    }

    #[test]
    fn q_while_filtering_is_a_letter_not_a_quit() {
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        assert_eq!(press(&mut state, KeyCode::Char('q')), Action::Continue);
        assert_eq!(state.filter.as_deref(), Some("q"));
    }

    #[test]
    fn ctrl_c_still_leaves_while_filtering() {
        // The one escape that must never be swallowed.
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        assert_eq!(ctrl(&mut state, 'c'), Action::Quit);
    }

    #[test]
    fn esc_clears_the_filter_and_then_leaves() {
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        press(&mut state, KeyCode::Char('m'));

        assert_eq!(press(&mut state, KeyCode::Esc), Action::Continue);
        assert!(!state.is_filtering());
        assert_eq!(press(&mut state, KeyCode::Esc), Action::Quit);
    }

    #[test]
    fn enter_keeps_the_filter_text_and_hands_navigation_back() {
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        for ch in "merge".chars() {
            press(&mut state, KeyCode::Char(ch));
        }
        press(&mut state, KeyCode::Enter);

        assert_eq!(state.filter.as_deref(), Some("merge"), "the text survives");
        // And now the letter keys are commands again.
        press(&mut state, KeyCode::Char('s'));
        assert_eq!(state.write_scope, ConfigScope::Global);
    }

    #[test]
    fn esc_still_reaches_a_filter_that_was_committed() {
        // After Enter the prompt is closed but the list is still narrowed, so
        // Esc has to clear it rather than drop the user out of the screen with
        // no idea why the list looked short.
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        for ch in "merge".chars() {
            press(&mut state, KeyCode::Char(ch));
        }
        press(&mut state, KeyCode::Enter);
        assert!(state.is_filtering() && !state.is_prompt_open());

        assert_eq!(press(&mut state, KeyCode::Esc), Action::Continue);
        assert!(!state.is_filtering());
        assert_eq!(press(&mut state, KeyCode::Esc), Action::Quit);
    }

    #[test]
    fn an_empty_filter_closes_itself_on_enter() {
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        press(&mut state, KeyCode::Enter);
        assert!(
            !state.is_filtering(),
            "an empty prompt is not worth a line of the screen"
        );
    }

    #[test]
    fn arrows_navigate_while_the_filter_is_open() {
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        for ch in "merge".chars() {
            press(&mut state, KeyCode::Char(ch));
        }
        let first = state.selected().map(|r| r.spec.key.to_string());
        press(&mut state, KeyCode::Down);
        assert_ne!(state.selected().map(|r| r.spec.key.to_string()), first);
        assert!(state.is_filtering(), "and the prompt stays open");
    }

    #[test]
    fn backspace_widens_the_filter() {
        let mut state = state();
        press(&mut state, KeyCode::Char('/'));
        press(&mut state, KeyCode::Char('m'));
        press(&mut state, KeyCode::Char('z'));
        let narrow = state.visible_count();
        press(&mut state, KeyCode::Backspace);
        assert!(state.visible_count() >= narrow);
        assert_eq!(state.filter.as_deref(), Some("m"));
    }

    #[test]
    fn s_flips_the_write_scope() {
        let mut state = state();
        assert_eq!(state.write_scope, ConfigScope::Local);
        press(&mut state, KeyCode::Char('s'));
        assert_eq!(state.write_scope, ConfigScope::Global);
    }

    #[test]
    fn g_and_shift_g_reach_both_ends() {
        let mut state = state();
        press(&mut state, KeyCode::Char('G'));
        let bottom = state.cursor();
        press(&mut state, KeyCode::Char('g'));
        assert!(state.cursor() < bottom);
        assert!(state.selected().is_some());
    }

    #[test]
    fn paging_moves_by_a_screenful_and_lands_on_a_setting() {
        let mut state = state();
        press(&mut state, KeyCode::PageDown);
        let after = state.cursor();
        assert!(after > 0);
        assert!(state.selected().is_some());

        ctrl(&mut state, 'd');
        assert!(state.cursor() > after);
        assert!(state.selected().is_some());

        ctrl(&mut state, 'u');
        assert!(state.selected().is_some());
    }

    #[test]
    fn tab_walks_the_panes_and_the_rail_changes_the_mode() {
        let mut state = state();
        press(&mut state, KeyCode::Tab);
        assert_eq!(state.focus, Focus::Rail);

        press(&mut state, KeyCode::Char('j'));
        assert_eq!(state.mode, Mode::Modified);

        press(&mut state, KeyCode::Tab);
        assert_eq!(state.focus, Focus::List);
    }

    #[test]
    fn an_unbound_key_is_quietly_ignored() {
        let mut state = state();
        let before = state.cursor();
        assert_eq!(press(&mut state, KeyCode::Char('z')), Action::Continue);
        assert_eq!(state.cursor(), before);
    }

    #[test]
    fn a_new_keystroke_clears_the_last_report() {
        let mut state = state();
        state.set_status("something happened", super::super::state::StatusKind::Info);
        press(&mut state, KeyCode::Char('j'));
        assert!(
            state.status.is_none(),
            "a stale status line lies about what just happened"
        );
    }
}
