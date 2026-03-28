//! Generic state management for the shared picker TUI.

use std::path::PathBuf;

/// Which panel has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusPanel {
    /// Tab bar at the top — used on stub tabs where there is no worktree list.
    TabBar,
    WorktreeList,
    Preview,
    Footer,
}

/// State for a single file tab.
#[derive(Debug, Clone)]
pub struct FileTabState {
    pub rel_path: String,
    /// All worktrees — `has_file` indicates which have a real copy.
    pub entries: Vec<WorktreeEntry>,
    pub list_cursor: usize,
    /// Index of the worktree selected as the collection source.
    pub selected: Option<usize>,
    /// Per-worktree materialization preference (parallel to `entries`).
    /// `true` = materialized, `false` = linked.
    /// Only meaningful when `selected` is `Some`.
    pub materialized: Vec<bool>,
    pub preview_scroll: u16,
    /// Number of content lines in the preview (set by the renderer).
    pub preview_content_lines: u16,
    /// Height of the preview viewport (set by the renderer).
    pub preview_viewport_height: u16,
    /// Whether no worktree has a copy of this file.
    pub is_stub: bool,
    /// Warning message from timed-out deep comparison, if any.
    pub compare_warning: Option<String>,
}

impl FileTabState {
    /// Create a new tab with the given relative path and entries.
    pub fn new(rel_path: String, entries: Vec<WorktreeEntry>) -> Self {
        let is_stub = entries.iter().all(|e| !e.has_file);
        let len = entries.len();
        let initial_cursor = entries.iter().position(|e| e.has_file).unwrap_or(0);
        Self {
            rel_path,
            entries,
            list_cursor: initial_cursor,
            selected: None,
            materialized: vec![false; len],
            preview_scroll: 0,
            preview_content_lines: 0,
            preview_viewport_height: 0,
            is_stub,
            compare_warning: None,
        }
    }
}

/// A single entry in the worktree list.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub worktree_name: String,
    pub worktree_path: PathBuf,
    /// Whether this worktree has a real copy of the file.
    pub has_file: bool,
}

/// Top-level state for the shared picker (navigation only).
#[derive(Debug)]
pub struct PickerState {
    pub tabs: Vec<FileTabState>,
    pub active_tab: usize,
    pub focus: FocusPanel,
}

impl PickerState {
    /// Create a picker state from pre-built tabs.
    pub fn from_tabs(tabs: Vec<FileTabState>) -> Self {
        Self {
            tabs,
            active_tab: 0,
            focus: FocusPanel::WorktreeList,
        }
    }

    /// Set the list cursor for the active tab, resetting preview scroll.
    fn set_cursor(&mut self, idx: usize) {
        let tab = &mut self.tabs[self.active_tab];
        tab.list_cursor = idx;
        tab.preview_scroll = 0;
    }

    pub fn current_tab(&self) -> &FileTabState {
        &self.tabs[self.active_tab]
    }

    /// Navigate to the next tab. `extra_tabs` is the number of virtual tabs
    /// appended by the mode (e.g., the "+" tab in manage mode).
    pub fn next_tab(&mut self, extra_tabs: usize) {
        let total = self.tabs.len() + extra_tabs;
        if total > 0 {
            self.active_tab = (self.active_tab + 1) % total;
            self.focus = if self.is_virtual_tab() || self.current_tab().is_stub {
                FocusPanel::TabBar
            } else {
                FocusPanel::WorktreeList
            };
        }
    }

    /// Navigate to the previous tab.
    pub fn prev_tab(&mut self, extra_tabs: usize) {
        let total = self.tabs.len() + extra_tabs;
        if total > 0 {
            self.active_tab = if self.active_tab == 0 {
                total - 1
            } else {
                self.active_tab - 1
            };
            self.focus = if self.is_virtual_tab() || self.current_tab().is_stub {
                FocusPanel::TabBar
            } else {
                FocusPanel::WorktreeList
            };
        }
    }

    /// Whether the active tab is a virtual (extra) tab beyond the real tabs.
    pub fn is_virtual_tab(&self) -> bool {
        self.active_tab >= self.tabs.len()
    }

    /// Adjust initial focus when extra (virtual) tabs exist.
    /// Call after construction if the mode provides extra tabs.
    pub fn adjust_for_extra_tabs(&mut self, extra_tabs: usize) {
        if self.tabs.is_empty() && extra_tabs > 0 {
            self.focus = FocusPanel::TabBar;
        }
    }

    /// Move down. `all_traversable` controls whether entries without files are
    /// skipped (false) or traversable (true).
    pub fn move_down(&mut self, all_traversable: bool) {
        let tab = &self.tabs[self.active_tab];
        match self.focus {
            FocusPanel::TabBar => {
                self.focus = FocusPanel::Footer;
            }
            FocusPanel::WorktreeList => {
                if tab.is_stub {
                    return;
                }
                let current = tab.list_cursor;

                // Find the next traversable entry
                let next = if all_traversable {
                    // All entries traversable when source is selected
                    if current < tab.entries.len() - 1 {
                        Some(current + 1)
                    } else {
                        None
                    }
                } else {
                    // Skip entries without files
                    tab.entries
                        .iter()
                        .enumerate()
                        .skip(current + 1)
                        .find(|(_, e)| e.has_file)
                        .map(|(i, _)| i)
                };

                match next {
                    Some(idx) => self.set_cursor(idx),
                    None => self.focus = FocusPanel::Footer,
                }
            }
            FocusPanel::Preview => {
                let tab = &mut self.tabs[self.active_tab];
                let max_scroll = tab
                    .preview_content_lines
                    .saturating_sub(tab.preview_viewport_height);
                if tab.preview_scroll < max_scroll {
                    tab.preview_scroll = tab.preview_scroll.saturating_add(1);
                }
            }
            FocusPanel::Footer => {}
        }
    }

    /// Move up. `all_traversable` controls whether entries without files are
    /// skipped (false) or traversable (true).
    pub fn move_up(&mut self, all_traversable: bool) {
        match self.focus {
            FocusPanel::TabBar => {}
            FocusPanel::WorktreeList => {
                let tab = &self.tabs[self.active_tab];
                let current = tab.list_cursor;

                let prev = if all_traversable {
                    // All entries traversable
                    if current > 0 {
                        Some(current - 1)
                    } else {
                        None
                    }
                } else {
                    // Skip entries without files
                    tab.entries[..current]
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, e)| e.has_file)
                        .map(|(i, _)| i)
                };

                if let Some(idx) = prev {
                    self.set_cursor(idx);
                }
            }
            FocusPanel::Preview => {
                self.tabs[self.active_tab].preview_scroll =
                    self.tabs[self.active_tab].preview_scroll.saturating_sub(1);
            }
            FocusPanel::Footer => {
                if self.current_tab().is_stub {
                    self.focus = FocusPanel::TabBar;
                } else {
                    self.focus = FocusPanel::WorktreeList;
                    // Place cursor on the last traversable entry
                    let tab = &self.tabs[self.active_tab];
                    let last = if !all_traversable {
                        tab.entries
                            .iter()
                            .enumerate()
                            .rev()
                            .find(|(_, e)| e.has_file)
                            .map(|(i, _)| i)
                            .unwrap_or(0)
                    } else {
                        tab.entries.len().saturating_sub(1)
                    };
                    self.set_cursor(last);
                }
            }
        }
    }

    /// Scroll the preview pane down by one page.
    pub fn page_down(&mut self) {
        if self.focus != FocusPanel::Preview {
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        let page = tab.preview_viewport_height.max(1);
        let max_scroll = tab
            .preview_content_lines
            .saturating_sub(tab.preview_viewport_height);
        tab.preview_scroll = tab.preview_scroll.saturating_add(page).min(max_scroll);
    }

    /// Scroll the preview pane up by one page.
    pub fn page_up(&mut self) {
        if self.focus != FocusPanel::Preview {
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        let page = tab.preview_viewport_height.max(1);
        tab.preview_scroll = tab.preview_scroll.saturating_sub(page);
    }

    pub fn toggle_panel(&mut self) {
        if self.current_tab().is_stub {
            return;
        }
        self.focus = match self.focus {
            FocusPanel::TabBar => FocusPanel::WorktreeList,
            FocusPanel::WorktreeList => FocusPanel::Preview,
            FocusPanel::Preview => FocusPanel::WorktreeList,
            FocusPanel::Footer => FocusPanel::WorktreeList,
        };
    }
}
