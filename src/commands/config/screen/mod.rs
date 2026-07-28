//! The full-screen settings browser.
//!
//! Everything daft can be configured with, in one place, with the layer that
//! decided each value shown next to it. The three modules underneath split the
//! work so only this one touches a terminal: [`state`] is the model,
//! [`render`] turns it into a frame, [`input`] turns a keystroke into a state
//! change.
//!
//! The lifecycle follows `shared_picker`'s: a panic hook installed *before*
//! raw mode, the alternate screen on **stderr** (stdout carries the
//! `DAFT_CD_FILE` channel), and `restore_terminal` on every path out. daft
//! ships `panic = "abort"`, so a Drop guard would not run — the hook is the
//! only thing standing between a mid-frame panic and a terminal the user has
//! to `reset`.

pub mod input;
pub mod render;
pub mod state;

use std::io;

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::output::tui::restore_terminal;
use input::Action;
use state::ScreenState;

/// Open the settings browser.
pub fn run(state: ScreenState) -> Result<()> {
    // Before raw mode, not after: a panic in the two statements between would
    // otherwise reach the default hook with the terminal already switched.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));

    let outcome = run_inner(state);

    // Drop our hook so it does not follow the process into post-TUI code.
    let _ = std::panic::take_hook();

    outcome
}

fn run_inner(mut state: ScreenState) -> Result<()> {
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stderr))?;

    let result = event_loop(&mut terminal, &mut state);

    restore_terminal();
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stderr>>,
    state: &mut ScreenState,
) -> Result<()> {
    loop {
        let mut page = 1usize;
        terminal.draw(|frame| {
            let area = frame.area();
            // The frame decides whether the rail fits, and the state has to
            // know that before it interprets the next keystroke.
            state.set_rail_visible(render::rail_fits(area.width));
            page = render::list_height(area, state.is_filtering());
            state.follow_cursor(page);
            render::draw(frame, state);
        })?;

        // Resize and mouse events land here too; both just fall through to the
        // redraw at the top of the next turn.
        if let Event::Key(key) = event::read()?
            && input::handle_key(state, key, page) == Action::Quit
        {
            return Ok(());
        }
    }
}
