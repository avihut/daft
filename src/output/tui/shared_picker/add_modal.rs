//! Add-file modal for the manage picker TUI.
//!
//! Combines a fast synchronous tree view (browse mode) with background-indexed
//! fuzzy search (search mode). Uses `nucleo-matcher` for fuzzy matching and
//! the `ignore` crate for gitignore-aware file discovery.

use anyhow::Result;
use crossterm::event::KeyCode;
use ignore::WalkBuilder;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32String};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Terminal,
};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;
use std::{fs, thread};

use super::input::poll_key;

const MAX_SCAN_DEPTH: usize = 12;
const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;
/// Muted color for tracked (non-ignored) entries.
const MUTED: Color = Color::Indexed(243);

/// Result of the add-file modal interaction.
pub enum AddResult {
    /// User selected an existing file to share.
    Selected(PathBuf),
    /// User typed a name that doesn't exist — declare it.
    Declared(String),
    /// User cancelled.
    Cancelled,
}

// ---------------------------------------------------------------------------
// Tree types (browse mode)
// ---------------------------------------------------------------------------

struct TreeEntry {
    rel_path: String,
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    is_shared: bool,
    is_ignored: bool,
}

// ---------------------------------------------------------------------------
// Search types (search mode)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FileData {
    rel_path: String,
    is_dir: bool,
    is_shared: bool,
    is_ignored: bool,
    match_text: Utf32String,
}

struct ScoredEntry {
    entry_idx: usize,
    score: u32,
    indices: Vec<u32>,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct AddModalState {
    search: String,

    // Tree (browse mode) — loaded synchronously on open
    tree_entries: Vec<TreeEntry>,
    tree_visible: Vec<usize>,

    // Search index (background)
    search_entries: Vec<FileData>,
    entry_rx: mpsc::Receiver<Vec<FileData>>,
    indexing_done: Arc<AtomicBool>,
    search_results: Vec<ScoredEntry>,
    search_dirty: bool,

    // Gitignore set from background (for tree expand)
    non_ignored: Arc<Mutex<Option<HashSet<String>>>>,

    // UI
    cursor: usize,
    scroll: usize,
    matcher: Matcher,
    list_height: usize,

    worktree_root: PathBuf,
    shared_paths: Vec<String>,
}

impl AddModalState {
    fn new(worktree_root: &Path, shared_paths: &[String]) -> Self {
        // Synchronous top-level scan (instant)
        let tree_entries = scan_top_level(worktree_root, shared_paths);
        let tree_visible = compute_tree_visible(&tree_entries);

        // Background indexing
        let (tx, rx) = mpsc::channel();
        let indexing_done = Arc::new(AtomicBool::new(false));
        let non_ignored: Arc<Mutex<Option<HashSet<String>>>> = Arc::new(Mutex::new(None));

        let done = indexing_done.clone();
        let ni = non_ignored.clone();
        let root = worktree_root.to_path_buf();
        let shared = shared_paths.to_vec();

        thread::spawn(move || {
            index_files(&root, &shared, &ni, &tx);
            done.store(true, Ordering::Release);
        });

        Self {
            search: String::new(),
            tree_entries,
            tree_visible,
            search_entries: Vec::new(),
            entry_rx: rx,
            indexing_done,
            search_results: Vec::new(),
            search_dirty: true,
            non_ignored,
            cursor: 0,
            scroll: 0,
            matcher: Matcher::new(MatcherConfig::DEFAULT),
            list_height: 20,
            worktree_root: worktree_root.to_path_buf(),
            shared_paths: shared_paths.to_vec(),
        }
    }

    /// Number of visible items in the current mode.
    fn visible_count(&self) -> usize {
        if self.search.is_empty() {
            self.tree_visible.len()
        } else {
            self.search_results.len()
        }
    }

    fn receive_search_entries(&mut self) {
        let mut received = false;
        while let Ok(batch) = self.entry_rx.try_recv() {
            self.search_entries.extend(batch);
            received = true;
        }
        if received {
            self.search_dirty = true;
        }
    }

    fn recompute_search(&mut self) {
        if !self.search_dirty || self.search.is_empty() {
            return;
        }
        self.search_dirty = false;
        let mut new_results = Vec::new();

        let pattern = Pattern::parse(&self.search, CaseMatching::Ignore, Normalization::Smart);
        let entries = &self.search_entries;
        let matcher = &mut self.matcher;

        for (idx, entry) in entries.iter().enumerate() {
            let mut indices = Vec::new();
            if let Some(score) = pattern.indices(entry.match_text.slice(..), matcher, &mut indices)
            {
                new_results.push(ScoredEntry {
                    entry_idx: idx,
                    score,
                    indices,
                });
            }
        }

        new_results.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                self.search_entries[a.entry_idx]
                    .rel_path
                    .len()
                    .cmp(&self.search_entries[b.entry_idx].rel_path.len())
            })
        });

        self.search_results = new_results;

        if self.search_results.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.search_results.len() {
            self.cursor = self.search_results.len() - 1;
        }
    }

    fn clamp_scroll(&mut self) {
        let page = self.list_height.max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + page {
            self.scroll = self.cursor.saturating_sub(page - 1);
        }
    }

    // -- Tree operations (browse mode) --

    fn expand_current(&mut self) {
        if self.tree_visible.is_empty() {
            return;
        }
        let entry_idx = self.tree_visible[self.cursor];
        if !self.tree_entries[entry_idx].is_dir || self.tree_entries[entry_idx].expanded {
            return;
        }
        self.tree_entries[entry_idx].expanded = true;

        let child_depth = self.tree_entries[entry_idx].depth + 1;
        let has_children = self
            .tree_entries
            .get(entry_idx + 1)
            .is_some_and(|e| e.depth == child_depth);

        if !has_children {
            self.insert_children(entry_idx);
        }
        self.tree_visible = compute_tree_visible(&self.tree_entries);
    }

    fn insert_children(&mut self, dir_idx: usize) {
        let dir_path = self
            .worktree_root
            .join(&self.tree_entries[dir_idx].rel_path);
        let child_depth = self.tree_entries[dir_idx].depth + 1;

        let mut children: Vec<(String, PathBuf, bool)> = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir_path) {
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name == ".git" {
                    continue;
                }
                let path = entry.path();
                let is_dir = path.is_dir();
                children.push((name, path, is_dir));
            }
        }
        sort_children(&mut children);

        let ni_guard = self.non_ignored.lock().unwrap();
        let ni_set = ni_guard.as_ref();

        let new_entries: Vec<TreeEntry> = children
            .iter()
            .map(|(name, full_path, is_dir)| {
                let rel = full_path
                    .strip_prefix(&self.worktree_root)
                    .unwrap_or(full_path);
                let rel_str = rel.to_string_lossy().to_string();
                let is_shared = self.shared_paths.iter().any(|s| s == &rel_str);
                let is_ignored = match ni_set {
                    Some(set) => !set.contains(&rel_str),
                    None => false,
                };
                TreeEntry {
                    rel_path: rel_str,
                    name: name.clone(),
                    depth: child_depth,
                    is_dir: *is_dir,
                    expanded: false,
                    is_shared,
                    is_ignored,
                }
            })
            .collect();
        drop(ni_guard);

        let insert_pos = dir_idx + 1;
        let tail = self.tree_entries.split_off(insert_pos);
        self.tree_entries.extend(new_entries);
        self.tree_entries.extend(tail);
    }

    fn collapse_current(&mut self) {
        if self.tree_visible.is_empty() {
            return;
        }
        let entry_idx = self.tree_visible[self.cursor];

        if self.tree_entries[entry_idx].is_dir && self.tree_entries[entry_idx].expanded {
            self.tree_entries[entry_idx].expanded = false;
            let dir_depth = self.tree_entries[entry_idx].depth;
            let remove_count = self.tree_entries[entry_idx + 1..]
                .iter()
                .take_while(|e| e.depth > dir_depth)
                .count();
            if remove_count > 0 {
                self.tree_entries
                    .drain(entry_idx + 1..entry_idx + 1 + remove_count);
            }
        } else {
            // Navigate to parent directory
            let current_depth = self.tree_entries[entry_idx].depth;
            if current_depth == 0 {
                return;
            }
            for i in (0..self.cursor).rev() {
                let idx = self.tree_visible[i];
                if self.tree_entries[idx].is_dir && self.tree_entries[idx].depth < current_depth {
                    self.cursor = i;
                    break;
                }
            }
        }
        self.tree_visible = compute_tree_visible(&self.tree_entries);
    }
}

// ---------------------------------------------------------------------------
// Tree helpers
// ---------------------------------------------------------------------------

/// Sort children: directories first, then alphabetical.
fn sort_children(children: &mut [(String, PathBuf, bool)]) {
    children.sort_by(|(a_name, _, a_dir), (b_name, _, b_dir)| {
        b_dir
            .cmp(a_dir)
            .then_with(|| a_name.to_lowercase().cmp(&b_name.to_lowercase()))
    });
}

/// Synchronous top-level scan with gitignore awareness (instant).
fn scan_top_level(root: &Path, shared_paths: &[String]) -> Vec<TreeEntry> {
    // Quick gitignore check via ignore crate (only reads root dir)
    let mut non_ignored_top = HashSet::new();
    for entry in WalkBuilder::new(root)
        .hidden(false)
        .max_depth(Some(1))
        .build()
        .flatten()
    {
        if entry.path() == root {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            non_ignored_top.insert(rel.to_string_lossy().to_string());
        }
    }

    let mut children: Vec<(String, PathBuf, bool)> = Vec::new();
    if let Ok(rd) = fs::read_dir(root) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            let path = entry.path();
            let is_dir = path.is_dir();
            children.push((name, path, is_dir));
        }
    }
    sort_children(&mut children);

    children
        .iter()
        .map(|(name, full_path, is_dir)| {
            let rel = full_path.strip_prefix(root).unwrap_or(full_path);
            let rel_str = rel.to_string_lossy().to_string();
            let is_shared = shared_paths.iter().any(|s| s == &rel_str);
            let is_ignored = !non_ignored_top.contains(&rel_str);
            TreeEntry {
                rel_path: rel_str,
                name: name.clone(),
                depth: 0,
                is_dir: *is_dir,
                expanded: false,
                is_shared,
                is_ignored,
            }
        })
        .collect()
}

/// Compute which tree entries are visible given expansion state.
fn compute_tree_visible(entries: &[TreeEntry]) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut depth_visible: Vec<bool> = vec![true];

    for (idx, entry) in entries.iter().enumerate() {
        let is_vis = if entry.depth == 0 {
            true
        } else {
            entry.depth < depth_visible.len() && depth_visible[entry.depth]
        };
        if is_vis {
            visible.push(idx);
            if entry.is_dir {
                let child_depth = entry.depth + 1;
                while depth_visible.len() <= child_depth {
                    depth_visible.push(false);
                }
                depth_visible[child_depth] = entry.expanded;
            }
        }
    }
    visible
}

// ---------------------------------------------------------------------------
// Background indexing (for search mode)
// ---------------------------------------------------------------------------

fn index_files(
    root: &Path,
    shared_paths: &[String],
    non_ignored_out: &Arc<Mutex<Option<HashSet<String>>>>,
    tx: &mpsc::Sender<Vec<FileData>>,
) {
    // Step 1: Walk with gitignore → build non-ignored set
    let mut non_ignored = HashSet::new();
    for entry in WalkBuilder::new(root)
        .hidden(false)
        .max_depth(Some(MAX_SCAN_DEPTH))
        .build()
        .flatten()
    {
        if entry.path() == root {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            non_ignored.insert(rel.to_string_lossy().to_string());
        }
    }

    // Share set with main thread immediately (for tree expand)
    *non_ignored_out.lock().unwrap() = Some(non_ignored.clone());

    // Step 2: Walk ALL files and build search entries
    let mut all_entries: Vec<FileData> = Vec::new();

    for entry in WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .max_depth(Some(MAX_SCAN_DEPTH))
        .build()
        .flatten()
    {
        if entry.path() == root {
            continue;
        }
        let rel = match entry.path().strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();

        if rel_str == ".git" || rel_str.starts_with(".git/") || rel_str.starts_with(".git\\") {
            continue;
        }

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let is_shared = shared_paths.iter().any(|s| s == &rel_str);
        let is_ignored = !non_ignored.contains(&rel_str);
        let match_text = Utf32String::from(rel_str.as_str());

        all_entries.push(FileData {
            rel_path: rel_str,
            is_dir,
            is_shared,
            is_ignored,
            match_text,
        });
    }

    all_entries.sort_by(|a, b| a.rel_path.to_lowercase().cmp(&b.rel_path.to_lowercase()));
    let _ = tx.send(all_entries);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn show_add_modal(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    worktree_root: &Path,
    shared_paths: &[String],
) -> Result<AddResult> {
    let mut modal = AddModalState::new(worktree_root, shared_paths);

    loop {
        modal.receive_search_entries();
        if !modal.search.is_empty() {
            modal.recompute_search();
        }

        terminal.draw(|frame| {
            render_add_modal(frame, &mut modal);
        })?;

        let Some(key) = poll_key(Duration::from_millis(50)) else {
            continue;
        };

        let count = modal.visible_count();

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
                if modal.search.is_empty() {
                    // Browse mode: expand dir or select file
                    if let Some(&idx) = modal.tree_visible.get(modal.cursor) {
                        if modal.tree_entries[idx].is_dir {
                            modal.expand_current();
                        } else {
                            return Ok(AddResult::Selected(PathBuf::from(
                                &modal.tree_entries[idx].rel_path,
                            )));
                        }
                    }
                } else {
                    // Search mode: select or declare
                    if modal.search_results.is_empty() {
                        if modal.indexing_done.load(Ordering::Acquire) {
                            return Ok(AddResult::Declared(modal.search.clone()));
                        }
                    } else if let Some(r) = modal.search_results.get(modal.cursor) {
                        return Ok(AddResult::Selected(PathBuf::from(
                            &modal.search_entries[r.entry_idx].rel_path,
                        )));
                    }
                }
            }
            KeyCode::Up => {
                if count > 0 && modal.cursor > 0 {
                    modal.cursor -= 1;
                    modal.clamp_scroll();
                }
            }
            KeyCode::Down => {
                if count > 0 && modal.cursor < count - 1 {
                    modal.cursor += 1;
                    modal.clamp_scroll();
                }
            }
            KeyCode::Right => {
                if modal.search.is_empty() {
                    modal.expand_current();
                }
            }
            KeyCode::Left => {
                if modal.search.is_empty() {
                    modal.collapse_current();
                    modal.clamp_scroll();
                }
            }
            KeyCode::PageUp => {
                if count > 0 {
                    let page = modal.list_height.max(1);
                    modal.cursor = modal.cursor.saturating_sub(page);
                    modal.clamp_scroll();
                }
            }
            KeyCode::PageDown => {
                if count > 0 {
                    let page = modal.list_height.max(1);
                    modal.cursor = (modal.cursor + page).min(count - 1);
                    modal.clamp_scroll();
                }
            }
            KeyCode::Backspace => {
                if !modal.search.is_empty() {
                    modal.search.pop();
                    modal.search_dirty = true;
                    modal.cursor = 0;
                    modal.scroll = 0;
                }
            }
            KeyCode::Char(c) => {
                modal.search.push(c);
                modal.search_dirty = true;
                modal.cursor = 0;
                modal.scroll = 0;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn render_add_modal(frame: &mut ratatui::Frame, modal: &mut AddModalState) {
    let area = frame.area();
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " Add Shared File ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    // inner height - search (1) - blank (1) - help (1) - count (1) - blank before help (1)
    let list_height = inner.height.saturating_sub(5) as usize;
    modal.list_height = list_height.max(1);
    modal.clamp_scroll();

    let mut lines: Vec<Line> = Vec::new();

    // -- Search bar --
    if modal.search.is_empty() {
        lines.push(Line::from(Span::styled(
            "Search: (type to filter)",
            Style::default().fg(DIM),
        )));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Search: ", Style::default().fg(DIM)),
            Span::styled(&modal.search, Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::raw(""));

    // -- Results area --
    if modal.search.is_empty() {
        render_tree_entries(&mut lines, modal, list_height);
    } else {
        render_search_entries(&mut lines, modal, list_height);
    }

    lines.push(Line::raw(""));

    // -- Help & count --
    let key_style = Style::default().fg(Color::Cyan);
    let dim_style = Style::default().fg(DIM);

    if !modal.search.is_empty() && modal.search_results.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Enter", key_style),
            Span::styled(" declare  ", dim_style),
            Span::styled("Esc", key_style),
            Span::styled(" cancel", dim_style),
        ]));
    } else if modal.search.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("\u{2191}\u{2193}", key_style),
            Span::styled(" navigate  ", dim_style),
            Span::styled("\u{2192}", key_style),
            Span::styled(" expand  ", dim_style),
            Span::styled("\u{2190}", key_style),
            Span::styled(" collapse  ", dim_style),
            Span::styled("Enter", key_style),
            Span::styled(" select  ", dim_style),
            Span::styled("Esc", key_style),
            Span::styled(" cancel", dim_style),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("\u{2191}\u{2193}", key_style),
            Span::styled(" navigate  ", dim_style),
            Span::styled("Enter", key_style),
            Span::styled(" select  ", dim_style),
            Span::styled("Esc", key_style),
            Span::styled(" cancel", dim_style),
        ]));
    }

    // Count line (search mode only)
    if !modal.search.is_empty() {
        let count_text = format!(
            "{}/{} matched",
            modal.search_results.len(),
            modal.search_entries.len()
        );
        lines.push(Line::from(Span::styled(count_text, dim_style)));
    } else {
        lines.push(Line::raw(""));
    }

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the tree view (browse mode).
fn render_tree_entries(lines: &mut Vec<Line>, modal: &AddModalState, list_height: usize) {
    if modal.tree_visible.is_empty() {
        lines.push(Line::styled("No files found", Style::default().fg(DIM)));
        for _ in 0..list_height.saturating_sub(1) {
            lines.push(Line::raw(""));
        }
        return;
    }

    let end = modal.tree_visible.len().min(modal.scroll + list_height);
    let visible = &modal.tree_visible[modal.scroll..end];

    for (offset, &entry_idx) in visible.iter().enumerate() {
        let entry = &modal.tree_entries[entry_idx];
        let is_cursor = modal.scroll + offset == modal.cursor;

        let indent = "  ".repeat(entry.depth);
        let prefix = if entry.is_dir {
            if entry.expanded {
                "\u{25be} "
            } else {
                "\u{25b8} "
            }
        } else {
            "  "
        };
        let suffix = if entry.is_dir { "/" } else { "" };
        let display = format!("{indent}{prefix}{}{suffix}", entry.name);

        let style = entry_style(is_cursor, entry.is_shared, entry.is_ignored, entry.is_dir);

        let mut spans = vec![Span::styled(display, style)];

        if entry.is_shared && !is_cursor {
            spans.push(Span::styled(" (shared)", Style::default().fg(DIM)));
        }

        lines.push(Line::from(spans));
    }

    for _ in visible.len()..list_height {
        lines.push(Line::raw(""));
    }
}

/// Render fuzzy search results (search mode).
fn render_search_entries(lines: &mut Vec<Line>, modal: &AddModalState, list_height: usize) {
    if modal.search_results.is_empty() {
        if modal.indexing_done.load(Ordering::Acquire) {
            lines.push(Line::styled(
                "No matching files found",
                Style::default().fg(DIM),
            ));
            lines.push(Line::styled(
                format!(
                    "Press Enter to declare '{}' as a new shared file",
                    modal.search
                ),
                Style::default().fg(Color::Yellow),
            ));
        }
        for _ in 0..list_height.saturating_sub(2) {
            lines.push(Line::raw(""));
        }
        return;
    }

    let end = modal.search_results.len().min(modal.scroll + list_height);
    let visible = &modal.search_results[modal.scroll..end];

    for (offset, result) in visible.iter().enumerate() {
        let entry = &modal.search_entries[result.entry_idx];
        let is_cursor = modal.scroll + offset == modal.cursor;

        let suffix = if entry.is_dir { "/" } else { "" };
        let display = format!("{}{suffix}", entry.rel_path);

        let style = entry_style(is_cursor, entry.is_shared, entry.is_ignored, entry.is_dir);

        let mut spans: Vec<Span> = if !result.indices.is_empty() {
            highlight_matches(&display, &result.indices, style)
        } else {
            vec![Span::styled(display, style)]
        };

        if entry.is_shared && !is_cursor {
            spans.push(Span::styled(" (shared)", Style::default().fg(DIM)));
        }

        lines.push(Line::from(spans));
    }

    for _ in visible.len()..list_height {
        lines.push(Line::raw(""));
    }
}

/// Compute entry style based on state.
///
/// Colors: orange = ignored dir, white = ignored file, muted = tracked.
fn entry_style(is_cursor: bool, is_shared: bool, is_ignored: bool, is_dir: bool) -> Style {
    if is_cursor {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else if is_shared {
        Style::default().fg(DIM)
    } else if is_ignored {
        if is_dir {
            Style::default().fg(ACCENT)
        } else {
            Style::default().fg(Color::White)
        }
    } else {
        Style::default().fg(MUTED)
    }
}

/// Render a string with matched positions underlined.
fn highlight_matches(text: &str, indices: &[u32], base_style: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let match_set: HashSet<usize> = indices.iter().map(|&i| i as usize).collect();
    let underline_style = base_style.add_modifier(Modifier::UNDERLINED);

    let mut spans = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let is_match = match_set.contains(&i);
        let style = if is_match {
            underline_style
        } else {
            base_style
        };
        let start = i;
        while i < chars.len() && match_set.contains(&i) == is_match {
            i += 1;
        }
        let segment: String = chars[start..i].iter().collect();
        spans.push(Span::styled(segment, style));
    }

    spans
}
