//! Layout transformation engine.
//!
//! The transform engine computes a plan of discrete operations by diffing the
//! current repository layout state against a target layout. Operations are
//! sequenced via path-conflict analysis and executed with rollback support.

pub mod decide;
pub mod execute;
pub mod plan;
pub mod preflight;
pub mod print;
pub mod report;
pub mod state;
pub mod status_snapshot;

pub use execute::{ExecuteResult, ExecutionContext, describe_op, execute_plan};

pub use print::print_plan;

pub use plan::{TransformOp, TransformPlan, build_plan, classify_worktrees, paths_equivalent};

pub use decide::{ConfirmDecision, DirnameDecision, PivotDecision};
pub use plan::CarriedState;
pub use preflight::{Blocker, BlockerKind, OpProgress, PivotCandidate, ProbeReason};
pub use state::{
    ClassifiedWorktree, LayoutState, PivotOverride, RootSituation, WorktreeDisposition,
    WorktreeEntry, compute_target_git_dir, compute_target_state, compute_target_worktree_path,
    parse_porcelain_to_entries, read_source_state, root_situation,
};
pub use status_snapshot::{Artifacts, StatusSnapshot};
