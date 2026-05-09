//! Layout transformation engine.
//!
//! The transform engine computes a plan of discrete operations by diffing the
//! current repository layout state against a target layout. Operations are
//! sequenced via path-conflict analysis and executed with rollback support.

pub mod execute;
pub mod legacy;
pub mod plan;
pub mod print;
pub mod state;

// Re-export legacy items that are still used by adopt/eject and other callers
pub use legacy::{
    CollapseBareParams, CollapseBareResult, ConvertToBareParams, ConvertToBareResult,
    ConvertToNonBareParams, ConvertToNonBareResult, WorktreeInfo, collapse_bare_to_non_bare,
    convert_to_bare, convert_to_non_bare, is_bare_worktree_layout, parse_worktrees,
};

pub use execute::{ExecuteResult, ExecutionContext, describe_op, execute_plan};

pub use print::print_plan;

pub use plan::{TransformOp, TransformPlan, build_plan, classify_worktrees};

pub use state::{
    ClassifiedWorktree, LayoutState, WorktreeDisposition, WorktreeEntry, compute_target_git_dir,
    compute_target_state, compute_target_worktree_path, parse_porcelain_to_entries,
    read_source_state,
};
