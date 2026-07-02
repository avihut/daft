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
pub mod stage;
mod tui_bridge;
pub mod worktree;

pub use tui_bridge::TuiBridge;

pub use progress::{CommandBridge, OutputSink, TimelineBridge, TimelineSink};
pub use stage::{PlanCommit, Row, StageEvent, StageId, StepKey, StepSpec};

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

    // ── Plan-then-execute timeline (#651) ────────────────────────────────
    // Default no-ops so every existing sink (OutputSink, CommandBridge,
    // TuiBridge, NullSink, test sinks) compiles and behaves unchanged.
    // Only timeline-aware sinks (TimelineBridge) override these.

    /// Commit the execution plan. Emitted exactly once, after all
    /// resolution/validation and before the first mutation. Cores that
    /// return early (nothing to do, validation failure) never call this.
    fn on_plan(&mut self, plan: stage::PlanCommit) {
        let _ = plan;
    }

    /// Report a lifecycle event for one committed plan step.
    fn on_stage(&mut self, key: &stage::StepKey, event: stage::StageEvent) {
        let _ = (key, event);
    }

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

// ─────────────────────────────────────────────────────────────────────────
// Test support
// ─────────────────────────────────────────────────────────────────────────

/// Recording sink for core-level timeline contract tests: captures the
/// committed plan, every stage event, free-text steps, and each hook type
/// that fired — so tests can assert "plan committed after validation",
/// "Push failed but post-create hooks still ran", etc., without a terminal.
#[cfg(test)]
#[derive(Default)]
pub struct RecordingStageSink {
    pub plan: Option<stage::PlanCommit>,
    pub events: Vec<(stage::StepKey, stage::StageEvent)>,
    pub steps: Vec<String>,
    pub warnings: Vec<String>,
    pub hooks_run: Vec<crate::hooks::HookType>,
}

#[cfg(test)]
impl ProgressSink for RecordingStageSink {
    fn on_step(&mut self, msg: &str) {
        self.steps.push(msg.to_string());
    }

    fn on_warning(&mut self, msg: &str) {
        self.warnings.push(msg.to_string());
    }

    fn on_debug(&mut self, _msg: &str) {}

    fn on_plan(&mut self, plan: stage::PlanCommit) {
        assert!(self.plan.is_none(), "plan committed twice");
        self.plan = Some(plan);
    }

    fn on_stage(&mut self, key: &stage::StepKey, event: stage::StageEvent) {
        self.events.push((key.clone(), event));
    }
}

#[cfg(test)]
impl HookRunner for RecordingStageSink {
    fn run_hook(&mut self, ctx: &HookContext) -> Result<HookOutcome> {
        self.hooks_run.push(ctx.hook_type);
        Ok(HookOutcome {
            success: true,
            skipped: true,
            skip_reason: Some("hooks disabled".to_string()),
        })
    }
}

#[cfg(test)]
impl ConsolidationPrompter for RecordingStageSink {}
