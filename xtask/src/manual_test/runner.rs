//! Step executor, assertion checker, and non-interactive mode for the manual
//! test framework.
//!
//! Each step runs a shell command and optionally verifies a set of
//! expectations (exit code, file/directory existence, content checks, git
//! state). The non-interactive runner executes all steps sequentially and
//! reports pass/fail.

use anyhow::Result;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::executor::CommandExecutor;
use super::reporter::{FailingStep, Reporter, ScenarioStatus, StepReport};
use super::sandbox::Sandbox;
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
    /// Wall-clock duration of the scenario's step phase (excludes sandbox
    /// setup and cleanup). Surfaced on every footer and in the failed-
    /// scenarios block; the bench harness still reads it via the
    /// `DAFT_MANUAL_TEST_EMIT_TIMING` opt-in.
    pub duration: Duration,
    /// The first failing step (full detail), captured for the run summary.
    pub failing_step: Option<FailingStep>,
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
                    Some(format_diff_detail("expected", content, &data))
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
                    Some(format_diff_detail("unexpected", content, &data))
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

pub fn check_output_contains(output: &str, expected: &str) -> AssertionResult {
    let found = output.contains(expected);
    AssertionResult {
        passed: found,
        label: format!("Output contains \"{expected}\""),
        detail: if !found {
            Some(format_diff_detail("expected", expected, output))
        } else {
            None
        },
    }
}

pub fn check_output_not_contains(output: &str, unexpected: &str) -> AssertionResult {
    let found = output.contains(unexpected);
    AssertionResult {
        passed: !found,
        label: format!("Output not contains \"{unexpected}\""),
        detail: if found {
            Some(format_diff_detail("unexpected", unexpected, output))
        } else {
            None
        },
    }
}

/// Format an `expected: …` / `actual: …` block for substring assertions.
///
/// Single-line content stays on one line. Multi-line `actual` content is
/// rendered as `actual:` followed by each line indented. The reporter
/// owns the outer indent (per-line `    ` prefix), so this helper produces
/// content with no leading whitespace.
fn format_diff_detail(label: &str, needle: &str, actual: &str) -> String {
    let mut out = format!("{label}: {needle}\n");
    if actual.is_empty() {
        out.push_str("actual:   <empty>");
    } else if actual.contains('\n') {
        out.push_str("actual:\n");
        for line in actual.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        // Strip the trailing newline we just added so the reporter's per-line
        // emitter doesn't print a blank trailing row.
        if out.ends_with('\n') {
            out.pop();
        }
    } else {
        out.push_str("actual:   ");
        out.push_str(actual);
    }
    out
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
/// The `output` parameter is the combined stdout+stderr from the command,
/// used for `output_contains` / `output_not_contains` assertions.
pub fn run_assertions(
    expectations: &Expectations,
    exit_code: i32,
    cwd: &Path,
    sandbox: &Sandbox,
    output: Option<&str>,
) -> Vec<AssertionResult> {
    let mut results = Vec::new();

    let resolve = |raw: &str| -> String {
        let expanded = sandbox.expand_vars(raw);
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
        let expanded_content = sandbox.expand_vars(&fc.content);
        results.push(check_file_contains(&resolve(&fc.path), &expanded_content));
    }

    for fc in &expectations.file_not_contains {
        let expanded_content = sandbox.expand_vars(&fc.content);
        results.push(check_file_not_contains(
            &resolve(&fc.path),
            &expanded_content,
        ));
    }

    let output_str = output.unwrap_or("");
    for expected in &expectations.output_contains {
        let expanded = sandbox.expand_vars(expected);
        results.push(check_output_contains(output_str, &expanded));
    }

    for unexpected in &expectations.output_not_contains {
        let expanded = sandbox.expand_vars(unexpected);
        results.push(check_output_not_contains(output_str, &expanded));
    }

    for wt in &expectations.is_git_worktree {
        let expanded_branch = sandbox.expand_vars(&wt.branch);
        results.push(check_git_worktree(&resolve(&wt.dir), &expanded_branch));
    }

    for bc in &expectations.branch_exists {
        let expanded_branch = sandbox.expand_vars(&bc.branch);
        results.push(check_branch_exists(&resolve(&bc.repo), &expanded_branch));
    }

    results
}

// ---------------------------------------------------------------------------
// Step executor
// ---------------------------------------------------------------------------

/// Resolve the working directory for a step.
fn resolve_step_cwd(step: &Step, sandbox: &Sandbox) -> PathBuf {
    step.cwd
        .as_deref()
        .map(|c| {
            let expanded = PathBuf::from(sandbox.expand_vars(c));
            if expanded.is_absolute() {
                expanded
            } else {
                sandbox.work_dir.join(expanded)
            }
        })
        .unwrap_or_else(|| sandbox.work_dir.clone())
}

/// Execute a single test step and verify its expectations.
///
/// The command is dispatched through `executor`, which owns project-specific
/// concerns (env construction, binary resolution). Output is always captured;
/// when `quiet` is false, captured output is printed to the terminal after
/// execution so the user can see it.
pub fn execute_step(
    step: &Step,
    sandbox: &Sandbox,
    executor: &dyn CommandExecutor,
    quiet: bool,
) -> Result<StepResult> {
    let cwd = resolve_step_cwd(step, sandbox);
    let output = executor.execute(&step.run, &cwd, sandbox)?;
    let exit_code = output.exit_code;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !quiet {
        if !stdout.is_empty() {
            eprint!("{stdout}");
        }
        if !stderr.is_empty() {
            eprint!("{stderr}");
        }
    }

    let combined = combine_captured(&Some(stdout.clone()), &Some(stderr.clone()));
    let assertions = step
        .expect
        .as_ref()
        .map(|e| run_assertions(e, exit_code, &cwd, sandbox, Some(&combined)))
        .unwrap_or_default();

    let all_passed = assertions.iter().all(|a| a.passed);

    Ok(StepResult {
        exit_code,
        assertions,
        all_passed,
        stdout: Some(stdout),
        stderr: Some(stderr),
    })
}

/// Execute only the command part of a step (no assertions).
///
/// Returns `(exit_code, combined_output)`. Output is always captured and printed
/// to the terminal. Used by interactive mode where checks are optional.
pub fn run_step_command(
    step: &Step,
    sandbox: &Sandbox,
    executor: &dyn CommandExecutor,
) -> Result<(i32, String)> {
    let cwd = resolve_step_cwd(step, sandbox);
    let output = executor.execute(&step.run, &cwd, sandbox)?;
    let exit_code = output.exit_code;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // Print to terminal so user sees what happened.
    if !stdout.is_empty() {
        eprint!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }

    let combined = combine_captured(&Some(stdout), &Some(stderr));

    Ok((exit_code, combined))
}

/// Run only the assertions for a step given an exit code and captured output.
///
/// Used by interactive mode where checks are triggered explicitly.
pub fn check_step(
    step: &Step,
    exit_code: i32,
    sandbox: &Sandbox,
    output: Option<&str>,
) -> Vec<AssertionResult> {
    let cwd = resolve_step_cwd(step, sandbox);
    step.expect
        .as_ref()
        .map(|e| run_assertions(e, exit_code, &cwd, sandbox, output))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Non-interactive runner
// ---------------------------------------------------------------------------

/// Run all steps in a scenario sequentially, emitting through `reporter`.
///
/// Command output is always captured; the reporter decides whether/how to
/// render it based on verbosity. The first failing step's full detail is
/// captured into the returned [`ScenarioResult`] so the orchestrator can
/// surface it in the end-of-run summary block.
pub fn run_non_interactive(
    scenario: &Scenario,
    sandbox: &Sandbox,
    executor: &dyn CommandExecutor,
    reporter: &dyn Reporter,
    out: &mut impl Write,
) -> Result<ScenarioResult> {
    reporter.scenario_header(out, scenario)?;

    let total = scenario.steps.len();
    let mut passed = 0;
    let mut failed = 0;
    let mut failing_step: Option<FailingStep> = None;

    let started = Instant::now();
    for (i, step) in scenario.steps.iter().enumerate() {
        reporter.step_start(out, i, total, step)?;

        let result = execute_step(step, sandbox, executor, true)?;
        let expanded = sandbox.expand_vars(&step.run);
        let report = StepReport {
            expanded_command: Some(&expanded),
            assertions: &result.assertions,
            stdout: result.stdout.as_deref(),
            stderr: result.stderr.as_deref(),
        };

        if result.all_passed {
            reporter.step_pass(out, &report)?;
            passed += 1;
        } else {
            reporter.step_fail(out, &report)?;
            if failing_step.is_none() {
                failing_step = Some(snapshot_failing_step(&result, step, i, total));
            }
            failed += 1;
        }
    }
    let duration = started.elapsed();

    let status = if failed == 0 {
        ScenarioStatus::Pass
    } else {
        ScenarioStatus::Fail
    };
    reporter.scenario_footer(out, scenario, status, duration)?;

    Ok(ScenarioResult {
        steps: total,
        passed,
        failed,
        duration,
        failing_step,
    })
}

/// Copy the first failing step's detail into an owned snapshot the summary
/// block can render even after the per-scenario buffers are dropped.
fn snapshot_failing_step(
    result: &StepResult,
    step: &Step,
    index: usize,
    total: usize,
) -> FailingStep {
    let failed_assertions = result
        .assertions
        .iter()
        .filter(|a| !a.passed)
        .map(|a| AssertionResult {
            passed: a.passed,
            label: a.label.clone(),
            detail: a.detail.clone(),
        })
        .collect();
    FailingStep {
        index,
        total,
        step_name: step.name.clone(),
        line: step.line,
        failed_assertions,
        captured_stdout: trim_or_empty(&result.stdout),
        captured_stderr: trim_or_empty(&result.stderr),
    }
}

/// Trim trailing whitespace, returning an empty string when the source is
/// missing. Kept separate from `combine_captured` so the FailingStep snapshot
/// can preserve the stdout/stderr split for the summary block.
fn trim_or_empty(s: &Option<String>) -> String {
    s.as_ref().map(|v| v.trim().to_string()).unwrap_or_default()
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

        let sandbox = Sandbox::new_with_vars(std::collections::HashMap::new());

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
            output_contains: vec![],
            output_not_contains: vec![],
            is_git_worktree: vec![],
            branch_exists: vec![],
        };

        let results = run_assertions(&expectations, 0, dir.path(), &sandbox, None);
        for r in &results {
            assert!(r.passed, "assertion failed: {}", r.label);
        }
    }

    #[test]
    fn test_check_output_contains_pass() {
        let r = check_output_contains("hello world", "hello");
        assert!(r.passed);
    }

    #[test]
    fn test_check_output_contains_fail() {
        let r = check_output_contains("hello world", "goodbye");
        assert!(!r.passed);
        assert!(r.detail.is_some());
    }

    #[test]
    fn test_check_output_not_contains_pass() {
        let r = check_output_not_contains("hello world", "goodbye");
        assert!(r.passed);
    }

    #[test]
    fn test_check_output_not_contains_fail() {
        let r = check_output_not_contains("hello world", "hello");
        assert!(!r.passed);
        assert!(r.detail.is_some());
    }

    // -----------------------------------------------------------------------
    // FakeExecutor — proves the seam holds.
    //
    // The runner core compiles and runs against an executor that has nothing
    // to do with daft. If this test ever stops compiling because daft types
    // crept back into `runner.rs`, the seam was breached. Removing or
    // gutting this test is the wrong fix — restore the seam.
    // -----------------------------------------------------------------------

    use super::super::executor::{CommandExecutor, CommandOutput};
    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// In-memory executor that records invocations and returns canned outputs.
    /// Knows nothing about daft.
    struct FakeExecutor {
        responses: Mutex<VecDeque<CommandOutput>>,
        invocations: Mutex<Vec<String>>,
        cwds: Mutex<Vec<PathBuf>>,
    }

    impl FakeExecutor {
        fn new(responses: Vec<CommandOutput>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                invocations: Mutex::new(Vec::new()),
                cwds: Mutex::new(Vec::new()),
            }
        }

        fn invocations(&self) -> Vec<String> {
            self.invocations.lock().unwrap().clone()
        }

        fn cwds(&self) -> Vec<PathBuf> {
            self.cwds.lock().unwrap().clone()
        }
    }

    impl CommandExecutor for FakeExecutor {
        fn execute(&self, command: &str, cwd: &Path, sandbox: &Sandbox) -> Result<CommandOutput> {
            // Mirror what a real adapter does: expand variables *before*
            // recording, so the test sees what the user-facing command would
            // actually have executed.
            let expanded = sandbox.expand_vars(command);
            self.invocations.lock().unwrap().push(expanded);
            self.cwds.lock().unwrap().push(cwd.to_path_buf());
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default())
        }
    }

    fn make_step(name: &str, run: &str) -> Step {
        Step {
            name: name.to_string(),
            run: run.to_string(),
            cwd: None,
            expect: None,
            line: None,
        }
    }

    #[test]
    fn execute_step_runs_command_through_executor() {
        let sandbox = Sandbox::new_with_vars(HashMap::new());
        let executor = FakeExecutor::new(vec![CommandOutput {
            exit_code: 0,
            stdout: b"hello\n".to_vec(),
            stderr: Vec::new(),
        }]);

        let step = make_step("greet", "echo hello");
        let result = execute_step(&step, &sandbox, &executor, true).unwrap();

        assert_eq!(result.exit_code, 0);
        assert!(result.all_passed); // no assertions defined → trivially true
        assert_eq!(executor.invocations(), vec!["echo hello"]);
    }

    #[test]
    fn execute_step_expands_sandbox_vars_in_command() {
        let mut vars = HashMap::new();
        vars.insert("GREETING".into(), "hi".into());
        let sandbox = Sandbox::new_with_vars(vars);
        let executor = FakeExecutor::new(vec![CommandOutput::default()]);

        let step = make_step("greet", "echo $GREETING");
        let _ = execute_step(&step, &sandbox, &executor, true).unwrap();

        // Adapter is responsible for expansion; FakeExecutor mirrors that
        // contract so the recorded invocation matches what `bash -c` would see.
        assert_eq!(executor.invocations(), vec!["echo hi"]);
    }

    #[test]
    fn run_step_command_returns_exit_code_and_combined_output() {
        let sandbox = Sandbox::new_with_vars(HashMap::new());
        let executor = FakeExecutor::new(vec![CommandOutput {
            exit_code: 2,
            stdout: b"out".to_vec(),
            stderr: b"err".to_vec(),
        }]);

        let step = make_step("fail", "false");
        let (code, combined) = run_step_command(&step, &sandbox, &executor).unwrap();
        assert_eq!(code, 2);
        assert!(combined.contains("out"));
        assert!(combined.contains("err"));
    }

    /// Regression test for the cwd plumbing fix in 8a3ce9b2.
    ///
    /// The first cut of the port took only `(command, sandbox)`, so
    /// `step.cwd` was resolved by the runner core but never reached the
    /// adapter — every step ran in `sandbox.work_dir`, breaking any scenario
    /// (e.g. `clone/all-branches.yml`) whose later steps `cd` into a worktree
    /// subdirectory before running `git`. The fix added `cwd: &Path` to the
    /// trait; this test holds the contract at the unit-test layer so it
    /// regresses immediately rather than only when the full YAML suite is run.
    #[test]
    fn execute_step_passes_resolved_cwd_to_executor() {
        let sandbox = Sandbox::new_with_vars(HashMap::new());
        let executor = FakeExecutor::new(vec![CommandOutput::default()]);

        let mut step = make_step("cwd-honored", "true");
        step.cwd = Some("sub/dir".to_string());

        execute_step(&step, &sandbox, &executor, true).unwrap();

        let cwds = executor.cwds();
        assert_eq!(cwds.len(), 1);
        // Relative cwd is resolved against sandbox.work_dir (the dummy
        // `new_with_vars` value is `/tmp/test-dummy/work`), not the process
        // cwd. Asserting the trailing segments avoids coupling to the dummy.
        assert!(
            cwds[0].ends_with("sub/dir"),
            "executor received cwd {:?}, expected suffix sub/dir",
            cwds[0],
        );
    }

    #[test]
    fn run_step_command_passes_resolved_cwd_to_executor() {
        let sandbox = Sandbox::new_with_vars(HashMap::new());
        let executor = FakeExecutor::new(vec![CommandOutput::default()]);

        let mut step = make_step("cwd-honored", "true");
        step.cwd = Some("other/place".to_string());

        run_step_command(&step, &sandbox, &executor).unwrap();

        let cwds = executor.cwds();
        assert_eq!(cwds.len(), 1);
        assert!(
            cwds[0].ends_with("other/place"),
            "executor received cwd {:?}, expected suffix other/place",
            cwds[0],
        );
    }
}
