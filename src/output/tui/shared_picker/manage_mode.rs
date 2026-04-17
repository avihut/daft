//! ManageMode — implements PickerMode for the manage workflow.
//!
//! Provides an interactive TUI for inspecting and modifying the status of
//! shared files across worktrees: toggle between linked/materialized,
//! fix broken symlinks, and resolve conflicts.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame, Terminal,
};
use similar::{ChangeTag, TextDiff};
use std::fs;
use std::io;
use std::path::PathBuf;

use crate::core::layout;
use crate::core::shared::{self, MaterializedState, SharedFileInfo, WorktreeStatus};

use super::add_modal::{show_add_modal, AddResult};
use super::dialog::show_confirm_dialog;
use super::remove_modal::{show_remove_modal, RemoveDecision};
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
    /// Root of the current worktree (for add-file modal).
    pub worktree_root: PathBuf,
    /// Temporary info/warning message for the user.
    pub info_message: Option<String>,
    /// If `Some`, diff mode is active. The value is `(tab_idx, entry_idx)` of the pivot.
    pub diff_pivot: Option<(usize, usize)>,
    /// When true, the event loop will call `show_modal` to display the remove confirmation.
    pub pending_remove: bool,
    /// When true, the event loop will call `show_modal` to display the add-file modal.
    pub pending_add: bool,
    /// When true, the event loop will show a confirmation dialog before
    /// switching a materialized file to linked (overwriting local changes).
    pub pending_link_confirm: bool,
    /// Active inline file editor session, if any.
    pub edit_state: Option<super::editor::EditSession>,
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

    /// Start an inline editing session for the currently highlighted entry.
    fn start_edit(&mut self, state: &PickerState) {
        if state.is_virtual_tab() || state.tabs.is_empty() {
            return;
        }
        let tab = state.current_tab();
        let tab_idx = state.active_tab;
        let entry_idx = tab.list_cursor;
        let status = self
            .statuses
            .get(tab_idx)
            .and_then(|t| t.get(entry_idx))
            .copied()
            .unwrap_or(WorktreeStatus::Missing);
        let entry = &tab.entries[entry_idx];
        self.edit_state = super::editor::try_start_edit(
            status,
            &tab.rel_path,
            &entry.worktree_path,
            &entry.worktree_name,
            &self.git_common_dir,
        );
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
        let wt_name = &tab.entries[entry_idx].worktree_name;
        let wt_path = &tab.entries[entry_idx].worktree_path;
        let file_path = wt_path.join(rel_path);
        let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);

        match status {
            WorktreeStatus::Linked => {
                // Linked -> Materialized: remove symlink, copy from shared
                if let Err(e) = fs::remove_file(&file_path) {
                    self.info_message = Some(format!("{wt_name}: failed to remove symlink: {e}"));
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
                    self.info_message = Some(format!("{wt_name}: failed to copy shared file: {e}"));
                    return;
                }
                self.materialized.add(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = Some(format!("{wt_name}: materialized (local copy created)"));
            }
            WorktreeStatus::Materialized => {
                // If the local copy differs from the shared copy, defer to a
                // confirmation dialog (needs terminal access via show_modal).
                let compare = shared::deep_compare(
                    &shared_target,
                    &file_path,
                    std::time::Duration::from_secs(1),
                );
                if compare != shared::CompareResult::Identical {
                    self.pending_link_confirm = true;
                    return;
                }
                // Identical — safe to link without confirmation
                self.execute_link(state);
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
                    self.info_message = Some(format!("{wt_name}: failed to copy shared file: {e}"));
                    return;
                }
                self.materialized.add(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = Some(format!("{wt_name}: materialized (copied from shared)"));
            }
            WorktreeStatus::Conflict => {
                self.info_message = Some(format!(
                    "{wt_name}: conflict \u{2014} use 'i' to fix symlink, or materialize manually"
                ));
            }
            WorktreeStatus::Broken => {
                self.info_message =
                    Some(format!("{wt_name}: broken symlink \u{2014} use 'i' to fix"));
            }
            WorktreeStatus::NotCollected => {
                self.info_message = Some(format!(
                    "{wt_name}: not collected \u{2014} run 'daft shared sync' first"
                ));
            }
        }
    }

    /// Check whether the currently highlighted entry is in NotCollected state.
    fn is_not_collected(&self, state: &PickerState) -> bool {
        let tab_idx = state.active_tab;
        let entry_idx = state.current_tab().list_cursor;
        self.statuses
            .get(tab_idx)
            .and_then(|t| t.get(entry_idx))
            .copied()
            == Some(WorktreeStatus::NotCollected)
    }

    /// Collect an uncollected file using the currently highlighted worktree as
    /// the source. Only works if that worktree actually has a copy of the file
    /// on disk; otherwise shows an info message.
    fn collect_from_worktree(&mut self, state: &mut PickerState) {
        let tab = state.current_tab();
        let rel_path = tab.rel_path.clone();
        let entry_idx = tab.list_cursor;
        let wt_name = tab.entries[entry_idx].worktree_name.clone();
        let wt_path = tab.entries[entry_idx].worktree_path.clone();
        let source_path = wt_path.join(&rel_path);

        // The worktree must have a real (non-symlink) copy of the file.
        if !source_path.exists() || source_path.is_symlink() {
            self.info_message = Some(format!(
                "{wt_name}: no copy of {rel_path} \u{2014} select a worktree that has it"
            ));
            return;
        }

        // Ensure shared storage directory exists.
        if let Err(e) = shared::ensure_shared_dir(&self.git_common_dir) {
            self.info_message = Some(format!("Failed to create shared storage: {e}"));
            return;
        }

        // Find the source index in worktree_paths.
        let source_idx = match self.worktree_paths.iter().position(|p| p == &wt_path) {
            Some(idx) => idx,
            None => {
                self.info_message = Some(format!("{wt_name}: worktree path not found"));
                return;
            }
        };

        // Compute materialization defaults for other worktrees.
        let (mat, _timed_out) = shared::compute_materialization_defaults(
            &source_path,
            &rel_path,
            &self.worktree_paths,
            source_idx,
            std::time::Duration::from_secs(1),
        );

        let materialize_in: Vec<PathBuf> = self
            .worktree_paths
            .iter()
            .enumerate()
            .filter(|(i, _)| mat[*i])
            .map(|(_, p)| p.clone())
            .collect();

        let decision = shared::CollectDecision {
            rel_path: rel_path.clone(),
            source_worktree: wt_path,
            materialize_in,
        };

        if let Err(e) = shared::execute_collect(
            &decision,
            &self.worktree_paths,
            &self.git_common_dir,
            &self.config_root,
            &mut self.materialized,
        ) {
            self.info_message = Some(format!("Collect failed: {e}"));
            return;
        }

        let _ = self.materialized.save(&self.git_common_dir);
        self.info_message = Some(format!("Collected {rel_path} from {wt_name}"));
        self.refresh_all(state);
    }

    /// Execute the Materialized -> Linked conversion for the current entry.
    /// Removes the local copy and creates a symlink to the shared file.
    pub fn execute_link(&mut self, state: &PickerState) {
        let tab = state.current_tab();
        let tab_idx = state.active_tab;
        let entry_idx = tab.list_cursor;
        let rel_path = &tab.rel_path;
        let wt_name = &tab.entries[entry_idx].worktree_name;
        let wt_path = &tab.entries[entry_idx].worktree_path;
        let file_path = wt_path.join(rel_path);

        let remove_result = if file_path.is_dir() {
            fs::remove_dir_all(&file_path)
        } else {
            fs::remove_file(&file_path)
        };
        if let Err(e) = remove_result {
            self.info_message = Some(format!("{wt_name}: failed to remove local copy: {e}"));
            return;
        }
        if let Err(e) = shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
            self.info_message = Some(format!("{wt_name}: failed to create symlink: {e}"));
            return;
        }
        self.materialized.remove(rel_path, wt_path);
        let _ = self.materialized.save(&self.git_common_dir);
        self.statuses[tab_idx][entry_idx] = WorktreeStatus::Linked;
        self.info_message = Some(format!("{wt_name}: linked (symlink restored)"));
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
        let wt_name = &tab.entries[entry_idx].worktree_name;
        let wt_path = &tab.entries[entry_idx].worktree_path;
        let file_path = wt_path.join(rel_path);

        match status {
            WorktreeStatus::Missing => {
                match shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
                    Ok(shared::LinkResult::Created) => {
                        self.info_message = Some(format!("{wt_name}: symlink created"));
                    }
                    Ok(shared::LinkResult::NoSource) => {
                        self.info_message = Some(format!("{wt_name}: no shared file to link to"));
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.info_message =
                            Some(format!("{wt_name}: failed to create symlink: {e}"));
                        return;
                    }
                }
                self.materialized.remove(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
            }
            WorktreeStatus::Broken => {
                // Remove bad symlink, then create correct one
                if let Err(e) = fs::remove_file(&file_path) {
                    self.info_message =
                        Some(format!("{wt_name}: failed to remove broken symlink: {e}"));
                    return;
                }
                match shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
                    Ok(shared::LinkResult::Created) => {
                        self.info_message = Some(format!("{wt_name}: symlink fixed"));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.info_message =
                            Some(format!("{wt_name}: failed to create symlink: {e}"));
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
                    self.info_message =
                        Some(format!("{wt_name}: failed to remove conflicting file: {e}"));
                    return;
                }
                match shared::create_shared_symlink(wt_path, rel_path, &self.git_common_dir) {
                    Ok(shared::LinkResult::Created) => {
                        self.info_message =
                            Some(format!("{wt_name}: conflict resolved, symlink created"));
                    }
                    Ok(_) => {}
                    Err(e) => {
                        self.info_message =
                            Some(format!("{wt_name}: failed to create symlink: {e}"));
                        return;
                    }
                }
                self.materialized.remove(rel_path, wt_path);
                let _ = self.materialized.save(&self.git_common_dir);
            }
            WorktreeStatus::Linked => {
                self.info_message = Some(format!("{wt_name}: already linked"));
            }
            WorktreeStatus::Materialized => {
                self.info_message = Some(format!(
                    "{wt_name}: materialized \u{2014} use 'm' to switch to linked"
                ));
            }
            WorktreeStatus::NotCollected => {
                self.info_message = Some(format!(
                    "{wt_name}: not collected \u{2014} run 'daft shared sync' first"
                ));
            }
        }
    }

    /// Toggle diff mode. If off, enter with current entry as pivot.
    /// If on and `d` is pressed again, exit.
    fn toggle_diff_mode(&mut self, state: &PickerState) {
        if self.diff_pivot.is_some() {
            self.diff_pivot = None;
            self.info_message = None;
        } else {
            self.diff_pivot = Some((state.active_tab, state.current_tab().list_cursor));
            self.info_message = Some("Diff mode: navigate to compare against pivot".to_string());
        }
    }

    /// Compute diff preview lines for the current entry against the pivot.
    fn diff_preview_lines(&self, state: &PickerState) -> Vec<Line<'static>> {
        let Some((pivot_tab, pivot_entry)) = self.diff_pivot else {
            return vec![];
        };

        let tab = state.current_tab();
        let current_entry = tab.list_cursor;

        // Same entry as pivot
        if state.active_tab == pivot_tab && current_entry == pivot_entry {
            return vec![Line::styled(
                "(pivot \u{2014} select another worktree to compare)",
                Style::default().fg(Color::DarkGray),
            )];
        }

        // Different tab than pivot
        if state.active_tab != pivot_tab {
            return vec![Line::styled(
                "(diff not available \u{2014} pivot is on a different file)",
                Style::default().fg(Color::DarkGray),
            )];
        }

        let pivot_wt = &tab.entries[pivot_entry].worktree_path;
        let current_wt = &tab.entries[current_entry].worktree_path;
        let rel_path = &tab.rel_path;

        let pivot_path = pivot_wt.join(rel_path);
        let current_path = current_wt.join(rel_path);

        let pivot_content = fs::read_to_string(&pivot_path).unwrap_or_default();
        let current_content = match fs::read_to_string(&current_path) {
            Ok(c) => c,
            Err(_) => {
                return vec![Line::styled(
                    "(no file in this worktree)",
                    Style::default().fg(Color::DarkGray),
                )];
            }
        };

        if pivot_content == current_content {
            return vec![Line::styled(
                "(identical to pivot)",
                Style::default().fg(Color::Green),
            )];
        }

        // Compute line-level diff
        let diff = TextDiff::from_lines(&pivot_content, &current_content);
        let mut lines = Vec::new();

        for change in diff.iter_all_changes() {
            let (style, prefix) = match change.tag() {
                ChangeTag::Delete => (Style::default().fg(Color::Red), "-"),
                ChangeTag::Insert => (Style::default().fg(Color::Green), "+"),
                ChangeTag::Equal => (Style::default().fg(Color::DarkGray), " "),
            };
            let text = change.to_string_lossy();
            let text = text.trim_end_matches('\n');
            lines.push(Line::from(Span::styled(format!("{prefix} {text}"), style)));
        }

        lines
    }

    /// Execute the removal of a shared file.
    ///
    /// If `materialize_checks` is `Some`, the file is materialized (copied)
    /// into checked worktrees and symlinks are removed from unchecked ones.
    /// If `None` (delete everywhere), all copies and symlinks are removed.
    ///
    /// After removal:
    /// - Shared storage file/dir is deleted
    /// - `materialized.json` is updated
    /// - `daft.yml` is updated
    /// - The tab is removed from the tab bar
    fn execute_remove(
        &mut self,
        state: &mut PickerState,
        tab_idx: usize,
        rel_path: &str,
        materialize_checks: Option<&[bool]>,
    ) -> Result<()> {
        let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);
        let entries: Vec<(PathBuf, String)> = state.tabs[tab_idx]
            .entries
            .iter()
            .map(|e| (e.worktree_path.clone(), e.worktree_name.clone()))
            .collect();

        for (i, (wt_path, _wt_name)) in entries.iter().enumerate() {
            let file_path = wt_path.join(rel_path);

            match materialize_checks {
                Some(checks) if checks[i] => {
                    // Materialize: ensure a real copy exists in the worktree.
                    if file_path.is_symlink() {
                        // Remove symlink, copy from shared storage
                        let _ = fs::remove_file(&file_path);
                        if shared_target.is_dir() {
                            let _ = shared::copy_dir_all(&shared_target, &file_path);
                        } else {
                            let _ = fs::copy(&shared_target, &file_path);
                        }
                    } else if !file_path.exists() {
                        // Missing — copy from shared
                        if let Some(parent) = file_path.parent() {
                            if !parent.exists() {
                                let _ = fs::create_dir_all(parent);
                            }
                        }
                        if shared_target.is_dir() {
                            let _ = shared::copy_dir_all(&shared_target, &file_path);
                        } else {
                            let _ = fs::copy(&shared_target, &file_path);
                        }
                    }
                    // If it's already a real file (materialized/conflict), leave it.
                }
                Some(_) => {
                    // Not checked — remove any symlink or file
                    if file_path.is_symlink() {
                        let _ = fs::remove_file(&file_path);
                    }
                    // If the file is materialized (real), leave it — user unchecked means
                    // "don't materialize", but the file may already be a local copy.
                    // For unchecked worktrees we only clean up symlinks.
                }
                None => {
                    // Delete everywhere — remove file/symlink/dir
                    if file_path.is_symlink() || file_path.is_file() {
                        let _ = fs::remove_file(&file_path);
                    } else if file_path.is_dir() {
                        let _ = fs::remove_dir_all(&file_path);
                    }
                }
            }
        }

        // Remove shared storage file/dir
        if shared_target.is_dir() {
            let _ = fs::remove_dir_all(&shared_target);
        } else if shared_target.exists() {
            let _ = fs::remove_file(&shared_target);
        }

        // Update materialized.json — remove all entries for this path
        self.materialized.remove_all(rel_path);
        let _ = self.materialized.save(&self.git_common_dir);

        // Update daft.yml — remove this path from the shared list
        let _ = shared::remove_from_daft_yml(&self.config_root, &[rel_path]);

        // Remove the tab and its statuses
        state.tabs.remove(tab_idx);
        self.statuses.remove(tab_idx);

        // Adjust active tab if needed
        if state.tabs.is_empty() {
            // No tabs left — will show empty state
            state.active_tab = 0;
        } else if tab_idx >= state.tabs.len() {
            // Removed the last tab — move to the new last tab
            state.active_tab = state.tabs.len() - 1;
        }
        // else: tab_idx is still valid (we removed a tab before the end)

        // Reset diff pivot if it referenced the removed tab
        if let Some((pivot_tab, _)) = self.diff_pivot {
            if pivot_tab == tab_idx {
                self.diff_pivot = None;
            } else if pivot_tab > tab_idx {
                // Shift pivot index down
                self.diff_pivot = Some((pivot_tab - 1, self.diff_pivot.unwrap().1));
            }
        }

        if state.tabs.is_empty() {
            self.info_message = Some("No shared files remaining".to_string());
        } else {
            self.info_message = Some(format!("Removed {rel_path}"));
        }

        Ok(())
    }

    /// Re-detect the status of all entries in a single tab.
    pub fn refresh_tab_status(&mut self, state: &PickerState, tab_idx: usize) {
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

    /// Show the add-file modal and execute the result.
    fn show_add_modal(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
        state: &mut PickerState,
    ) -> Result<()> {
        let shared_paths = shared::read_shared_paths(&self.worktree_root).unwrap_or_default();

        let result = show_add_modal(terminal, &self.worktree_root, &shared_paths)?;

        match result {
            AddResult::Selected(rel_path) => {
                let rel_str = rel_path.to_string_lossy().to_string();

                // Already shared?
                if shared_paths.contains(&rel_str) {
                    self.info_message = Some(format!("{rel_str} is already shared"));
                    return Ok(());
                }

                // Git-tracked?
                if shared::is_git_tracked(&self.worktree_root, &rel_str)? {
                    self.info_message = Some(format!(
                        "{rel_str} is tracked by git. Untrack it first with: git rm --cached {rel_str}"
                    ));
                    return Ok(());
                }

                self.execute_add(state, &rel_str)?;
            }
            AddResult::Declared(name) => {
                if shared_paths.contains(&name) {
                    self.info_message = Some(format!("{name} is already shared"));
                    return Ok(());
                }

                self.execute_declare(state, &name)?;
            }
            AddResult::Cancelled => {}
        }

        Ok(())
    }

    /// Execute adding an existing file to shared storage.
    ///
    /// Mirrors the sync/collect behavior using `compute_materialization_defaults`:
    /// - Move selected file to `.git/.daft/shared/`
    /// - Deep-compare each worktree's copy against the source
    /// - Identical copies: remove and symlink (linked)
    /// - Different copies: keep local copy (materialized)
    /// - No copy: create symlink (linked)
    /// - Add to `daft.yml` and `.gitignore`
    fn execute_add(&mut self, state: &mut PickerState, rel_path: &str) -> Result<()> {
        shared::ensure_shared_dir(&self.git_common_dir)?;

        let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);
        if let Some(parent) = shared_target.parent() {
            fs::create_dir_all(parent)?;
        }

        // Compute materialization defaults BEFORE moving the source file
        let source_idx = self
            .worktree_paths
            .iter()
            .position(|p| p == &self.worktree_root)
            .unwrap_or(0);
        let source_path = self.worktree_root.join(rel_path);
        let (mat, _timed_out) = shared::compute_materialization_defaults(
            &source_path,
            rel_path,
            &self.worktree_paths,
            source_idx,
            std::time::Duration::from_secs(1),
        );

        // Move source file to shared storage
        if fs::rename(&source_path, &shared_target).is_err() {
            if source_path.is_dir() {
                shared::copy_dir_all(&source_path, &shared_target)?;
                fs::remove_dir_all(&source_path)?;
            } else {
                fs::copy(&source_path, &shared_target)?;
                fs::remove_file(&source_path)?;
            }
        }

        // Source worktree: create symlink (file was moved out)
        shared::create_shared_symlink(&self.worktree_root, rel_path, &self.git_common_dir)?;

        // Process all other worktrees using computed defaults
        for (i, wt) in self.worktree_paths.iter().enumerate() {
            if i == source_idx {
                continue;
            }
            let file_path = wt.join(rel_path);
            let has_file = file_path.exists() && !file_path.is_symlink();

            if mat[i] {
                // Different content — keep local copy, mark materialized
                if has_file {
                    self.materialized.add(rel_path, wt);
                } else {
                    // Shouldn't happen (mat=true only for has_file), but handle gracefully
                    shared::create_shared_symlink(wt, rel_path, &self.git_common_dir)?;
                }
            } else {
                // Identical or no file — remove existing copy if any, create symlink
                if has_file {
                    if file_path.is_dir() {
                        fs::remove_dir_all(&file_path)?;
                    } else {
                        fs::remove_file(&file_path)?;
                    }
                }
                if !file_path.exists() {
                    if let Some(parent) = file_path.parent() {
                        if parent != wt.as_path() && !parent.exists() {
                            fs::create_dir_all(parent)?;
                        }
                    }
                    shared::create_shared_symlink(wt, rel_path, &self.git_common_dir)?;
                }
            }
        }

        // Persist materialization state
        let _ = self.materialized.save(&self.git_common_dir);

        // Update daft.yml and .gitignore
        layout::ensure_gitignore_entry(&self.config_root, rel_path)?;
        shared::add_to_daft_yml(&self.config_root, &[rel_path])?;

        self.info_message = Some(format!("Shared: {rel_path}"));
        self.refresh_all(state);

        Ok(())
    }

    /// Execute declaring a file as shared (without collecting).
    ///
    /// Equivalent to `daft shared add --declare <path>`:
    /// - Add to `daft.yml`
    /// - Add to `.gitignore`
    fn execute_declare(&mut self, state: &mut PickerState, rel_path: &str) -> Result<()> {
        layout::ensure_gitignore_entry(&self.config_root, rel_path)?;
        shared::add_to_daft_yml(&self.config_root, &[rel_path])?;

        self.info_message = Some(format!("Declared: {rel_path}"));

        // Refresh tabs
        self.refresh_all(state);

        Ok(())
    }

    /// Re-read shared paths, re-detect statuses, and rebuild tabs.
    fn refresh_all(&mut self, state: &mut PickerState) {
        let shared_paths = shared::read_shared_paths(&self.worktree_root).unwrap_or_default();
        let infos = shared::detect_shared_statuses(
            &shared_paths,
            &self.worktree_paths,
            &self.git_common_dir,
            &self.materialized,
        );
        let new_tabs = Self::build_tabs(&infos);
        self.set_statuses(&infos);
        state.tabs = new_tabs;

        // Clamp active_tab and ensure focus is on the worktree list
        if !state.tabs.is_empty() {
            if state.active_tab >= state.tabs.len() {
                state.active_tab = state.tabs.len().saturating_sub(1);
            }
            state.focus = FocusPanel::WorktreeList;
        } else {
            state.active_tab = 0;
            state.focus = FocusPanel::TabBar;
        }
    }
}

impl PickerMode for ManageMode {
    fn all_entries_traversable(&self, _tab: &FileTabState) -> bool {
        true
    }

    fn pre_handle_key(&mut self, key: KeyCode, _state: &mut PickerState) -> bool {
        if self.diff_pivot.is_some() && key == KeyCode::Esc {
            self.diff_pivot = None;
            self.info_message = None;
            return true; // Consume Esc — don't navigate to footer
        }
        false
    }

    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        if key == KeyCode::Enter && state.focus == FocusPanel::Preview {
            self.start_edit(state);
            return LoopAction::Continue;
        }
        match key {
            KeyCode::Char('d') => {
                self.toggle_diff_mode(state);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.info_message = None;
                if self.is_not_collected(state) {
                    self.collect_from_worktree(state);
                    // collect_from_worktree calls refresh_all on success
                } else {
                    self.toggle_materialize(state);
                    let tab_idx = state.active_tab;
                    self.refresh_tab_status(state, tab_idx);
                }
            }
            KeyCode::Char('m') => {
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
            KeyCode::Char('r') | KeyCode::Delete | KeyCode::Backspace => {
                self.info_message = None;
                self.pending_remove = true;
            }
            KeyCode::Char('a') => {
                self.info_message = None;
                self.pending_add = true;
            }
            _ => {}
        }
        LoopAction::Continue
    }

    fn handle_footer_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return LoopAction::Exit,
            KeyCode::Up | KeyCode::Char('k') => {
                if state.is_virtual_tab() {
                    state.focus = FocusPanel::TabBar;
                } else {
                    let all = self.all_entries_traversable(state.current_tab());
                    state.move_up(all);
                }
            }
            KeyCode::Tab if !state.is_virtual_tab() => {
                state.toggle_panel();
            }
            _ => {}
        }
        LoopAction::Continue
    }

    fn extra_tab_labels(&self) -> Vec<String> {
        vec!["+".to_string()]
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

        let diff_active = self.diff_pivot.is_some();
        let diff_desc = if diff_active {
            " exit diff  "
        } else {
            " diff  "
        };
        let esc_desc = if diff_active { " exit diff" } else { " quit" };

        let help = Line::from(vec![
            Span::raw("  "),
            Span::styled("a", key_style),
            Span::styled(" add  ", desc_style),
            Span::styled("d", key_style),
            Span::styled(diff_desc, desc_style),
            Span::styled("m", key_style),
            Span::styled(" materialize/link  ", desc_style),
            Span::styled("i", key_style),
            Span::styled(" fix symlink  ", desc_style),
            Span::styled("r", key_style),
            Span::styled(" remove  ", desc_style),
            Span::styled("Tab", key_style),
            Span::styled(" panel  ", desc_style),
            Span::styled("Esc", key_style),
            Span::styled(esc_desc, desc_style),
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

    fn render_editor(&mut self, frame: &mut Frame, area: Rect) -> bool {
        if let Some(ref mut session) = self.edit_state {
            session.render(frame, area);
            true
        } else {
            false
        }
    }

    fn is_editing_shared(&self) -> bool {
        self.edit_state.as_ref().is_some_and(|s| s.is_shared)
    }

    fn preview_override(&self, state: &PickerState) -> Option<Vec<Line<'static>>> {
        if self.diff_pivot.is_some() {
            Some(self.diff_preview_lines(state))
        } else {
            None
        }
    }

    fn needs_modal(&self) -> bool {
        self.pending_remove || self.pending_add || self.pending_link_confirm
    }

    fn show_modal(
        &mut self,
        terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
        state: &mut PickerState,
    ) -> Result<()> {
        if self.pending_add {
            self.pending_add = false;
            return self.show_add_modal(terminal, state);
        }

        self.pending_remove = false;

        if state.tabs.is_empty() {
            return Ok(());
        }

        let tab_idx = state.active_tab;
        let rel_path = state.tabs[tab_idx].rel_path.clone();
        let worktree_names: Vec<String> = state.tabs[tab_idx]
            .entries
            .iter()
            .map(|e| e.worktree_name.clone())
            .collect();

        let decision = show_remove_modal(terminal, &rel_path, &worktree_names)?;

        match decision {
            RemoveDecision::Cancelled => {}
            RemoveDecision::DeleteAll => {
                // Secondary confirmation
                let confirmed = show_confirm_dialog(
                    terminal,
                    "Confirm deletion",
                    &[
                        &format!("This will delete {rel_path} from all worktrees."),
                        "Are you sure?",
                    ],
                )?;
                if confirmed {
                    self.execute_remove(state, tab_idx, &rel_path, None)?;
                }
            }
            RemoveDecision::Materialize(checks) => {
                self.execute_remove(state, tab_idx, &rel_path, Some(&checks))?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Set up a minimal manage mode + picker state for testing.
    fn setup(worktree_names: &[&str], rel_path: &str) -> (TempDir, ManageMode, PickerState) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let git_dir = root.join(".git");
        fs::create_dir_all(git_dir.join(".daft/shared")).unwrap();

        let mut wt_paths = Vec::new();
        let mut entries = Vec::new();
        for name in worktree_names {
            let wt = root.join(name);
            fs::create_dir_all(&wt).unwrap();
            entries.push(WorktreeEntry {
                worktree_name: name.to_string(),
                worktree_path: wt.clone(),
                has_file: true,
            });
            wt_paths.push(wt);
        }

        let tab = FileTabState::new(rel_path.to_string(), entries);
        let n = worktree_names.len();
        let statuses = vec![vec![WorktreeStatus::NotCollected; n]];
        let state = PickerState::from_tabs(vec![tab]);

        // Write a .gitignore so execute_collect can append to it.
        fs::write(root.join(".gitignore"), "").unwrap();

        // Write daft.yml in worktree_root so refresh_all can find declared shared paths.
        let wt_root = wt_paths[0].clone();
        fs::write(
            wt_root.join("daft.yml"),
            format!("shared:\n  - {rel_path}\n"),
        )
        .unwrap();

        let mode = ManageMode {
            git_common_dir: git_dir,
            config_root: root,
            materialized: shared::MaterializedState::default(),
            statuses,
            worktree_paths: wt_paths,
            worktree_root: tmp.path().join(worktree_names[0]),
            info_message: None,
            diff_pivot: None,
            pending_remove: false,
            pending_add: false,
            pending_link_confirm: false,
            edit_state: None,
        };

        (tmp, mode, state)
    }

    #[test]
    fn collect_from_worktree_with_file_succeeds() {
        let (_tmp, mut mode, mut state) = setup(&["main", "develop"], ".env");

        // Create .env in both worktrees with different content.
        fs::write(mode.worktree_paths[0].join(".env"), "FROM_MAIN=1").unwrap();
        fs::write(mode.worktree_paths[1].join(".env"), "FROM_DEVELOP=1").unwrap();

        // Cursor is on worktree 0 (main). Trigger collect.
        state.tabs[0].list_cursor = 0;
        mode.collect_from_worktree(&mut state);

        // Shared storage should have main's content.
        let shared = shared::shared_file_path(&mode.git_common_dir, ".env");
        assert!(shared.exists(), "shared file should exist after collect");
        assert_eq!(fs::read_to_string(&shared).unwrap(), "FROM_MAIN=1");

        // main: should be symlinked (source worktree).
        assert!(
            mode.worktree_paths[0].join(".env").is_symlink(),
            "source worktree should be symlinked"
        );

        // develop: had different content, should be materialized.
        let dev_env = mode.worktree_paths[1].join(".env");
        assert!(!dev_env.is_symlink(), "develop should keep its local copy");
        assert_eq!(fs::read_to_string(&dev_env).unwrap(), "FROM_DEVELOP=1");

        // Info message should confirm success.
        assert!(
            mode.info_message
                .as_ref()
                .unwrap()
                .contains("Collected .env"),
            "info message should confirm collection"
        );

        // Statuses should have been refreshed (no longer NotCollected).
        assert!(
            !mode.statuses[0].contains(&WorktreeStatus::NotCollected),
            "no entries should be NotCollected after collect"
        );
    }

    #[test]
    fn collect_from_worktree_without_file_shows_message() {
        let (_tmp, mut mode, mut state) = setup(&["main", "develop"], ".env");

        // Only develop has the file, not main.
        fs::write(mode.worktree_paths[1].join(".env"), "FROM_DEVELOP=1").unwrap();

        // Cursor is on worktree 0 (main) which has no .env.
        state.tabs[0].list_cursor = 0;
        mode.collect_from_worktree(&mut state);

        // Should NOT have collected.
        let shared = shared::shared_file_path(&mode.git_common_dir, ".env");
        assert!(!shared.exists(), "shared file should not exist");

        // Info message should tell user to pick a worktree that has it.
        let msg = mode.info_message.as_ref().unwrap();
        assert!(
            msg.contains("no copy of .env"),
            "info message should explain the file is missing: {msg}"
        );
    }
}
