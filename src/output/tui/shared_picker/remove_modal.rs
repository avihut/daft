//! Remove modal for the manage picker TUI.
//!
//! Presents a two-option modal when the user requests removal of a shared file:
//! 1. Materialize in selected worktrees (expandable per-worktree checklist)
//! 2. Delete everywhere
//!
//! The modal drives its own event loop and returns a `RemoveDecision`.

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

/// What the user decided in the remove modal.
pub enum RemoveDecision {
    /// Materialize the shared file into selected worktrees before removing.
    /// Each bool corresponds to a worktree: `true` = materialize, `false` = skip.
    Materialize(Vec<bool>),
    /// Delete the shared file from all worktrees and shared storage.
    DeleteAll,
    /// User cancelled — do nothing.
    Cancelled,
}

/// Which top-level option is focused.
#[derive(Clone, Copy, PartialEq)]
enum ModalOption {
    Materialize,
    DeleteAll,
}

/// Show the remove modal and return the user's decision.
///
/// The modal renders as an overlay on top of the existing TUI content.
/// It has its own event loop for key handling.
pub fn show_remove_modal(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    file_name: &str,
    worktree_names: &[String],
) -> Result<RemoveDecision> {
    let mut focused = ModalOption::Materialize;
    let mut expanded = false;
    // All worktrees start checked (materialized)
    let mut checks: Vec<bool> = vec![true; worktree_names.len()];
    // Which worktree is focused when expanded (index into worktree_names)
    let mut wt_cursor: usize = 0;

    loop {
        terminal.draw(|frame| {
            render_remove_modal(
                frame,
                file_name,
                worktree_names,
                focused,
                expanded,
                &checks,
                wt_cursor,
            );
        })?;

        let Some(key) = poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match key.code {
            KeyCode::Esc => return Ok(RemoveDecision::Cancelled),
            KeyCode::Char('c')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                return Ok(RemoveDecision::Cancelled);
            }
            KeyCode::Enter => {
                return match focused {
                    ModalOption::Materialize => Ok(RemoveDecision::Materialize(checks)),
                    ModalOption::DeleteAll => Ok(RemoveDecision::DeleteAll),
                };
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if expanded && focused == ModalOption::Materialize {
                    if wt_cursor > 0 {
                        wt_cursor -= 1;
                    } else {
                        // At top of worktree list — stay
                    }
                } else if focused == ModalOption::DeleteAll {
                    focused = ModalOption::Materialize;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if expanded && focused == ModalOption::Materialize {
                    if wt_cursor < worktree_names.len().saturating_sub(1) {
                        wt_cursor += 1;
                    } else {
                        // Past last worktree — move to DeleteAll
                        expanded = false;
                        focused = ModalOption::DeleteAll;
                    }
                } else if focused == ModalOption::Materialize {
                    focused = ModalOption::DeleteAll;
                }
            }
            KeyCode::Right | KeyCode::Char('l')
                if focused == ModalOption::Materialize && !expanded =>
            {
                expanded = true;
                wt_cursor = 0;
            }
            KeyCode::Left | KeyCode::Char('h') if expanded => {
                expanded = false;
            }
            KeyCode::Char(' ') if expanded && focused == ModalOption::Materialize => {
                checks[wt_cursor] = !checks[wt_cursor];
            }
            _ => {}
        }
    }
}

/// Render the remove modal overlay.
fn render_remove_modal(
    frame: &mut ratatui::Frame,
    file_name: &str,
    worktree_names: &[String],
    focused: ModalOption,
    expanded: bool,
    checks: &[bool],
    wt_cursor: usize,
) {
    let area = frame.area();

    // Compute dialog dimensions
    let dialog_width = 50u16.min(area.width.saturating_sub(4));
    let content_lines = if expanded {
        // header + worktree lines + blank + delete option + blank + help
        2 + worktree_names.len() as u16 + 1 + 1 + 1 + 1
    } else {
        // header + materialize option + delete option + blank + help
        2 + 1 + 1 + 1 + 1
    };
    // +2 for border
    let dialog_height = (content_lines + 2).min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    frame.render_widget(Clear, dialog_area);

    let title = format!(" Remove {file_name} ");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .title(Span::styled(
            title,
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));

    let mut lines: Vec<Line> = Vec::new();

    let key_style = Style::default().fg(Color::Cyan);
    let dim_style = Style::default().fg(Color::DarkGray);
    let highlight_style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(Color::Reset);

    if expanded {
        // Expanded materialize view
        let header_style = if focused == ModalOption::Materialize {
            highlight_style
        } else {
            normal_style
        };
        lines.push(Line::from(vec![Span::styled(
            "\u{25be} Materialize in worktrees:",
            header_style,
        )]));

        for (i, name) in worktree_names.iter().enumerate() {
            let check = if checks[i] { "\u{2713}" } else { " " };
            let is_focused = focused == ModalOption::Materialize && i == wt_cursor;
            if is_focused {
                let sel_style = Style::default()
                    .fg(Color::White)
                    .bg(Color::Indexed(208))
                    .add_modifier(Modifier::BOLD);
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(format!("[{check}] {name}"), sel_style),
                ]));
            } else {
                lines.push(Line::from(vec![Span::styled(
                    format!("    [{check}] {name}"),
                    normal_style,
                )]));
            }
        }
    } else {
        // Collapsed two-option view
        let mat_style = if focused == ModalOption::Materialize {
            highlight_style
        } else {
            normal_style
        };
        let arrow = if focused == ModalOption::Materialize {
            "\u{25b8}"
        } else {
            " "
        };
        lines.push(Line::from(vec![Span::styled(
            format!("{arrow} Materialize in all worktrees"),
            mat_style,
        )]));
    }

    // Delete option
    let del_arrow = if focused == ModalOption::DeleteAll {
        "\u{25b8}"
    } else {
        " "
    };
    let del_style = if focused == ModalOption::DeleteAll {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        normal_style
    };
    lines.push(Line::from(vec![Span::styled(
        format!("{del_arrow} Delete everywhere"),
        del_style,
    )]));

    lines.push(Line::raw(""));

    // Help line
    if expanded {
        lines.push(Line::from(vec![
            Span::styled("Space", key_style),
            Span::styled(" toggle  ", dim_style),
            Span::styled("\u{2190}/h", key_style),
            Span::styled(" collapse  ", dim_style),
            Span::styled("Enter", key_style),
            Span::styled(" confirm", dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("jk", key_style),
            Span::styled(" navigate  ", dim_style),
            Span::styled("\u{2192}/l", key_style),
            Span::styled(" expand  ", dim_style),
            Span::styled("Enter", key_style),
            Span::styled(" confirm", dim_style),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("Esc", key_style),
        Span::styled(" cancel", dim_style),
    ]));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, dialog_area);
}
