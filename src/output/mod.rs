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

mod cli;
mod test;

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
    // Special Output
    // ─────────────────────────────────────────────────────────────────────────

    /// Output the cd path marker for shell wrappers.
    /// Only outputs if DAFT_SHELL_WRAPPER env var is set.
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
}
