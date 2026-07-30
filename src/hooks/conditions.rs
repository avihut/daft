//! Skip/Only condition evaluation for YAML hooks.
//!
//! Evaluates `skip` and `only` conditions at both the hook and job level.
//! - `skip`: If any rule matches, the hook/job is skipped.
//! - `only`: If any rule does NOT match, the hook/job is skipped.

use super::changed_files::{ChangedFilesProvider, FileFilter};
use super::yaml_config::{
    JobDef, OnlyCondition, OnlyRule, OnlyRuleStructured, SkipCondition, SkipRule,
    SkipRuleStructured, StringOrList, TargetOs,
};
use crate::git::op_state::{OpKind, probe_op_state};
use anyhow::{Context, Result, bail};
use std::path::Path;

/// Information about why a job was skipped.
#[derive(Debug, Clone)]
pub struct SkipInfo {
    /// Human-readable reason for the skip.
    pub reason: String,
    /// Whether the skip evaluation involved running a command check.
    pub ran_command: bool,
}

/// Resolve the current OS to a `TargetOs` variant.
fn resolve_current_target_os() -> Option<TargetOs> {
    match std::env::consts::OS {
        "macos" => Some(TargetOs::Macos),
        "linux" => Some(TargetOs::Linux),
        "windows" => Some(TargetOs::Windows),
        _ => None,
    }
}

/// Check whether a hook/job should be skipped based on `skip` condition.
///
/// Returns `Ok(Some(SkipInfo))` if it should be skipped, `Ok(None)` if it
/// should run. `changed` is the operation's changed-file source, consulted by
/// `changed:` rules; a `changed:` rule with no source is a configuration
/// error (`Err`), never a silent skip or pass.
pub fn should_skip(
    condition: &SkipCondition,
    worktree: &Path,
    changed: Option<&ChangedFilesProvider>,
) -> Result<Option<SkipInfo>> {
    match condition {
        SkipCondition::Bool(true) => Ok(Some(SkipInfo {
            reason: "skip: true".to_string(),
            ran_command: false,
        })),
        SkipCondition::Bool(false) => Ok(None),
        SkipCondition::EnvVar(var) => {
            if is_env_truthy(var) {
                Ok(Some(SkipInfo {
                    reason: format!("skip: env ${var} is set"),
                    ran_command: false,
                }))
            } else {
                Ok(None)
            }
        }
        SkipCondition::Platform(map) => {
            let current_os = resolve_current_target_os();
            if let Some(os) = current_os
                && let Some(rules) = map.get(&os)
            {
                for rule in rules {
                    if let Some(info) = eval_skip_rule(rule, worktree, changed)? {
                        return Ok(Some(info));
                    }
                }
            }
            Ok(None)
        }
        SkipCondition::Rules(rules) => {
            // Any rule match → skip
            for rule in rules {
                if let Some(info) = eval_skip_rule(rule, worktree, changed)? {
                    return Ok(Some(info));
                }
            }
            Ok(None)
        }
    }
}

/// Check whether a hook/job should run based on `only` condition.
///
/// Returns `Ok(Some(SkipInfo))` if it should be skipped (condition NOT met),
/// `Ok(None)` if it should run. See [`should_skip`] for the `changed`
/// parameter's contract.
pub fn should_only_skip(
    condition: &OnlyCondition,
    worktree: &Path,
    changed: Option<&ChangedFilesProvider>,
) -> Result<Option<SkipInfo>> {
    match condition {
        OnlyCondition::Bool(true) => Ok(None),
        OnlyCondition::Bool(false) => Ok(Some(SkipInfo {
            reason: "only: false".to_string(),
            ran_command: false,
        })),
        OnlyCondition::EnvVar(var) => {
            if is_env_truthy(var) {
                Ok(None)
            } else {
                Ok(Some(SkipInfo {
                    reason: format!("only: env ${var} is not set"),
                    ran_command: false,
                }))
            }
        }
        OnlyCondition::Platform(map) => {
            let current_os = resolve_current_target_os();
            if let Some(os) = current_os
                && let Some(rules) = map.get(&os)
            {
                for rule in rules {
                    if let Some(info) = eval_only_rule(rule, worktree, changed)? {
                        return Ok(Some(info));
                    }
                }
            }
            Ok(None)
        }
        OnlyCondition::Rules(rules) => {
            // All rules must match for the job to run; if any fails → skip
            for rule in rules {
                if let Some(info) = eval_only_rule(rule, worktree, changed)? {
                    return Ok(Some(info));
                }
            }
            Ok(None)
        }
    }
}

/// Evaluate a single skip rule.
fn eval_skip_rule(
    rule: &SkipRule,
    worktree: &Path,
    changed: Option<&ChangedFilesProvider>,
) -> Result<Option<SkipInfo>> {
    match rule {
        SkipRule::Named(name) => Ok(eval_named_condition(name, worktree).map(|reason| SkipInfo {
            reason,
            ran_command: false,
        })),
        SkipRule::Structured(s) => eval_structured_skip(s, worktree, changed),
    }
}

/// Evaluate a single only rule.
///
/// Returns `Ok(Some(SkipInfo))` if the condition is NOT met (i.e., should skip).
fn eval_only_rule(
    rule: &OnlyRule,
    worktree: &Path,
    changed: Option<&ChangedFilesProvider>,
) -> Result<Option<SkipInfo>> {
    match rule {
        OnlyRule::Named(name) => {
            // For "only", the condition must be met. If it is NOT met → skip.
            if eval_named_condition(name, worktree).is_some() {
                // Named condition triggered (e.g., "merge" is true) → condition IS met → run
                Ok(None)
            } else {
                // Named condition NOT triggered → condition NOT met → skip
                Ok(Some(SkipInfo {
                    reason: format!("only: not in {name} state"),
                    ran_command: false,
                }))
            }
        }
        OnlyRule::Structured(s) => eval_structured_only(s, worktree, changed),
    }
}

/// Evaluate named conditions: "merge", "rebase", "merge-commit".
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
        // Distinct from "merge", which asks whether a merge is *in progress*.
        // This asks what HEAD is, and exists for the commit stages: a merge
        // commit's content is the merge, not the author's work, so linting it
        // reports on files nobody in this commit touched.
        "merge-commit" => {
            if head_is_merge_commit(worktree) {
                Some("skip: HEAD is a merge commit".to_string())
            } else {
                None
            }
        }
        _ => None, // Unknown named condition — don't skip
    }
}

/// Whether HEAD has more than one parent.
///
/// `rev-list --parents -1 HEAD` prints the commit and its parents on one
/// line, so three or more fields means a merge. An unborn HEAD (no commits
/// yet) or any other failure answers "no" — the question is about a commit
/// that exists, and refusing to run a gate because a probe failed would be
/// the wrong way to be wrong.
fn head_is_merge_commit(worktree: &Path) -> bool {
    let Ok(output) = crate::utils::git_command_at(worktree)
        .args(["rev-list", "--parents", "-1", "HEAD"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .count()
        > 2
}

/// Evaluate structured skip rule (ref, env, run, changed).
fn eval_structured_skip(
    rule: &SkipRuleStructured,
    worktree: &Path,
    changed: Option<&ChangedFilesProvider>,
) -> Result<Option<SkipInfo>> {
    if let Some(ref pattern) = rule.ref_pattern
        && let Some(branch) = current_ref(worktree)
        && branch_matches_pattern(&branch, pattern)
    {
        return Ok(Some(SkipInfo {
            reason: rule
                .desc
                .clone()
                .unwrap_or_else(|| format!("skip: ref matches '{pattern}'")),
            ran_command: false,
        }));
    }

    if let Some(ref var) = rule.env
        && is_env_truthy(var)
    {
        return Ok(Some(SkipInfo {
            reason: rule
                .desc
                .clone()
                .unwrap_or_else(|| format!("skip: env ${var} is set")),
            ran_command: false,
        }));
    }

    if let Some(ref cmd) = rule.run
        && run_check_command(cmd, worktree)
    {
        return Ok(Some(SkipInfo {
            reason: rule
                .desc
                .clone()
                .unwrap_or_else(|| format!("skip: command succeeded: {cmd}")),
            ran_command: true,
        }));
    }

    if let Some(ref patterns) = rule.changed
        && any_changed_file_matches(patterns, changed, "skip")?
    {
        return Ok(Some(SkipInfo {
            reason: rule.desc.clone().unwrap_or_else(|| {
                format!(
                    "skip: changed files match {}",
                    patterns.as_slice().join(", ")
                )
            }),
            ran_command: false,
        }));
    }

    Ok(None)
}

/// Evaluate structured only rule.
///
/// Returns `Ok(Some(SkipInfo))` if any sub-condition is NOT met.
fn eval_structured_only(
    rule: &OnlyRuleStructured,
    worktree: &Path,
    changed: Option<&ChangedFilesProvider>,
) -> Result<Option<SkipInfo>> {
    if let Some(ref pattern) = rule.ref_pattern {
        let branch = current_ref(worktree).unwrap_or_default();
        if !branch_matches_pattern(&branch, pattern) {
            return Ok(Some(SkipInfo {
                reason: rule
                    .desc
                    .clone()
                    .unwrap_or_else(|| format!("only: ref does not match '{pattern}'")),
                ran_command: false,
            }));
        }
    }

    if let Some(ref var) = rule.env
        && !is_env_truthy(var)
    {
        return Ok(Some(SkipInfo {
            reason: rule
                .desc
                .clone()
                .unwrap_or_else(|| format!("only: env ${var} is not set")),
            ran_command: false,
        }));
    }

    if let Some(ref cmd) = rule.run
        && !run_check_command(cmd, worktree)
    {
        return Ok(Some(SkipInfo {
            reason: rule
                .desc
                .clone()
                .unwrap_or_else(|| format!("only: command failed: {cmd}")),
            ran_command: true,
        }));
    }

    if let Some(ref patterns) = rule.changed
        && !any_changed_file_matches(patterns, changed, "only")?
    {
        return Ok(Some(SkipInfo {
            reason: rule.desc.clone().unwrap_or_else(|| {
                format!(
                    "only: no changed files match {}",
                    patterns.as_slice().join(", ")
                )
            }),
            ran_command: false,
        }));
    }

    Ok(None)
}

/// Whether any of the operation's changed files matches `patterns`. A
/// `changed:` rule on a hook with no changed-file source, and a source that
/// fails to resolve, are both loud errors — a gate condition must never
/// silently degrade.
fn any_changed_file_matches(
    patterns: &StringOrList,
    changed: Option<&ChangedFilesProvider>,
    rule_kind: &str,
) -> Result<bool> {
    let Some(provider) = changed else {
        bail!(
            "`{rule_kind}:` rule uses `changed:`, but this hook type has no \
             changed-file source"
        );
    };
    let filter = FileFilter::new(patterns.as_slice(), &[])
        .with_context(|| format!("`{rule_kind}:` rule `changed:`"))?;
    Ok(provider.files()?.iter().any(|f| filter.matches(f)))
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
    matches!(probe_op_state(worktree), Some(state) if state.kind == OpKind::Merge)
}

/// Check if git is currently in a rebase state.
///
/// `git am` counts, as it always has here: it shares the apply backend's
/// directory, and a hook that wants to stay out of a rebase's way wants to
/// stay out of an am's way too.
fn is_in_rebase(worktree: &Path) -> bool {
    matches!(
        probe_op_state(worktree),
        Some(state) if matches!(state.kind, OpKind::Rebase | OpKind::Am)
    )
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

/// Check arch constraint for a job.
pub fn check_arch_constraint(job: &JobDef) -> Option<String> {
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
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_some());
    }

    #[test]
    fn test_skip_bool_false() {
        let cond = SkipCondition::Bool(false);
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_none());
    }

    #[test]
    fn test_skip_env_var_set() {
        unsafe {
            std::env::set_var("DAFT_TEST_SKIP_VAR", "1");
        }
        let cond = SkipCondition::EnvVar("DAFT_TEST_SKIP_VAR".to_string());
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_some());
        unsafe {
            std::env::remove_var("DAFT_TEST_SKIP_VAR");
        }
    }

    #[test]
    fn test_skip_env_var_unset() {
        unsafe {
            std::env::remove_var("DAFT_TEST_SKIP_NONEXIST");
        }
        let cond = SkipCondition::EnvVar("DAFT_TEST_SKIP_NONEXIST".to_string());
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_none());
    }

    #[test]
    fn test_only_bool_true() {
        let cond = OnlyCondition::Bool(true);
        assert!(
            should_only_skip(&cond, Path::new("."), None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_only_bool_false() {
        let cond = OnlyCondition::Bool(false);
        assert!(
            should_only_skip(&cond, Path::new("."), None)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn test_skip_run_command_succeeds() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            changed: None,
            run: Some("true".to_string()),
            desc: None,
        })]);
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_some());
    }

    #[test]
    fn test_skip_run_command_fails() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            changed: None,
            run: Some("false".to_string()),
            desc: None,
        })]);
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_none());
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
        unsafe {
            std::env::set_var("DAFT_TRUTHY_TEST", "1");
        }
        assert!(is_env_truthy("DAFT_TRUTHY_TEST"));

        unsafe {
            std::env::set_var("DAFT_TRUTHY_TEST", "0");
        }
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));

        unsafe {
            std::env::set_var("DAFT_TRUTHY_TEST", "false");
        }
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));

        unsafe {
            std::env::set_var("DAFT_TRUTHY_TEST", "");
        }
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));

        unsafe {
            std::env::remove_var("DAFT_TRUTHY_TEST");
        }
        assert!(!is_env_truthy("DAFT_TRUTHY_TEST"));
    }

    #[test]
    fn test_skip_rule_desc_override() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            changed: None,
            run: Some("true".to_string()),
            desc: Some("Brew is already installed".to_string()),
        })]);
        let info = should_skip(&cond, Path::new("."), None).unwrap().unwrap();
        assert_eq!(info.reason, "Brew is already installed");
        assert!(info.ran_command);
    }

    #[test]
    fn test_skip_rule_no_desc_uses_default() {
        let cond = SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            changed: None,
            run: Some("true".to_string()),
            desc: None,
        })]);
        let info = should_skip(&cond, Path::new("."), None).unwrap().unwrap();
        assert!(info.reason.starts_with("skip: command succeeded:"));
        assert!(info.ran_command);
    }

    #[test]
    fn test_only_rule_desc_override() {
        let cond = OnlyCondition::Rules(vec![OnlyRule::Structured(OnlyRuleStructured {
            ref_pattern: None,
            env: None,
            changed: None,
            run: Some("false".to_string()),
            desc: Some("Only when package.json exists".to_string()),
        })]);
        let info = should_only_skip(&cond, Path::new("."), None)
            .unwrap()
            .unwrap();
        assert_eq!(info.reason, "Only when package.json exists");
        assert!(info.ran_command);
    }

    #[test]
    fn test_should_skip_platform_matching_os() {
        use super::super::yaml_config::TargetOs;
        let current_os = match std::env::consts::OS {
            "macos" => TargetOs::Macos,
            "linux" => TargetOs::Linux,
            _ => return,
        };
        let mut map = std::collections::HashMap::new();
        map.insert(
            current_os,
            vec![SkipRule::Structured(SkipRuleStructured {
                ref_pattern: None,
                env: None,
                changed: None,
                run: Some("true".to_string()),
                desc: Some("already installed".to_string()),
            })],
        );
        let cond = SkipCondition::Platform(map);
        let info = should_skip(&cond, Path::new("."), None).unwrap().unwrap();
        assert_eq!(info.reason, "already installed");
    }

    #[test]
    fn test_should_skip_platform_non_matching_os() {
        use super::super::yaml_config::TargetOs;
        let other_os = if std::env::consts::OS == "macos" {
            TargetOs::Linux
        } else {
            TargetOs::Macos
        };
        let mut map = std::collections::HashMap::new();
        map.insert(
            other_os,
            vec![SkipRule::Structured(SkipRuleStructured {
                ref_pattern: None,
                env: None,
                changed: None,
                run: Some("true".to_string()),
                desc: Some("already installed".to_string()),
            })],
        );
        let cond = SkipCondition::Platform(map);
        assert!(should_skip(&cond, Path::new("."), None).unwrap().is_none());
    }

    /// Build a linked worktree — `.git` is a *file* pointing at the private
    /// gitdir, which is daft's default layout — and return (tempdir, worktree
    /// path, private gitdir).
    fn linked_worktree() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let private = tmp.path().join("repo/.git/worktrees/wt-a");
        std::fs::create_dir_all(&private).unwrap();
        let worktree = tmp.path().join("wt-a");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", private.display()),
        )
        .unwrap();
        (tmp, worktree, private)
    }

    /// Regression: the merge/rebase conditions used to join `<worktree>/.git`
    /// as a directory, so in a linked worktree — every daft worktree — the
    /// state files were invisible and the conditions silently read "no
    /// operation in progress".
    #[test]
    fn detects_merge_in_a_linked_worktree() {
        let (_tmp, worktree, private) = linked_worktree();
        std::fs::write(private.join("MERGE_HEAD"), "deadbeef\n").unwrap();

        let cond = SkipCondition::Rules(vec![SkipRule::Named("merge".to_string())]);
        assert!(
            should_skip(&cond, &worktree, None).unwrap().is_some(),
            "a paused merge in a linked worktree must satisfy `skip: [merge]`"
        );
    }

    #[test]
    fn detects_rebase_in_a_linked_worktree() {
        let (_tmp, worktree, private) = linked_worktree();
        std::fs::create_dir_all(private.join("rebase-merge")).unwrap();

        let cond = SkipCondition::Rules(vec![SkipRule::Named("rebase".to_string())]);
        assert!(
            should_skip(&cond, &worktree, None).unwrap().is_some(),
            "a paused rebase in a linked worktree must satisfy `skip: [rebase]`"
        );
    }

    #[test]
    fn idle_linked_worktree_matches_no_operation_condition() {
        let (_tmp, worktree, _private) = linked_worktree();

        for name in ["merge", "rebase"] {
            let cond = SkipCondition::Rules(vec![SkipRule::Named(name.to_string())]);
            assert!(
                should_skip(&cond, &worktree, None).unwrap().is_none(),
                "an idle worktree must not satisfy `skip: [{name}]`"
            );
        }
    }

    // ── changed: rules ──────────────────────────────────────────────────

    fn changed_provider(files: &[&str]) -> ChangedFilesProvider {
        ChangedFilesProvider::preresolved(files.iter().map(|s| s.to_string()).collect())
    }

    fn skip_changed(patterns: StringOrList) -> SkipCondition {
        SkipCondition::Rules(vec![SkipRule::Structured(SkipRuleStructured {
            ref_pattern: None,
            env: None,
            run: None,
            changed: Some(patterns),
            desc: None,
        })])
    }

    fn only_changed(patterns: StringOrList) -> OnlyCondition {
        OnlyCondition::Rules(vec![OnlyRule::Structured(OnlyRuleStructured {
            ref_pattern: None,
            env: None,
            run: None,
            changed: Some(patterns),
            desc: None,
        })])
    }

    #[test]
    fn skip_changed_rule_matches_changed_file() {
        let provider = changed_provider(&["migrations/001.sql", "src/lib.rs"]);
        let cond = skip_changed(StringOrList::Single("migrations/**".into()));
        let info = should_skip(&cond, Path::new("."), Some(&provider))
            .unwrap()
            .unwrap();
        assert_eq!(info.reason, "skip: changed files match migrations/**");
    }

    #[test]
    fn skip_changed_rule_passes_when_nothing_matches() {
        let provider = changed_provider(&["src/lib.rs"]);
        let cond = skip_changed(StringOrList::Single("migrations/**".into()));
        assert!(
            should_skip(&cond, Path::new("."), Some(&provider))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn only_changed_rule_skips_when_nothing_matches() {
        let provider = changed_provider(&["src/lib.rs"]);
        let cond = only_changed(StringOrList::List(vec!["docs/**".into(), "*.md".into()]));
        let info = should_only_skip(&cond, Path::new("."), Some(&provider))
            .unwrap()
            .unwrap();
        assert_eq!(info.reason, "only: no changed files match docs/**, *.md");
    }

    #[test]
    fn only_changed_rule_runs_when_a_file_matches() {
        let provider = changed_provider(&["docs/index.md"]);
        let cond = only_changed(StringOrList::Single("docs/**".into()));
        assert!(
            should_only_skip(&cond, Path::new("."), Some(&provider))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn changed_rule_without_a_source_is_a_loud_error() {
        let cond = skip_changed(StringOrList::Single("docs/**".into()));
        let err = should_skip(&cond, Path::new("."), None).unwrap_err();
        assert!(
            format!("{err:#}").contains("no changed-file source"),
            "{err:#}"
        );

        let cond = only_changed(StringOrList::Single("docs/**".into()));
        let err = should_only_skip(&cond, Path::new("."), None).unwrap_err();
        assert!(
            format!("{err:#}").contains("no changed-file source"),
            "{err:#}"
        );
    }
}
