//! Output abstraction layer for separating IO from business logic.
//!
//! This module provides the `Output` trait that abstracts all output operations,
//! enabling future TUI interfaces while maintaining current CLI behavior.
//!
//! # Usage
//!
//! Commands should accept `&mut dyn Output` and use its methods instead of
//! direct `println!` or `eprintln!` calls:
//!
//! ```ignore
//! pub fn run_with_output(args: Args, output: &mut dyn Output) -> Result<()> {
//!     output.info("Starting operation...");
//!     output.progress("Processing files");
//!     output.success("Operation completed!");
//!     Ok(())
//! }
//! ```

mod buffering;
mod cli;
pub mod emit;
pub mod format;
pub mod hook_progress;
pub mod outline;
pub mod pager;
pub(crate) mod palette;
pub(crate) mod term_guard;
mod test;
pub mod timeline;
pub mod tui;

pub use buffering::BufferingOutput;
pub use cli::CliOutput;
pub use test::{OutputEntry, TestOutput};

use std::path::Path;

/// Configuration for output behavior.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// Suppress most output when true.
    pub quiet: bool,
    /// Enable debug/verbose output when true.
    pub verbose: bool,
    /// Enable auto-cd into new worktrees when true.
    pub autocd: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            quiet: false,
            verbose: false,
            autocd: true,
        }
    }
}

impl OutputConfig {
    /// Create a new output configuration with autocd enabled by default.
    pub fn new(quiet: bool, verbose: bool) -> Self {
        Self {
            quiet,
            verbose,
            autocd: true,
        }
    }

    /// Create a new output configuration with explicit autocd setting.
    pub fn with_autocd(quiet: bool, verbose: bool, autocd: bool) -> Self {
        Self {
            quiet,
            verbose,
            autocd,
        }
    }
}

/// Trait for abstracting output operations.
///
/// This trait separates output concerns from business logic, enabling:
/// - CLI output (current behavior)
/// - Future TUI interfaces with spinners/progress bars
/// - Test implementations for verifying output
///
/// Implementors should respect `quiet` and `verbose` modes where appropriate.
pub trait Output {
    // ─────────────────────────────────────────────────────────────────────────
    // Basic Messages
    // ─────────────────────────────────────────────────────────────────────────

    /// Display an informational message.
    /// Respects quiet mode.
    fn info(&mut self, msg: &str);

    /// Display a success message.
    /// Respects quiet mode.
    fn success(&mut self, msg: &str);

    /// Display a warning message to stderr.
    /// Always shown (not affected by quiet mode).
    fn warning(&mut self, msg: &str);

    /// Display a neutral notice to stderr — no severity prefix, always shown.
    ///
    /// Unlike [`warning`](Output::warning)/[`error`](Output::error), this adds
    /// no `warning:`/`error:` tag: it is for by-design, informational facts
    /// that are *not* problems (e.g. "this repo isn't trusted, so its hooks
    /// were skipped"). It stays on stderr (not stdout, so it never pollutes a
    /// command's machine-readable output) and ignores quiet mode (like a
    /// warning, the user should still see it). Implementations add no styling
    /// of their own; callers may embed it (gated on a live stderr).
    fn notice(&mut self, msg: &str);

    /// Display an error message to stderr.
    /// Always shown (not affected by quiet mode).
    fn error(&mut self, msg: &str);

    /// Display a debug message.
    /// Only shown in verbose mode.
    fn debug(&mut self, msg: &str);

    // ─────────────────────────────────────────────────────────────────────────
    // Structured Output
    // ─────────────────────────────────────────────────────────────────────────

    /// Display an intermediate step message.
    /// Only shown in verbose mode (not in default output).
    /// Use this for step-by-step progress during operations.
    fn step(&mut self, msg: &str);

    /// Display a final result message.
    /// The primary success output shown in default mode.
    /// Use this for the 1-2 line summary at the end of a command.
    fn result(&mut self, msg: &str);

    /// Display a progress/action message.
    /// Renders as "--> msg" in CLI.
    /// Respects quiet mode.
    #[deprecated(since = "0.4.0", note = "Use step() for verbose output instead")]
    fn progress(&mut self, msg: &str);

    /// Display a visual divider.
    /// Renders as "---" in CLI.
    /// Respects quiet mode.
    #[deprecated(
        since = "0.4.0",
        note = "Dividers are no longer used in git-like output"
    )]
    fn divider(&mut self);

    /// Display a key-value detail.
    /// Renders as "  Key: value" in CLI.
    /// Respects quiet mode.
    fn detail(&mut self, key: &str, value: &str);

    /// Display a list item.
    /// Renders as " - item" in CLI.
    /// Respects quiet mode.
    fn list_item(&mut self, item: &str);

    // ─────────────────────────────────────────────────────────────────────────
    // Operation Lifecycle (for future TUI spinners)
    // ─────────────────────────────────────────────────────────────────────────

    /// Signal the start of a long-running operation.
    /// In CLI, this might just print a message.
    /// In TUI, this could start a spinner.
    fn operation_start(&mut self, operation: &str);

    /// Signal the end of a long-running operation.
    /// In CLI, this might print success/failure.
    /// In TUI, this could stop a spinner and show result.
    fn operation_end(&mut self, operation: &str, success: bool);

    // ─────────────────────────────────────────────────────────────────────────
    // Spinner
    // ─────────────────────────────────────────────────────────────────────────

    /// Start a spinner with the given message.
    /// While active, `step()` updates the spinner text instead of printing.
    /// No-op in quiet mode, non-TTY, or when `DAFT_TESTING` is set.
    fn start_spinner(&mut self, msg: &str);

    /// Stop and clear the active spinner.
    /// Called explicitly before printing results, and also on `Drop` as safety net.
    fn finish_spinner(&mut self);

    /// Temporarily hide the active spinner so another component (e.g. a hook's
    /// own `indicatif::MultiProgress`) can render without fighting for the
    /// same stderr cursor. The current spinner message is remembered so
    /// `resume_spinner` can restore it. No-op when no spinner is active.
    fn pause_spinner(&mut self) {
        let _ = self;
    }

    /// Restore the spinner previously hidden by `pause_spinner`. No-op if no
    /// spinner was paused.
    fn resume_spinner(&mut self) {
        let _ = self;
    }

    /// Whether stderr messages (`warning()`/`notice()`) reach the user
    /// immediately (a real stderr).
    ///
    /// `BufferingOutput` (TUI mode, where ratatui owns the terminal) returns
    /// `false`: its output lands in a buffer, not in front of the user.
    /// Callers that must guarantee a message is *seen* — like the untrusted-
    /// hook notice — use this to decide between emitting now and deferring
    /// to a post-TUI flush.
    fn live_warnings(&self) -> bool {
        true
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Special Output
    // ─────────────────────────────────────────────────────────────────────────

    /// Write the cd target path for shell wrappers.
    /// Writes to the file specified by DAFT_CD_FILE env var, if set.
    fn cd_path(&mut self, path: &Path);

    /// Output raw, unformatted content.
    /// Useful for machine-readable output or passing through external command output.
    fn raw(&mut self, content: &str);

    // ─────────────────────────────────────────────────────────────────────────
    // State Queries
    // ─────────────────────────────────────────────────────────────────────────

    /// Check if quiet mode is enabled.
    fn is_quiet(&self) -> bool;

    /// Check if verbose mode is enabled.
    fn is_verbose(&self) -> bool;

    // ─────────────────────────────────────────────────────────────────────────
    // Domain-specific notice lines
    // ─────────────────────────────────────────────────────────────────────────

    /// Render the "Updated repository defaults" notice that follows a
    /// successful `daft merge --set-default` invocation. Shown in cyan to
    /// distinguish it from the primary result line.
    ///
    /// The default implementation delegates to `info`, which is suppressed in
    /// quiet mode. Concrete implementations may apply cyan styling.
    fn defaults_updated(
        &mut self,
        style: crate::core::worktree::merge::MergeStyle,
        cleanup: crate::core::worktree::merge::CleanupKind,
    ) {
        let line = format!(
            "Updated repository defaults: merge.style={}, merge.cleanup={}",
            style, cleanup
        );
        self.info(&line);
    }

    /// Render the "I heard you" header line at the very start of `daft merge`.
    ///
    /// Two complaints from field testing motivated this: users couldn't tell
    /// whether daft had understood their flags at all (the only signal was a
    /// transient spinner that often vanished before they read it), and the
    /// hook box for cleanup left them unsure which worktree was being touched.
    /// This line names the operation, the style, the cleanup outcome, and
    /// whether `--set-default` is going to persist the choices — so the rest
    /// of the output is read in context.
    ///
    /// Default impl delegates to `info`. Concrete implementations may apply
    /// dim styling so the line reads as a header rather than a result.
    fn merge_intent(
        &mut self,
        sources: &[String],
        target: &str,
        style: crate::core::worktree::merge::MergeStyle,
        cleanup: crate::core::worktree::merge::CleanupKind,
        set_default: bool,
    ) {
        let sources_display = sources.join(", ");
        let mut bits = vec![style.to_string(), cleanup.to_string()];
        if set_default {
            bits.push("saving as default".to_string());
        }
        let line = format!(
            "Merging {sources_display} \u{2192} {target} ({})",
            bits.join(" \u{00b7} ")
        );
        self.info(&line);
    }

    /// Render a per-source section heading right before `worktree-pre-remove`
    /// hooks fire during cleanup. The hook-box title also names the target
    /// now, but the heading lands in the user's main output stream (stdout)
    /// — it survives even when stderr is redirected, and it labels the scope
    /// before the box appears so the user reads the box knowing what to
    /// expect. Suppressed in quiet mode via the default `info` delegation.
    fn cleanup_target(&mut self, name: &str) {
        self.info(&format!("Cleaning up {name} (worktree, local branch)"));
    }
}
