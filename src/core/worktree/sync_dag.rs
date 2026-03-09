//! Dependency graph for parallelized sync/prune operations.
//!
//! Defines task types, status tracking, and operation phases used by
//! the sync and prune TUI renderers.

use std::path::PathBuf;

/// Identifies a single executable task in the sync DAG.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TaskId {
    /// Fetch from remote with --prune.
    Fetch,
    /// Prune a single gone branch (and its worktree if present).
    Prune(String),
    /// Update (pull) a single worktree.
    Update(String),
    /// Rebase a worktree onto a base branch.
    Rebase(String),
}

/// Execution status of a single task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    /// Skipped because a dependency failed.
    DepFailed,
}

impl TaskStatus {
    /// Whether this status is a terminal state (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::DepFailed
        )
    }
}

/// A high-level operation phase shown in the operation header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationPhase {
    Fetch,
    Prune,
    Update,
    Rebase(String),
}

impl OperationPhase {
    /// Human-readable label for the operation header.
    pub fn label(&self) -> String {
        match self {
            Self::Fetch => "Fetching remote branches".into(),
            Self::Prune => "Pruning stale branches".into(),
            Self::Update => "Updating worktrees".into(),
            Self::Rebase(branch) => format!("Rebasing onto {branch}"),
        }
    }
}

/// A node in the sync dependency graph.
#[derive(Debug)]
pub struct SyncTask {
    /// Unique identifier for this task.
    pub id: TaskId,
    /// Which operation phase this task belongs to.
    pub phase: OperationPhase,
    /// Worktree path (None for Fetch tasks).
    pub worktree_path: Option<PathBuf>,
    /// Branch name associated with this task.
    pub branch_name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_display() {
        let id = TaskId::Fetch;
        assert_eq!(format!("{id:?}"), "Fetch");

        let id = TaskId::Prune("feat/old".into());
        assert_eq!(format!("{id:?}"), "Prune(\"feat/old\")");
    }

    #[test]
    fn task_status_is_terminal() {
        assert!(!TaskStatus::Pending.is_terminal());
        assert!(!TaskStatus::Running.is_terminal());
        assert!(TaskStatus::Succeeded.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
        assert!(TaskStatus::Skipped.is_terminal());
        assert!(TaskStatus::DepFailed.is_terminal());
    }

    #[test]
    fn operation_phase_label() {
        assert_eq!(OperationPhase::Fetch.label(), "Fetching remote branches");
        assert_eq!(
            OperationPhase::Rebase("master".into()).label(),
            "Rebasing onto master"
        );
    }
}
