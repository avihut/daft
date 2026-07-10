//! Core business logic abstractions.
//!
//! This module defines the traits and types that allow core operations to
//! report progress and trigger hooks without depending on specific UI
//! implementations (CLI, TUI, tests, etc.).

pub mod cache;
pub mod columns;
pub mod config;
pub mod global_config;
pub mod install;
pub mod layout;
pub mod multi_remote;
pub mod ownership;
mod progress;
pub mod remote;
pub mod repo;
pub mod repo_identity;
pub mod settings;
pub mod shared;
pub mod sort;
mod tui_bridge;
pub mod worktree;

pub use tui_bridge::TuiBridge;

pub use progress::{CommandBridge, OutputSink};

use crate::hooks::HookContext;
use anyhow::Result;

// ─────────────────────────────────────────────────────────────────────────
// Progress reporting
// ─────────────────────────────────────────────────────────────────────────

/// Trait for core operations to report progress without depending on `Output`.
///
/// Commands create an adapter (e.g., `OutputSink`) that bridges this trait
/// to the actual output implementation. Tests can use `NullSink` to suppress
/// all output.
pub trait ProgressSink {
    /// Report an intermediate step (shown in verbose mode).
    fn on_step(&mut self, msg: &str);

    /// Report a warning (always shown).
    fn on_warning(&mut self, msg: &str);

    /// Report a debug message (shown in verbose mode).
    fn on_debug(&mut self, msg: &str);

    /// Suspend any running command-level spinner so a nested progress UI
    /// (e.g. the pre-push hook's `MultiProgress`) can own the terminal without
    /// the two clobbering each other. No-op by default; CLI adapters forward
    /// to `Output`. Must be paired with [`resume_spinner`](Self::resume_spinner).
    fn pause_spinner(&mut self) {}

    /// Restore a spinner previously hidden by [`pause_spinner`](Self::pause_spinner).
    /// No-op by default.
    fn resume_spinner(&mut self) {}
}

/// A no-op sink that discards all progress messages.
///
/// Useful for tests and contexts where no output is desired.
pub struct NullSink;

impl ProgressSink for NullSink {
    fn on_step(&mut self, _msg: &str) {}
    fn on_warning(&mut self, _msg: &str) {}
    fn on_debug(&mut self, _msg: &str) {}
}

// ─────────────────────────────────────────────────────────────────────────
// Hook execution
// ─────────────────────────────────────────────────────────────────────────

/// Outcome of a hook execution.
///
/// This is a simplified view of `HookResult` suitable for core operations
/// that need to know whether to proceed or abort, without caring about
/// the specific hook output details.
pub struct HookOutcome {
    /// Whether the hook completed successfully.
    pub success: bool,
    /// Whether the hook was skipped (not run).
    pub skipped: bool,
    /// Reason for skipping, if applicable.
    pub skip_reason: Option<String>,
}

/// Trait for core operations to trigger lifecycle hooks.
///
/// Commands provide a concrete implementation that wraps `HookExecutor`
/// and the appropriate renderer. Tests can use `NoopHookRunner` to skip
/// all hook execution.
pub trait HookRunner {
    /// Execute the hook described by `ctx`.
    fn run_hook(&mut self, ctx: &HookContext) -> Result<HookOutcome>;
}

/// A no-op hook runner that reports all hooks as successful.
///
/// Useful for tests and contexts where hooks should be skipped.
pub struct NoopHookRunner;

impl HookRunner for NoopHookRunner {
    fn run_hook(&mut self, _ctx: &HookContext) -> Result<HookOutcome> {
        Ok(HookOutcome {
            success: true,
            skipped: true,
            skip_reason: Some("hooks disabled".to_string()),
        })
    }
}

/// A combined no-op sink that implements both `ProgressSink` and `HookRunner`.
///
/// Discards all progress messages and reports all hooks as successful/skipped.
/// Useful for DAG worker threads where the TUI handles all display and hooks
/// are not needed (or will be handled separately).
pub struct NullBridge;

impl ProgressSink for NullBridge {
    fn on_step(&mut self, _msg: &str) {}
    fn on_warning(&mut self, _msg: &str) {}
    fn on_debug(&mut self, _msg: &str) {}
}

impl HookRunner for NullBridge {
    fn run_hook(&mut self, _ctx: &HookContext) -> Result<HookOutcome> {
        Ok(HookOutcome {
            success: true,
            skipped: true,
            skip_reason: Some("hooks disabled in TUI mode".to_string()),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Visitor daft-file consolidation prompts
// ─────────────────────────────────────────────────────────────────────────

/// Summary of one refined untracked daft file, shown before the
/// consolidation prompt.
#[derive(Debug, Clone)]
pub struct RefinedFileSummary {
    /// `daft.yml` or `daft.local.yml`.
    pub filename: String,
    /// Key paths consolidation would adopt into the target.
    pub adopt_keys: Vec<String>,
    /// Key paths both sides changed — consolidating requires picking a side.
    pub conflict_keys: Vec<String>,
    /// True when there is no usable seed base (pre-provenance worktree,
    /// unparseable YAML): consolidation overlays the whole source file onto
    /// the target instead of merging per key.
    pub whole_file: bool,
}

/// Everything the prompter needs to render the consolidation question for
/// one worktree about to be removed.
pub struct ConsolidationRequest {
    pub branch: String,
    pub worktree_display: String,
    pub target_display: String,
    pub files: Vec<RefinedFileSummary>,
}

/// Answer to "this worktree has refined daft files — what now?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsolidationChoice {
    /// Merge the refinements into the target, then remove the worktree.
    Consolidate,
    /// Stash the files under `.daft/discarded/` and remove the worktree.
    Discard,
    /// Refuse the removal.
    Abort,
}

/// Answer to "these keys were changed on both sides — which version wins?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictSide {
    /// Keep the target's values for the conflicted keys.
    Target,
    /// Take the removed worktree's values for the conflicted keys.
    Source,
    /// Refuse the removal.
    Abort,
}

/// Decision surface for visitor daft-file consolidation during worktree
/// removal. Non-interactive contexts use the defaults — always `Abort` — so
/// nothing is ever merged or discarded without an explicit interactive
/// answer or a `--force`.
pub trait ConsolidationPrompter {
    fn on_refined(&mut self, _req: &ConsolidationRequest) -> ConsolidationChoice {
        ConsolidationChoice::Abort
    }

    fn on_conflicts(&mut self, _filename: &str, _keys: &[String]) -> ConflictSide {
        ConflictSide::Abort
    }
}

impl ConsolidationPrompter for NullBridge {}
impl ConsolidationPrompter for NullSink {}
