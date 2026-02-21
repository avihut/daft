//! Core business logic abstractions.
//!
//! This module defines the traits and types that allow core operations to
//! report progress and trigger hooks without depending on specific UI
//! implementations (CLI, TUI, tests, etc.).

mod progress;
pub mod worktree;

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
