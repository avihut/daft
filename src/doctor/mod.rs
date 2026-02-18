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

/// A closure that can fix an issue found by a check.
type FixFn = Box<dyn Fn() -> Result<(), String>>;

/// A single planned action from a dry-run simulation.
pub struct FixAction {
    /// What would be done, e.g. "Create symlink gwtco -> daft in /usr/local/bin"
    pub description: String,
    /// Whether preconditions are met for this action to succeed.
    pub would_succeed: bool,
    /// Why it would fail, if would_succeed is false.
    pub failure_reason: Option<String>,
}

/// A closure that simulates a fix, checking preconditions without applying changes.
type DryRunFn = Box<dyn Fn() -> Vec<FixAction>>;

/// Result of a single diagnostic check.
pub struct CheckResult {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    /// Additional details shown in --verbose mode.
    pub details: Vec<String>,
    /// Actionable suggestion shown on Warning/Fail.
    pub suggestion: Option<String>,
    /// Optional fix closure. When present, --fix can auto-fix this issue.
    pub fix: Option<FixFn>,
    /// Optional dry-run closure. Simulates the fix, returning planned actions.
    pub dry_run_fix: Option<DryRunFn>,
}

impl std::fmt::Debug for CheckResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CheckResult")
            .field("name", &self.name)
            .field("status", &self.status)
            .field("message", &self.message)
            .field("details", &self.details)
            .field("suggestion", &self.suggestion)
            .field("fix", &self.fix.is_some())
            .field("dry_run_fix", &self.dry_run_fix.is_some())
            .finish()
    }
}

impl CheckResult {
    pub fn pass(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Pass,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fix: None,
            dry_run_fix: None,
        }
    }

    pub fn warning(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Warning,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fix: None,
            dry_run_fix: None,
        }
    }

    pub fn fail(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Fail,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fix: None,
            dry_run_fix: None,
        }
    }

    pub fn skipped(name: &str, message: &str) -> Self {
        Self {
            name: name.to_string(),
            status: CheckStatus::Skipped,
            message: message.to_string(),
            details: Vec::new(),
            suggestion: None,
            fix: None,
            dry_run_fix: None,
        }
    }

    pub fn with_suggestion(mut self, suggestion: &str) -> Self {
        self.suggestion = Some(suggestion.to_string());
        self
    }

    pub fn with_fix(mut self, fix: FixFn) -> Self {
        self.fix = Some(fix);
        self
    }

    pub fn with_dry_run_fix(mut self, dry_run_fix: DryRunFn) -> Self {
        self.dry_run_fix = Some(dry_run_fix);
        self
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }

    /// Returns true if this check has an auto-fix available.
    pub fn fixable(&self) -> bool {
        self.fix.is_some()
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
    pub warning_names: Vec<String>,
    pub failure_names: Vec<String>,
}

impl DoctorSummary {
    pub fn from_categories(categories: &[CheckCategory]) -> Self {
        let mut passed = 0;
        let mut warnings = 0;
        let mut failures = 0;
        let mut skipped = 0;
        let mut warning_names = Vec::new();
        let mut failure_names = Vec::new();

        for category in categories {
            for result in &category.results {
                match result.status {
                    CheckStatus::Pass => passed += 1,
                    CheckStatus::Warning => {
                        warnings += 1;
                        warning_names.push(result.name.clone());
                    }
                    CheckStatus::Fail => {
                        failures += 1;
                        failure_names.push(result.name.clone());
                    }
                    CheckStatus::Skipped => skipped += 1,
                }
            }
        }

        Self {
            passed,
            warnings,
            failures,
            skipped,
            warning_names,
            failure_names,
        }
    }

    pub fn has_failures(&self) -> bool {
        self.failures > 0
    }
}

/// Returns the status symbol for a check result (with brackets).
pub fn status_symbol(status: CheckStatus) -> String {
    use crate::styles::{dim, green, red, yellow};
    match status {
        CheckStatus::Pass => green("[\u{2713}]"),  // [✓]
        CheckStatus::Warning => yellow("[!]"),     // [!]
        CheckStatus::Fail => red("[\u{2717}]"),    // [✗]
        CheckStatus::Skipped => dim("[\u{2212}]"), // [−]
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
        assert!(!result.fixable());
    }

    #[test]
    fn test_check_result_warning_with_suggestion_and_fix() {
        let result = CheckResult::warning("test", "something off")
            .with_suggestion("fix it")
            .with_fix(Box::new(|| Ok(())));
        assert_eq!(result.status, CheckStatus::Warning);
        assert_eq!(result.suggestion.as_deref(), Some("fix it"));
        assert!(result.fixable());
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
        assert_eq!(summary.warning_names, vec!["c"]);
        assert_eq!(summary.failure_names, vec!["d"]);
    }

    #[test]
    fn test_fix_action_success() {
        let action = FixAction {
            description: "Create symlink foo -> daft".to_string(),
            would_succeed: true,
            failure_reason: None,
        };
        assert!(action.would_succeed);
        assert!(action.failure_reason.is_none());
    }

    #[test]
    fn test_fix_action_failure() {
        let action = FixAction {
            description: "Create symlink foo -> daft".to_string(),
            would_succeed: false,
            failure_reason: Some("Directory not writable".to_string()),
        };
        assert!(!action.would_succeed);
        assert_eq!(
            action.failure_reason.as_deref(),
            Some("Directory not writable")
        );
    }

    #[test]
    fn test_check_result_with_dry_run_fix() {
        let result = CheckResult::warning("test", "something off")
            .with_fix(Box::new(|| Ok(())))
            .with_dry_run_fix(Box::new(|| {
                vec![FixAction {
                    description: "Would do thing".to_string(),
                    would_succeed: true,
                    failure_reason: None,
                }]
            }));
        assert!(result.fixable());
        assert!(result.dry_run_fix.is_some());
        let actions = (result.dry_run_fix.unwrap())();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].description, "Would do thing");
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
