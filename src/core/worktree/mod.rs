//! Core worktree operations.
//!
//! Each submodule contains the business logic for a daft command, separated
//! from argument parsing and output rendering. Functions accept structured
//! params, a `GitCommand`, and a `ProgressSink`, and return structured results.

pub mod branch_delete;
pub mod carry;
pub mod checkout;
pub mod checkout_branch;
pub mod clone;
pub mod fetch;
pub mod flow_adopt;
pub mod flow_eject;
pub mod init;
pub mod list;
pub mod previous;
pub mod prune;
pub mod rebase;
pub mod rename;

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
