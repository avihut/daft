//! Core worktree operations.
//!
//! Each submodule contains the business logic for a daft command, separated
//! from argument parsing and output rendering. Functions accept structured
//! params, a `GitCommand`, and a `ProgressSink`, and return structured results.

pub mod carry;
pub mod checkout;
pub mod checkout_branch;
pub mod clone;
pub mod fetch;
pub mod flow_adopt;
pub mod flow_eject;
pub mod init;
