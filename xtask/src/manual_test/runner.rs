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
}

// ---------------------------------------------------------------------------
// Individual assertion functions
// ---------------------------------------------------------------------------

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
                    Some(format!("substring \"{content}\" not found in file"))
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

pub fn check_git_worktree(dir: &str, branch: &str) -> AssertionResult {
    let label = format!("Git worktree on branch \"{branch}\": {dir}");

    let output = std::process::Command::new("git")
        .args(["-C", dir, "rev-parse", "--abbrev-ref", "HEAD"])
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
                "git rev-parse failed: {}",
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

/// Execute a single test step and verify its expectations.
pub fn execute_step(step: &Step, env: &TestEnv) -> Result<StepResult> {
    let expanded_cmd = env.expand_vars(&step.run);
    let cwd = step
        .cwd
        .as_deref()
        .map(|c| PathBuf::from(env.expand_vars(c)))
        .unwrap_or_else(|| env.work_dir.clone());

    // Run with inherited stdio so output passes through to the terminal.
    let status = std::process::Command::new("bash")
        .args(["-c", &expanded_cmd])
        .current_dir(&cwd)
        .envs(env.command_env())
        .status()
        .with_context(|| format!("Failed to execute: {expanded_cmd}"))?;

    let exit_code = status.code().unwrap_or(-1);

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
    })
}

// ---------------------------------------------------------------------------
// Non-interactive runner
// ---------------------------------------------------------------------------

/// Run all steps in a scenario sequentially, printing a test report to stderr.
///
/// Returns `Ok(())` when all assertions pass, or an error describing how many
/// assertions failed.
pub fn run_non_interactive(scenario: &Scenario, env: &TestEnv) -> Result<()> {
    use daft::styles;

    eprintln!();
    eprintln!(
        "  {} ({})",
        styles::bold(&scenario.name),
        styles::dim(&format!("{} steps", scenario.steps.len()))
    );
    eprintln!("  {}", "\u{2500}".repeat(40));

    let mut total = 0;
    let mut passed = 0;
    let mut failed = 0;

    for (i, step) in scenario.steps.iter().enumerate() {
        eprint!(
            "  [{}] {} ... ",
            styles::dim(&format!("{}/{}", i + 1, scenario.steps.len())),
            &step.name
        );

        let result = execute_step(step, env)?;

        if result.all_passed {
            eprintln!("{}", styles::green("ok"));
            passed += 1;
        } else {
            eprintln!("{}", styles::red("FAIL"));
            for a in &result.assertions {
                if !a.passed {
                    eprintln!("    {} {}", styles::red("x"), a.label);
                    if let Some(detail) = &a.detail {
                        eprintln!("      {}", styles::dim(detail));
                    }
                }
            }
            failed += 1;
        }
        total += 1;
    }

    eprintln!();
    eprintln!(
        "  {} total, {} passed, {} failed",
        total,
        styles::green(&passed.to_string()),
        if failed > 0 {
            styles::red(&failed.to_string())
        } else {
            failed.to_string()
        }
    );

    if failed > 0 {
        anyhow::bail!("{failed} assertion(s) failed");
    }
    Ok(())
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
            is_git_worktree: vec![],
            branch_exists: vec![],
        };

        let results = run_assertions(&expectations, 0, dir.path(), &env);
        for r in &results {
            assert!(r.passed, "assertion failed: {}", r.label);
        }
    }
}
