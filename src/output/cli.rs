//! CLI output implementation.
//!
//! This implementation preserves the exact current output behavior of the codebase,
//! ensuring backward compatibility during the migration.

use super::{Output, OutputConfig};
use crate::styles::{self, colors_enabled, colors_enabled_stderr};
use crate::{CD_PATH_MARKER, SHELL_WRAPPER_ENV};
use std::env;
use std::path::Path;

/// CLI output implementation that writes directly to stdout/stderr.
///
/// Git-like output format:
/// - `step()` → verbose only, no prefix
/// - `result()` → primary output, always shown (unless quiet)
/// - `warning()` → `eprintln!("warning: {msg}")`
/// - `error()` → `eprintln!("error: {msg}")`
/// - `progress()` → deprecated, delegates to `step()`
/// - `divider()` → deprecated, no-op
#[derive(Debug)]
pub struct CliOutput {
    config: OutputConfig,
}

impl CliOutput {
    /// Create a new CLI output with the given configuration.
    pub fn new(config: OutputConfig) -> Self {
        Self { config }
    }

    /// Create a CLI output with default (non-quiet, non-verbose) settings.
    pub fn default_output() -> Self {
        Self::new(OutputConfig::default())
    }

    /// Create a CLI output in quiet mode.
    pub fn quiet() -> Self {
        Self::new(OutputConfig::new(true, false))
    }

    /// Create a CLI output in verbose mode.
    pub fn verbose() -> Self {
        Self::new(OutputConfig::new(false, true))
    }
}

impl Output for CliOutput {
    fn info(&mut self, msg: &str) {
        if !self.config.quiet {
            println!("{msg}");
        }
    }

    fn success(&mut self, msg: &str) {
        if !self.config.quiet {
            if colors_enabled() {
                println!("{}{msg}{}", styles::GREEN, styles::RESET);
            } else {
                println!("{msg}");
            }
        }
    }

    fn warning(&mut self, msg: &str) {
        // Warnings are always shown (not affected by quiet mode)
        // Git-like format: lowercase prefix
        if colors_enabled_stderr() {
            eprintln!("{}warning:{} {msg}", styles::YELLOW, styles::RESET);
        } else {
            eprintln!("warning: {msg}");
        }
    }

    fn error(&mut self, msg: &str) {
        // Errors are always shown (not affected by quiet mode)
        // Git-like format: lowercase prefix
        if colors_enabled_stderr() {
            eprintln!("{}error:{} {msg}", styles::RED, styles::RESET);
        } else {
            eprintln!("error: {msg}");
        }
    }

    fn debug(&mut self, msg: &str) {
        if self.config.verbose {
            if colors_enabled() {
                println!("{}debug: {msg}{}", styles::DIM, styles::RESET);
            } else {
                println!("debug: {msg}");
            }
        }
    }

    fn step(&mut self, msg: &str) {
        // Steps are only shown in verbose mode
        if self.config.verbose && !self.config.quiet {
            if colors_enabled() {
                println!("{}{msg}{}", styles::DIM, styles::RESET);
            } else {
                println!("{msg}");
            }
        }
    }

    fn result(&mut self, msg: &str) {
        // Result is the primary output - always shown unless quiet
        if !self.config.quiet {
            if colors_enabled() {
                println!("{}{msg}{}", styles::BOLD, styles::RESET);
            } else {
                println!("{msg}");
            }
        }
    }

    #[allow(deprecated)]
    fn progress(&mut self, msg: &str) {
        // Legacy: now delegates to step() for verbose-only output
        self.step(msg);
    }

    #[allow(deprecated)]
    fn divider(&mut self) {
        // No-op: dividers are no longer used in git-like output
    }

    fn detail(&mut self, key: &str, value: &str) {
        if !self.config.quiet {
            if colors_enabled() {
                println!("  {}{key}:{} {value}", styles::BOLD, styles::RESET);
            } else {
                println!("  {key}: {value}");
            }
        }
    }

    fn list_item(&mut self, item: &str) {
        if !self.config.quiet {
            println!(" - {item}");
        }
    }

    fn operation_start(&mut self, operation: &str) {
        // In CLI mode, just print a step message (verbose only)
        self.step(operation);
    }

    fn operation_end(&mut self, operation: &str, success: bool) {
        if self.config.verbose && !self.config.quiet {
            if success {
                println!("{operation} completed");
            } else {
                eprintln!("{operation} failed");
            }
        }
    }

    fn cd_path(&mut self, path: &Path) {
        // Only output if autocd is enabled and the shell wrapper environment variable is set.
        // This keeps output clean for users who don't use wrappers.
        if self.config.autocd && env::var(SHELL_WRAPPER_ENV).is_ok() {
            println!("{CD_PATH_MARKER}{}", path.display());
        }
    }

    fn raw(&mut self, content: &str) {
        // Raw output is not affected by quiet mode - it's explicit content
        print!("{content}");
    }

    fn is_quiet(&self) -> bool {
        self.config.quiet
    }

    fn is_verbose(&self) -> bool {
        self.config.verbose
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_output_default() {
        let output = CliOutput::default_output();
        assert!(!output.is_quiet());
        assert!(!output.is_verbose());
    }

    #[test]
    fn test_cli_output_quiet() {
        let output = CliOutput::quiet();
        assert!(output.is_quiet());
        assert!(!output.is_verbose());
    }

    #[test]
    fn test_cli_output_verbose() {
        let output = CliOutput::verbose();
        assert!(!output.is_quiet());
        assert!(output.is_verbose());
    }

    #[test]
    fn test_cli_output_config() {
        let config = OutputConfig::new(true, true);
        let output = CliOutput::new(config);
        assert!(output.is_quiet());
        assert!(output.is_verbose());
    }
}
