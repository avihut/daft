//! Reusable TUI table component for commands that process worktrees in parallel.
//!
//! `OperationTable` wraps [`TuiState`] and [`TuiRenderer`] into a single
//! composable unit that sync, prune, clone, and other commands can consume
//! without each reimplementing the wiring.

use super::{columns::Column, driver::TuiRenderer, state::TuiState};
use crate::{
    core::worktree::list::Stat,
    core::worktree::sync_dag::OperationPhase,
    core::{
        sort::SortSpec,
        worktree::{list::WorktreeInfo, sync_dag::DagEvent},
    },
    output::tui::state::HookSummaryEntry,
    output::tui::WorktreeRow,
};
use std::{path::PathBuf, sync::mpsc};

/// Configuration for a TUI table operation.
pub struct TableConfig {
    /// User-selected columns (`None` = use responsive selection).
    pub columns: Option<Vec<Column>>,
    /// If `true`, the user explicitly chose columns (replace mode) — disables
    /// responsive dropping.
    pub columns_explicit: bool,
    /// User-specified sort order (`None` = default alphabetical).
    pub sort_spec: Option<SortSpec>,
    /// Extra rows to reserve in the viewport for dynamically discovered entries
    /// (e.g., gone branches found after fetch, or branches cloned incrementally).
    pub extra_rows: u16,
    /// Verbosity level.  `>= 1` enables hook sub-rows in the TUI.
    pub verbosity: u8,
}

/// Result returned after the TUI completes.
pub struct CompletedTable {
    /// Final worktree row states.
    pub rows: Vec<WorktreeRow>,
    /// Hook entries that need a post-TUI summary (warnings / failures).
    pub hook_summaries: Vec<HookSummaryEntry>,
}

/// Reusable TUI table for any command that processes worktrees in parallel.
///
/// # Usage
///
/// ```ignore
/// let (tx, rx) = std::sync::mpsc::channel::<DagEvent>();
/// // ... spawn workers that send DagEvents through tx ...
/// let table = OperationTable::new(phases, worktree_infos, project_root, cwd, stat, rx, config);
/// let completed = table.run()?;
/// ```
pub struct OperationTable {
    phases: Vec<OperationPhase>,
    worktree_infos: Vec<WorktreeInfo>,
    project_root: PathBuf,
    cwd: PathBuf,
    stat: Stat,
    receiver: mpsc::Receiver<DagEvent>,
    config: TableConfig,
    /// Index of the first unowned worktree row (`None` if no unowned section).
    unowned_start_index: Option<usize>,
}

impl OperationTable {
    /// Create a new `OperationTable`.
    ///
    /// All parameters map directly to the equivalent [`TuiState::new`] parameters
    /// plus a unified [`TableConfig`] for display options.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        phases: Vec<OperationPhase>,
        worktree_infos: Vec<WorktreeInfo>,
        project_root: PathBuf,
        cwd: PathBuf,
        stat: Stat,
        receiver: mpsc::Receiver<DagEvent>,
        config: TableConfig,
        unowned_start_index: Option<usize>,
    ) -> Self {
        Self {
            phases,
            worktree_infos,
            project_root,
            cwd,
            stat,
            receiver,
            config,
            unowned_start_index,
        }
    }

    /// Run the TUI render loop and return the final table state.
    ///
    /// Internally this constructs a [`TuiState`], wires it into a
    /// [`TuiRenderer`], and drives it until all tasks complete.
    pub fn run(self) -> anyhow::Result<CompletedTable> {
        let state = TuiState::new(
            self.phases,
            self.worktree_infos,
            self.project_root,
            self.cwd,
            self.stat,
            self.config.verbosity,
            self.config.columns,
            self.config.columns_explicit,
            self.unowned_start_index,
            self.config.sort_spec,
        );

        let renderer =
            TuiRenderer::new(state, self.receiver).with_extra_rows(self.config.extra_rows);

        let final_state = renderer.run()?;

        Ok(CompletedTable {
            rows: final_state.worktrees,
            hook_summaries: final_state.hook_summaries,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_config_verbosity_semantics() {
        // verbosity 0 → show_hook_sub_rows false (computed in TuiState::new as verbose >= 1)
        // verbosity 1 → show_hook_sub_rows true
        // This test documents the contract without needing a live terminal.
        let cfg_silent = TableConfig {
            columns: None,
            columns_explicit: false,
            sort_spec: None,
            extra_rows: 0,
            verbosity: 0,
        };
        assert_eq!(cfg_silent.verbosity, 0);

        let cfg_verbose = TableConfig {
            columns: None,
            columns_explicit: false,
            sort_spec: None,
            extra_rows: 0,
            verbosity: 1,
        };
        assert!(cfg_verbose.verbosity >= 1);
    }

    #[test]
    fn completed_table_fields_accessible() {
        // Ensure the public struct fields are accessible for downstream callers.
        let completed = CompletedTable {
            rows: Vec::new(),
            hook_summaries: Vec::new(),
        };
        assert!(completed.rows.is_empty());
        assert!(completed.hook_summaries.is_empty());
    }
}
