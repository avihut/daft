//! CollectMode — implements PickerMode for the collect (sync) workflow.

use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::time::Duration;

use crate::core::shared::{self, CollectDecision, UncollectedFile};

use super::state::{FileTabState, FocusPanel, PickerState, WorktreeEntry};
use super::{EntryDecoration, LoopAction, PickerMode};

/// Timeout for deep file comparison.
const COMPARE_TIMEOUT: Duration = Duration::from_secs(1);

/// Accent color matching the project's ACCENT_COLOR_INDEX (orange 208).
const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;

/// A footer button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FooterButton {
    Submit,
    Cancel,
}

/// Collect-specific mode state.
pub struct CollectMode {
    pub footer_cursor: FooterButton,
    pub submitted: bool,
    pub cancelled: bool,
}

impl Default for CollectMode {
    fn default() -> Self {
        Self::new()
    }
}

impl CollectMode {
    pub fn new() -> Self {
        Self {
            footer_cursor: FooterButton::Submit,
            submitted: false,
            cancelled: false,
        }
    }

    /// Build `FileTabState` tabs from uncollected files.
    pub fn build_tabs(uncollected: Vec<UncollectedFile>) -> Vec<FileTabState> {
        uncollected
            .into_iter()
            .map(|uf| {
                let entries: Vec<WorktreeEntry> = uf
                    .worktrees
                    .into_iter()
                    .map(|w| WorktreeEntry {
                        worktree_name: w.worktree_name,
                        worktree_path: w.worktree_path,
                        has_file: w.has_file,
                    })
                    .collect();
                FileTabState::new(uf.rel_path, entries)
            })
            .collect()
    }

    /// Extract decisions from the picker state after the user submits.
    pub fn into_decisions(state: PickerState) -> Vec<CollectDecision> {
        state
            .tabs
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

    pub fn is_submitted(&self) -> bool {
        self.submitted
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Select or deselect the highlighted worktree as the collection source.
    /// Only works on worktrees that have the file.
    /// When selecting, uses deep comparison to set smart materialization defaults.
    /// Changing or clearing selection resets preferences and clears the compare warning.
    fn toggle_selection(state: &mut PickerState) {
        if state.focus != FocusPanel::WorktreeList || state.current_tab().is_stub {
            return;
        }
        let tab = &state.tabs[state.active_tab];
        let cursor = tab.list_cursor;

        // Can only select worktrees that have the file
        if !tab.entries[cursor].has_file {
            return;
        }

        if tab.selected == Some(cursor) {
            // Deselect — clear everything
            let tab = &mut state.tabs[state.active_tab];
            tab.selected = None;
            tab.materialized.fill(false);
            tab.compare_warning = None;
        } else {
            // Select new source — compute smart defaults via deep compare
            let source_path = tab.entries[cursor].worktree_path.join(&tab.rel_path);
            let wt_paths: Vec<std::path::PathBuf> = tab
                .entries
                .iter()
                .map(|e| e.worktree_path.clone())
                .collect();

            let (mat, timed_out) = shared::compute_materialization_defaults(
                &source_path,
                &tab.rel_path,
                &wt_paths,
                cursor,
                COMPARE_TIMEOUT,
            );

            let tab = &mut state.tabs[state.active_tab];
            if timed_out {
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
    fn toggle_materialized(state: &mut PickerState) {
        let tab = &mut state.tabs[state.active_tab];
        let Some(selected) = tab.selected else {
            return;
        };
        if tab.list_cursor == selected || tab.is_stub {
            return;
        }
        let idx = tab.list_cursor;
        tab.materialized[idx] = !tab.materialized[idx];
    }

    /// How many non-stub files have a selection.
    fn decided_count(state: &PickerState) -> usize {
        state
            .tabs
            .iter()
            .filter(|t| !t.is_stub && t.selected.is_some())
            .count()
    }

    /// Total number of files that need a decision (excludes stubs).
    fn decidable_count(state: &PickerState) -> usize {
        state.tabs.iter().filter(|t| !t.is_stub).count()
    }

    /// Whether all decidable files have a selection.
    pub fn all_decided(state: &PickerState) -> bool {
        Self::decided_count(state) == Self::decidable_count(state)
    }

    pub fn has_any_selection(state: &PickerState) -> bool {
        state.tabs.iter().any(|t| t.selected.is_some())
    }

    pub fn undecided_files(state: &PickerState) -> Vec<&str> {
        state
            .tabs
            .iter()
            .filter(|t| !t.is_stub && t.selected.is_none())
            .map(|t| t.rel_path.as_str())
            .collect()
    }
}

impl PickerMode for CollectMode {
    fn all_entries_traversable(&self, tab: &FileTabState) -> bool {
        tab.selected.is_some()
    }

    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char(' ') | KeyCode::Enter => Self::toggle_selection(state),
            KeyCode::Char('m') => Self::toggle_materialized(state),
            _ => {}
        }
        LoopAction::Continue
    }

    fn handle_footer_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                self.footer_cursor = match self.footer_cursor {
                    FooterButton::Submit => FooterButton::Cancel,
                    FooterButton::Cancel => FooterButton::Submit,
                };
            }
            KeyCode::Char('q') | KeyCode::Esc => return LoopAction::Exit,
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.footer_cursor {
                    FooterButton::Submit => self.submitted = true,
                    FooterButton::Cancel => self.cancelled = true,
                }
                return LoopAction::Exit;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let all = self.all_entries_traversable(state.current_tab());
                state.move_up(all);
            }
            KeyCode::Tab => state.toggle_panel(),
            _ => {}
        }
        LoopAction::Continue
    }

    fn tab_decided(&self, tab: &FileTabState) -> bool {
        tab.selected.is_some() || tab.is_stub
    }

    fn tab_warning<'a>(&'a self, tab: &'a FileTabState) -> Option<&'a str> {
        tab.compare_warning.as_deref()
    }

    fn entry_decoration(
        &self,
        tab: &FileTabState,
        _tab_idx: usize,
        entry_idx: usize,
    ) -> EntryDecoration {
        let has_selection = tab.selected.is_some();
        let is_selected = tab.selected == Some(entry_idx);
        let is_materialized = has_selection && tab.materialized[entry_idx];

        let marker = if is_selected {
            "\u{2713} ".to_string()
        } else if is_materialized {
            "M ".to_string()
        } else {
            "  ".to_string()
        };

        let tag = if has_selection && !is_selected {
            if is_materialized {
                Some(("materialized".to_string(), Color::Yellow))
            } else {
                Some(("linked".to_string(), Color::Cyan))
            }
        } else {
            None
        };

        EntryDecoration { marker, tag }
    }

    fn render_footer(&self, state: &PickerState, frame: &mut Frame, area: Rect) {
        let is_focused = state.focus == FocusPanel::Footer;
        let all_decided = Self::all_decided(state);

        let submit_check = if all_decided { " \u{2713}" } else { "" };

        let submit_style = if is_focused && self.footer_cursor == FooterButton::Submit {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ACCENT)
        };

        let cancel_style = if is_focused && self.footer_cursor == FooterButton::Cancel {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        };

        let buttons = Line::from(vec![
            Span::raw("  "),
            Span::styled(format!(" Submit{submit_check} "), submit_style),
            Span::raw("  "),
            Span::styled(" Cancel ", cancel_style),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{}/{} files ready",
                    Self::decided_count(state),
                    Self::decidable_count(state)
                ),
                Style::default().fg(DIM),
            ),
        ]);

        let key_style = Style::default().fg(Color::Cyan);
        let desc_style = Style::default().fg(DIM);

        // Context-sensitive description for hl/arrows
        let hl_desc = match state.focus {
            FocusPanel::TabBar => " tabs  ",
            FocusPanel::WorktreeList => " tabs  ",
            FocusPanel::Preview => " tabs  ",
            FocusPanel::Footer => " buttons  ",
        };

        let help = Line::from(vec![
            Span::raw("  "),
            Span::styled("jk/\u{2191}\u{2193}", key_style),
            Span::styled(" navigate  ", desc_style),
            Span::styled("hl/\u{2190}\u{2192}", key_style),
            Span::styled(hl_desc, desc_style),
            Span::styled("PgUp/PgDn", key_style),
            Span::styled(" scroll  ", desc_style),
            Span::styled("Space", key_style),
            Span::styled(" select  ", desc_style),
            Span::styled("m", key_style),
            Span::styled(" materialize  ", desc_style),
            Span::styled("Tab", key_style),
            Span::styled(" panel  ", desc_style),
            Span::styled("Esc", key_style),
            Span::styled(" footer/cancel", desc_style),
        ]);

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::TOP)
            .border_style(Style::default().fg(DIM));

        let paragraph = Paragraph::new(vec![buttons, Line::raw(""), help]).block(block);
        frame.render_widget(paragraph, area);
    }

    fn footer_height(&self) -> u16 {
        5
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::shared::WorktreeCopy;
    use std::path::PathBuf;

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

    fn make_state(files: Vec<UncollectedFile>) -> PickerState {
        let tabs = CollectMode::build_tabs(files);
        PickerState::from_tabs(tabs)
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
        let state = make_state(files);

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
        let mut state = make_state(files);

        assert_eq!(state.active_tab, 0);
        state.next_tab(0);
        assert_eq!(state.active_tab, 1);
        state.next_tab(0);
        assert_eq!(state.active_tab, 0);
        state.prev_tab(0);
        assert_eq!(state.active_tab, 1);
    }

    #[test]
    fn stub_tab_gets_tab_bar_focus() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main", true)]),
            make_uncollected(".secrets", &[("main", "/repo/main", false)]),
        ];
        let mut state = make_state(files);

        state.next_tab(0);
        assert_eq!(state.focus, FocusPanel::TabBar);
        // Stub tab — no selection, so all_traversable = false
        state.move_down(false);
        assert_eq!(state.focus, FocusPanel::Footer);
        state.move_up(false);
        assert_eq!(state.focus, FocusPanel::TabBar);
        state.prev_tab(0);
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn cannot_select_worktree_without_file() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main", true), ("feat", "/repo/feat", false)],
        )];
        let mut state = make_state(files);

        // Move cursor to feat (no file) and try to select
        state.tabs[0].list_cursor = 1;
        CollectMode::toggle_selection(&mut state);
        assert_eq!(state.current_tab().selected, None);

        // Select main (has file) — works
        state.tabs[0].list_cursor = 0;
        CollectMode::toggle_selection(&mut state);
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
        let mut state = make_state(files);

        // Initial cursor should be on main (first with file)
        assert_eq!(state.current_tab().list_cursor, 0);

        // No selection → all_traversable = false
        // Move down — should skip empty1 and empty2, land on dev
        state.move_down(false);
        assert_eq!(state.current_tab().list_cursor, 3);

        // Move down again — should go to footer (no more entries with files)
        state.move_down(false);
        assert_eq!(state.focus, FocusPanel::Footer);

        // Move up from footer — back to list at last entry with file (dev)
        state.move_up(false);
        assert_eq!(state.focus, FocusPanel::WorktreeList);
        assert_eq!(state.current_tab().list_cursor, 3);

        // Move up — should skip empty2 and empty1, land on main
        state.move_up(false);
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
        let state = make_state(files);
        assert_eq!(state.current_tab().list_cursor, 1);
    }

    #[test]
    fn toggle_materialized_works_on_worktrees_without_file() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main", true), ("feat", "/repo/feat", false)],
        )];
        let mut state = make_state(files);

        // Select main as source
        CollectMode::toggle_selection(&mut state);
        // feat defaults to linked (no file)
        assert!(!state.current_tab().materialized[1]);
        // Toggle feat to materialized
        state.tabs[0].list_cursor = 1;
        CollectMode::toggle_materialized(&mut state);
        assert!(state.current_tab().materialized[1]);
    }

    #[test]
    fn move_down_navigates_to_footer() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = make_state(files);

        state.move_down(false);
        assert_eq!(state.focus, FocusPanel::Footer);
    }

    #[test]
    fn move_up_from_footer_returns_to_list() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = make_state(files);

        state.focus = FocusPanel::Footer;
        state.move_up(false);
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn preview_scroll_clamps_to_content() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = make_state(files);
        state.focus = FocusPanel::Preview;
        state.tabs[0].preview_content_lines = 30;
        state.tabs[0].preview_viewport_height = 10;

        state.move_down(false);
        assert_eq!(state.current_tab().preview_scroll, 1);
        state.move_up(false);
        state.move_up(false);
        assert_eq!(state.current_tab().preview_scroll, 0);

        for _ in 0..25 {
            state.move_down(false);
        }
        assert_eq!(state.current_tab().preview_scroll, 20);
    }

    #[test]
    fn preview_scroll_blocked_when_content_fits() {
        let files = vec![make_uncollected(".env", &[("main", "/repo/main", true)])];
        let mut state = make_state(files);
        state.focus = FocusPanel::Preview;
        state.tabs[0].preview_content_lines = 5;
        state.tabs[0].preview_viewport_height = 20;

        state.move_down(false);
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
        let mut state = make_state(files);

        // Select main for .env — deep compare finds dev is different → materialized
        CollectMode::toggle_selection(&mut state);
        assert!(state.tabs[0].materialized[1]); // dev: different → materialized
        assert!(!state.tabs[0].materialized[2]); // feat: no file → linked

        // Toggle feat to materialized
        state.tabs[0].list_cursor = 2;
        CollectMode::toggle_materialized(&mut state);

        let decisions = CollectMode::into_decisions(state);
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
        let mut state = make_state(files);

        // Select main — dev has identical content → should default to linked
        CollectMode::toggle_selection(&mut state);
        assert!(!state.tabs[0].materialized[1]);
    }

    #[test]
    fn all_decided_true_when_only_stubs() {
        let files = vec![
            make_uncollected(".secrets", &[("main", "/repo/main", false)]),
            make_uncollected(".tokens", &[("main", "/repo/main", false)]),
        ];
        let state = make_state(files);
        assert!(CollectMode::all_decided(&state));
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
        let mut state = make_state(files);

        CollectMode::toggle_selection(&mut state); // Select .env
        let undecided = CollectMode::undecided_files(&state);
        assert_eq!(undecided, vec![".idea"]);
    }
}
