//! Skip/Only condition evaluation for YAML hooks.
//!
//! Evaluates `skip` and `only` conditions at both the hook and job level.
//! - `skip`: If any rule matches, the hook/job is skipped.
//! - `only`: If any rule does NOT match, the hook/job is skipped.

use super::yaml_config::{
    JobDef, OnlyCondition, OnlyRule, OnlyRuleStructured, SkipCondition, SkipRule,
    SkipRuleStructured,
};
use std::path::Path;

/// Information about why a job was skipped.
#[derive(Debug, Clone)]
pub struct SkipInfo {
    /// Human-readable reason for the skip.
    pub reason: String,
    /// Whether the skip evaluation involved running a command check.
    pub ran_command: bool,
}

/// Check whether a hook/job should be skipped based on `skip` condition.
///
/// Returns `Some(SkipInfo)` if it should be skipped, `None` if it should run.
pub fn should_skip(condition: &SkipCondition, worktree: &Path) -> Option<SkipInfo> {
    match condition {
        SkipCondition::Bool(true) => Some(SkipInfo {
            reason: "skip: true".to_string(),
            ran_command: false,
        }),
        SkipCondition::Bool(false) => None,
        SkipCondition::EnvVar(var) => {
            if is_env_truthy(var) {
                Some(SkipInfo {
                    reason: format!("skip: env ${var} is set"),
                    ran_command: false,
                })
            } else {
                None
            }
        }
        SkipCondition::Rules(rules) => {
            // Any rule match → skip
            for rule in rules {
                if let Some(info) = eval_skip_rule(rule, worktree) {
                    return Some(info);
                }
            }
            None
        }
    }
}

/// Check whether a hook/job should run based on `only` condition.
///
/// Returns `Some(SkipInfo)` if it should be skipped (condition NOT met), `None` if it should run.
pub fn should_only_skip(condition: &OnlyCondition, worktree: &Path) -> Option<SkipInfo> {
    match condition {
        OnlyCondition::Bool(true) => None,
        OnlyCondition::Bool(false) => Some(SkipInfo {
            reason: "only: false".to_string(),
            ran_command: false,
        }),
        OnlyCondition::EnvVar(var) => {
            if is_env_truthy(var) {
                None
            } else {
                Some(SkipInfo {
                    reason: format!("only: env ${var} is not set"),
                    ran_command: false,
                })
            }
        }
        OnlyCondition::Rules(rules) => {
            // All rules must match for the job to run; if any fails → skip
            for rule in rules {
                if let Some(info) = eval_only_rule(rule, worktree) {
                    return Some(info);
                }
            }
            None
        }
    }
}

/// Evaluate a single skip rule.
fn eval_skip_rule(rule: &SkipRule, worktree: &Path) -> Option<SkipInfo> {
    match rule {
        SkipRule::Named(name) => eval_named_condition(name, worktree).map(|reason| SkipInfo {
            reason,
            ran_command: false,
        }),
        SkipRule::Structured(s) => eval_structured_skip(s, worktree),
    }
}

/// Evaluate a single only rule.
///
/// Returns `Some(SkipInfo)` if the condition is NOT met (i.e., should skip).
fn eval_only_rule(rule: &OnlyRule, worktree: &Path) -> Option<SkipInfo> {
    match rule {
        OnlyRule::Named(name) => {
            // For "only", the condition must be met. If it is NOT met → skip.
            if eval_named_condition(name, worktree).is_some() {
                // Named condition triggered (e.g., "merge" is true) → condition IS met → run
                None
            } else {
                // Named condition NOT triggered → condition NOT met → skip
                Some(SkipInfo {
                    reason: format!("only: not in {name} state"),
                    ran_command: false,
                })
            }
        }
        OnlyRule::Structured(s) => eval_structured_only(s, worktree),
    }
}

/// Evaluate named conditions: "merge", "rebase".
fn eval_named_condition(name: &str, worktree: &Path) -> Option<String> {
    match name {
        "merge" => {
            if is_in_merge(worktree) {
                Some("skip: in merge state".to_string())
            } else {
                None
            }
        }
        "rebase" => {
            if is_in_rebase(worktree) {
                Some("skip: in rebase state".to_string())
            } else {
                None
            }
        }
        _ => None, // Unknown named condition — don't skip
    }
}

/// Evaluate structured skip rule (ref, env, run).
fn eval_structured_skip(rule: &SkipRuleStructured, worktree: &Path) -> Option<SkipInfo> {
    if let Some(ref pattern) = rule.ref_pattern {
        if let Some(branch) = current_ref(worktree) {
            if branch_matches_pattern(&branch, pattern) {
                return Some(SkipInfo {
                    reason: rule
                        .desc
                        .clone()
                        .unwrap_or_else(|| format!("skip: ref matches '{pattern}'")),
                    ran_command: false,
                });
            }
        }
    }

    if let Some(ref var) = rule.env {
        if is_env_truthy(var) {
            return Some(SkipInfo {
                reason: rule
                    .desc
                    .clone()
                    .unwrap_or_else(|| format!("skip: env ${var} is set")),
                ran_command: false,
            });
        }
    }

    if let Some(ref cmd) = rule.run {
        if run_check_command(cmd, worktree) {
            return Some(SkipInfo {
                reason: rule
                    .desc
                    .clone()
                    .unwrap_or_else(|| format!("skip: command succeeded: {cmd}")),
                ran_command: true,
            });
        }
    }

    None
}

/// Evaluate structured only rule.
///
/// Returns `Some(SkipInfo)` if any sub-condition is NOT met.
fn eval_structured_only(rule: &OnlyRuleStructured, worktree: &Path) -> Option<SkipInfo> {
    if let Some(ref pattern) = rule.ref_pattern {
        let branch = current_ref(worktree).unwrap_or_default();
        if !branch_matches_pattern(&branch, pattern) {
            return Some(SkipInfo {
                reason: rule
                    .desc
                    .clone()
                    .unwrap_or_else(|| format!("only: ref does not match '{pattern}'")),
                ran_command: false,
            });
        }
    }

    if let Some(ref var) = rule.env {
        if !is_env_truthy(var) {
            return Some(SkipInfo {
                reason: rule
                    .desc
                    .clone()
                    .unwrap_or_else(|| format!("only: env ${var} is not set")),
                ran_command: false,
            });
        }
    }

    if let Some(ref cmd) = rule.run {
        if !run_check_command(cmd, worktree) {
            return Some(SkipInfo {
                reason: rule
                    .desc
                    .clone()
                    .unwrap_or_else(|| format!("only: command failed: {cmd}")),
                ran_command: true,
            });
        }
    }

    None
}

/// Check if an environment variable is set and truthy.
fn is_env_truthy(var: &str) -> bool {
    std::env::var(var)
        .ok()
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(false)
}

/// Check if git is currently in a merge state.
fn is_in_merge(worktree: &Path) -> bool {
    let git_dir = worktree.join(".git");
    // MERGE_HEAD exists during a merge
    git_dir.join("MERGE_HEAD").exists()
        || std::process::Command::new("git")
            .args(["rev-parse", "--verify", "MERGE_HEAD"])
            .current_dir(worktree)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Check if git is currently in a rebase state.
fn is_in_rebase(worktree: &Path) -> bool {
    let git_dir = worktree.join(".git");
    git_dir.join("rebase-merge").exists()
        || git_dir.join("rebase-apply").exists()
        || std::process::Command::new("git")
            .args(["rev-parse", "--verify", "REBASE_HEAD"])
            .current_dir(worktree)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
}

/// Get the current branch/ref name.
fn current_ref(worktree: &Path) -> Option<String> {
    std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(worktree)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

/// Check if a branch name matches a pattern (supports simple glob with *).
fn branch_matches_pattern(branch: &str, pattern: &str) -> bool {
    if pattern.contains('*') {
        // Simple glob: convert to a basic matcher
        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            let (prefix, suffix) = (parts[0], parts[1]);
            branch.starts_with(prefix) && branch.ends_with(suffix)
        } else {
            // More complex pattern — use globset
            globset::Glob::new(pattern)
                .ok()
                .and_then(|g| g.compile_matcher().is_match(branch).then_some(()))
                .is_some()
        }
    } else {
        branch == pattern
    }
}

/// Check platform constraints (os/arch) for a job.
///
/// Returns `Some(reason)` if the current platform does not match the job's constraints.
pub fn check_platform_constraints(job: &JobDef) -> Option<String> {
    if let Some(ref os_constraint) = job.os {
        let current_os = std::env::consts::OS;
        let matches = os_constraint
            .as_slice()
            .iter()
            .any(|target| target.as_str() == current_os);
        if !matches {
            let allowed: Vec<&str> = os_constraint
                .as_slice()
                .iter()
                .map(|t| t.as_str())
                .collect();
            return Some(format!(
                "not on {} (current: {current_os})",
                allowed.join("/")
            ));
        }
    }

    if let Some(ref arch_constraint) = job.arch {
        let current_arch = std::env::consts::ARCH;
        let matches = arch_constraint
            .as_slice()
            .iter()
            .any(|target| target.as_str() == current_arch);
        if !matches {
            let allowed: Vec<&str> = arch_constraint
                .as_slice()
                .iter()
                .map(|t| t.as_str())
                .collect();
            return Some(format!(
                "not on {} (current: {current_arch})",
                allowed.join("/")
            ));
        }
    }

    None
}

/// Run a check command and return whether it exited 0.
fn run_check_command(cmd: &str, worktree: &Path) -> bool {
    std::process::Command::new("sh")
        .args(["-c", cmd])
        .current_dir(worktree)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_skip_bool_true() {
        let cond = SkipCondition::Bool(true);
        assert!(should_skip(&cond, Path::new(".")).is_some());
    }

    #[test]
    fn test_skip_bool_false() {
        let cond = SkipCondition::Bool(false);
        assert!(should_skip(&cond, Path::new(".")).is_none());
    }

    #[test]
    fn test_skip_env_var_set() {
        std::env::set_var("DAFT_TEST_SKIP_VAR", "1");
        let cond = SkipCondition::EnvVar("DAFT_TEST_SKIP_VAR".to_string());
        assert!(should_skip(&cond, Path::new(".")).is_some());
        std::env::remove_var("DAFT_TEST_SKIP_VAR");
    }

    #[test]
    fn test_skip_env_var_unset() {
        std::env::remove_var("DAFT_TEST_SKIP_NONEXIST");
        let cond = SkipCondition::EnvVar("DAFT_TEST_SKIP_NONEXIST".to_string());
        assert!(should_skip(&cond, Path::new(".")).is_none());
    }

    #[test]
    fn test_only_bool_true() {
        let cond = OnlyCondition::Bool(true);
        assert!(should_only_skip(&cond, Path::new(".")).is_none());
    }

    #[test]
    fn test_only_bool_false() {
        let cond = OnlyCondition::Bool(false);
        assert!(should_only_skip(&cond, Path::new(".")).is_some());
    }

    #[test]
    fn test_skip_run_command_succeeds() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("true".to_string()),
            desc: None,
        })]);
        assert!(should_skip(&cond, Path::new(".")).is_some());
    }

    #[test]
    fn test_skip_run_command_fails() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("false".to_string()),
            desc: None,
        })]);
        assert!(should_skip(&cond, Path::new(".")).is_none());
    }

    #[test]
    fn test_branch_matches_pattern_exact() {
        assert!(branch_matches_pattern("main", "main"));
        assert!(!branch_matches_pattern("main", "master"));
    }

    #[test]
    fn test_branch_matches_pattern_glob() {
        assert!(branch_matches_pattern("feature/foo", "feature/*"));
        assert!(!branch_matches_pattern("bugfix/bar", "feature/*"));
        assert!(branch_matches_pattern("release/v1.0", "release/*"));
    }

    #[test]
    fn test_is_env_truthy() {
        std::env::set_var("DAFT_TRUTHY_TEST", "1");
        assert!(is_env_truthy("DAFT_TRUTHY_TEST"));

        std::env::set_var("DAFT_TRUTHY_TEST", "0");
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));

        std::env::set_var("DAFT_TRUTHY_TEST", "false");
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));

        std::env::set_var("DAFT_TRUTHY_TEST", "");
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));

        std::env::remove_var("DAFT_TRUTHY_TEST");
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));
    }

    #[test]
    fn test_skip_rule_desc_override() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("true".to_string()),
            desc: Some("Brew is already installed".to_string()),
        })]);
        let info = should_skip(&cond, Path::new(".")).unwrap();
        assert_eq!(info.reason, "Brew is already installed");
        assert!(info.ran_command);
    }

    #[test]
    fn test_skip_rule_no_desc_uses_default() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("true".to_string()),
            desc: None,
        })]);
        let info = should_skip(&cond, Path::new(".")).unwrap();
        assert!(info.reason.starts_with("skip: command succeeded:"));
        assert!(info.ran_command);
    }

    #[test]
    fn test_only_rule_desc_override() {
        let cond = OnlyCondition::Rules(vec![OnlyRule::Structured(OnlyRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("false".to_string()),
            desc: Some("Only when package.json exists".to_string()),
        })]);
        let info = should_only_skip(&cond, Path::new(".")).unwrap();
        assert_eq!(info.reason, "Only when package.json exists");
        assert!(info.ran_command);
    }

    #[test]
    fn test_check_platform_constraints_matching_os() {
        use super::super::yaml_config::{PlatformConstraint, TargetOs};
        let current_os = std::env::consts::OS;
        let target_os = match current_os {
            "macos" => TargetOs::Macos,
            "linux" => TargetOs::Linux,
            "windows" => TargetOs::Windows,
            _ => return, // Skip test on unknown OS
        };
        let job = JobDef {
            os: Some(PlatformConstraint::Single(target_os)),
            ..Default::default()
        };
        assert!(check_platform_constraints(&job).is_none());
    }

    #[test]
    fn test_check_platform_constraints_non_matching_os() {
        use super::super::yaml_config::{PlatformConstraint, TargetOs};
        let non_matching_os = if std::env::consts::OS == "macos" {
            TargetOs::Linux
        } else {
            TargetOs::Macos
        };
        let job = JobDef {
            os: Some(PlatformConstraint::Single(non_matching_os)),
            ..Default::default()
        };
        let reason = check_platform_constraints(&job).unwrap();
        assert!(reason.starts_with("not on "));
    }

    #[test]
    fn test_check_platform_constraints_os_list() {
        use super::super::yaml_config::{PlatformConstraint, TargetOs};
        let job = JobDef {
            os: Some(PlatformConstraint::List(vec![
                TargetOs::Macos,
                TargetOs::Linux,
            ])),
            ..Default::default()
        };
        // On macOS or Linux this should pass; on Windows it should fail
        let result = check_platform_constraints(&job);
        if std::env::consts::OS == "macos" || std::env::consts::OS == "linux" {
            assert!(result.is_none());
        } else {
            assert!(result.is_some());
        }
    }

    #[test]
    fn test_check_platform_constraints_no_constraints() {
        let job = JobDef::default();
        assert!(check_platform_constraints(&job).is_none());
    }
}
