//! Relocating a whole repository.
//!
//! A repo's identity is the uuid at `<git-common-dir>/daft-id`, so everything
//! keyed by it — job logs, coordinator state, the per-repo store — follows the
//! repo for free when it moves. Everything keyed by *path* does not: git's
//! worktree linkage, the trust grant, the layout override, recorded worktree
//! identities, cached sizes. A plain `mv` breaks all of them, and reports
//! nothing, because the catalog heals its own path lazily and leaves
//! `daft repo list` looking correct.
//!
//! This module moves the path-keyed state in lockstep with the directory:
//! [`plan`] decides and refuses, [`execute`] applies, [`print`] renders the
//! plan for `--dry-run`.

pub mod execute;
pub mod plan;
pub mod print;

pub use execute::{MoveOutcome, apply};
pub use plan::{Disposition, MovePlan, MoveRequest, NamePlan, PlannedWorktree, build};
pub use print::print_plan;
