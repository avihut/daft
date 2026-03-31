//! Reusable confirmation dialog overlay for the shared picker TUI.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame, Terminal,
};
use std::io;
use std::time::Duration;

use super::input::poll_key;

/// Generic yes/no confirmation dialog rendered as an overlay.
/// Supports h/l and arrow keys to toggle focus, Enter to confirm selection.
pub fn show_confirm_dialog(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
) -> Result<bool> {
    run_dialog(terminal, title, body_lines, true, &mut |_| {})
}

/// Confirmation dialog with configurable default focus.
#[allow(dead_code)]
pub fn show_confirm_dialog_with_default(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
    default_yes: bool,
) -> Result<bool> {
    run_dialog(terminal, title, body_lines, default_yes, &mut |_| {})
}

/// Confirmation dialog overlaid on top of a background rendered by `bg`.
pub fn show_confirm_dialog_over(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
    default_yes: bool,
    bg: &mut dyn FnMut(&mut Frame),
) -> Result<bool> {
    run_dialog(terminal, title, body_lines, default_yes, bg)
}

fn run_dialog(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
    default_yes: bool,
    bg: &mut dyn FnMut(&mut Frame),
) -> Result<bool> {
    let mut yes_focused = default_yes;

    loop {
        terminal.draw(|frame| {
            bg(frame);
            render_dialog(frame, title, body_lines, yes_focused);
        })?;

        if let Some(key) = poll_key(Duration::from_millis(100)) {
            match key.code {
                KeyCode::Char('y') => return Ok(true),
                KeyCode::Char('n') | KeyCode::Esc => return Ok(false),
                KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                    yes_focused = !yes_focused;
                }
                KeyCode::Enter | KeyCode::Char(' ') => return Ok(yes_focused),
                _ => {}
            }
        }
    }
}

/// Dim all cells in the frame buffer to create a modal backdrop effect.
fn dim_background(frame: &mut Frame) {
    let area = frame.area();
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &mut buf[(x, y)];
            cell.set_style(cell.style().add_modifier(Modifier::DIM));
        }
    }
}

/// Render the dialog overlay on top of the current frame content.
fn render_dialog(frame: &mut Frame, title: &str, body_lines: &[&str], yes_focused: bool) {
    dim_background(frame);
    let area = frame.area();

    // Width: fit the longest line + padding (4 chars indent) + 2 for border
    let max_line_len = body_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let min_width = (max_line_len as u16 + 6).max(30);
    let dialog_width = min_width.min(area.width.saturating_sub(4));

    // Content: top pad (1) + body lines + blank (1) + buttons (1) + bottom pad (1)
    let inner_height = body_lines.len() as u16 + 4;
    // Total: inner + 2 for border
    let dialog_height = (inner_height + 2).min(area.height.saturating_sub(2));
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

    let yes_style = if yes_focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let no_style = if yes_focused {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    };

    let mut text_lines: Vec<Line> = Vec::new();

    // Top padding
    text_lines.push(Line::raw(""));

    // Body lines with left padding
    for &line in body_lines {
        text_lines.push(Line::from(Span::raw(format!("  {line}"))));
    }

    // Blank line before buttons
    text_lines.push(Line::raw(""));

    // Buttons with padding
    text_lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(" [Y]es ", yes_style),
        Span::raw("  "),
        Span::styled(" [N]o ", no_style),
    ]));

    let paragraph = Paragraph::new(text_lines).block(block);

    frame.render_widget(paragraph, dialog_area);
}
