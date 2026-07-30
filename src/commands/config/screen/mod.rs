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
pub mod modal;
pub mod render;
pub mod state;

use std::io;

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::execute;
use crossterm::terminal::{self, EnterAlternateScreen};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::commands::config::resolve::Snapshot;
use crate::commands::config::{resolve, write};
use crate::git::GitCommand;
use crate::output::tui::restore_terminal;
use input::Action;
use modal::Apply;
use state::{ScreenState, StatusKind};

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
        let Event::Key(key) = event::read()? else {
            continue;
        };

        match input::handle_key(state, key, page) {
            Action::Quit => return Ok(()),
            Action::Continue => {}
            Action::Write(apply) => perform(state, apply),
        }
    }
}

/// Carry out a change and show the result.
///
/// The only place in the screen that touches git, which is why it is here and
/// not in the key handler. A refusal is reported and the editor stays open on
/// the value that caused it — closing would throw away what the user typed at
/// the moment they need to fix it.
fn perform(state: &mut ScreenState, apply: Apply) {
    // A behavior write has to re-read before it can narrate — clearing three
    // local keys can reveal a global value, and "unset" that does not say
    // where it landed is the single-scope claim this screen exists to avoid.
    // It hands the fresh resolution back so the screen does not pay twice.
    let outcome: Result<(String, Option<resolve::ResolvedSet>)> = match &apply {
        Apply::Set { key, scope, value } => resolve::lookup(key)
            .map_err(anyhow::Error::msg)
            .and_then(|spec| write::set(&spec, *scope, value, &state.config))
            .map(|applied| (applied.message, None)),
        Apply::Unset { key, scope } => resolve::lookup(key)
            .map_err(anyhow::Error::msg)
            .and_then(|spec| write::unset(&spec, *scope))
            .map(|applied| (applied.message, None)),
        Apply::SetBehavior {
            name,
            preset,
            scope,
        } => behavior_and_preset(name, preset).and_then(|(behavior, preset)| {
            write::set_behavior(behavior, preset, *scope, &state.config, || {
                resolve_fresh(state, true)
            })
            .map(|written| (written.message, Some(written.config)))
        }),
        Apply::UnsetBehavior { name, scope } => behavior_for(name).and_then(|behavior| {
            write::unset_behavior(behavior, *scope, || resolve_fresh(state, true))
                .map(|written| (written.message, Some(written.config)))
        }),
    };

    match outcome {
        Ok((narration, fresh)) => {
            state.close_modal();
            // Re-read rather than patching the row in place: a write can move
            // which layer wins for *other* settings too, and a stale ladder is
            // the one thing this screen cannot afford to show.
            let reloaded = match fresh {
                Some(config) => {
                    state.reload(config);
                    Ok(())
                }
                None => reload(state, &apply),
            };
            match reloaded {
                Ok(()) => state.set_status(narration, StatusKind::Success),
                Err(error) => state.set_status(
                    format!("{narration}, but re-reading the config failed: {error}"),
                    StatusKind::Error,
                ),
            }
        }
        Err(error) => {
            let message = error.to_string();
            match state.modal.as_mut() {
                Some(modal) => modal.error = Some(message),
                None => state.set_status(message, StatusKind::Error),
            }
        }
    }
}

fn reload(state: &mut ScreenState, applied: &Apply) -> Result<()> {
    let config = resolve_fresh(state, !touches_layout(applied))?;
    state.reload(config);
    Ok(())
}

/// Re-read everything.
///
/// Capturing the layout chain walks the filesystem to detect where worktrees
/// are. Only a layout write can change what it says, so `keep_layout` lets
/// every other write — a `space` toggle included — reuse the chain it already
/// had rather than paying for a walk that cannot return anything new.
fn resolve_fresh(state: &ScreenState, keep_layout: bool) -> Result<resolve::ResolvedSet> {
    let mut snapshot = if state.in_repo {
        Snapshot::capture(&GitCommand::new(false))?
    } else {
        Snapshot::capture_global_only()?
    };

    if keep_layout && let Some(previous) = previous_layout(state) {
        snapshot.layout = Some(previous);
    }

    Ok(resolve::resolve_all(&snapshot))
}

fn behavior_for(name: &str) -> Result<&'static crate::core::settings_spec::BehaviorSpec> {
    crate::core::settings_spec::find_behavior(name)
        .ok_or_else(|| anyhow::anyhow!("unknown behavior: {name}"))
}

fn behavior_and_preset(
    name: &str,
    preset: &str,
) -> Result<(
    &'static crate::core::settings_spec::BehaviorSpec,
    &'static crate::core::settings_spec::Preset,
)> {
    let behavior = behavior_for(name)?;
    let preset = behavior
        .preset(preset)
        .ok_or_else(|| anyhow::anyhow!("{preset} is not a state of {name}"))?;
    Ok((behavior, preset))
}

fn touches_layout(applied: &Apply) -> bool {
    use crate::core::settings_spec::Backend;

    let key = match applied {
        Apply::Set { key, .. } | Apply::Unset { key, .. } => key,
        // A behavior touches the chain only if one of its members lives on
        // it. None do today; asking rather than assuming keeps that true.
        Apply::SetBehavior { name, .. } | Apply::UnsetBehavior { name, .. } => {
            return behavior_for(name).is_ok_and(|behavior| {
                behavior.members.iter().any(|member| {
                    resolve::lookup(member).is_ok_and(|spec| spec.backend == Backend::LayoutChain)
                })
            });
        }
    };
    resolve::lookup(key)
        .map(|spec| spec.backend == Backend::LayoutChain)
        .unwrap_or(true)
}

/// The layout chain the screen is currently showing, rebuilt from its rungs.
fn previous_layout(state: &ScreenState) -> Option<resolve::LayoutRungs> {
    state.config.layout_rungs.clone()
}
