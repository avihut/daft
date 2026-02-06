//! Skip/Only condition evaluation for YAML hooks.
//!
//! Evaluates `skip` and `only` conditions at both the hook and job level.
//! - `skip`: If any rule matches, the hook/job is skipped.
//! - `only`: If any rule does NOT match, the hook/job is skipped.

use super::yaml_config::{
    OnlyCondition, OnlyRule, OnlyRuleStructured, SkipCondition, SkipRule, SkipRuleStructured,
};
use std::path::Path;

/// Check whether a hook/job should be skipped based on `skip` condition.
///
/// Returns `Some(reason)` if it should be skipped, `None` if it should run.
pub fn should_skip(condition: &SkipCondition, worktree: &Path) -> Option<String> {
    match condition {
        SkipCondition::Bool(true) => Some("skip: true".to_string()),
        SkipCondition::Bool(false) => None,
        SkipCondition::EnvVar(var) => {
            if is_env_truthy(var) {
                Some(format!("skip: env ${var} is set"))
            } else {
                None
            }
        }
        SkipCondition::Rules(rules) => {
            // Any rule match → skip
            for rule in rules {
                if let Some(reason) = eval_skip_rule(rule, worktree) {
                    return Some(reason);
                }
            }
            None
        }
    }
}

/// Check whether a hook/job should run based on `only` condition.
///
/// Returns `Some(reason)` if it should be skipped (condition NOT met), `None` if it should run.
pub fn should_only_skip(condition: &OnlyCondition, worktree: &Path) -> Option<String> {
    match condition {
        OnlyCondition::Bool(true) => None,
        OnlyCondition::Bool(false) => Some("only: false".to_string()),
        OnlyCondition::EnvVar(var) => {
            if is_env_truthy(var) {
                None
            } else {
                Some(format!("only: env ${var} is not set"))
            }
        }
        OnlyCondition::Rules(rules) => {
            // All rules must match for the job to run; if any fails → skip
            for rule in rules {
                if let Some(reason) = eval_only_rule(rule, worktree) {
                    return Some(reason);
                }
            }
            None
        }
    }
}

/// Evaluate a single skip rule.
fn eval_skip_rule(rule: &SkipRule, worktree: &Path) -> Option<String> {
    match rule {
        SkipRule::Named(name) => eval_named_condition(name, worktree),
        SkipRule::Structured(s) => eval_structured_skip(s, worktree),
    }
}

/// Evaluate a single only rule.
///
/// Returns `Some(reason)` if the condition is NOT met (i.e., should skip).
fn eval_only_rule(rule: &OnlyRule, worktree: &Path) -> Option<String> {
    match rule {
        OnlyRule::Named(name) => {
            // For "only", the condition must be met. If it is NOT met → skip.
            if eval_named_condition(name, worktree).is_some() {
                // Named condition triggered (e.g., "merge" is true) → condition IS met → run
                None
            } else {
                // Named condition NOT triggered → condition NOT met → skip
                Some(format!("only: not in {name} state"))
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
fn eval_structured_skip(rule: &SkipRuleStructured, worktree: &Path) -> Option<String> {
    if let Some(ref pattern) = rule.ref_pattern {
        if let Some(branch) = current_ref(worktree) {
            if branch_matches_pattern(&branch, pattern) {
                return Some(format!("skip: ref matches '{pattern}'"));
            }
        }
    }

    if let Some(ref var) = rule.env {
        if is_env_truthy(var) {
            return Some(format!("skip: env ${var} is set"));
        }
    }

    if let Some(ref cmd) = rule.run {
        if run_check_command(cmd, worktree) {
            return Some(format!("skip: command succeeded: {cmd}"));
        }
    }

    None
}

/// Evaluate structured only rule.
///
/// Returns `Some(reason)` if any sub-condition is NOT met.
fn eval_structured_only(rule: &OnlyRuleStructured, worktree: &Path) -> Option<String> {
    if let Some(ref pattern) = rule.ref_pattern {
        let branch = current_ref(worktree).unwrap_or_default();
        if !branch_matches_pattern(&branch, pattern) {
            return Some(format!("only: ref does not match '{pattern}'"));
        }
    }

    if let Some(ref var) = rule.env {
        if !is_env_truthy(var) {
            return Some(format!("only: env ${var} is not set"));
        }
    }

    if let Some(ref cmd) = rule.run {
        if !run_check_command(cmd, worktree) {
            return Some(format!("only: command failed: {cmd}"));
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
        })]);
        assert!(should_skip(&cond, Path::new(".")).is_some());
    }

    #[test]
    fn test_skip_run_command_fails() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: Some("false".to_string()),
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
}
