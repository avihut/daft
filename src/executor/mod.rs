//! Generic job execution engine.
//!
//! Provides format-agnostic types for defining, scheduling, and presenting
//! job execution. This module is the foundation for both the hooks system
//! and the sync/prune DAG executor.

pub mod cli_presenter;
pub mod command;
pub mod dag;
pub mod presenter;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────
// Core types
// ─────────────────────────────────────────────────────────────────────────

/// Format-agnostic definition of a job to execute.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// Unique name identifying this job within a phase.
    pub name: String,
    /// Shell command to execute.
    pub command: String,
    /// Working directory for the command.
    pub working_dir: PathBuf,
    /// Extra environment variables to set.
    pub env: HashMap<String, String>,
    /// Optional human-readable description shown in progress output.
    pub description: Option<String>,
    /// Names of jobs that must complete successfully before this one starts.
    pub needs: Vec<String>,
    /// Whether the job needs direct terminal access (stdin/stdout passthrough).
    pub interactive: bool,
    /// Text to display on failure (e.g., a hint for the user).
    pub fail_text: Option<String>,
    /// Maximum time the job is allowed to run.
    pub timeout: Duration,
}

impl JobSpec {
    /// Default timeout for non-interactive jobs (5 minutes).
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
}

impl Default for JobSpec {
    fn default() -> Self {
        Self {
            name: String::new(),
            command: String::new(),
            working_dir: PathBuf::new(),
            env: HashMap::new(),
            description: None,
            needs: Vec::new(),
            interactive: false,
            fail_text: None,
            timeout: JobSpec::DEFAULT_TIMEOUT,
        }
    }
}

/// How to schedule a set of jobs.
///
/// DAG mode is implicit: when any job has non-empty `needs`, the runner
/// builds a dependency graph automatically. The hooks-specific `Follow`
/// mode (attach to an existing session) is handled in the hooks layer,
/// not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Run jobs one at a time, continue on failure.
    Sequential,
    /// Run jobs one at a time, stop on first failure.
    Piped,
    /// Run all jobs concurrently (bounded by available CPUs).
    Parallel,
}

/// Status of a job node during execution.
///
/// Mirrors `core::worktree::sync_dag::TaskStatus`. Once the sync DAG is
/// migrated to use the generic executor, `TaskStatus` should be replaced
/// by this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    /// Not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Completed successfully.
    Succeeded,
    /// Completed with a non-zero exit code.
    Failed,
    /// Skipped (e.g., due to a condition).
    Skipped,
    /// Skipped because a dependency failed.
    DepFailed,
}

impl NodeStatus {
    /// Whether this status is a terminal state (no further transitions).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Skipped | Self::DepFailed
        )
    }
}

/// Result of a single job execution.
#[derive(Debug, Clone)]
pub struct JobResult {
    /// Name of the job that was executed.
    pub name: String,
    /// Final status of the job.
    pub status: NodeStatus,
    /// How long the job ran.
    pub duration: Duration,
    /// Process exit code, if available.
    pub exit_code: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error.
    pub stderr: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── JobSpec ──────────────────────────────────────────────────────────

    #[test]
    fn job_spec_default_has_empty_name() {
        let spec = JobSpec::default();
        assert!(spec.name.is_empty());
        assert!(spec.command.is_empty());
        assert!(spec.env.is_empty());
        assert!(spec.needs.is_empty());
        assert!(!spec.interactive);
        assert!(spec.fail_text.is_none());
        assert!(spec.description.is_none());
        assert_eq!(spec.timeout, JobSpec::DEFAULT_TIMEOUT);
    }

    #[test]
    fn job_spec_default_timeout_is_five_minutes() {
        assert_eq!(JobSpec::DEFAULT_TIMEOUT, Duration::from_secs(300));
    }

    #[test]
    fn job_spec_fields_round_trip() {
        let mut env = HashMap::new();
        env.insert("KEY".into(), "VALUE".into());

        let spec = JobSpec {
            name: "install".into(),
            command: "pnpm install".into(),
            working_dir: PathBuf::from("/project"),
            env: env.clone(),
            description: Some("Install deps".into()),
            needs: vec!["fetch".into()],
            interactive: true,
            fail_text: Some("install failed".into()),
            timeout: Duration::from_secs(60),
        };

        assert_eq!(spec.name, "install");
        assert_eq!(spec.command, "pnpm install");
        assert_eq!(spec.working_dir, PathBuf::from("/project"));
        assert_eq!(spec.env, env);
        assert_eq!(spec.description.as_deref(), Some("Install deps"));
        assert_eq!(spec.needs, vec!["fetch"]);
        assert!(spec.interactive);
        assert_eq!(spec.fail_text.as_deref(), Some("install failed"));
        assert_eq!(spec.timeout, Duration::from_secs(60));
    }

    // ── ExecutionMode ───────────────────────────────────────────────────

    #[test]
    fn execution_mode_variants_are_distinct() {
        assert_ne!(ExecutionMode::Sequential, ExecutionMode::Parallel);
        assert_ne!(ExecutionMode::Sequential, ExecutionMode::Piped);
        assert_ne!(ExecutionMode::Parallel, ExecutionMode::Piped);
    }

    #[test]
    fn execution_mode_is_copy() {
        let mode = ExecutionMode::Parallel;
        let copy = mode;
        assert_eq!(mode, copy);
    }

    // ── NodeStatus ──────────────────────────────────────────────────────

    #[test]
    fn node_status_terminal_states() {
        assert!(!NodeStatus::Pending.is_terminal());
        assert!(!NodeStatus::Running.is_terminal());
        assert!(NodeStatus::Succeeded.is_terminal());
        assert!(NodeStatus::Failed.is_terminal());
        assert!(NodeStatus::Skipped.is_terminal());
        assert!(NodeStatus::DepFailed.is_terminal());
    }

    #[test]
    fn node_status_is_copy() {
        let status = NodeStatus::Running;
        let copy = status;
        assert_eq!(status, copy);
    }

    // ── JobResult ───────────────────────────────────────────────────────

    #[test]
    fn job_result_success() {
        let result = JobResult {
            name: "build".into(),
            status: NodeStatus::Succeeded,
            duration: Duration::from_secs(2),
            exit_code: Some(0),
            stdout: "compiled ok\n".into(),
            stderr: String::new(),
        };

        assert_eq!(result.name, "build");
        assert_eq!(result.status, NodeStatus::Succeeded);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.status.is_terminal());
    }

    #[test]
    fn job_result_failure() {
        let result = JobResult {
            name: "test".into(),
            status: NodeStatus::Failed,
            duration: Duration::from_millis(500),
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "assertion failed\n".into(),
        };

        assert_eq!(result.status, NodeStatus::Failed);
        assert_eq!(result.exit_code, Some(1));
        assert_eq!(result.stderr, "assertion failed\n");
    }

    #[test]
    fn job_result_no_exit_code() {
        let result = JobResult {
            name: "killed".into(),
            status: NodeStatus::Failed,
            duration: Duration::from_secs(30),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        };

        assert!(result.exit_code.is_none());
    }

    #[test]
    fn job_result_clone() {
        let result = JobResult {
            name: "a".into(),
            status: NodeStatus::Succeeded,
            duration: Duration::from_secs(1),
            exit_code: Some(0),
            stdout: "ok".into(),
            stderr: String::new(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.name, result.name);
        assert_eq!(cloned.status, result.status);
    }
}
