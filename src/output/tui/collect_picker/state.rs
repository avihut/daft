//! State management for the collect picker TUI.

use crate::core::shared::{CollectDecision, CollectSource, UncollectedFile};
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
    pub selected: Option<usize>,
    pub preview_scroll: usize,
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

    pub fn current_tab_mut(&mut self) -> &mut FileTabState {
        &mut self.tabs[self.active_tab]
    }

    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            if !self.current_tab().is_stub {
                self.focus = FocusPanel::WorktreeList;
            }
        }
    }

    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
            if !self.current_tab().is_stub {
                self.focus = FocusPanel::WorktreeList;
            }
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
                self.tabs[self.active_tab].preview_scroll += 1;
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
                let scroll = &mut self.tabs[self.active_tab].preview_scroll;
                *scroll = scroll.saturating_sub(1);
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

    pub fn toggle_selection(&mut self) {
        if self.focus != FocusPanel::WorktreeList || self.current_tab().is_stub {
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        let cursor = tab.list_cursor;
        if tab.selected == Some(cursor) {
            tab.selected = None;
        } else {
            tab.selected = Some(cursor);
        }
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

    pub fn decided_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|t| t.selected.is_some() || t.is_stub)
            .count()
    }

    pub fn all_decided(&self) -> bool {
        self.decided_count() == self.tabs.len()
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
                    Some(CollectDecision {
                        rel_path: tab.rel_path,
                        source: CollectSource::Stub,
                    })
                } else {
                    tab.selected.map(|idx| CollectDecision {
                        rel_path: tab.rel_path,
                        source: CollectSource::FromWorktree(tab.copies[idx].worktree_path.clone()),
                    })
                }
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
    fn toggle_selection_sets_and_clears() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main"), ("dev", "/repo/dev")],
        )];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.current_tab().selected, None);
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, Some(0));
        state.toggle_selection();
        assert_eq!(state.current_tab().selected, None);
    }

    #[test]
    fn move_down_navigates_to_footer() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main")])];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.focus, FocusPanel::WorktreeList);
        state.move_down(); // cursor at max (only 1 entry) → footer
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
    fn into_decisions_builds_correctly() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main"), ("dev", "/repo/dev")]),
            make_uncollected(".secrets", &[]),
        ];
        let mut state = CollectPickerState::new(files);

        state.tabs[0].list_cursor = 1;
        state.toggle_selection();

        let decisions = state.into_decisions();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].rel_path, ".env");
        assert_eq!(
            decisions[0].source,
            CollectSource::FromWorktree(PathBuf::from("/repo/dev"))
        );
        assert_eq!(decisions[1].rel_path, ".secrets");
        assert_eq!(decisions[1].source, CollectSource::Stub);
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
