//! Layout state representation for transform planning.
//!
//! `LayoutState` captures where everything is (source) or should be (target):
//! git_dir location, bare flag, and all worktree positions.

use std::path::PathBuf;

/// Snapshot of a repository's layout state.
#[derive(Debug, Clone)]
pub struct LayoutState {
    /// Absolute path to the `.git` directory.
    pub git_dir: PathBuf,
    /// Whether `core.bare` is true.
    pub is_bare: bool,
    /// The default branch name (the branch co-located with `.git` for non-bare,
    /// or the first/primary worktree for bare).
    pub default_branch: String,
    /// The project root / wrapper directory. For bare and wrapped non-bare
    /// layouts this is the parent of worktrees. For regular non-bare layouts
    /// this is the repo root itself.
    pub project_root: PathBuf,
    /// All worktree entries (including the default branch for bare layouts).
    pub worktrees: Vec<WorktreeEntry>,
}

/// A single worktree's position in a layout state.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    /// Branch name (e.g., "main", "feature/auth").
    pub branch: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Whether this is the default branch.
    pub is_default: bool,
}

/// Classification of a worktree during transform planning.
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeDisposition {
    /// Worktree conforms to the target template — will be relocated if needed.
    Conforming,
    /// Worktree does not match the target template — skipped by default.
    NonConforming,
    /// Worktree is the default branch and needs special handling (collapse/nest).
    DefaultBranch,
}

/// A classified worktree entry in the transform plan.
#[derive(Debug, Clone)]
pub struct ClassifiedWorktree {
    pub branch: String,
    pub current_path: PathBuf,
    pub target_path: PathBuf,
    pub disposition: WorktreeDisposition,
}
