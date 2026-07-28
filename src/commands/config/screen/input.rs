//! Key dispatch. Turns a keystroke into a state change, and nothing else.
//!
//! Separated from the event loop so every binding is testable without a
//! terminal: the loop's job is to read events and draw, and this decides what
//! they mean.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::modal::{Apply, toggled_value};
use super::state::{ScreenState, StatusKind};
use crate::commands::config::write::WriteScope;

/// What the event loop should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Redraw and keep going.
    Continue,
    /// Leave the screen.
    Quit,
    /// Carry out a change and re-resolve. The loop does this rather than the
    /// key handler so everything above stays free of git.
    Write(Apply),
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

    // The editor owns the keyboard while it is open — including the letters,
    // which are values rather than commands in there.
    if state.modal_open() {
        return modal_key(state, key);
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

        (KeyCode::Enter, _) => {
            state.open_modal();
            Action::Continue
        }
        // The two shortcuts that skip the editor for the things it would only
        // slow down: flipping a boolean, and clearing a value you can see.
        (KeyCode::Char(' '), _) => quick_toggle(state),
        (KeyCode::Char('u'), _) => quick_unset(state),

        _ => Action::Continue,
    }
}

/// `space`: flip a two-state setting at the pill scope.
fn quick_toggle(state: &mut ScreenState) -> Action {
    let Some(resolved) = state.selected() else {
        return Action::Continue;
    };
    let key = resolved.spec.key.to_string();

    if let Some(owner) = resolved.spec.managed_by {
        state.set_status(
            format!("{key} is managed by `{owner}` — change it there"),
            StatusKind::Info,
        );
        return Action::Continue;
    }

    match toggled_value(resolved, state.write_scope) {
        Some(value) => Action::Write(Apply::Set {
            key,
            scope: write_scope(state),
            value,
        }),
        // A duration has no opposite; say so rather than picking one.
        None => {
            state.set_status(
                format!("{key} is not a toggle — press enter to edit it"),
                StatusKind::Info,
            );
            Action::Continue
        }
    }
}

/// `u`: clear the value at the pill scope.
fn quick_unset(state: &mut ScreenState) -> Action {
    let Some(resolved) = state.selected() else {
        return Action::Continue;
    };
    let key = resolved.spec.key.to_string();

    if let Some(owner) = resolved.spec.managed_by {
        state.set_status(
            format!("{key} is managed by `{owner}` — change it there"),
            StatusKind::Info,
        );
        return Action::Continue;
    }

    Action::Write(Apply::Unset {
        key,
        scope: write_scope(state),
    })
}

fn write_scope(state: &ScreenState) -> WriteScope {
    match state.write_scope {
        crate::git::ConfigScope::Global => WriteScope::Global,
        _ => WriteScope::Local,
    }
}

/// Keys while the value editor is open.
fn modal_key(state: &mut ScreenState, key: KeyEvent) -> Action {
    // Ctrl-C leaves from anywhere; nothing may swallow it.
    if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
        return Action::Quit;
    }

    let picking = state.modal.as_ref().is_some_and(|m| m.is_picking());
    let Some(modal) = state.modal.as_mut() else {
        return Action::Continue;
    };

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) => {
            state.close_modal();
            Action::Continue
        }
        (KeyCode::Enter, _) => match modal.apply() {
            Ok(apply) => Action::Write(apply),
            // Refused: the reason stays in the box, next to the field that
            // caused it, rather than closing and reporting from a distance.
            Err(reason) => {
                modal.error = Some(reason);
                Action::Continue
            }
        },
        // Tab reaches the scope from either mode. h/l only when picking,
        // where the arrows are otherwise idle — in a text field they are the
        // caret, which is what a typist expects them to be.
        (KeyCode::Tab, _) | (KeyCode::BackTab, _) => {
            modal.cycle_scope();
            Action::Continue
        }
        (KeyCode::Left, _) if !picking => {
            modal.caret_left();
            Action::Continue
        }
        (KeyCode::Right, _) if !picking => {
            modal.caret_right();
            Action::Continue
        }
        (KeyCode::Left, _) | (KeyCode::Char('h'), _) if picking => {
            modal.set_scope(WriteScope::Local);
            Action::Continue
        }
        (KeyCode::Right, _) | (KeyCode::Char('l'), _) if picking => {
            modal.set_scope(WriteScope::Global);
            Action::Continue
        }
        (KeyCode::Down, _) => {
            modal.move_down();
            Action::Continue
        }
        (KeyCode::Up, _) => {
            modal.move_up();
            Action::Continue
        }
        (KeyCode::Char('j'), _) if picking => {
            modal.move_down();
            Action::Continue
        }
        (KeyCode::Char('k'), _) if picking => {
            modal.move_up();
            Action::Continue
        }
        (KeyCode::Backspace, _) if !picking => {
            modal.backspace();
            Action::Continue
        }
        (KeyCode::Char(ch), m) if !picking && (m.is_empty() || m == KeyModifiers::SHIFT) => {
            modal.type_char(ch);
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
    use crate::core::settings::keys;
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

    // ── The editor ───────────────────────────────────────────────────────

    fn on(key: &str) -> ScreenState {
        let mut state = state();
        while state.selected().is_some_and(|r| r.spec.key != key) {
            state.move_down();
        }
        assert_eq!(state.selected().unwrap().spec.key, key);
        state
    }

    #[test]
    fn enter_opens_the_editor_and_esc_closes_it() {
        let mut state = on(keys::MERGE_STYLE);
        press(&mut state, KeyCode::Enter);
        assert!(state.modal_open());

        assert_eq!(press(&mut state, KeyCode::Esc), Action::Continue);
        assert!(!state.modal_open());
        // And Esc is the screen's again.
        assert_eq!(press(&mut state, KeyCode::Esc), Action::Quit);
    }

    #[test]
    fn the_editor_swallows_the_command_letters() {
        // `q` inside the editor of a text setting is a character, not a quit.
        let mut state = on(keys::REMOTE);
        press(&mut state, KeyCode::Enter);
        assert_eq!(press(&mut state, KeyCode::Char('q')), Action::Continue);
        assert!(state.modal_open());

        match state.modal.as_ref().map(|m| m.apply()) {
            Some(Ok(Apply::Set { value, .. })) => assert!(value.ends_with('q')),
            other => panic!("expected the letter to be typed, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_c_leaves_even_from_inside_the_editor() {
        let mut state = on(keys::REMOTE);
        press(&mut state, KeyCode::Enter);
        assert_eq!(ctrl(&mut state, 'c'), Action::Quit);
    }

    #[test]
    fn enter_in_the_editor_hands_the_write_to_the_caller() {
        let mut state = on(keys::MERGE_STYLE);
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Char('j'));

        assert_eq!(
            press(&mut state, KeyCode::Enter),
            Action::Write(Apply::Set {
                key: keys::MERGE_STYLE.to_string(),
                scope: WriteScope::Local,
                value: "squash".to_string(),
            })
        );
    }

    #[test]
    fn a_refusal_stays_in_the_editor_rather_than_closing_it() {
        let mut state = on(keys::UPDATE_CHECK);
        press(&mut state, KeyCode::Enter);

        assert_eq!(press(&mut state, KeyCode::Enter), Action::Continue);
        assert!(
            state.modal_open(),
            "closing would discard the fix in progress"
        );
        assert!(
            state
                .modal
                .as_ref()
                .and_then(|m| m.error.as_deref())
                .is_some_and(|e| e.contains("global config only"))
        );
    }

    #[test]
    fn tab_reaches_the_scope_from_either_kind_of_field() {
        for key in [keys::MERGE_STYLE, keys::REMOTE] {
            let mut state = on(key);
            press(&mut state, KeyCode::Enter);
            assert_eq!(state.modal.as_ref().unwrap().scope, WriteScope::Local);
            press(&mut state, KeyCode::Tab);
            assert_eq!(
                state.modal.as_ref().unwrap().scope,
                WriteScope::Global,
                "{key}"
            );
        }
    }

    #[test]
    fn the_arrows_are_the_caret_in_a_text_field_and_the_scope_when_picking() {
        // Text: Left moves the caret, so an insert lands mid-word.
        let mut state = on(keys::REMOTE);
        press(&mut state, KeyCode::Enter);
        for ch in "abc".chars() {
            press(&mut state, KeyCode::Char(ch));
        }
        press(&mut state, KeyCode::Left);
        press(&mut state, KeyCode::Char('X'));
        match state.modal.as_ref().unwrap().apply() {
            Ok(Apply::Set { value, .. }) => assert_eq!(value, "abXc"),
            other => panic!("expected the caret to have moved, got {other:?}"),
        }

        // Picking: the same key picks a scope, where the caret has no meaning.
        let mut state = on(keys::MERGE_STYLE);
        press(&mut state, KeyCode::Enter);
        press(&mut state, KeyCode::Right);
        assert_eq!(state.modal.as_ref().unwrap().scope, WriteScope::Global);
        press(&mut state, KeyCode::Left);
        assert_eq!(state.modal.as_ref().unwrap().scope, WriteScope::Local);
    }

    // ── Quick actions ────────────────────────────────────────────────────

    #[test]
    fn space_toggles_a_boolean_at_the_pill_scope() {
        let mut state = on(keys::CHECKOUT_FETCH);
        assert_eq!(
            press(&mut state, KeyCode::Char(' ')),
            Action::Write(Apply::Set {
                key: keys::CHECKOUT_FETCH.to_string(),
                scope: WriteScope::Local,
                value: "true".to_string(),
            })
        );

        // Flip the pill and the write follows it.
        press(&mut state, KeyCode::Char('s'));
        assert_eq!(
            press(&mut state, KeyCode::Char(' ')),
            Action::Write(Apply::Set {
                key: keys::CHECKOUT_FETCH.to_string(),
                scope: WriteScope::Global,
                value: "true".to_string(),
            })
        );
    }

    #[test]
    fn space_on_something_that_is_not_a_toggle_says_so() {
        let mut state = on(keys::hooks::TIMEOUT);
        assert_eq!(press(&mut state, KeyCode::Char(' ')), Action::Continue);
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|s| s.text.contains("not a toggle")),
            "a key that does nothing must say why"
        );
    }

    #[test]
    fn u_unsets_at_the_pill_scope() {
        let mut state = on(keys::MERGE_STYLE);
        assert_eq!(
            press(&mut state, KeyCode::Char('u')),
            Action::Write(Apply::Unset {
                key: keys::MERGE_STYLE.to_string(),
                scope: WriteScope::Local,
            })
        );
    }

    #[test]
    fn a_row_another_command_owns_refuses_every_way_in() {
        let mut state = on("shared");

        press(&mut state, KeyCode::Enter);
        assert!(
            !state.modal_open(),
            "an editor that can only refuse is a lie"
        );
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|s| s.text.contains("daft shared"))
        );

        assert_eq!(press(&mut state, KeyCode::Char(' ')), Action::Continue);
        assert_eq!(press(&mut state, KeyCode::Char('u')), Action::Continue);
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
