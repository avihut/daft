//! Ratatui rendering for the shared picker TUI.
//!
//! Uses `PickerMode` for tab decorations, entry markers/tags, warnings,
//! and footer rendering.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs,
    },
};

use super::super::columns::truncate_with_ellipsis;
use super::PickerMode;
use super::highlight::Highlighter;
use super::state::{FileTabState, FocusPanel, PickerState};

/// Accent color matching the project's ACCENT_COLOR_INDEX (orange 208).
const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;
const GREEN: Color = Color::Green;
const SELECTED_BG: Color = Color::Indexed(236);

/// Below this width the worktree-name column is meaningless, so we stop
/// shrinking and accept that the right-aligned state tag may be clipped at
/// extreme terminal narrowness. Mirrors `BRANCH_MIN_WIDTH` in `columns.rs`
/// by convention.
const WORKTREE_NAME_MIN_WIDTH: usize = 12;

/// Compute the budget (in display chars) for a worktree-name cell.
///
/// `tag_column_width` is the max tag length across all rows in the tab so
/// the name-column right edge stays stable as you scroll, even when some
/// rows have shorter tags or no tag at all.
fn name_budget(
    inner_width: usize,
    pointer_len: usize,
    marker_len: usize,
    tag_column_width: usize,
    right_margin: usize,
) -> u16 {
    let chrome = pointer_len + marker_len + tag_column_width + right_margin;
    inner_width
        .saturating_sub(chrome)
        .max(WORKTREE_NAME_MIN_WIDTH) as u16
}

/// Render the entire picker UI.
pub fn render(
    state: &mut PickerState,
    mode: &mut dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
) {
    let area = frame.area();

    frame.render_widget(Clear, area);

    let footer_height = mode.footer_height();

    // Layout: tabs (1) | info/spacer (1, always present) | body (fill) | footer
    // The info row doubles as the spacer between tabs and body. When a
    // warning/info message is active it fills this row; otherwise it stays
    // blank. This avoids pushing the body down when a message appears.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(footer_height),
        ])
        .split(area);

    render_tabs(state, mode, frame, chunks[0]);
    if !state.is_virtual_tab()
        && let Some(msg) = mode.tab_warning(state.current_tab())
    {
        let line = Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(Color::Yellow),
        ));
        frame.render_widget(Paragraph::new(line), chunks[1]);
    }
    render_body(state, mode, highlighter, frame, chunks[2]);
    mode.render_footer(state, frame, chunks[3]);
}

/// Render the tab bar at the top.
fn render_tabs(state: &PickerState, mode: &mut dyn PickerMode, frame: &mut Frame, area: Rect) {
    let tab_bar_focused = state.focus == FocusPanel::TabBar;

    let mut titles: Vec<Line> = state
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

    // Append extra virtual tabs from the mode
    for label in mode.extra_tab_labels() {
        titles.push(Line::from(Span::styled(
            format!(" {label} "),
            Style::default().fg(DIM),
        )));
    }

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
    mode: &mut dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    if state.is_virtual_tab() {
        // Virtual tab (e.g., "+") — show a simple message
        let text = vec![
            Line::raw(""),
            Line::styled(
                "  Press Enter or 'a' to add a shared file.",
                Style::default().add_modifier(Modifier::BOLD),
            ),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM));
        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, area);
    } else if state.current_tab().is_stub {
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
    mode: &mut dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    let tab = state.current_tab();
    render_worktree_list(state.focus, tab, state.active_tab, mode, frame, chunks[0]);
    render_preview(state, mode, highlighter, frame, chunks[1]);
}

/// Render the worktree list panel (left).
fn render_worktree_list(
    focus: FocusPanel,
    tab: &FileTabState,
    tab_idx: usize,
    mode: &mut dyn PickerMode,
    frame: &mut Frame,
    area: Rect,
) {
    let is_focused = focus == FocusPanel::WorktreeList;
    let border_color = if is_focused { ACCENT } else { DIM };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " Worktrees ",
            Style::default().fg(border_color),
        ));

    // Inner width for right-aligning tags
    let inner_width = block.inner(area).width as usize;
    let editing_shared = mode.is_editing_shared();

    // Reserve a uniform state-tag column based on the widest tag in the
    // tab, so name-column right edges line up across rows and don't jitter
    // when shorter tags scroll past longer ones.
    let tag_column_width: usize = (0..tab.entries.len())
        .filter_map(|idx| mode.entry_decoration(tab, tab_idx, idx).tag)
        .map(|(t, _)| t.chars().count())
        .max()
        .unwrap_or(0);

    let items: Vec<ListItem> = tab
        .entries
        .iter()
        .enumerate()
        .map(|(idx, entry)| {
            let is_current = idx == tab.list_cursor;
            let is_cursor = is_current && is_focused;
            let decoration = mode.entry_decoration(tab, tab_idx, idx);
            let is_selected = tab.selected == Some(idx);

            // When editing a shared file, highlight all linked worktrees
            let is_co_edited = editing_shared
                && !is_current
                && decoration.tag.as_ref().is_some_and(|(t, _)| t == "linked");

            let pointer = if is_current { "\u{25b8} " } else { "  " };

            // Worktrees with the file get normal color, those without are muted.
            // Active cursor: bright with background. Inactive cursor: subtle indicator.
            let style = if is_cursor {
                Style::default()
                    .fg(Color::White)
                    .bg(SELECTED_BG)
                    .add_modifier(Modifier::BOLD)
            } else if is_current && !is_focused {
                // Show which entry is selected even when focus is elsewhere
                Style::default().fg(Color::White).bg(SELECTED_BG)
            } else if is_co_edited {
                // Other linked worktrees affected by the shared edit
                Style::default().fg(Color::Green).bg(SELECTED_BG)
            } else if is_selected {
                Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
            } else if !entry.has_file {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(Color::Reset)
            };

            // Truncate the name to a budget that leaves room for the
            // reserved state-tag column on the right. Without this, long
            // branch names overflow and clip the tag (issue #503).
            let right_margin = 1;
            let pointer_len = pointer.chars().count();
            let marker_len = decoration.marker.chars().count();
            let budget = name_budget(
                inner_width,
                pointer_len,
                marker_len,
                tag_column_width,
                right_margin,
            );
            let displayed_name = truncate_with_ellipsis(&entry.worktree_name, budget);

            let mut spans = vec![
                Span::styled(pointer, style),
                Span::styled(decoration.marker.clone(), style),
                Span::styled(displayed_name.clone(), style),
            ];

            // Right-align the status tag (use char count for correct Unicode width)
            if let Some((tag_text, tag_color)) = decoration.tag {
                let left_len = pointer_len + marker_len + displayed_name.chars().count();
                let tag_len = tag_text.chars().count();
                let padding = inner_width.saturating_sub(left_len + tag_len + right_margin);

                let highlight_row = is_current || is_co_edited;
                let pad_style = if highlight_row {
                    Style::default().bg(SELECTED_BG)
                } else {
                    Style::default()
                };
                let tag_style = if highlight_row {
                    Style::default()
                        .fg(if is_co_edited {
                            Color::Green
                        } else {
                            Color::White
                        })
                        .bg(SELECTED_BG)
                } else {
                    Style::default().fg(tag_color)
                };

                spans.push(Span::styled(" ".repeat(padding), pad_style));
                spans.push(Span::styled(tag_text, tag_style));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render the file preview panel (right).
fn render_preview(
    state: &mut PickerState,
    mode: &mut dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    if mode.render_editor(frame, area) {
        return;
    }

    let tab = state.current_tab();
    let is_focused = state.focus == FocusPanel::Preview;
    let border_color = if is_focused { ACCENT } else { DIM };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(" Preview ", Style::default().fg(border_color)));

    let highlighted_lines = if let Some(lines) = mode.preview_override(state) {
        lines
    } else {
        let entry = &tab.entries[tab.list_cursor];
        if !entry.has_file {
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
        }
    };

    // Prepend line numbers
    let line_count = highlighted_lines.len();
    let num_width = if line_count == 0 {
        1
    } else {
        (line_count as f64).log10().floor() as usize + 1
    };
    let line_num_style = Style::default().fg(Color::Indexed(239));
    let numbered_lines: Vec<Line> = highlighted_lines
        .into_iter()
        .enumerate()
        .map(|(i, mut line)| {
            let num = format!("{:>width$} ", i + 1, width = num_width);
            line.spans.insert(0, Span::styled(num, line_num_style));
            line
        })
        .collect();

    let content_lines = numbered_lines.len() as u16;
    // Viewport height = area minus 2 for border
    let viewport_height = area.height.saturating_sub(2);
    let scroll = tab.preview_scroll;
    let is_scrollable = content_lines > viewport_height;

    // Update state with content dimensions for scroll clamping
    let tab_mut = &mut state.tabs[state.active_tab];
    tab_mut.preview_content_lines = content_lines;
    tab_mut.preview_viewport_height = viewport_height;

    let paragraph = Paragraph::new(numbered_lines)
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

#[cfg(test)]
mod tests {
    use super::*;

    // Standard chrome: pointer=2 ("▸ " / "  "), marker=2, right_margin=1.
    // tag_col varies per case.

    #[test]
    fn name_budget_wide_terminal_returns_full_remainder() {
        // 80 inner, chrome=2+2+7+1=12, budget=68 — full names render untruncated
        // for any realistic branch name.
        assert_eq!(name_budget(80, 2, 2, 7, 1), 68);
    }

    #[test]
    fn name_budget_medium_terminal_shrinks_name_above_min() {
        // 28 inner (100-col terminal × 30% panel ≈ 30, minus borders = 28),
        // chrome=12, budget=16. Long names get ellipsis-truncated to 16.
        assert_eq!(name_budget(28, 2, 2, 7, 1), 16);
    }

    #[test]
    fn name_budget_tight_terminal_clamps_to_minimum() {
        // 18 inner, chrome=12, naive budget=6 — clamped up to MIN=12 so
        // names remain readable; tag may begin to overflow.
        assert_eq!(name_budget(18, 2, 2, 7, 1), WORKTREE_NAME_MIN_WIDTH as u16);
    }

    #[test]
    fn name_budget_extreme_narrow_still_clamps_to_minimum() {
        // 10 inner — way too narrow for chrome. saturating_sub yields 0,
        // clamp lifts to MIN=12. Tag will be clipped by panel; acceptable
        // at this width.
        assert_eq!(name_budget(10, 2, 2, 7, 1), WORKTREE_NAME_MIN_WIDTH as u16);
    }

    #[test]
    fn name_budget_no_tags_in_tab_uses_remaining_space() {
        // tag_col=0 (no entries have tags), so the name gets all the space
        // minus pointer + marker + right margin.
        assert_eq!(name_budget(28, 2, 2, 0, 1), 23);
    }

    #[test]
    fn truncates_long_name_with_ellipsis_at_budget() {
        let name = "daft-503/very-long-feature-name-that-takes-many-columns";
        assert!(name.chars().count() > 16);
        let budget = name_budget(28, 2, 2, 7, 1);
        assert_eq!(budget, 16);

        let displayed = truncate_with_ellipsis(name, budget);
        assert_eq!(displayed.chars().count(), 16);
        assert!(displayed.ends_with("..."));
        assert!(displayed.starts_with("daft-503/"));
    }

    #[test]
    fn keeps_short_name_unchanged_when_under_budget() {
        let name = "main";
        let budget = name_budget(80, 2, 2, 7, 1);
        let displayed = truncate_with_ellipsis(name, budget);
        assert_eq!(displayed, "main");
    }

    #[test]
    fn truncation_respects_char_boundaries_on_multibyte_names() {
        // CJK characters are multi-byte; truncation must not split a
        // codepoint. `truncate_with_ellipsis` operates on `chars()` so this
        // is safe — verify the output is a valid String and the right
        // number of chars.
        let name = "日本語のとても長いブランチ名";
        let budget: u16 = 8;
        let displayed = truncate_with_ellipsis(name, budget);
        assert_eq!(displayed.chars().count(), 8);
        assert!(displayed.ends_with("..."));
    }
}
