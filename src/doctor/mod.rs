//! Diagnostic check framework for `daft doctor`.
//!
//! Provides types and display helpers for running health checks on
//! daft installation, repository configuration, and hooks setup.

pub mod hooks_checks;
pub mod installation;
pub mod repository;

/// Status of a single diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
    Skipped,
}

/// Result of a single diagnostic check.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    /// Additional details shown in --verbose mode.
    pub details: Vec<String>,
    /// Actionable suggestion shown on Warning/Fail.
    pub suggestion: Option<String>,
    /// Whether this issue can be auto-fixed with --fix.
    pub fixable: bool,
}

impl CheckResult {
    pub fn pass(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fixable: false,
        }
    }

    pub fn warning(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Warning,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fixable: false,
        }
    }

    pub fn fail(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Fail,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fixable: false,
        }
    }

    pub fn skipped(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Skipped,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fixable: false,
        }
    }

    pub fn with_suggestion(mut self, suggestion: &str) -> Self {
        self.suggestion = Some(suggestion.to_string());
        self
    }

    pub fn with_fixable(mut self, fixable: bool) -> Self {
        self.fixable = fixable;
        self
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }
}

/// A category of checks with a title and results.
pub struct CheckCategory {
    pub title: String,
    pub results: Vec<CheckResult>,
}

/// Summary counts for all checks.
pub struct DoctorSummary {
    pub passed: usize,
    pub warnings: usize,
    pub failures: usize,
    pub skipped: usize,
}

impl DoctorSummary {
    pub fn from_categories(categories: &[CheckCategory]) -> Self {
        let mut passed = 0;
        let mut warnings = 0;
        let mut failures = 0;
        let mut skipped = 0;

        for category in categories {
            for result in &category.results {
                match result.status {
                    CheckStatus::Pass => passed += 1,
                    CheckStatus::Warning => warnings += 1,
                    CheckStatus::Fail => failures += 1,
                    CheckStatus::Skipped => skipped += 1,
                }
            }
        }

        Self {
            passed,
            warnings,
            failures,
            skipped,
        }
    }

    pub fn has_failures(&self) -> bool {
        self.failures > 0
    }
}

/// Returns the status symbol for a check result.
pub fn status_symbol(status: CheckStatus) -> String {
    use crate::styles::{dim, green, red, yellow};
    match status {
        CheckStatus::Pass => green("\u{2713}"), // ✓
        CheckStatus::Warning => yellow("!"),    // !
        CheckStatus::Fail => red("\u{2717}"),   // ✗
        CheckStatus::Skipped => dim("-"),       // -
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_result_pass() {
        let result = CheckResult::pass("test", "everything ok");
        assert_eq!(result.status, CheckStatus::Pass);
        assert_eq!(result.name, "test");
        assert_eq!(result.message, "everything ok");
        assert!(result.suggestion.is_none());
        assert!(!result.fixable);
    }

    #[test]
    fn test_check_result_warning_with_suggestion() {
        let result = CheckResult::warning("test", "something off")
            .with_suggestion("fix it")
            .with_fixable(true);
        assert_eq!(result.status, CheckStatus::Warning);
        assert_eq!(result.suggestion.as_deref(), Some("fix it"));
        assert!(result.fixable);
    }

    #[test]
    fn test_check_result_fail() {
        let result = CheckResult::fail("test", "broken");
        assert_eq!(result.status, CheckStatus::Fail);
    }

    #[test]
    fn test_check_result_skipped() {
        let result = CheckResult::skipped("test", "not applicable");
        assert_eq!(result.status, CheckStatus::Skipped);
    }

    #[test]
    fn test_check_result_with_details() {
        let result =
            CheckResult::pass("test", "ok").with_details(vec!["detail1".into(), "detail2".into()]);
        assert_eq!(result.details.len(), 2);
    }

    #[test]
    fn test_doctor_summary() {
        let categories = vec![
            CheckCategory {
                title: "Test".to_string(),
                results: vec![
                    CheckResult::pass("a", "ok"),
                    CheckResult::pass("b", "ok"),
                    CheckResult::warning("c", "warn"),
                ],
            },
            CheckCategory {
                title: "Test2".to_string(),
                results: vec![
                    CheckResult::fail("d", "fail"),
                    CheckResult::skipped("e", "skip"),
                ],
            },
        ];

        let summary = DoctorSummary::from_categories(&categories);
        assert_eq!(summary.passed, 2);
        assert_eq!(summary.warnings, 1);
        assert_eq!(summary.failures, 1);
        assert_eq!(summary.skipped, 1);
        assert!(summary.has_failures());
    }

    #[test]
    fn test_doctor_summary_no_failures() {
        let categories = vec![CheckCategory {
            title: "Test".to_string(),
            results: vec![
                CheckResult::pass("a", "ok"),
                CheckResult::warning("b", "warn"),
            ],
        }];

        let summary = DoctorSummary::from_categories(&categories);
        assert!(!summary.has_failures());
    }
}
