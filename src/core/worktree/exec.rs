//! Core logic for `daft worktree-exec`.
//!
//! Target resolution, per-worktree command pipeline, scheduler, and the
//! `ExecReport` data type that the command layer renders. No IO to stdout
//! lives here; renderers are separate.

use std::path::PathBuf;
use std::time::Duration;

/// A worktree that has been matched by the user's selectors and is ready
/// to receive commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub worktree_path: PathBuf,
    pub branch_name: String,
}

/// One command in a per-worktree pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    /// Direct argv exec, e.g. from `-- CMD ARGS…`. First element is the
    /// program; remaining are its arguments. Never touches a shell.
    Argv(Vec<String>),
    /// Shell-parsed string, e.g. from `-x 'CMD'`. Run via `$SHELL -c`,
    /// falling back to `sh -c` if `$SHELL` is unset.
    Shell(String),
}

impl CommandSpec {
    /// A short one-line representation of the command for UI display.
    pub fn display(&self) -> String {
        match self {
            CommandSpec::Argv(parts) => parts.join(" "),
            CommandSpec::Shell(s) => s.clone(),
        }
    }
}

/// How the scheduler fans work out across worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// All worktrees concurrently. Default.
    Parallel,
    /// One worktree at a time; stop on the first failing worktree.
    Sequential,
    /// One worktree at a time; continue through failures.
    KeepGoing,
}

/// Outcome of running the full pipeline on one worktree.
#[derive(Debug)]
pub struct WorktreeOutcome {
    pub target: ResolvedTarget,
    /// Index into the pipeline of the last command attempted (0-based). If
    /// the first command failed, this is 0. If all succeeded, this equals
    /// pipeline.len() - 1.
    pub last_command_index: usize,
    /// Exit code of the last command attempted. 0 when all succeeded.
    pub exit_code: i32,
    /// Wall-clock duration from spawn of first command to finish of last.
    pub elapsed: Duration,
    /// Captured stdout+stderr, truncated at `OUTPUT_CAP_BYTES` keeping the tail.
    pub captured_output: Vec<u8>,
    /// Whether the worktree was cancelled by SIGINT before finishing.
    pub cancelled: bool,
}

impl WorktreeOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && !self.cancelled
    }
}

/// The aggregate result of an exec invocation used by renderers and the
/// outer command layer.
#[derive(Debug, Default)]
pub struct ExecReport {
    pub outcomes: Vec<WorktreeOutcome>,
    pub orphan_branches_skipped: Vec<String>,
}

impl ExecReport {
    /// 0 if every worktree succeeded, 1 otherwise. Single-target
    /// pass-through never builds an `ExecReport`; it propagates the child
    /// exit code directly.
    pub fn aggregate_exit_code(&self) -> i32 {
        if self.outcomes.iter().all(|o| o.succeeded()) {
            0
        } else {
            1
        }
    }
}

/// Output-capture cap per worktree (ring buffer keeps the tail). Internal
/// constant — not user-configurable in v1.
pub const OUTPUT_CAP_BYTES: usize = 1024 * 1024; // 1 MiB
