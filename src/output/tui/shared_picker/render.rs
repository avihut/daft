//! Ratatui rendering for the shared picker TUI.
//!
//! Uses `PickerMode` for tab decorations, entry markers/tags, warnings,
//! and footer rendering.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs,
    },
    Frame,
};

use super::highlight::Highlighter;
use super::state::{FileTabState, FocusPanel, PickerState};
use super::PickerMode;

/// Accent color matching the project's ACCENT_COLOR_INDEX (orange 208).
const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;
const GREEN: Color = Color::Green;
const SELECTED_BG: Color = Color::Indexed(236);

/// Render the entire picker UI.
pub fn render(
    state: &mut PickerState,
    mode: &dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
) {
    let area = frame.area();

    frame.render_widget(Clear, area);

    let has_warning = mode.tab_warning(state.current_tab()).is_some();
    let warning_height = if has_warning { 1 } else { 0 };
    let footer_height = mode.footer_height();

    // Layout: tabs (2) | warning (0-1) | body (fill) | footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(warning_height),
            Constraint::Min(5),
            Constraint::Length(footer_height),
        ])
        .split(area);

    render_tabs(state, mode, frame, chunks[0]);
    if has_warning {
        render_warning(state.current_tab(), mode, frame, chunks[1]);
    }
    render_body(state, mode, highlighter, frame, chunks[2]);
    mode.render_footer(state, frame, chunks[3]);
}

/// Render a warning between tabs and body.
fn render_warning(tab: &FileTabState, mode: &dyn PickerMode, frame: &mut Frame, area: Rect) {
    if let Some(msg) = mode.tab_warning(tab) {
        let line = Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(Color::Yellow),
        ));
        frame.render_widget(Paragraph::new(line), area);
    }
}

/// Render the tab bar at the top.
fn render_tabs(state: &PickerState, mode: &dyn PickerMode, frame: &mut Frame, area: Rect) {
    let tab_bar_focused = state.focus == FocusPanel::TabBar;

    let titles: Vec<Line> = state
        .tabs
        .iter()
        .map(|tab| {
            let has_decision = mode.tab_decided(tab);
            let icon = if has_decision { " \u{2713}" } else { "" };
            let style = if has_decision {
                Style::default().fg(GREEN)
            } else {
                Style::default()
            };
            Line::from(Span::styled(format!(" {}{} ", tab.rel_path, icon), style))
        })
        .collect();

    let highlight_style = if tab_bar_focused {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(ACCENT)
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED)
    };

    let tabs = Tabs::new(titles)
        .select(state.active_tab)
        .highlight_style(highlight_style)
        .divider(Span::raw(" | "));

    frame.render_widget(tabs, area);
}

/// Render the main body.
fn render_body(
    state: &mut PickerState,
    mode: &dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    if state.current_tab().is_stub {
        let tab = state.current_tab();
        render_stub_body(tab, frame, area);
    } else {
        render_split_body(state, mode, highlighter, frame, area);
    }
}

/// Render the stub body for files that exist in no worktree.
fn render_stub_body(tab: &FileTabState, frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::raw(""),
        Line::styled(
            "  No copies found in any worktree.",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(
            "  This path will be skipped. Use `daft shared add`",
            Style::default().fg(DIM),
        ),
        Line::styled(
            "  to collect it after creating it in a worktree.",
            Style::default().fg(DIM),
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            format!(" {} ", tab.rel_path),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the split body with worktree list (left) and preview (right).
fn render_split_body(
    state: &mut PickerState,
    mode: &dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    let tab = state.current_tab();
    render_worktree_list(state.focus, tab, mode, frame, chunks[0]);
    render_preview(state, highlighter, frame, chunks[1]);
}

/// Render the worktree list panel (left).
fn render_worktree_list(
    focus: FocusPanel,
    tab: &FileTabState,
    mode: &dyn PickerMode,
    frame: &mut Frame,
    area: Rect,
) {
    let is_focused = focus == FocusPanel::WorktreeList;
    let border_color = if is_focused { ACCENT } else { DIM };

    let items: Vec<ListItem> = tab
        .entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_cursor = idx == tab.list_cursor && is_focused;
            let decoration = mode.entry_decoration(tab, idx);
            let is_selected = tab.selected == Some(idx);

            let pointer = if is_cursor { "\u{25b8} " } else { "  " };

            // Worktrees with the file get normal color, those without are muted
            let style = if is_selected {
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
            } else if !entry.has_file {
                Style::default().fg(DIM)
            } else if is_cursor {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::Reset)
            };

            let bg_style = if is_cursor {
                style.bg(SELECTED_BG)
            } else {
                style
            };

            let mut spans = vec![
                Span::styled(pointer, bg_style),
                Span::styled(decoration.marker, bg_style),
                Span::styled(entry.worktree_name.clone(), bg_style),
            ];

            // Show tag from mode (e.g. "materialized" / "linked")
            if let Some((tag_text, tag_color)) = decoration.tag {
                spans.push(Span::styled(
                    format!(" {tag_text}"),
                    Style::default().fg(tag_color),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " Worktrees ",
            Style::default().fg(border_color),
        ));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render the file preview panel (right).
fn render_preview(
    state: &mut PickerState,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    let tab = state.current_tab();
    let is_focused = state.focus == FocusPanel::Preview;
    let border_color = if is_focused { ACCENT } else { DIM };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(" Preview ", Style::default().fg(border_color)));

    let entry = &tab.entries[tab.list_cursor];
    let highlighted_lines = if !entry.has_file {
        vec![Line::styled(
            "(no file in this worktree)",
            Style::default().fg(DIM),
        )]
    } else {
        let file_path = entry.worktree_path.join(&tab.rel_path);
        if file_path.is_dir() {
            dir_listing_lines(&file_path)
        } else {
            match std::fs::read_to_string(&file_path) {
                Ok(content) if content.is_empty() => {
                    vec![Line::styled("(empty file)", Style::default().fg(DIM))]
                }
                Ok(content) => highlighter.highlight(&content, &tab.rel_path),
                Err(_) => vec![Line::styled(
                    "(unable to read file)",
                    Style::default().fg(DIM),
                )],
            }
        }
    };

    let content_lines = highlighted_lines.len() as u16;
    // Viewport height = area minus 2 for border
    let viewport_height = area.height.saturating_sub(2);
    let scroll = tab.preview_scroll;
    let is_scrollable = content_lines > viewport_height;

    // Update state with content dimensions for scroll clamping
    let tab_mut = &mut state.tabs[state.active_tab];
    tab_mut.preview_content_lines = content_lines;
    tab_mut.preview_viewport_height = viewport_height;

    let paragraph = Paragraph::new(highlighted_lines)
        .block(block)
        .scroll((scroll, 0));

    frame.render_widget(paragraph, area);

    // Show scrollbar when preview is focused and content is scrollable
    if is_focused && is_scrollable {
        let max_scroll = content_lines.saturating_sub(viewport_height);
        let mut scrollbar_state =
            ScrollbarState::new(max_scroll as usize).position(scroll as usize);
        let scrollbar =
            Scrollbar::new(ScrollbarOrientation::VerticalRight).style(Style::default().fg(DIM));
        // Render inside the block's inner area (inset by 1 for borders)
        let scrollbar_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: viewport_height,
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

/// Build preview lines for a directory, showing its contents as a tree.
fn dir_listing_lines(dir: &std::path::Path) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        "(directory)",
        Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
    )];

    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                (name, is_dir)
            })
            .collect(),
        Err(_) => {
            lines.push(Line::styled(
                "(unable to read directory)",
                Style::default().fg(DIM),
            ));
            return lines;
        }
    };
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    for (name, is_dir) in &entries {
        let suffix = if *is_dir { "/" } else { "" };
        lines.push(Line::from(Span::styled(
            format!("  {name}{suffix}"),
            Style::default().fg(if *is_dir {
                Color::Indexed(208)
            } else {
                Color::White
            }),
        )));
    }

    if entries.is_empty() {
        lines.push(Line::styled(
            "  (empty directory)",
            Style::default().fg(DIM),
        ));
    }

    lines
}
