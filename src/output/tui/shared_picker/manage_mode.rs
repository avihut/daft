//! ManageMode — implements PickerMode for the manage workflow.
//!
//! Provides an interactive TUI for inspecting and modifying the status of
//! shared files across worktrees: toggle between linked/materialized,
//! fix broken symlinks, and resolve conflicts.

use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::fs;
use std::path::PathBuf;

use crate::core::shared::{self, MaterializedState, SharedFileInfo, WorktreeStatus};

use super::state::{FileTabState, FocusPanel, PickerState, WorktreeEntry};
use super::{EntryDecoration, LoopAction, PickerMode};

const DIM: Color = Color::DarkGray;

/// Manage-specific mode state.
pub struct ManageMode {
    /// Absolute path to the git common dir (`.git/`).
    pub git_common_dir: PathBuf,
    /// Root directory for daft config (where `daft.yml` lives).
    pub config_root: PathBuf,
    /// Materialization tracking state.
    pub materialized: MaterializedState,
    /// Per-tab, per-entry status vectors — `statuses[tab_idx][entry_idx]`.
    pub statuses: Vec<Vec<WorktreeStatus>>,
    /// Absolute paths of all worktrees (parallel to entries within each tab).
    pub worktree_paths: Vec<PathBuf>,
    /// Temporary info/warning message for the user.
    pub info_message: Option<String>,
}

impl ManageMode {
    /// Build `FileTabState` tabs from status info.
    ///
    /// In manage mode, all entries are always traversable (`has_file: true`)
    /// because even missing/broken entries need to be selectable for actions.
    pub fn build_tabs(infos: &[SharedFileInfo]) -> Vec<FileTabState> {
        infos
            .iter()
            .map(|info| {
                let entries: Vec<WorktreeEntry> = info
                    .statuses
                    .iter()
                    .map(|(wt_path, wt_name, _status)| WorktreeEntry {
                        worktree_name: wt_name.clone(),
                        worktree_path: wt_path.clone(),
                        has_file: true, // All traversable in manage mode
                    })
                    .collect();
                FileTabState::new(info.rel_path.clone(), entries)
            })
            .collect()
    }

    /// Store the per-tab, per-entry status vectors.
    pub fn set_statuses(&mut self, infos: &[SharedFileInfo]) {
        self.statuses = infos
            .iter()
            .map(|info| info.statuses.iter().map(|(_, _, s)| *s).collect())
            .collect();
    }

    /// Toggle between linked and materialized for the currently highlighted entry.
    ///
    /// Performs the filesystem action immediately:
    /// - Linked -> Materialized: remove symlink, copy shared file, update tracking
    /// - Materialized -> Linked: remove file, create symlink, update tracking
    /// - Missing -> Materialized: copy shared file into worktree
    /// - Conflict/Broken/NotCollected -> no-op, show info message
    fn toggle_materialize(&mut self, state: &PickerState) {
        let tab = state.current_tab();
        let tab_idx = state.active_tab;
        let entry_idx = tab.list_cursor;
        let status = self.statuses[tab_idx][entry_idx];
        let rel_path = &tab.rel_path;
        let wt_path = &tab.entries[entry_idx].worktree_path;
        let file_path = wt_path.join(rel_path);
        let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);

        match status {
            WorktreeStatus::Linked => {
                // Linked -> Materialized: remove symlink, copy from shared
                if let Err(e) = fs::remove_file(&file_path) {
                    self.info_message = Some(format!("Failed to remove symlink: {e}"));
                    return;
                }
                let copy_result = if shared_target.is_dir() {
                    shared::copy_dir_all(&shared_target, &file_path)
                } else {
                    fs::copy(&shared_target, &file_path)
                        .map(|_| ())
                        .map_err(|e| e.into())
                };
                if let Err(e) = copy_result {
                    self.info_message = Some(format!("Failed to copy shared file: {e}"));
                    return;
                }
                self.materialized.add(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = Some("Materialized (local copy created)".to_string());
            }
            WorktreeStatus::Materialized => {
                // Materialized -> Linked: remove file, create symlink
                let remove_result = if file_path.is_dir() {
                    fs::remove_dir_all(&file_path)
                } else {
                    fs::remove_file(&file_path)
                };
                if let Err(e) = remove_result {
                    self.info_message = Some(format!("Failed to remove local copy: {e}"));
                    return;
                }
                if let Err(e) =
                    shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir)
                {
                    self.info_message = Some(format!("Failed to create symlink: {e}"));
                    return;
                }
                self.materialized.remove(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = Some("Linked (symlink restored)".to_string());
            }
            WorktreeStatus::Missing => {
                // Missing -> Materialized: copy from shared into worktree
                if let Some(parent) = file_path.parent() {
                    if !parent.exists() {
                        let _ = fs::create_dir_all(parent);
                    }
                }
                let copy_result = if shared_target.is_dir() {
                    shared::copy_dir_all(&shared_target, &file_path)
                } else {
                    fs::copy(&shared_target, &file_path)
                        .map(|_| ())
                        .map_err(|e| e.into())
                };
                if let Err(e) = copy_result {
                    self.info_message = Some(format!("Failed to copy shared file: {e}"));
                    return;
                }
                self.materialized.add(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = Some("Materialized (copied from shared)".to_string());
            }
            WorktreeStatus::Conflict => {
                self.info_message =
                    Some("Conflict: use 'i' to fix symlink, or materialize manually".to_string());
            }
            WorktreeStatus::Broken => {
                self.info_message = Some("Broken symlink: use 'i' to fix".to_string());
            }
            WorktreeStatus::NotCollected => {
                self.info_message = Some("Not collected: run 'daft shared sync' first".to_string());
            }
        }
    }

    /// Fix symlink for the currently highlighted entry.
    ///
    /// - Missing -> create symlink
    /// - Broken -> remove bad symlink, create correct one
    /// - Conflict -> remove file, create symlink
    /// - Others -> no-op
    fn link_entry(&mut self, state: &PickerState) {
        let tab = state.current_tab();
        let tab_idx = state.active_tab;
        let entry_idx = tab.list_cursor;
        let status = self.statuses[tab_idx][entry_idx];
        let rel_path = &tab.rel_path;
        let wt_path = &tab.entries[entry_idx].worktree_path;
        let file_path = wt_path.join(rel_path);

        match status {
            WorktreeStatus::Missing => {
                match shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
                    Ok(shared::LinkResult::Created) => {
                        self.info_message = Some("Symlink created".to_string());
                    }
                    Ok(shared::LinkResult::NoSource) => {
                        self.info_message = Some("No shared file to link to".to_string());
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.info_message = Some(format!("Failed to create symlink: {e}"));
                        return;
                    }
                }
                self.materialized.remove(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
            }
            WorktreeStatus::Broken => {
                // Remove bad symlink, then create correct one
                if let Err(e) = fs::remove_file(&file_path) {
                    self.info_message = Some(format!("Failed to remove broken symlink: {e}"));
                    return;
                }
                match shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
                    Ok(shared::LinkResult::Created) => {
                        self.info_message = Some("Symlink fixed".to_string());
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.info_message = Some(format!("Failed to create symlink: {e}"));
                        return;
                    }
                }
                self.materialized.remove(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
            }
            WorktreeStatus::Conflict => {
                // Remove conflicting file, create symlink
                let remove_result = if file_path.is_dir() {
                    fs::remove_dir_all(&file_path)
                } else {
                    fs::remove_file(&file_path)
                };
                if let Err(e) = remove_result {
                    self.info_message = Some(format!("Failed to remove conflicting file: {e}"));
                    return;
                }
                match shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
                    Ok(shared::LinkResult::Created) => {
                        self.info_message = Some("Conflict resolved: symlink created".to_string());
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.info_message = Some(format!("Failed to create symlink: {e}"));
                        return;
                    }
                }
                self.materialized.remove(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
            }
            WorktreeStatus::Linked => {
                self.info_message = Some("Already linked".to_string());
            }
            WorktreeStatus::Materialized => {
                self.info_message = Some("Materialized: use 'm' to switch to linked".to_string());
            }
            WorktreeStatus::NotCollected => {
                self.info_message = Some("Not collected: run 'daft shared sync' first".to_string());
            }
        }
    }

    /// Re-detect the status of all entries in a single tab.
    fn refresh_tab_status(&mut self, state: &PickerState, tab_idx: usize) {
        let tab = &state.tabs[tab_idx];
        let rel_path = &tab.rel_path;
        let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);
        let collected = shared_target.exists();

        self.statuses[tab_idx] = tab
            .entries
            .iter()
            .map(|entry| {
                if !collected {
                    WorktreeStatus::NotCollected
                } else {
                    let file_path = entry.worktree_path.join(rel_path);
                    if file_path.is_symlink() {
                        let actual = fs::read_link(&file_path).ok();
                        let expected = shared::relative_symlink_target(
                            file_path.parent().unwrap_or(&entry.worktree_path),
                            &shared_target,
                        )
                        .ok();
                        if actual == expected {
                            WorktreeStatus::Linked
                        } else {
                            WorktreeStatus::Broken
                        }
                    } else if file_path.exists() {
                        if self
                            .materialized
                            .is_materialized(rel_path, &entry.worktree_path)
                        {
                            WorktreeStatus::Materialized
                        } else {
                            WorktreeStatus::Conflict
                        }
                    } else {
                        WorktreeStatus::Missing
                    }
                }
            })
            .collect();
    }
}

impl PickerMode for ManageMode {
    fn all_entries_traversable(&self, _tab: &FileTabState) -> bool {
        true
    }

    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char('m') | KeyCode::Enter | KeyCode::Char(' ') => {
                self.info_message = None;
                self.toggle_materialize(state);
                let tab_idx = state.active_tab;
                self.refresh_tab_status(state, tab_idx);
            }
            KeyCode::Char('i') => {
                self.info_message = None;
                self.link_entry(state);
                let tab_idx = state.active_tab;
                self.refresh_tab_status(state, tab_idx);
            }
            _ => {}
        }
        LoopAction::Continue
    }

    fn handle_footer_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return LoopAction::Exit,
            KeyCode::Up | KeyCode::Char('k') => {
                let all = self.all_entries_traversable(state.current_tab());
                state.move_up(all);
            }
            KeyCode::Tab => state.toggle_panel(),
            _ => {}
        }
        LoopAction::Continue
    }

    fn tab_decided(&self, _tab: &FileTabState) -> bool {
        false
    }

    fn tab_warning<'a>(&'a self, _tab: &'a FileTabState) -> Option<&'a str> {
        self.info_message.as_deref()
    }

    fn entry_decoration(
        &self,
        _tab: &FileTabState,
        tab_idx: usize,
        entry_idx: usize,
    ) -> EntryDecoration {
        let status = self
            .statuses
            .get(tab_idx)
            .and_then(|tab| tab.get(entry_idx))
            .copied()
            .unwrap_or(WorktreeStatus::Missing);

        let (marker, tag) = match status {
            WorktreeStatus::Linked => (
                "\u{2192} ".to_string(), // →
                Some(("linked".to_string(), Color::Green)),
            ),
            WorktreeStatus::Materialized => (
                "M ".to_string(),
                Some(("materialized".to_string(), Color::Yellow)),
            ),
            WorktreeStatus::Missing => ("  ".to_string(), Some(("missing".to_string(), DIM))),
            WorktreeStatus::Conflict => {
                ("! ".to_string(), Some(("conflict".to_string(), Color::Red)))
            }
            WorktreeStatus::Broken => (
                "? ".to_string(),
                Some(("broken".to_string(), Color::Yellow)),
            ),
            WorktreeStatus::NotCollected => {
                ("  ".to_string(), Some(("not collected".to_string(), DIM)))
            }
        };

        EntryDecoration { marker, tag }
    }

    fn render_footer(&self, state: &PickerState, frame: &mut Frame, area: Rect) {
        let key_style = Style::default().fg(Color::Cyan);
        let desc_style = Style::default().fg(DIM);

        let help = Line::from(vec![
            Span::raw("  "),
            Span::styled("m", key_style),
            Span::styled(" materialize/link  ", desc_style),
            Span::styled("i", key_style),
            Span::styled(" fix symlink  ", desc_style),
            Span::styled("Tab", key_style),
            Span::styled(" panel  ", desc_style),
            Span::styled("Esc", key_style),
            Span::styled(" quit", desc_style),
        ]);

        let nav_help = Line::from(vec![
            Span::raw("  "),
            Span::styled("jk/\u{2191}\u{2193}", key_style),
            Span::styled(" navigate  ", desc_style),
            Span::styled("hl/\u{2190}\u{2192}", key_style),
            Span::styled(
                match state.focus {
                    FocusPanel::Footer => " buttons  ",
                    _ => " tabs  ",
                },
                desc_style,
            ),
            Span::styled("PgUp/PgDn", key_style),
            Span::styled(" scroll  ", desc_style),
        ]);

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::TOP)
            .border_style(Style::default().fg(DIM));

        let title = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                "Shared File Manager",
                Style::default()
                    .fg(Color::Indexed(208))
                    .add_modifier(Modifier::BOLD),
            ),
        ]);

        let paragraph = Paragraph::new(vec![title, Line::raw(""), help, nav_help]).block(block);
        frame.render_widget(paragraph, area);
    }

    fn footer_height(&self) -> u16 {
        6
    }
}
