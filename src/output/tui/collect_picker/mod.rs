//! Interactive TUI for collecting declared-but-uncollected shared files.
//!
//! Presents a tabbed interface where each tab represents a declared shared
//! file. The user selects which worktree's copy to promote to shared storage,
//! with a syntax-highlighted preview of the file content.

mod highlight;
mod input;
mod render;
pub mod state;

use anyhow::Result;
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, KeyCode},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::time::Duration;

use crate::core::shared::{CollectDecision, UncollectedFile};
use highlight::Highlighter;
use input::InputResult;
pub use state::CollectPickerState;

/// Outcome of the collect picker.
pub enum PickerOutcome {
    /// User submitted — execute these decisions.
    Decisions(Vec<CollectDecision>),
    /// User cancelled — do nothing.
    Cancelled,
}

/// Run the interactive collect picker TUI.
///
/// Enters alternate screen mode, runs the event loop, and returns the user's
/// decisions. Restores the terminal on exit (including on panic).
pub fn run_collect_picker(uncollected: Vec<UncollectedFile>) -> Result<PickerOutcome> {
    // Set up terminal
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let highlighter = Highlighter::new();
    let mut state = CollectPickerState::new(uncollected);

    let result = run_event_loop(&mut terminal, &mut state, &highlighter);

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    match result {
        Ok(true) => Ok(PickerOutcome::Decisions(state.into_decisions())),
        Ok(false) => Ok(PickerOutcome::Cancelled),
        Err(e) => Err(e),
    }
}

/// Inner event loop. Returns `Ok(true)` for submit, `Ok(false)` for cancel.
fn run_event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    state: &mut CollectPickerState,
    highlighter: &Highlighter,
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            render::render(state, highlighter, frame);
        })?;

        let Some(key) = input::poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match input::handle_key(key, state) {
            InputResult::Continue => {}
            InputResult::Cancel => {
                if state.has_any_selection() {
                    let confirmed = show_cancel_confirm(terminal)?;
                    if confirmed {
                        return Ok(false);
                    }
                    state.cancelled = false;
                } else {
                    return Ok(false);
                }
            }
            InputResult::Submit => {
                if !state.all_decided() {
                    let undecided = state
                        .undecided_files()
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                    let confirmed = show_partial_submit_confirm(terminal, &undecided)?;
                    if confirmed {
                        return Ok(true);
                    }
                    state.submitted = false;
                } else {
                    return Ok(true);
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

/// Generic yes/no confirmation dialog rendered as an overlay.
fn show_confirm_dialog(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            let area = frame.area();

            let dialog_width = 50u16.min(area.width.saturating_sub(4));
            let dialog_height = (body_lines.len() as u16 + 5).min(area.height.saturating_sub(2));
            let x = (area.width.saturating_sub(dialog_width)) / 2;
            let y = (area.height.saturating_sub(dialog_height)) / 2;
            let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

            frame.render_widget(Clear, dialog_area);

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));

            let mut text_lines: Vec<Line> = body_lines
                .iter()
                .map(|&line| Line::raw(line.to_string()))
                .collect();
            text_lines.push(Line::raw(""));
            text_lines.push(Line::from(vec![
                Span::styled(
                    " [Y]es ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(" [N]o ", Style::default().fg(Color::DarkGray)),
            ]));

            let paragraph = Paragraph::new(text_lines)
                .block(block)
                .wrap(Wrap { trim: false });

            frame.render_widget(paragraph, dialog_area);
        })?;

        if let Some(key) = input::poll_key(Duration::from_millis(100)) {
            match key.code {
                KeyCode::Char('y') | KeyCode::Enter => return Ok(true),
                KeyCode::Char('n') | KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}
