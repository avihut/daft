//! State management for the collect picker TUI.

use crate::core::shared::{CollectDecision, UncollectedFile};
use std::path::PathBuf;

/// Which panel has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusPanel {
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
    pub copies: Vec<CopyEntry>,
    pub list_cursor: usize,
    /// Index of the worktree selected as the collection source.
    pub selected: Option<usize>,
    /// Per-worktree materialization preference (parallel to `copies`).
    /// `true` = keep local copy (materialized), `false` = replace with symlink.
    /// Only meaningful when `selected` is `Some`.
    pub materialized: Vec<bool>,
    pub preview_scroll: u16,
    pub is_stub: bool,
}

/// A single entry in the worktree list.
#[derive(Debug, Clone)]
pub struct CopyEntry {
    pub worktree_name: String,
    pub worktree_path: PathBuf,
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

impl CollectPickerState {
    pub fn new(uncollected: Vec<UncollectedFile>) -> Self {
        let tabs: Vec<FileTabState> = uncollected
            .into_iter()
            .map(|uf| {
                let is_stub = uf.copies.is_empty();
                let len = uf.copies.len();
                let copies: Vec<CopyEntry> = uf
                    .copies
                    .into_iter()
                    .map(|c| CopyEntry {
                        worktree_name: c.worktree_name,
                        worktree_path: c.worktree_path,
                    })
                    .collect();
                FileTabState {
                    rel_path: uf.rel_path,
                    copies,
                    list_cursor: 0,
                    selected: None,
                    materialized: vec![false; len],
                    preview_scroll: 0,
                    is_stub,
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
                FocusPanel::Footer
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
                FocusPanel::Footer
            } else {
                FocusPanel::WorktreeList
            };
        }
    }

    pub fn move_down(&mut self) {
        let tab = &self.tabs[self.active_tab];
        match self.focus {
            FocusPanel::WorktreeList => {
                if tab.is_stub {
                    return;
                }
                let max = tab.copies.len().saturating_sub(1);
                if self.tabs[self.active_tab].list_cursor >= max {
                    self.focus = FocusPanel::Footer;
                } else {
                    self.tabs[self.active_tab].list_cursor += 1;
                }
            }
            FocusPanel::Preview => {
                self.tabs[self.active_tab].preview_scroll =
                    self.tabs[self.active_tab].preview_scroll.saturating_add(1);
            }
            FocusPanel::Footer => {}
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            FocusPanel::WorktreeList => {
                let cursor = &mut self.tabs[self.active_tab].list_cursor;
                *cursor = cursor.saturating_sub(1);
            }
            FocusPanel::Preview => {
                self.tabs[self.active_tab].preview_scroll =
                    self.tabs[self.active_tab].preview_scroll.saturating_sub(1);
            }
            FocusPanel::Footer => {
                if !self.current_tab().is_stub {
                    self.focus = FocusPanel::WorktreeList;
                }
            }
        }
    }

    pub fn toggle_panel(&mut self) {
        if self.current_tab().is_stub {
            return;
        }
        self.focus = match self.focus {
            FocusPanel::WorktreeList => FocusPanel::Preview,
            FocusPanel::Preview => FocusPanel::WorktreeList,
            FocusPanel::Footer => FocusPanel::WorktreeList,
        };
    }

    /// Select or deselect the highlighted worktree as the collection source.
    /// When selecting, all other worktrees default to materialized.
    /// Changing or clearing selection resets materialization preferences.
    pub fn toggle_selection(&mut self) {
        if self.focus != FocusPanel::WorktreeList || self.current_tab().is_stub {
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        let cursor = tab.list_cursor;
        if tab.selected == Some(cursor) {
            // Deselect — clear everything
            tab.selected = None;
            tab.materialized.fill(false);
        } else {
            // Select new source — all others default to materialized
            tab.selected = Some(cursor);
            tab.materialized.fill(true);
            tab.materialized[cursor] = false; // source gets linked, not materialized
        }
    }

    /// Toggle materialization for the highlighted worktree.
    /// Only available when a source is selected and the cursor is not on the source.
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
    /// Returns true when there are no decidable files (all stubs).
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
                        .copies
                        .iter()
                        .enumerate()
                        .filter(|&(i, _)| i != idx && tab.materialized[i])
                        .map(|(_, c)| c.worktree_path.clone())
                        .collect();
                    CollectDecision {
                        rel_path: tab.rel_path,
                        source_worktree: tab.copies[idx].worktree_path.clone(),
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

    fn make_uncollected(rel_path: &str, worktrees: &[(&str, &str)]) -> UncollectedFile {
        UncollectedFile {
            rel_path: rel_path.to_string(),
            copies: worktrees
                .iter()
                .map(|(name, path)| WorktreeCopy {
                    worktree_name: name.to_string(),
                    worktree_path: PathBuf::from(path),
                })
                .collect(),
        }
    }

    #[test]
    fn new_state_initializes_correctly() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main"), ("dev", "/repo/dev")]),
            make_uncollected(".secrets", &[]),
        ];
        let state = CollectPickerState::new(files);

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, 0);
        assert_eq!(state.focus, FocusPanel::WorktreeList);
        assert!(!state.tabs[0].is_stub);
        assert!(state.tabs[1].is_stub);
        assert_eq!(state.tabs[0].copies.len(), 2);
        assert_eq!(state.tabs[0].materialized, vec![false, false]);
    }

    #[test]
    fn tab_navigation_wraps() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main")]),
            make_uncollected(".idea", &[("main", "/repo/main")]),
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
    fn stub_tab_gets_footer_focus() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main")]),
            make_uncollected(".secrets", &[]),
        ];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.focus, FocusPanel::WorktreeList);
        state.next_tab(); // switch to stub tab
        assert_eq!(state.focus, FocusPanel::Footer);
        state.prev_tab(); // back to non-stub
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn toggle_selection_sets_materialized_defaults() {
        let files = vec![make_uncollected(
            ".env",
            &[
                ("main", "/repo/main"),
                ("dev", "/repo/dev"),
                ("feat", "/repo/feat"),
            ],
        )];
        let mut state = CollectPickerState::new(files);

        // Select main (index 0) as source
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, Some(0));
        // Source is not materialized, others are
        assert_eq!(state.current_tab().materialized, vec![false, true, true]);

        // Deselect clears everything
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, None);
        assert_eq!(state.current_tab().materialized, vec![false, false, false]);
    }

    #[test]
    fn changing_selection_resets_materialized() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main"), ("dev", "/repo/dev")],
        )];
        let mut state = CollectPickerState::new(files);

        // Select main
        state.toggle_selection();
        assert_eq!(state.current_tab().materialized, vec![false, true]);

        // Toggle materialization on dev
        state.tabs[0].list_cursor = 1;
        state.toggle_materialized();
        assert_eq!(state.current_tab().materialized, vec![false, false]);

        // Now select dev instead — materialization resets
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, Some(1));
        assert_eq!(state.current_tab().materialized, vec![true, false]);
    }

    #[test]
    fn toggle_materialized_only_works_on_non_source() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main"), ("dev", "/repo/dev")],
        )];
        let mut state = CollectPickerState::new(files);

        // Select main
        state.toggle_selection();

        // Try to toggle materialization on the source — should be no-op
        state.tabs[0].list_cursor = 0;
        state.toggle_materialized();
        assert_eq!(state.current_tab().materialized, vec![false, true]);

        // Toggle on dev — should work
        state.tabs[0].list_cursor = 1;
        state.toggle_materialized();
        assert_eq!(state.current_tab().materialized, vec![false, false]);
    }

    #[test]
    fn toggle_materialized_noop_without_selection() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main"), ("dev", "/repo/dev")],
        )];
        let mut state = CollectPickerState::new(files);

        state.toggle_materialized();
        assert_eq!(state.current_tab().materialized, vec![false, false]);
    }

    #[test]
    fn move_down_navigates_to_footer() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main")])];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.focus, FocusPanel::WorktreeList);
        state.move_down();
        assert_eq!(state.focus, FocusPanel::Footer);
    }

    #[test]
    fn move_up_from_footer_returns_to_list() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main")])];
        let mut state = CollectPickerState::new(files);

        state.focus = FocusPanel::Footer;
        state.move_up();
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn preview_scroll_uses_saturating_arithmetic() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main")])];
        let mut state = CollectPickerState::new(files);
        state.focus = FocusPanel::Preview;

        // Scroll down
        state.move_down();
        assert_eq!(state.current_tab().preview_scroll, 1);

        // Scroll up past zero
        state.move_up();
        state.move_up();
        assert_eq!(state.current_tab().preview_scroll, 0);
    }

    #[test]
    fn into_decisions_includes_materialization() {
        let files = vec![
            make_uncollected(
                ".env",
                &[
                    ("main", "/repo/main"),
                    ("dev", "/repo/dev"),
                    ("feat", "/repo/feat"),
                ],
            ),
            make_uncollected(".secrets", &[]),
        ];
        let mut state = CollectPickerState::new(files);

        // Select dev (index 1) for .env
        state.tabs[0].list_cursor = 1;
        state.toggle_selection();
        // Un-materialize feat
        state.tabs[0].list_cursor = 2;
        state.toggle_materialized();

        let decisions = state.into_decisions();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].rel_path, ".env");
        assert_eq!(decisions[0].source_worktree, PathBuf::from("/repo/dev"));
        // Only main is materialized (feat was toggled off)
        assert_eq!(
            decisions[0].materialize_in,
            vec![PathBuf::from("/repo/main")]
        );
    }

    #[test]
    fn all_decided_true_when_only_stubs() {
        let files = vec![
            make_uncollected(".secrets", &[]),
            make_uncollected(".tokens", &[]),
        ];
        let state = CollectPickerState::new(files);
        assert!(state.all_decided());
    }

    #[test]
    fn undecided_files_excludes_stubs_and_selected() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main")]),
            make_uncollected(".idea", &[("main", "/repo/main")]),
            make_uncollected(".secrets", &[]),
        ];
        let mut state = CollectPickerState::new(files);

        state.toggle_selection(); // Select .env

        let undecided = state.undecided_files();
        assert_eq!(undecided, vec![".idea"]);
    }
}
