//! State management for the collect picker TUI.

use crate::core::shared::{self, CollectDecision, CompareResult, UncollectedFile};
use std::path::PathBuf;
use std::time::Duration;

/// Which panel has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusPanel {
    /// Tab bar at the top — used on stub tabs where there is no worktree list.
    TabBar,
    WorktreeList,
    Preview,
    Footer,
}

/// A footer button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FooterButton {
    Submit,
    Cancel,
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

/// A single entry in the worktree list.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    pub worktree_name: String,
    pub worktree_path: PathBuf,
    /// Whether this worktree has a real copy of the file.
    pub has_file: bool,
}

/// Top-level state for the collect picker.
#[derive(Debug)]
pub struct CollectPickerState {
    pub tabs: Vec<FileTabState>,
    pub active_tab: usize,
    pub focus: FocusPanel,
    pub footer_cursor: FooterButton,
    pub submitted: bool,
    pub cancelled: bool,
}

/// Timeout for deep file comparison.
const COMPARE_TIMEOUT: Duration = Duration::from_secs(1);

impl CollectPickerState {
    pub fn new(uncollected: Vec<UncollectedFile>) -> Self {
        let tabs: Vec<FileTabState> = uncollected
            .into_iter()
            .map(|uf| {
                let is_stub = !uf.has_any_copy();
                let len = uf.worktrees.len();
                let entries: Vec<WorktreeEntry> = uf
                    .worktrees
                    .into_iter()
                    .map(|w| WorktreeEntry {
                        worktree_name: w.worktree_name,
                        worktree_path: w.worktree_path,
                        has_file: w.has_file,
                    })
                    .collect();
                // Start cursor on the first entry that has a file
                let initial_cursor = entries.iter().position(|e| e.has_file).unwrap_or(0);
                FileTabState {
                    rel_path: uf.rel_path,
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
            })
            .collect();

        Self {
            tabs,
            active_tab: 0,
            focus: FocusPanel::WorktreeList,
            footer_cursor: FooterButton::Submit,
            submitted: false,
            cancelled: false,
        }
    }

    pub fn current_tab(&self) -> &FileTabState {
        &self.tabs[self.active_tab]
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.focus = if self.current_tab().is_stub {
                FocusPanel::TabBar
            } else {
                FocusPanel::WorktreeList
            };
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
            self.focus = if self.current_tab().is_stub {
                FocusPanel::TabBar
            } else {
                FocusPanel::WorktreeList
            };
        }
    }

    pub fn move_down(&mut self) {
        let tab = &self.tabs[self.active_tab];
        match self.focus {
            FocusPanel::TabBar => {
                self.focus = FocusPanel::Footer;
            }
            FocusPanel::WorktreeList => {
                if tab.is_stub {
                    return;
                }
                let has_selection = tab.selected.is_some();
                let current = tab.list_cursor;

                // Find the next traversable entry
                let next = if has_selection {
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
                    Some(idx) => self.tabs[self.active_tab].list_cursor = idx,
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

    pub fn move_up(&mut self) {
        match self.focus {
            FocusPanel::TabBar => {}
            FocusPanel::WorktreeList => {
                let tab = &self.tabs[self.active_tab];
                let has_selection = tab.selected.is_some();
                let current = tab.list_cursor;

                let prev = if has_selection {
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
                    self.tabs[self.active_tab].list_cursor = idx;
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
                    if tab.selected.is_none() {
                        if let Some(last) = tab
                            .entries
                            .iter()
                            .enumerate()
                            .rev()
                            .find(|(_, e)| e.has_file)
                            .map(|(i, _)| i)
                        {
                            self.tabs[self.active_tab].list_cursor = last;
                        }
                    } else {
                        self.tabs[self.active_tab].list_cursor =
                            tab.entries.len().saturating_sub(1);
                    }
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

    /// Select or deselect the highlighted worktree as the collection source.
    /// Only works on worktrees that have the file.
    /// When selecting, uses deep comparison to set smart materialization defaults.
    /// Changing or clearing selection resets preferences and clears the compare warning.
    pub fn toggle_selection(&mut self) {
        if self.focus != FocusPanel::WorktreeList || self.current_tab().is_stub {
            return;
        }
        let tab = &self.tabs[self.active_tab];
        let cursor = tab.list_cursor;

        // Can only select worktrees that have the file
        if !tab.entries[cursor].has_file {
            return;
        }

        if tab.selected == Some(cursor) {
            // Deselect — clear everything
            let tab = &mut self.tabs[self.active_tab];
            tab.selected = None;
            tab.materialized.fill(false);
            tab.compare_warning = None;
        } else {
            // Select new source — compute smart defaults via deep compare
            let source_path = tab.entries[cursor].worktree_path.join(&tab.rel_path);
            let mut mat = vec![false; tab.entries.len()];
            let mut timed_out = false;

            for (i, entry) in tab.entries.iter().enumerate() {
                if i == cursor {
                    continue; // Source gets linked
                }
                if entry.has_file {
                    let other_path = entry.worktree_path.join(&tab.rel_path);
                    match shared::deep_compare(&source_path, &other_path, COMPARE_TIMEOUT) {
                        CompareResult::Identical => {
                            mat[i] = false; // Same content → link
                        }
                        CompareResult::Different => {
                            mat[i] = true; // Different → materialize to preserve
                        }
                        CompareResult::TimedOut => {
                            timed_out = true;
                            break;
                        }
                    }
                }
                // Worktrees without the file default to linked (false)
            }

            let tab = &mut self.tabs[self.active_tab];
            if timed_out {
                // Fallback: materialize all that have the file
                for (i, entry) in tab.entries.iter().enumerate() {
                    mat[i] = i != cursor && entry.has_file;
                }
                tab.compare_warning =
                    Some("File too large to compare — defaulting to materialize all copies".into());
            } else {
                tab.compare_warning = None;
            }
            tab.selected = Some(cursor);
            tab.materialized = mat;
        }
    }

    /// Toggle materialization for the highlighted worktree.
    /// Available when a source is selected and the cursor is not on the source.
    /// Works on all worktrees (including those without the file).
    /// Cannot select worktrees without the file as source (Enter/Space handles that).
    pub fn toggle_materialized(&mut self) {
        let tab = &mut self.tabs[self.active_tab];
        let Some(selected) = tab.selected else {
            return;
        };
        if tab.list_cursor == selected || tab.is_stub {
            return;
        }
        let idx = tab.list_cursor;
        tab.materialized[idx] = !tab.materialized[idx];
    }

    pub fn activate_footer(&mut self) {
        if self.focus != FocusPanel::Footer {
            return;
        }
        match self.footer_cursor {
            FooterButton::Submit => self.submitted = true,
            FooterButton::Cancel => self.cancelled = true,
        }
    }

    pub fn footer_next(&mut self) {
        if self.focus == FocusPanel::Footer {
            self.footer_cursor = match self.footer_cursor {
                FooterButton::Submit => FooterButton::Cancel,
                FooterButton::Cancel => FooterButton::Submit,
            };
        }
    }

    /// How many non-stub files have a selection.
    pub fn decided_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|t| !t.is_stub && t.selected.is_some())
            .count()
    }

    /// Total number of files that need a decision (excludes stubs).
    pub fn decidable_count(&self) -> usize {
        self.tabs.iter().filter(|t| !t.is_stub).count()
    }

    /// Whether all decidable files have a selection.
    pub fn all_decided(&self) -> bool {
        self.decided_count() == self.decidable_count()
    }

    pub fn has_any_selection(&self) -> bool {
        self.tabs.iter().any(|t| t.selected.is_some())
    }

    pub fn undecided_files(&self) -> Vec<&str> {
        self.tabs
            .iter()
            .filter(|t| !t.is_stub && t.selected.is_none())
            .map(|t| t.rel_path.as_str())
            .collect()
    }

    pub fn into_decisions(self) -> Vec<CollectDecision> {
        self.tabs
            .into_iter()
            .filter_map(|tab| {
                if tab.is_stub {
                    return None;
                }
                tab.selected.map(|idx| {
                    let materialize_in = tab
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|&(i, _)| i != idx && tab.materialized[i])
                        .map(|(_, e)| e.worktree_path.clone())
                        .collect();
                    CollectDecision {
                        rel_path: tab.rel_path,
                        source_worktree: tab.entries[idx].worktree_path.clone(),
                        materialize_in,
                    }
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::shared::WorktreeCopy;

    fn make_uncollected(rel_path: &str, worktrees: &[(&str, &str, bool)]) -> UncollectedFile {
        UncollectedFile {
            rel_path: rel_path.to_string(),
            worktrees: worktrees
                .iter()
                .map(|(name, path, has_file)| WorktreeCopy {
                    worktree_name: name.to_string(),
                    worktree_path: PathBuf::from(path),
                    has_file: *has_file,
                })
                .collect(),
        }
    }

    #[test]
    fn new_state_initializes_correctly() {
        let files = vec![
            make_uncollected(
                ".env",
                &[
                    ("main", "/repo/main", true),
                    ("dev", "/repo/dev", true),
                    ("feat", "/repo/feat", false),
                ],
            ),
            make_uncollected(
                ".secrets",
                &[("main", "/repo/main", false), ("dev", "/repo/dev", false)],
            ),
        ];
        let state = CollectPickerState::new(files);

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, 0);
        assert_eq!(state.focus, FocusPanel::WorktreeList);
        assert!(!state.tabs[0].is_stub);
        assert!(state.tabs[1].is_stub);
        assert_eq!(state.tabs[0].entries.len(), 3);
        assert!(state.tabs[0].entries[0].has_file);
        assert!(!state.tabs[0].entries[2].has_file);
    }

    #[test]
    fn tab_navigation_wraps() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main", true)]),
            make_uncollected(".idea", &[("main", "/repo/main", true)]),
        ];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.active_tab, 0);
        state.next_tab();
        assert_eq!(state.active_tab, 1);
        state.next_tab();
        assert_eq!(state.active_tab, 0);
        state.prev_tab();
        assert_eq!(state.active_tab, 1);
    }

    #[test]
    fn stub_tab_gets_tab_bar_focus() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main", true)]),
            make_uncollected(".secrets", &[("main", "/repo/main", false)]),
        ];
        let mut state = CollectPickerState::new(files);

        state.next_tab();
        assert_eq!(state.focus, FocusPanel::TabBar);
        state.move_down();
        assert_eq!(state.focus, FocusPanel::Footer);
        state.move_up();
        assert_eq!(state.focus, FocusPanel::TabBar);
        state.prev_tab();
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn cannot_select_worktree_without_file() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main", true), ("feat", "/repo/feat", false)],
        )];
        let mut state = CollectPickerState::new(files);

        // Move cursor to feat (no file) and try to select
        state.tabs[0].list_cursor = 1;
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, None);

        // Select main (has file) — works
        state.tabs[0].list_cursor = 0;
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, Some(0));
    }

    #[test]
    fn cursor_skips_worktrees_without_file_before_selection() {
        let files = vec![make_uncollected(
            ".env",
            &[
                ("main", "/repo/main", true),
                ("empty1", "/repo/empty1", false),
                ("empty2", "/repo/empty2", false),
                ("dev", "/repo/dev", true),
            ],
        )];
        let mut state = CollectPickerState::new(files);

        // Initial cursor should be on main (first with file)
        assert_eq!(state.current_tab().list_cursor, 0);

        // Move down — should skip empty1 and empty2, land on dev
        state.move_down();
        assert_eq!(state.current_tab().list_cursor, 3);

        // Move down again — should go to footer (no more entries with files)
        state.move_down();
        assert_eq!(state.focus, FocusPanel::Footer);

        // Move up from footer — back to list at last entry with file (dev)
        state.move_up();
        assert_eq!(state.focus, FocusPanel::WorktreeList);
        assert_eq!(state.current_tab().list_cursor, 3);

        // Move up — should skip empty2 and empty1, land on main
        state.move_up();
        assert_eq!(state.current_tab().list_cursor, 0);
    }

    #[test]
    fn initial_cursor_skips_to_first_entry_with_file() {
        let files = vec![make_uncollected(
            ".env",
            &[
                ("empty", "/repo/empty", false),
                ("main", "/repo/main", true),
            ],
        )];
        let state = CollectPickerState::new(files);
        assert_eq!(state.current_tab().list_cursor, 1);
    }

    #[test]
    fn toggle_materialized_works_on_worktrees_without_file() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main", true), ("feat", "/repo/feat", false)],
        )];
        let mut state = CollectPickerState::new(files);

        // Select main as source
        state.toggle_selection();
        // feat defaults to linked (no file)
        assert!(!state.current_tab().materialized[1]);
        // Toggle feat to materialized
        state.tabs[0].list_cursor = 1;
        state.toggle_materialized();
        assert!(state.current_tab().materialized[1]);
    }

    #[test]
    fn move_down_navigates_to_footer() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = CollectPickerState::new(files);

        state.move_down();
        assert_eq!(state.focus, FocusPanel::Footer);
    }

    #[test]
    fn move_up_from_footer_returns_to_list() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = CollectPickerState::new(files);

        state.focus = FocusPanel::Footer;
        state.move_up();
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn preview_scroll_clamps_to_content() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = CollectPickerState::new(files);
        state.focus = FocusPanel::Preview;
        state.tabs[0].preview_content_lines = 30;
        state.tabs[0].preview_viewport_height = 10;

        state.move_down();
        assert_eq!(state.current_tab().preview_scroll, 1);
        state.move_up();
        state.move_up();
        assert_eq!(state.current_tab().preview_scroll, 0);

        for _ in 0..25 {
            state.move_down();
        }
        assert_eq!(state.current_tab().preview_scroll, 20);
    }

    #[test]
    fn preview_scroll_blocked_when_content_fits() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = CollectPickerState::new(files);
        state.focus = FocusPanel::Preview;
        state.tabs[0].preview_content_lines = 5;
        state.tabs[0].preview_viewport_height = 20;

        state.move_down();
        assert_eq!(state.current_tab().preview_scroll, 0);
    }

    #[test]
    fn into_decisions_includes_materialization() {
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let main_dir = tmp.path().join("main");
        let dev_dir = tmp.path().join("dev");
        let feat_dir = tmp.path().join("feat");
        fs::create_dir_all(&main_dir).unwrap();
        fs::create_dir_all(&dev_dir).unwrap();
        fs::create_dir_all(&feat_dir).unwrap();
        // main and dev have .env with DIFFERENT content
        fs::write(main_dir.join(".env"), "FROM_MAIN=1").unwrap();
        fs::write(dev_dir.join(".env"), "FROM_DEV=1").unwrap();

        let files = vec![
            UncollectedFile {
                rel_path: ".env".to_string(),
                worktrees: vec![
                    WorktreeCopy {
                        worktree_name: "main".into(),
                        worktree_path: main_dir.clone(),
                        has_file: true,
                    },
                    WorktreeCopy {
                        worktree_name: "dev".into(),
                        worktree_path: dev_dir.clone(),
                        has_file: true,
                    },
                    WorktreeCopy {
                        worktree_name: "feat".into(),
                        worktree_path: feat_dir.clone(),
                        has_file: false,
                    },
                ],
            },
            make_uncollected(
                ".secrets",
                &[("main", "/repo/main", false), ("dev", "/repo/dev", false)],
            ),
        ];
        let mut state = CollectPickerState::new(files);

        // Select main for .env — deep compare finds dev is different → materialized
        state.toggle_selection();
        assert!(state.tabs[0].materialized[1]); // dev: different → materialized
        assert!(!state.tabs[0].materialized[2]); // feat: no file → linked

        // Toggle feat to materialized
        state.tabs[0].list_cursor = 2;
        state.toggle_materialized();

        let decisions = state.into_decisions();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].rel_path, ".env");
        assert_eq!(decisions[0].source_worktree, main_dir);
        // dev (different) and feat (toggled) are materialized
        assert!(decisions[0].materialize_in.contains(&dev_dir));
        assert!(decisions[0].materialize_in.contains(&feat_dir));
    }

    #[test]
    fn deep_compare_links_identical_copies() {
        use std::fs;
        let tmp = tempfile::TempDir::new().unwrap();
        let main_dir = tmp.path().join("main");
        let dev_dir = tmp.path().join("dev");
        fs::create_dir_all(&main_dir).unwrap();
        fs::create_dir_all(&dev_dir).unwrap();
        // Same content
        fs::write(main_dir.join(".env"), "SAME=1").unwrap();
        fs::write(dev_dir.join(".env"), "SAME=1").unwrap();

        let files = vec![UncollectedFile {
            rel_path: ".env".to_string(),
            worktrees: vec![
                WorktreeCopy {
                    worktree_name: "main".into(),
                    worktree_path: main_dir,
                    has_file: true,
                },
                WorktreeCopy {
                    worktree_name: "dev".into(),
                    worktree_path: dev_dir,
                    has_file: true,
                },
            ],
        }];
        let mut state = CollectPickerState::new(files);

        // Select main — dev has identical content → should default to linked
        state.toggle_selection();
        assert!(!state.tabs[0].materialized[1]);
    }

    #[test]
    fn all_decided_true_when_only_stubs() {
        let files = vec![
            make_uncollected(".secrets", &[("main", "/repo/main", false)]),
            make_uncollected(".tokens", &[("main", "/repo/main", false)]),
        ];
        let state = CollectPickerState::new(files);
        assert!(state.all_decided());
    }

    #[test]
    fn undecided_files_excludes_stubs_and_selected() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main", true)]),
            make_uncollected(".idea", &[("main", "/repo/main", true)]),
            make_uncollected(
                ".secrets",
                &[("main", "/repo/main", false), ("dev", "/repo/dev", false)],
            ),
        ];
        let mut state = CollectPickerState::new(files);

        state.toggle_selection(); // Select .env
        let undecided = state.undecided_files();
        assert_eq!(undecided, vec![".idea"]);
    }
}
