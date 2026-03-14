//! Step executor, assertion checker, and non-interactive mode for the manual
//! test framework.
//!
//! Each step runs a shell command and optionally verifies a set of
//! expectations (exit code, file/directory existence, content checks, git
//! state). The non-interactive runner executes all steps sequentially and
//! reports pass/fail.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::env::TestEnv;
use super::schema::{Expectations, Scenario, Step};

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// Outcome of a single assertion check.
pub struct AssertionResult {
    /// Whether the assertion passed.
    pub passed: bool,
    /// Human-readable label describing what was checked.
    pub label: String,
    /// Optional detail shown on failure (e.g. expected vs actual).
    pub detail: Option<String>,
}

/// Outcome of executing a single step.
pub struct StepResult {
    /// Process exit code (`-1` if the process was killed by a signal).
    #[allow(dead_code)]
    pub exit_code: i32,
    /// All assertion results for this step.
    pub assertions: Vec<AssertionResult>,
    /// `true` when every assertion passed (or there were none).
    pub all_passed: bool,
    /// Captured stdout (only in quiet mode).
    pub stdout: Option<String>,
    /// Captured stderr (only in quiet mode).
    pub stderr: Option<String>,
}

/// Outcome of running a full scenario.
pub struct ScenarioResult {
    /// Number of steps executed.
    pub steps: usize,
    /// Number of steps where all assertions passed.
    pub passed: usize,
    /// Number of steps with at least one failed assertion.
    pub failed: usize,
}

// ---------------------------------------------------------------------------
// Individual assertion functions
// ---------------------------------------------------------------------------

/// Truncate content for display in failure messages.
/// Escapes newlines and trims to `max_len` characters.
fn truncate_content(s: &str, max_len: usize) -> String {
    let escaped = s.replace('\n', "\\n").replace('\r', "\\r");
    if escaped.len() <= max_len {
        escaped
    } else {
        format!("{}...", &escaped[..max_len])
    }
}

pub fn check_exit_code(actual: i32, expected: i32) -> AssertionResult {
    AssertionResult {
        passed: actual == expected,
        label: format!("Exit code: expected {expected}, got {actual}"),
        detail: if actual != expected {
            Some(format!("expected {expected}, got {actual}"))
        } else {
            None
        },
    }
}

pub fn check_dir_exists(path: &str) -> AssertionResult {
    let exists = Path::new(path).is_dir();
    AssertionResult {
        passed: exists,
        label: format!("Directory exists: {path}"),
        detail: if !exists {
            Some("directory not found".into())
        } else {
            None
        },
    }
}

pub fn check_file_exists(path: &str) -> AssertionResult {
    let exists = Path::new(path).is_file();
    AssertionResult {
        passed: exists,
        label: format!("File exists: {path}"),
        detail: if !exists {
            Some("file not found".into())
        } else {
            None
        },
    }
}

pub fn check_file_not_exists(path: &str) -> AssertionResult {
    let exists = Path::new(path).exists();
    AssertionResult {
        passed: !exists,
        label: format!("File not exists: {path}"),
        detail: if exists {
            Some("path unexpectedly exists".into())
        } else {
            None
        },
    }
}

pub fn check_file_contains(path: &str, content: &str) -> AssertionResult {
    match std::fs::read_to_string(path) {
        Ok(data) => {
            let found = data.contains(content);
            AssertionResult {
                passed: found,
                label: format!("File contains \"{content}\": {path}"),
                detail: if !found {
                    Some(format!(
                        "expected: \"{content}\"\n        actual: \"{}\"",
                        truncate_content(&data, 200)
                    ))
                } else {
                    None
                },
            }
        }
        Err(e) => AssertionResult {
            passed: false,
            label: format!("File contains \"{content}\": {path}"),
            detail: Some(format!("could not read file: {e}")),
        },
    }
}

pub fn check_file_not_contains(path: &str, content: &str) -> AssertionResult {
    match std::fs::read_to_string(path) {
        Ok(data) => {
            let found = data.contains(content);
            AssertionResult {
                passed: !found,
                label: format!("File not contains \"{content}\": {path}"),
                detail: if found {
                    Some(format!(
                        "unexpected: \"{content}\"\n        actual: \"{}\"",
                        truncate_content(&data, 200)
                    ))
                } else {
                    None
                },
            }
        }
        Err(e) => AssertionResult {
            passed: false,
            label: format!("File not contains \"{content}\": {path}"),
            detail: Some(format!("could not read file: {e}")),
        },
    }
}

pub fn check_git_worktree(dir: &str, branch: &str) -> AssertionResult {
    let label = format!("Git worktree on branch \"{branch}\": {dir}");

    let output = std::process::Command::new("git")
        .args(["-C", dir, "branch", "--show-current"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let actual = String::from_utf8_lossy(&out.stdout).trim().to_string();
            let passed = actual == branch;
            AssertionResult {
                passed,
                label,
                detail: if !passed {
                    Some(format!("expected branch \"{branch}\", got \"{actual}\""))
                } else {
                    None
                },
            }
        }
        Ok(out) => AssertionResult {
            passed: false,
            label,
            detail: Some(format!(
                "git branch --show-current failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        },
        Err(e) => AssertionResult {
            passed: false,
            label,
            detail: Some(format!("failed to run git: {e}")),
        },
    }
}

pub fn check_branch_exists(repo: &str, branch: &str) -> AssertionResult {
    let label = format!("Branch \"{branch}\" exists in {repo}");

    // Use --git-dir if repo points to a bare repo, otherwise -C.
    // Try -C first — it works for both bare and non-bare repos when
    // pointing at the top-level directory.
    let output = std::process::Command::new("git")
        .args(["-C", repo, "branch", "--list", branch])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let found = !stdout.trim().is_empty();
            AssertionResult {
                passed: found,
                label,
                detail: if !found {
                    Some(format!("branch \"{branch}\" not found in repo"))
                } else {
                    None
                },
            }
        }
        Ok(out) => AssertionResult {
            passed: false,
            label,
            detail: Some(format!(
                "git branch --list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
        },
        Err(e) => AssertionResult {
            passed: false,
            label,
            detail: Some(format!("failed to run git: {e}")),
        },
    }
}

// ---------------------------------------------------------------------------
// Aggregate assertion runner
// ---------------------------------------------------------------------------

/// Run all assertions defined in `expectations` and return the results.
///
/// Relative paths (not starting with `/`) are resolved against `cwd`.
pub fn run_assertions(
    expectations: &Expectations,
    exit_code: i32,
    cwd: &Path,
    env: &TestEnv,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    let resolve = |raw: &str| -> String {
        let expanded = env.expand_vars(raw);
        if expanded.starts_with('/') {
            expanded
        } else {
            cwd.join(&expanded).to_string_lossy().into_owned()
        }
    };

    if let Some(expected) = expectations.exit_code {
        results.push(check_exit_code(exit_code, expected));
    }

    for dir in &expectations.dirs_exist {
        results.push(check_dir_exists(&resolve(dir)));
    }

    for file in &expectations.files_exist {
        results.push(check_file_exists(&resolve(file)));
    }

    for file in &expectations.files_not_exist {
        results.push(check_file_not_exists(&resolve(file)));
    }

    for fc in &expectations.file_contains {
        let expanded_content = env.expand_vars(&fc.content);
        results.push(check_file_contains(&resolve(&fc.path), &expanded_content));
    }

    for fc in &expectations.file_not_contains {
        let expanded_content = env.expand_vars(&fc.content);
        results.push(check_file_not_contains(
            &resolve(&fc.path),
            &expanded_content,
        ));
    }

    for wt in &expectations.is_git_worktree {
        let expanded_branch = env.expand_vars(&wt.branch);
        results.push(check_git_worktree(&resolve(&wt.dir), &expanded_branch));
    }

    for bc in &expectations.branch_exists {
        let expanded_branch = env.expand_vars(&bc.branch);
        results.push(check_branch_exists(&resolve(&bc.repo), &expanded_branch));
    }

    results
}

// ---------------------------------------------------------------------------
// Step executor
// ---------------------------------------------------------------------------

/// Resolve the working directory for a step.
fn resolve_step_cwd(step: &Step, env: &TestEnv) -> PathBuf {
    step.cwd
        .as_deref()
        .map(|c| {
            let expanded = PathBuf::from(env.expand_vars(c));
            if expanded.is_absolute() {
                expanded
            } else {
                env.work_dir.join(expanded)
            }
        })
        .unwrap_or_else(|| env.work_dir.clone())
}

/// Execute a single test step and verify its expectations.
///
/// When `quiet` is true, stdout/stderr are captured instead of inherited.
/// The captured output is stored in the result for display on failure.
pub fn execute_step(step: &Step, env: &TestEnv, quiet: bool) -> Result<StepResult> {
    let expanded_cmd = env.expand_vars(&step.run);
    let cwd = resolve_step_cwd(step, env);

    let (exit_code, stdout, stderr) = if quiet {
        let output = std::process::Command::new("bash")
            .args(["-c", &expanded_cmd])
            .current_dir(&cwd)
            .envs(env.command_env())
            .output()
            .with_context(|| format!("Failed to execute: {expanded_cmd}"))?;
        let code = output.status.code().unwrap_or(-1);
        let out = String::from_utf8_lossy(&output.stdout).into_owned();
        let err = String::from_utf8_lossy(&output.stderr).into_owned();
        (code, Some(out), Some(err))
    } else {
        let status = std::process::Command::new("bash")
            .args(["-c", &expanded_cmd])
            .current_dir(&cwd)
            .envs(env.command_env())
            .status()
            .with_context(|| format!("Failed to execute: {expanded_cmd}"))?;
        (status.code().unwrap_or(-1), None, None)
    };

    let assertions = step
        .expect
        .as_ref()
        .map(|e| run_assertions(e, exit_code, &cwd, env))
        .unwrap_or_default();

    let all_passed = assertions.iter().all(|a| a.passed);

    Ok(StepResult {
        exit_code,
        assertions,
        all_passed,
        stdout,
        stderr,
    })
}

/// Execute only the command part of a step (no assertions).
///
/// Returns the exit code. Used by interactive mode where checks are optional.
pub fn run_step_command(step: &Step, env: &TestEnv) -> Result<i32> {
    let expanded_cmd = env.expand_vars(&step.run);
    let cwd = resolve_step_cwd(step, env);

    let status = std::process::Command::new("bash")
        .args(["-c", &expanded_cmd])
        .current_dir(&cwd)
        .envs(env.command_env())
        .status()
        .with_context(|| format!("Failed to execute: {expanded_cmd}"))?;

    Ok(status.code().unwrap_or(-1))
}

/// Run only the assertions for a step given an exit code.
///
/// Used by interactive mode where checks are triggered explicitly.
pub fn check_step(step: &Step, exit_code: i32, env: &TestEnv) -> Vec<AssertionResult> {
    let cwd = resolve_step_cwd(step, env);
    step.expect
        .as_ref()
        .map(|e| run_assertions(e, exit_code, &cwd, env))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Non-interactive runner
// ---------------------------------------------------------------------------

/// Run all steps in a scenario sequentially, printing a concise test report.
///
/// Command output is captured (not shown) unless a step fails, in which case
/// captured output is printed for debugging. Returns a [`ScenarioResult`] with
/// pass/fail counts.
pub fn run_non_interactive(
    scenario: &Scenario,
    env: &TestEnv,
    verbose: bool,
) -> Result<ScenarioResult> {
    use daft::styles;

    eprintln!("{}", styles::cyan(&scenario.name));

    let mut passed = 0;
    let mut failed = 0;

    for (i, step) in scenario.steps.iter().enumerate() {
        eprint!(
            "{} {} ... ",
            styles::dim(&format!("[{}/{}]", i + 1, scenario.steps.len())),
            &step.name
        );

        let result = execute_step(step, env, true)?;

        if result.all_passed {
            let check_count = result.assertions.len();
            if check_count > 0 {
                eprintln!(
                    "{} {}",
                    styles::green("ok"),
                    styles::dim(&format!("({check_count} checks)"))
                );
            } else {
                eprintln!("{}", styles::green("ok"));
            }
            if verbose {
                for a in &result.assertions {
                    eprintln!("  {} {}", styles::green("✓"), styles::dim(&a.label));
                }
            }
            passed += 1;
        } else {
            let fail_count = result.assertions.iter().filter(|a| !a.passed).count();
            eprintln!(
                "{} {}",
                styles::red("FAIL"),
                styles::dim(&format!("({fail_count} failed)"))
            );
            for a in &result.assertions {
                if !a.passed {
                    eprintln!("  {} {}", styles::red("x"), a.label);
                    if let Some(detail) = &a.detail {
                        eprintln!("    {}", styles::dim(detail));
                    }
                }
            }
            // Show captured output for debugging.
            let captured = combine_captured(&result.stdout, &result.stderr);
            if !captured.is_empty() {
                eprintln!("  {}", styles::dim("--- captured output ---"));
                for line in captured.lines().take(20) {
                    eprintln!("  {}", styles::dim(line));
                }
            }
            failed += 1;
        }
    }

    Ok(ScenarioResult {
        steps: scenario.steps.len(),
        passed,
        failed,
    })
}

/// Combine captured stdout/stderr, trimming trailing whitespace.
fn combine_captured(stdout: &Option<String>, stderr: &Option<String>) -> String {
    let mut parts = Vec::new();
    if let Some(out) = stdout {
        let trimmed = out.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }
    if let Some(err) = stderr {
        let trimmed = err.trim();
        if !trimmed.is_empty() {
            parts.push(trimmed.to_string());
        }
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_exit_code_pass() {
        let r = check_exit_code(0, 0);
        assert!(r.passed);
    }

    #[test]
    fn test_check_exit_code_fail() {
        let r = check_exit_code(1, 0);
        assert!(!r.passed);
        assert!(r.detail.is_some());
    }

    #[test]
    fn test_check_dir_exists_pass() {
        let dir = tempfile::tempdir().unwrap();
        let r = check_dir_exists(dir.path().to_str().unwrap());
        assert!(r.passed);
    }

    #[test]
    fn test_check_dir_exists_fail() {
        let r = check_dir_exists("/tmp/nonexistent-dir-xyzzy-12345");
        assert!(!r.passed);
    }

    #[test]
    fn test_check_file_exists_pass() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();
        let r = check_file_exists(file.to_str().unwrap());
        assert!(r.passed);
    }

    #[test]
    fn test_check_file_exists_fail() {
        let r = check_file_exists("/tmp/nonexistent-file-xyzzy-12345");
        assert!(!r.passed);
    }

    #[test]
    fn test_check_file_not_exists_pass() {
        let r = check_file_not_exists("/tmp/nonexistent-file-xyzzy-12345");
        assert!(r.passed);
    }

    #[test]
    fn test_check_file_not_exists_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("exists.txt");
        std::fs::write(&file, "hi").unwrap();
        let r = check_file_not_exists(file.to_str().unwrap());
        assert!(!r.passed);
    }

    #[test]
    fn test_check_file_contains_pass() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();
        let r = check_file_contains(file.to_str().unwrap(), "hello");
        assert!(r.passed);
    }

    #[test]
    fn test_check_file_contains_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();
        let r = check_file_contains(file.to_str().unwrap(), "goodbye");
        assert!(!r.passed);
    }

    #[test]
    fn test_check_file_not_contains_pass() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();
        let r = check_file_not_contains(file.to_str().unwrap(), "goodbye");
        assert!(r.passed);
    }

    #[test]
    fn test_check_file_not_contains_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello world").unwrap();
        let r = check_file_not_contains(file.to_str().unwrap(), "hello");
        assert!(!r.passed);
    }

    #[test]
    fn test_check_file_not_contains_missing_file() {
        let r = check_file_not_contains("/tmp/nonexistent-file-xyzzy-12345", "anything");
        assert!(!r.passed);
        assert!(r.detail.unwrap().contains("could not read file"));
    }

    #[test]
    fn test_check_file_contains_missing_file() {
        let r = check_file_contains("/tmp/nonexistent-file-xyzzy-12345", "anything");
        assert!(!r.passed);
        assert!(r.detail.unwrap().contains("could not read file"));
    }

    #[test]
    fn test_run_assertions_resolves_relative_paths() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("file.txt"), "data").unwrap();

        let env = TestEnv::new_with_vars(std::collections::HashMap::new());

        let expectations = Expectations {
            exit_code: Some(0),
            dirs_exist: vec!["subdir".into()],
            files_exist: vec!["subdir/file.txt".into()],
            files_not_exist: vec!["subdir/missing.txt".into()],
            file_contains: vec![super::super::schema::FileContains {
                path: "subdir/file.txt".into(),
                content: "data".into(),
            }],
            file_not_contains: vec![super::super::schema::FileNotContains {
                path: "subdir/file.txt".into(),
                content: "missing".into(),
            }],
            is_git_worktree: vec![],
            branch_exists: vec![],
        };

        let results = run_assertions(&expectations, 0, dir.path(), &env);
        for r in &results {
            assert!(r.passed, "assertion failed: {}", r.label);
        }
    }
}
