//! Reusable shared-file picker TUI.
//!
//! Provides a tabbed interface where each tab represents a declared shared
//! file. The generic shell handles navigation and rendering while mode-specific
//! logic (e.g. collect, manage) is injected via the `PickerMode` trait.

pub mod collect_mode;
mod dialog;
mod highlight;
pub mod input;
mod render;
mod shell;
pub mod state;

use anyhow::Result;
use crossterm::{
    event::EnableMouseCapture,
    execute,
    terminal::{self, EnterAlternateScreen},
};
use ratatui::{layout::Rect, style::Color, Frame, Terminal};
use std::io;
use std::time::Duration;

use crate::core::shared::{CollectDecision, UncollectedFile};
use collect_mode::CollectMode;
use dialog::show_confirm_dialog;
use highlight::Highlighter;
use shell::restore_terminal;
use state::{FileTabState, PickerState};

/// What the event loop should do after handling an action.
pub enum LoopAction {
    Continue,
    Exit,
}

/// Marker and tag decoration for a worktree entry.
pub struct EntryDecoration {
    pub marker: String,
    pub tag: Option<(String, Color)>,
}

/// Outcome of the collect picker.
pub enum PickerOutcome {
    /// User submitted — execute these decisions.
    Decisions(Vec<CollectDecision>),
    /// User cancelled — do nothing.
    Cancelled,
}

/// Trait for mode-specific picker behavior.
///
/// The generic picker shell handles navigation, rendering chrome, and the
/// event loop. Each mode provides its own action handling, decorations,
/// and footer rendering.
pub trait PickerMode {
    /// Whether all worktree entries are traversable (vs. only those with files).
    fn all_entries_traversable(&self, tab: &FileTabState) -> bool;

    /// Handle a key press while focused on the worktree list.
    /// Called for keys not consumed by the shell (navigation).
    fn handle_list_key(
        &mut self,
        key: crossterm::event::KeyCode,
        state: &mut PickerState,
    ) -> LoopAction;

    /// Handle a key press while focused on the footer.
    /// The mode owns the entire footer interaction.
    fn handle_footer_key(
        &mut self,
        key: crossterm::event::KeyCode,
        state: &mut PickerState,
    ) -> LoopAction;

    /// Whether a tab is considered "decided" (shows a checkmark).
    fn tab_decided(&self, tab: &FileTabState) -> bool;

    /// Optional warning message to display below the tab bar.
    fn tab_warning<'a>(&'a self, tab: &'a FileTabState) -> Option<&'a str>;

    /// Decoration (marker + optional tag) for a worktree entry.
    fn entry_decoration(&self, tab: &FileTabState, entry_idx: usize) -> EntryDecoration;

    /// Render the footer area.
    fn render_footer(&self, state: &PickerState, frame: &mut Frame, area: Rect);

    /// Height of the footer in terminal rows.
    fn footer_height(&self) -> u16;
}

/// Run the interactive collect picker TUI.
///
/// Enters alternate screen mode, runs the event loop, and returns the user's
/// decisions. Restores the terminal on exit, including on panic.
pub fn run_collect_picker(uncollected: Vec<UncollectedFile>) -> Result<PickerOutcome> {
    // Install panic hook that restores the terminal before printing the panic
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    let outcome = run_collect_picker_inner(uncollected);

    // Restore the default panic hook
    let _ = std::panic::take_hook();

    outcome
}

fn run_collect_picker_inner(uncollected: Vec<UncollectedFile>) -> Result<PickerOutcome> {
    // Set up terminal
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let highlighter = Highlighter::new();
    let tabs = CollectMode::build_tabs(uncollected);
    let mut state = PickerState::from_tabs(tabs);
    let mut mode = CollectMode::new();

    let result = run_event_loop(&mut terminal, &mut state, &mut mode, &highlighter);

    // Restore terminal
    restore_terminal();

    match result {
        Ok(true) => Ok(PickerOutcome::Decisions(CollectMode::into_decisions(state))),
        Ok(false) => Ok(PickerOutcome::Cancelled),
        Err(e) => Err(e),
    }
}

/// Inner event loop. Returns `Ok(true)` for submit, `Ok(false)` for cancel.
fn run_event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    state: &mut PickerState,
    mode: &mut CollectMode,
    highlighter: &Highlighter,
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            render::render(state, mode, highlighter, frame);
        })?;

        let Some(key) = input::poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match input::handle_key(key, state, mode) {
            LoopAction::Continue => {}
            LoopAction::Exit => {
                if mode.is_cancelled() {
                    if CollectMode::has_any_selection(state) {
                        let confirmed = show_cancel_confirm(terminal)?;
                        if confirmed {
                            return Ok(false);
                        }
                        mode.cancelled = false;
                    } else {
                        return Ok(false);
                    }
                } else if mode.is_submitted() {
                    if !CollectMode::all_decided(state) {
                        let undecided = CollectMode::undecided_files(state)
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>();
                        let confirmed = show_partial_submit_confirm(terminal, &undecided)?;
                        if confirmed {
                            return Ok(true);
                        }
                        mode.submitted = false;
                    } else {
                        return Ok(true);
                    }
                } else {
                    // Ctrl+C path — treat as cancel
                    return Ok(false);
                }
            }
        }
    }
}

/// Show a "are you sure you want to cancel?" dialog.
fn show_cancel_confirm(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
) -> Result<bool> {
    show_confirm_dialog(
        terminal,
        "Cancel sync?",
        &["You have selections that will be lost.", "Are you sure?"],
    )
}

/// Show a "partial submit" confirmation dialog.
fn show_partial_submit_confirm(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    undecided: &[String],
) -> Result<bool> {
    let mut lines = vec![
        "The following files have no copy selected:".to_string(),
        String::new(),
    ];
    for file in undecided {
        lines.push(format!("  \u{2022} {file}"));
    }
    lines.push(String::new());
    lines.push("They will be skipped. Continue?".to_string());

    let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    show_confirm_dialog(terminal, "Partial submit", &line_refs)
}
