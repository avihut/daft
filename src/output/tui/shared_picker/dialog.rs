//! Reusable confirmation dialog overlay for the shared picker TUI.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
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
    let mut yes_focused = true;

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

            let mut text_lines: Vec<Line> = body_lines
                .iter()
                .map(|&line| Line::raw(line.to_string()))
                .collect();
            text_lines.push(Line::raw(""));
            text_lines.push(Line::from(vec![
                Span::styled(" [Y]es ", yes_style),
                Span::raw("  "),
                Span::styled(" [N]o ", no_style),
            ]));

            let paragraph = Paragraph::new(text_lines)
                .block(block)
                .wrap(Wrap { trim: false });

            frame.render_widget(paragraph, dialog_area);
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
