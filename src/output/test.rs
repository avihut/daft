//! Test output implementation for verifying command output in tests.
//!
//! This captures all output as structured data for easy assertions.

use super::{Output, OutputConfig};
use std::path::{Path, PathBuf};

/// Represents a single output entry captured during testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputEntry {
    /// Informational message
    Info(String),
    /// Success message
    Success(String),
    /// Warning message
    Warning(String),
    /// Error message
    Error(String),
    /// Debug message
    Debug(String),
    /// Progress/action message (rendered as "--> msg" in CLI)
    Progress(String),
    /// Visual divider (rendered as "---" in CLI)
    Divider,
    /// Key-value detail (rendered as "  Key: value" in CLI)
    Detail { key: String, value: String },
    /// List item (rendered as " - item" in CLI)
    ListItem(String),
    /// Operation started
    OperationStart(String),
    /// Operation ended with success/failure status
    OperationEnd { operation: String, success: bool },
    /// CD path marker for shell wrappers
    CdPath(PathBuf),
    /// Raw output
    Raw(String),
}

/// Test output implementation that captures all output for assertions.
///
/// # Example
///
/// ```ignore
/// let mut output = TestOutput::new();
/// some_command(&mut output)?;
///
/// assert!(output.has_info("Operation complete"));
/// assert!(!output.has_errors());
/// assert_eq!(output.cd_path(), Some(PathBuf::from("/path/to/worktree")));
/// ```
#[derive(Debug, Default)]
pub struct TestOutput {
    config: OutputConfig,
    entries: Vec<OutputEntry>,
}

impl TestOutput {
    /// Create a new test output with default configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a test output with custom configuration.
    pub fn with_config(config: OutputConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
        }
    }

    /// Create a test output in quiet mode.
    pub fn quiet() -> Self {
        Self::with_config(OutputConfig::new(true, false))
    }

    /// Create a test output in verbose mode.
    pub fn verbose() -> Self {
        Self::with_config(OutputConfig::new(false, true))
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Entry Access
    // ─────────────────────────────────────────────────────────────────────────

    /// Get all captured output entries.
    pub fn entries(&self) -> &[OutputEntry] {
        &self.entries
    }

    /// Clear all captured entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Filtered Access Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Get all info messages.
    pub fn infos(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::Info(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all success messages.
    pub fn successes(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::Success(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all warning messages.
    pub fn warnings(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::Warning(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all error messages.
    pub fn errors(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::Error(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all debug messages.
    pub fn debugs(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::Debug(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all progress messages.
    pub fn progress_messages(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::Progress(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get all list items.
    pub fn list_items(&self) -> Vec<&str> {
        self.entries
            .iter()
            .filter_map(|e| match e {
                OutputEntry::ListItem(s) => Some(s.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Get the cd path if one was output.
    /// Returns the last cd_path if multiple were output.
    pub fn get_cd_path(&self) -> Option<&PathBuf> {
        self.entries.iter().rev().find_map(|e| match e {
            OutputEntry::CdPath(p) => Some(p),
            _ => None,
        })
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Assertion Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Check if any info message contains the given substring.
    pub fn has_info(&self, substring: &str) -> bool {
        self.infos().iter().any(|s| s.contains(substring))
    }

    /// Check if any success message contains the given substring.
    pub fn has_success(&self, substring: &str) -> bool {
        self.successes().iter().any(|s| s.contains(substring))
    }

    /// Check if any warning message contains the given substring.
    pub fn has_warning(&self, substring: &str) -> bool {
        self.warnings().iter().any(|s| s.contains(substring))
    }

    /// Check if any error message contains the given substring.
    pub fn has_error(&self, substring: &str) -> bool {
        self.errors().iter().any(|s| s.contains(substring))
    }

    /// Check if any progress message contains the given substring.
    pub fn has_progress(&self, substring: &str) -> bool {
        self.progress_messages()
            .iter()
            .any(|s| s.contains(substring))
    }

    /// Check if any errors were output.
    pub fn has_errors(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, OutputEntry::Error(_)))
    }

    /// Check if any warnings were output.
    pub fn has_warnings(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, OutputEntry::Warning(_)))
    }

    /// Check if a divider was output.
    pub fn has_divider(&self) -> bool {
        self.entries
            .iter()
            .any(|e| matches!(e, OutputEntry::Divider))
    }

    /// Count the number of entries of a specific type.
    pub fn count_dividers(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e, OutputEntry::Divider))
            .count()
    }
}

impl Output for TestOutput {
    fn info(&mut self, msg: &str) {
        // Respect quiet mode to match CLI behavior
        if !self.config.quiet {
            self.entries.push(OutputEntry::Info(msg.to_string()));
        }
    }

    fn success(&mut self, msg: &str) {
        if !self.config.quiet {
            self.entries.push(OutputEntry::Success(msg.to_string()));
        }
    }

    fn warning(&mut self, msg: &str) {
        // Warnings are always captured (not affected by quiet mode)
        self.entries.push(OutputEntry::Warning(msg.to_string()));
    }

    fn error(&mut self, msg: &str) {
        // Errors are always captured (not affected by quiet mode)
        self.entries.push(OutputEntry::Error(msg.to_string()));
    }

    fn debug(&mut self, msg: &str) {
        // Only capture debug in verbose mode
        if self.config.verbose {
            self.entries.push(OutputEntry::Debug(msg.to_string()));
        }
    }

    fn progress(&mut self, msg: &str) {
        if !self.config.quiet {
            self.entries.push(OutputEntry::Progress(msg.to_string()));
        }
    }

    fn divider(&mut self) {
        if !self.config.quiet {
            self.entries.push(OutputEntry::Divider);
        }
    }

    fn detail(&mut self, key: &str, value: &str) {
        if !self.config.quiet {
            self.entries.push(OutputEntry::Detail {
                key: key.to_string(),
                value: value.to_string(),
            });
        }
    }

    fn list_item(&mut self, item: &str) {
        if !self.config.quiet {
            self.entries.push(OutputEntry::ListItem(item.to_string()));
        }
    }

    fn operation_start(&mut self, operation: &str) {
        self.entries
            .push(OutputEntry::OperationStart(operation.to_string()));
    }

    fn operation_end(&mut self, operation: &str, success: bool) {
        self.entries.push(OutputEntry::OperationEnd {
            operation: operation.to_string(),
            success,
        });
    }

    fn cd_path(&mut self, path: &Path) {
        // Always capture cd_path for test verification
        // (In real CLI, this only outputs if DAFT_SHELL_WRAPPER is set)
        self.entries.push(OutputEntry::CdPath(path.to_path_buf()));
    }

    fn raw(&mut self, content: &str) {
        self.entries.push(OutputEntry::Raw(content.to_string()));
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
    fn test_captures_info() {
        let mut output = TestOutput::new();
        output.info("Hello world");
        assert_eq!(output.infos(), vec!["Hello world"]);
        assert!(output.has_info("world"));
    }

    #[test]
    fn test_captures_progress() {
        let mut output = TestOutput::new();
        output.progress("Processing files");
        assert_eq!(output.progress_messages(), vec!["Processing files"]);
        assert!(output.has_progress("Processing"));
    }

    #[test]
    fn test_captures_warnings_and_errors() {
        let mut output = TestOutput::new();
        output.warning("Something is fishy");
        output.error("Something went wrong");

        assert!(output.has_warnings());
        assert!(output.has_errors());
        assert!(output.has_warning("fishy"));
        assert!(output.has_error("wrong"));
    }

    #[test]
    fn test_quiet_mode_suppresses_info() {
        let mut output = TestOutput::quiet();
        output.info("Should not appear");
        output.progress("Should not appear either");
        output.warning("Should appear");

        assert!(output.infos().is_empty());
        assert!(output.progress_messages().is_empty());
        assert!(!output.warnings().is_empty());
    }

    #[test]
    fn test_verbose_mode_enables_debug() {
        let mut output = TestOutput::verbose();
        output.debug("Debug message");
        assert_eq!(output.debugs(), vec!["Debug message"]);

        let mut non_verbose = TestOutput::new();
        non_verbose.debug("Should not appear");
        assert!(non_verbose.debugs().is_empty());
    }

    #[test]
    fn test_cd_path() {
        use super::Output;
        let mut output = TestOutput::new();
        output.cd_path(Path::new("/path/to/worktree"));

        assert_eq!(
            output.get_cd_path(),
            Some(&PathBuf::from("/path/to/worktree"))
        );
    }

    #[test]
    fn test_divider() {
        let mut output = TestOutput::new();
        output.divider();
        output.divider();

        assert!(output.has_divider());
        assert_eq!(output.count_dividers(), 2);
    }

    #[test]
    fn test_detail_and_list_item() {
        let mut output = TestOutput::new();
        output.detail("Path", "/some/path");
        output.list_item("item one");
        output.list_item("item two");

        assert_eq!(output.list_items(), vec!["item one", "item two"]);
        assert!(output.entries().iter().any(|e| matches!(
            e,
            OutputEntry::Detail { key, value } if key == "Path" && value == "/some/path"
        )));
    }

    #[test]
    fn test_clear() {
        let mut output = TestOutput::new();
        output.info("Message");
        output.clear();
        assert!(output.entries().is_empty());
    }
}
