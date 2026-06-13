//! Core worktree operations.
//!
//! Each submodule contains the business logic for a daft command, separated
//! from argument parsing and output rendering. Functions accept structured
//! params, a `GitCommand`, and a `ProgressSink`, and return structured results.
//!
//! **Note:** `flow_adopt` and `flow_eject` are deprecated compatibility
//! wrappers. The canonical bare/non-bare conversion logic lives in
//! [`crate::core::layout::transform`]. New code should call that module
//! directly.

pub mod branch_delete;
pub mod branch_source;
pub mod carry;
pub(crate) mod cell_cache;
pub mod checkout;
pub mod checkout_branch;
pub mod clone;
pub mod exec;
pub mod fetch;
pub mod flow_adopt;
pub mod flow_eject;
pub mod info_field;
pub mod init;
pub mod list;
pub mod list_stream;
pub mod merge;
pub mod merge_set_default;
pub mod porcelain;
pub mod ports;
pub mod previous;
pub mod prune;
pub mod push;
pub mod rebase;
pub mod remove_repo;
pub mod rename;
pub mod sync_dag;
pub mod temp_worktree;

/// Configuration for worktree operations.
#[derive(Debug, Clone)]
pub struct WorktreeConfig {
    pub remote_name: String,
    pub quiet: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            remote_name: "origin".to_string(),
            quiet: false,
        }
    }
}
