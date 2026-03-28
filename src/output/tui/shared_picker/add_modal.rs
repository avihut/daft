//! Add-file modal for the manage picker TUI.
//!
//! Presents a centered overlay with an interactive file tree browser.
//! The user can navigate the tree, search by typing, and select a file
//! to share or declare a new shared file path.

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
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::input::poll_key;

/// Maximum recursion depth when scanning the file tree.
const MAX_DEPTH: usize = 5;

/// Result of the add-file modal interaction.
pub enum AddResult {
    /// User selected an existing file to share.
    Selected(PathBuf),
    /// User typed a name that doesn't exist — declare it.
    Declared(String),
    /// User cancelled.
    Cancelled,
}

/// A single entry in the flattened file tree.
struct TreeEntry {
    /// Path relative to worktree root.
    path: PathBuf,
    /// Display name (file/dir basename).
    name: String,
    /// Nesting level (0 = top-level).
    depth: usize,
    /// Whether this entry is a directory.
    is_dir: bool,
    /// Whether this directory is expanded (only meaningful for dirs).
    expanded: bool,
    /// Whether this path is already shared.
    is_shared: bool,
}

/// Internal state for the add-file modal.
struct AddModalState {
    /// Current search string.
    search: String,
    /// All entries (flat, pre-sorted).
    entries: Vec<TreeEntry>,
    /// Indices into `entries` that are currently visible.
    visible: Vec<usize>,
    /// Cursor position within `visible`.
    cursor: usize,
    /// Scroll offset for the file list.
    scroll: usize,
    /// Worktree root directory.
    worktree_root: PathBuf,
    /// Already-shared paths (for dimming).
    shared_paths: Vec<String>,
}

impl AddModalState {
    fn new(worktree_root: &Path, shared_paths: &[String]) -> Self {
        let mut state = Self {
            search: String::new(),
            entries: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            scroll: 0,
            worktree_root: worktree_root.to_path_buf(),
            shared_paths: shared_paths.to_vec(),
        };
        state.scan_directory();
        state.recompute_visible();
        state
    }

    /// Recursively scan the worktree directory and build the flat entry list.
    fn scan_directory(&mut self) {
        self.entries.clear();
        self.scan_dir_recursive(&self.worktree_root.clone(), 0);
    }

    fn scan_dir_recursive(&mut self, dir: &Path, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }

        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();

        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip .git directory
            if name == ".git" {
                continue;
            }

            let path = entry.path();
            let is_dir = path.is_dir();
            children.push((name, path, is_dir));
        }

        // Sort: directories first, then files, alphabetically within each group
        children.sort_by(|(name_a, _, is_dir_a), (name_b, _, is_dir_b)| {
            is_dir_b
                .cmp(is_dir_a)
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase()))
        });

        for (name, full_path, is_dir) in children {
            let rel_path = full_path
                .strip_prefix(&self.worktree_root)
                .unwrap_or(&full_path)
                .to_path_buf();

            let rel_str = rel_path.to_string_lossy().to_string();
            let is_shared = self.shared_paths.iter().any(|s| s == &rel_str);

            self.entries.push(TreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                expanded: false,
                is_shared,
            });

            // Don't recurse here — directories start collapsed.
            // We scan children lazily when expanded.
        }
    }

    /// Recompute visible entries based on search and expansion state.
    fn recompute_visible(&mut self) {
        self.visible.clear();

        if self.search.is_empty() {
            // No search: show entries respecting collapsed/expanded state
            self.compute_visible_tree();
        } else {
            // Search active: filter and auto-expand
            self.compute_visible_search();
        }

        // Clamp cursor
        if self.visible.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.visible.len() {
            self.cursor = self.visible.len() - 1;
        }
        self.clamp_scroll();
    }

    /// Compute visible entries for the normal tree view (no search).
    fn compute_visible_tree(&mut self) {
        // We need to walk entries and only show those whose parent directories
        // are all expanded. Since entries are flat with depth, an entry at
        // depth N is visible if all ancestor dirs (depth 0..N-1) are expanded.
        //
        // Because our scan only produces top-level entries initially
        // (children are added when expanded), we can simply show all entries
        // at depth 0, and for deeper entries check if parent is expanded.

        let mut visible_depth_stack: Vec<bool> = vec![true]; // depth 0 always visible

        for (idx, entry) in self.entries.iter().enumerate() {
            // Check visibility: all ancestor levels must be "visible"
            let visible = if entry.depth == 0 {
                true
            } else {
                entry.depth < visible_depth_stack.len() && visible_depth_stack[entry.depth]
            };

            if visible {
                self.visible.push(idx);

                // Update stack: if this is an expanded dir, children are visible
                if entry.is_dir {
                    let child_depth = entry.depth + 1;
                    while visible_depth_stack.len() <= child_depth {
                        visible_depth_stack.push(false);
                    }
                    visible_depth_stack[child_depth] = entry.expanded;
                }
            }
        }
    }

    /// Compute visible entries for search mode.
    fn compute_visible_search(&mut self) {
        let search_lower = self.search.to_lowercase();

        for (idx, entry) in self.entries.iter().enumerate() {
            let path_str = entry.path.to_string_lossy().to_lowercase();
            if path_str.contains(&search_lower) {
                self.visible.push(idx);
            }
        }
    }

    /// Toggle expand/collapse for the currently highlighted directory.
    fn toggle_expand(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let entry_idx = self.visible[self.cursor];
        if !self.entries[entry_idx].is_dir {
            return;
        }

        let was_expanded = self.entries[entry_idx].expanded;
        self.entries[entry_idx].expanded = !was_expanded;

        if !was_expanded {
            // Expanding: insert children after this entry
            self.expand_dir(entry_idx);
        } else {
            // Collapsing: remove all children from entries
            self.collapse_dir(entry_idx);
        }

        self.recompute_visible();
    }

    /// Expand a directory: scan and insert its children after it in the entries list.
    fn expand_dir(&mut self, dir_idx: usize) {
        let dir_path = self.worktree_root.join(&self.entries[dir_idx].path);
        let child_depth = self.entries[dir_idx].depth + 1;

        if child_depth > MAX_DEPTH {
            return;
        }

        // Check if children are already present (re-expanding after collapse)
        let has_children = self
            .entries
            .get(dir_idx + 1)
            .is_some_and(|e| e.depth == child_depth);

        if has_children {
            // Children already in the list, just toggle expanded flag (already done)
            return;
        }

        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();

        if let Ok(read_dir) = std::fs::read_dir(&dir_path) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" {
                    continue;
                }
                let path = entry.path();
                let is_dir = path.is_dir();
                children.push((name, path, is_dir));
            }
        }

        children.sort_by(|(name_a, _, is_dir_a), (name_b, _, is_dir_b)| {
            is_dir_b
                .cmp(is_dir_a)
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase()))
        });

        let insert_pos = dir_idx + 1;
        let mut new_entries: Vec<TreeEntry> = Vec::new();

        for (name, full_path, is_dir) in children {
            let rel_path = full_path
                .strip_prefix(&self.worktree_root)
                .unwrap_or(&full_path)
                .to_path_buf();
            let rel_str = rel_path.to_string_lossy().to_string();
            let is_shared = self.shared_paths.iter().any(|s| s == &rel_str);

            new_entries.push(TreeEntry {
                path: rel_path,
                name,
                depth: child_depth,
                is_dir,
                expanded: false,
                is_shared,
            });
        }

        // Insert children into entries list
        let tail = self.entries.split_off(insert_pos);
        self.entries.extend(new_entries);
        self.entries.extend(tail);
    }

    /// Collapse a directory: remove all descendant entries.
    fn collapse_dir(&mut self, dir_idx: usize) {
        let dir_depth = self.entries[dir_idx].depth;
        let mut remove_count = 0;

        for entry in &self.entries[dir_idx + 1..] {
            if entry.depth > dir_depth {
                remove_count += 1;
            } else {
                break;
            }
        }

        if remove_count > 0 {
            self.entries.drain(dir_idx + 1..dir_idx + 1 + remove_count);
        }
    }

    /// Expand the currently highlighted directory (or do nothing for files).
    fn expand_current(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let entry_idx = self.visible[self.cursor];
        if self.entries[entry_idx].is_dir && !self.entries[entry_idx].expanded {
            self.toggle_expand();
        }
    }

    /// Collapse the currently highlighted entry's parent, or collapse it if it's a dir.
    fn collapse_current(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        let entry_idx = self.visible[self.cursor];

        // If it's an expanded dir, collapse it
        if self.entries[entry_idx].is_dir && self.entries[entry_idx].expanded {
            self.toggle_expand();
            return;
        }

        // Otherwise, find the parent directory and move cursor to it
        let current_depth = self.entries[entry_idx].depth;
        if current_depth == 0 {
            return;
        }

        // Walk backward in visible to find a dir at depth - 1
        for i in (0..self.cursor).rev() {
            let idx = self.visible[i];
            if self.entries[idx].is_dir && self.entries[idx].depth < current_depth {
                self.cursor = i;
                self.clamp_scroll();
                break;
            }
        }
    }

    /// Clamp scroll so the cursor is always in view.
    fn clamp_scroll(&mut self) {
        // We'll compute the visible height dynamically in render, but keep
        // a reasonable default here.
        let page_size = 15usize;
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + page_size {
            self.scroll = self.cursor.saturating_sub(page_size - 1);
        }
    }

    /// Get the currently selected entry, if any.
    fn current_entry(&self) -> Option<&TreeEntry> {
        if self.visible.is_empty() {
            return None;
        }
        Some(&self.entries[self.visible[self.cursor]])
    }
}

/// Show the add-file modal and return the user's selection.
///
/// Renders as a centered overlay on top of the existing TUI content.
/// Has its own event loop for key handling.
pub fn show_add_modal(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    worktree_root: &Path,
    shared_paths: &[String],
) -> Result<AddResult> {
    let mut modal = AddModalState::new(worktree_root, shared_paths);

    loop {
        terminal.draw(|frame| {
            render_add_modal(frame, &modal);
        })?;

        let Some(key) = poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match key.code {
            KeyCode::Esc => return Ok(AddResult::Cancelled),
            KeyCode::Char('c')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                return Ok(AddResult::Cancelled);
            }
            KeyCode::Enter => {
                if modal.visible.is_empty() && !modal.search.is_empty() {
                    // No matches — declare mode
                    return Ok(AddResult::Declared(modal.search.clone()));
                }
                if let Some(entry) = modal.current_entry() {
                    if entry.is_dir {
                        // Expand/collapse directory on Enter
                        modal.toggle_expand();
                    } else {
                        return Ok(AddResult::Selected(entry.path.clone()));
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if !modal.visible.is_empty() && modal.cursor > 0 {
                    modal.cursor -= 1;
                    modal.clamp_scroll();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !modal.visible.is_empty() && modal.cursor < modal.visible.len() - 1 {
                    modal.cursor += 1;
                    modal.clamp_scroll();
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                modal.expand_current();
            }
            KeyCode::Left | KeyCode::Char('h') => {
                modal.collapse_current();
            }
            KeyCode::Backspace => {
                if !modal.search.is_empty() {
                    modal.search.pop();
                    // Re-scan when clearing search to get fresh tree state
                    if modal.search.is_empty() {
                        modal.scan_directory();
                    }
                    modal.recompute_visible();
                }
            }
            KeyCode::Char(c) => {
                // j/k are navigation only, not search
                if c == 'j' || c == 'k' {
                    // Already handled above
                } else {
                    modal.search.push(c);
                    // When search is active, re-scan to get all entries
                    // (so search can find nested files)
                    if modal.search.len() == 1 {
                        // Just started typing: do a full deep scan
                        modal.full_scan();
                    }
                    modal.recompute_visible();
                }
            }
            _ => {}
        }
    }
}

impl AddModalState {
    /// Full recursive scan of all entries (for search mode).
    fn full_scan(&mut self) {
        self.entries.clear();
        self.full_scan_recursive(&self.worktree_root.clone(), 0);
    }

    fn full_scan_recursive(&mut self, dir: &Path, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }

        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();

        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(_) => return,
        };

        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            let path = entry.path();
            let is_dir = path.is_dir();
            children.push((name, path, is_dir));
        }

        children.sort_by(|(name_a, _, is_dir_a), (name_b, _, is_dir_b)| {
            is_dir_b
                .cmp(is_dir_a)
                .then_with(|| name_a.to_lowercase().cmp(&name_b.to_lowercase()))
        });

        for (name, full_path, is_dir) in children {
            let rel_path = full_path
                .strip_prefix(&self.worktree_root)
                .unwrap_or(&full_path)
                .to_path_buf();
            let rel_str = rel_path.to_string_lossy().to_string();
            let is_shared = self.shared_paths.iter().any(|s| s == &rel_str);

            self.entries.push(TreeEntry {
                path: rel_path,
                name,
                depth,
                is_dir,
                expanded: false,
                is_shared,
            });

            if is_dir {
                self.full_scan_recursive(&full_path, depth + 1);
            }
        }
    }
}

/// Render the add-file modal overlay.
fn render_add_modal(frame: &mut ratatui::Frame, modal: &AddModalState) {
    let area = frame.area();

    // ~70% of screen
    let dialog_width = ((area.width as f32 * 0.7) as u16)
        .max(40)
        .min(area.width.saturating_sub(4));
    let dialog_height = ((area.height as f32 * 0.7) as u16)
        .max(12)
        .min(area.height.saturating_sub(2));
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Indexed(208)))
        .title(Span::styled(
            " Add Shared File ",
            Style::default()
                .fg(Color::Indexed(208))
                .add_modifier(Modifier::BOLD),
        ));

    // Inner area (inside border)
    let inner = block.inner(dialog_area);

    let mut lines: Vec<Line> = Vec::new();

    // Search bar
    let search_display = if modal.search.is_empty() {
        "Search: (type to filter)".to_string()
    } else {
        format!("Search: {}_", modal.search)
    };
    let search_style = if modal.search.is_empty() {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };
    lines.push(Line::from(Span::styled(search_display, search_style)));
    lines.push(Line::raw(""));

    // File tree or no-results message
    if modal.visible.is_empty() && !modal.search.is_empty() {
        // No results — declare mode
        lines.push(Line::styled(
            "No matching files found",
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::styled(
            format!(
                "Press Enter to declare '{}' as a new shared file",
                modal.search
            ),
            Style::default().fg(Color::Yellow),
        ));
    } else if modal.visible.is_empty() {
        lines.push(Line::styled(
            "No files found",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        // How many lines are available for the file list
        // inner height - search line (1) - blank (1) - help lines (2) - blank before help (1)
        let list_height = inner.height.saturating_sub(6) as usize;

        // Compute effective scroll for rendering
        let scroll = if modal.cursor < modal.scroll {
            modal.cursor
        } else if modal.cursor >= modal.scroll + list_height {
            modal.cursor.saturating_sub(list_height - 1)
        } else {
            modal.scroll
        };

        let visible_slice = &modal.visible[scroll..modal.visible.len().min(scroll + list_height)];

        for (vis_offset, &entry_idx) in visible_slice.iter().enumerate() {
            let entry = &modal.entries[entry_idx];
            let is_cursor = scroll + vis_offset == modal.cursor;

            let indent = "  ".repeat(entry.depth);
            let prefix = if entry.is_dir {
                if entry.expanded {
                    "\u{25be} " // ▾
                } else {
                    "\u{25b8} " // ▸
                }
            } else {
                "  "
            };

            let display = format!("{indent}{prefix}{}", entry.name);

            let style = if is_cursor {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Indexed(208))
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_shared {
                Style::default().fg(Color::DarkGray)
            } else if entry.is_dir {
                Style::default().fg(Color::Indexed(208))
            } else {
                Style::default().fg(Color::White)
            };

            let mut spans = vec![Span::styled(display, style)];

            if entry.is_shared {
                spans.push(Span::styled(
                    " (shared)",
                    Style::default().fg(Color::DarkGray),
                ));
            }

            lines.push(Line::from(spans));
        }

        // Pad remaining lines
        let rendered = visible_slice.len();
        for _ in rendered..list_height {
            lines.push(Line::raw(""));
        }
    }

    lines.push(Line::raw(""));

    // Help line
    let key_style = Style::default().fg(Color::Cyan);
    let dim_style = Style::default().fg(Color::DarkGray);

    if modal.visible.is_empty() && !modal.search.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Enter", key_style),
            Span::styled(" declare  ", dim_style),
            Span::styled("Esc", key_style),
            Span::styled(" cancel", dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("jk/\u{2191}\u{2193}", key_style),
            Span::styled(" navigate  ", dim_style),
            Span::styled("\u{2192}/l", key_style),
            Span::styled(" expand  ", dim_style),
            Span::styled("\u{2190}/h", key_style),
            Span::styled(" collapse", dim_style),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Enter", key_style),
            Span::styled(" select  ", dim_style),
            Span::styled("Esc", key_style),
            Span::styled(" cancel", dim_style),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });

    frame.render_widget(paragraph, dialog_area);
}
