//! Reusable TUI table component for commands that process worktrees in parallel.
//!
//! `OperationTable` wraps [`TuiState`] and [`TuiRenderer`] into a single
//! composable unit that sync, prune, clone, and other commands can consume
//! without each reimplementing the wiring.

use super::{columns::Column, driver::TuiRenderer, state::TuiState};
use crate::{
    core::worktree::info_field::FieldSet,
    core::worktree::list::Stat,
    core::worktree::sync_dag::OperationPhase,
    core::{
        sort::SortSpec,
        worktree::{list::WorktreeInfo, sync_dag::DagEvent},
    },
    output::tui::WorktreeRow,
    output::tui::state::{GovernorSummary, HookSummaryEntry},
};
use std::{path::PathBuf, sync::mpsc};

/// Configuration for a TUI table operation.
pub struct TableConfig {
    /// User-selected columns (`None` = use `ALL_COLUMNS` defaults).
    pub columns: Option<Vec<Column>>,
    // Unused by render after #494; pending removal in a follow-up.
    pub columns_explicit: bool,
    /// User-specified sort order (`None` = default alphabetical).
    pub sort_spec: Option<SortSpec>,
    /// Extra rows to reserve in the viewport for dynamically discovered entries
    /// (e.g., gone branches found after fetch, or branches cloned incrementally).
    pub extra_rows: u16,
    /// Verbosity level.  `>= 1` enables hook sub-rows in the TUI.
    pub verbosity: u8,
    /// Pin the default branch to the first row regardless of `--sort`.
    /// Defaults to `true` (prune/sync behavior). `daft list` will set
    /// `false` in Phase 2.
    pub pin_default_branch: bool,
    /// Split rows into "owned" and "unowned" sections by `info.owner`.
    /// Defaults to `true` for daft list (Phase 2). PRUNE/SYNC set this to
    /// `false` because they compute `unowned_start_index` externally using
    /// a richer predicate (`is_branch_included` with include_filters); the
    /// external value is injected into `live.unowned_start_index` after
    /// `TuiState::new` returns, and we must not let LiveTable overwrite it.
    pub partition_by_owner: bool,
    /// Forwarded to [`super::LiveTableConfig::seeded_fields`]. See its doc for semantics.
    pub seeded_fields: FieldSet,
    /// Forge-PR cache decorations for the PR column, when the caller selected
    /// it (post-set into `LiveTableConfig`, the same pattern `daft list` uses).
    /// Sync/prune serve the cache snapshot as-is — no mid-run refresh swap.
    pub forge_prs: Option<crate::core::worktree::forge_ref::ForgePrLookup>,
}

/// Result returned after the TUI completes.
pub struct CompletedTable {
    /// The render loop exited on a cancel (Ctrl+C key event or external
    /// cancel signal) rather than on all tasks completing.
    pub cancelled: bool,
    /// Final worktree row states.
    pub rows: Vec<WorktreeRow>,
    /// Hook entries that need a post-TUI summary (warnings / failures).
    pub hook_summaries: Vec<HookSummaryEntry>,
    /// Resource-governor throttle accounting (#678); `None` when nothing
    /// was ever held back.
    pub governor: Option<GovernorSummary>,
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
    /// External cancel signal forwarded to the renderer (see
    /// [`Self::with_cancel_signal`]).
    cancel_signal: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
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
            cancel_signal: None,
        }
    }

    /// Observe an external cancel signal (see `TuiRenderer::with_cancel_signal`):
    /// the renderer exits its loop when the flag flips, and Ctrl+C key
    /// events (raw mode) store into the same flag.
    pub fn with_cancel_signal(
        mut self,
        cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.cancel_signal = Some(cancel);
        self
    }

    /// Run the TUI render loop and return the final table state.
    ///
    /// Internally this constructs a [`TuiState`], wires it into a
    /// [`TuiRenderer`], and drives it until all tasks complete.
    pub fn run(self) -> anyhow::Result<CompletedTable> {
        let mut state = TuiState::new(
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
            self.config.pin_default_branch,
            self.config.partition_by_owner,
            self.config.seeded_fields,
        );
        // Post-set like `unowned_start_index`: TuiState::new stays untouched
        // for callers without forge data.
        state.live.cfg.forge_prs = self.config.forge_prs;

        let mut renderer =
            TuiRenderer::new(state, self.receiver).with_extra_rows(self.config.extra_rows);
        if let Some(cancel) = self.cancel_signal {
            renderer = renderer.with_cancel_signal(cancel);
        }

        let final_state = renderer.run()?;

        Ok(CompletedTable {
            cancelled: final_state.live.cancelled,
            governor: (final_state.governor.throttled_pushes > 0)
                .then(|| final_state.governor.clone()),
            rows: final_state.live.rows,
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
            pin_default_branch: true,
            partition_by_owner: true,
            seeded_fields: FieldSet::EMPTY,
            forge_prs: None,
        };
        assert_eq!(cfg_silent.verbosity, 0);

        let cfg_verbose = TableConfig {
            columns: None,
            columns_explicit: false,
            sort_spec: None,
            extra_rows: 0,
            verbosity: 1,
            pin_default_branch: true,
            partition_by_owner: true,
            seeded_fields: FieldSet::EMPTY,
            forge_prs: None,
        };
        assert!(cfg_verbose.verbosity >= 1);
    }

    #[test]
    fn completed_table_fields_accessible() {
        // Ensure the public struct fields are accessible for downstream callers.
        let completed = CompletedTable {
            cancelled: false,
            rows: Vec::new(),
            hook_summaries: Vec::new(),
            governor: None,
        };
        assert!(!completed.cancelled);
        assert!(completed.rows.is_empty());
        assert!(completed.hook_summaries.is_empty());
    }
}
