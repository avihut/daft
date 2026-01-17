//! CLI output implementation.
//!
//! This implementation preserves the exact current output behavior of the codebase,
//! ensuring backward compatibility during the migration.

use super::{Output, OutputConfig};
use crate::{CD_PATH_MARKER, SHELL_WRAPPER_ENV};
use std::env;
use std::path::Path;

/// CLI output implementation that writes directly to stdout/stderr.
///
/// This preserves the exact output format used throughout the codebase:
/// - `info()` → `println!("{msg}")`
/// - `progress()` → `println!("--> {msg}")`
/// - `divider()` → `println!("---")`
/// - `warning()` → `eprintln!("Warning: {msg}")`
/// - `error()` → `eprintln!("Error: {msg}")`
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
            println!("{msg}");
        }
    }

    fn warning(&mut self, msg: &str) {
        // Warnings are always shown (not affected by quiet mode)
        eprintln!("Warning: {msg}");
    }

    fn error(&mut self, msg: &str) {
        // Errors are always shown (not affected by quiet mode)
        eprintln!("Error: {msg}");
    }

    fn debug(&mut self, msg: &str) {
        if self.config.verbose {
            println!("Debug: {msg}");
        }
    }

    fn progress(&mut self, msg: &str) {
        if !self.config.quiet {
            println!("--> {msg}");
        }
    }

    fn divider(&mut self) {
        if !self.config.quiet {
            println!("---");
        }
    }

    fn detail(&mut self, key: &str, value: &str) {
        if !self.config.quiet {
            println!("  {key}: {value}");
        }
    }

    fn list_item(&mut self, item: &str) {
        if !self.config.quiet {
            println!(" - {item}");
        }
    }

    fn operation_start(&mut self, operation: &str) {
        // In CLI mode, just print a progress message
        self.progress(operation);
    }

    fn operation_end(&mut self, operation: &str, success: bool) {
        if !self.config.quiet {
            if success {
                println!("--> {operation} completed successfully.");
            } else {
                eprintln!("--> {operation} failed.");
            }
        }
    }

    fn cd_path(&mut self, path: &Path) {
        // Only output if the shell wrapper environment variable is set.
        // This keeps output clean for users who don't use wrappers.
        if env::var(SHELL_WRAPPER_ENV).is_ok() {
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
