//! Reporter unit tests. ANSI escapes are stripped from captured output so
//! assertions remain readable; format-only tweaks to terminal styles don't
//! cascade into churn here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::pretty::PrettyReporter;
use super::quiet::QuietReporter;
use super::{
    reproduce_token, FailedScenarioRecord, FailingStep, Reporter, RunSummary, ScenarioErrorRecord,
    ScenarioStatus, StepReport, Verbosity,
};
use crate::manual_test::runner::AssertionResult;
use crate::manual_test::schema::{Expectations, Scenario, Step};

// ---------------------------------------------------------------------------
// Verbosity + reproduce_token
// ---------------------------------------------------------------------------

#[test]
fn verbosity_from_flags_quiet_wins() {
    assert_eq!(Verbosity::from_flags(0, true), Verbosity::Quiet);
    assert_eq!(Verbosity::from_flags(2, true), Verbosity::Quiet);
}

#[test]
fn verbosity_from_flags_ladder() {
    assert_eq!(Verbosity::from_flags(0, false), Verbosity::Default);
    assert_eq!(Verbosity::from_flags(1, false), Verbosity::Verbose);
    assert_eq!(Verbosity::from_flags(2, false), Verbosity::VeryVerbose);
    assert_eq!(Verbosity::from_flags(7, false), Verbosity::VeryVerbose);
}

#[test]
fn reproduce_token_strips_scenarios_dir_and_extension() {
    let scenarios = Path::new("tests/manual/scenarios");
    let source = Path::new("tests/manual/scenarios/hooks/silent-job-logs.yml");
    assert_eq!(reproduce_token(source, scenarios), "hooks:silent-job-logs");
}

#[test]
fn reproduce_token_for_top_level_scenario() {
    let scenarios = Path::new("tests/manual/scenarios");
    let source = Path::new("tests/manual/scenarios/clone-basic.yml");
    assert_eq!(reproduce_token(source, scenarios), "clone-basic");
}

#[test]
fn reproduce_token_handles_yaml_extension() {
    let scenarios = Path::new("tests/manual/scenarios");
    let source = Path::new("tests/manual/scenarios/checkout-branch/carry-flag.yaml");
    assert_eq!(
        reproduce_token(source, scenarios),
        "checkout-branch:carry-flag"
    );
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Strip ANSI CSI sequences so golden-byte assertions are readable.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for ch in chars.by_ref() {
                if ch.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn step(name: &str, run: &str) -> Step {
    Step {
        name: name.to_string(),
        run: run.to_string(),
        cwd: None,
        expect: Some(Expectations::default()),
    }
}

fn scenario(name: &str, source: &str, steps: Vec<Step>) -> Scenario {
    Scenario {
        name: name.to_string(),
        description: None,
        repos: Vec::new(),
        env: Default::default(),
        steps,
        source_path: PathBuf::from(source),
    }
}

fn assertion(passed: bool, label: &str, detail: Option<&str>) -> AssertionResult {
    AssertionResult {
        passed,
        label: label.to_string(),
        detail: detail.map(|s| s.to_string()),
    }
}

fn render<F>(f: F) -> String
where
    F: FnOnce(&mut Vec<u8>),
{
    let mut buf = Vec::new();
    f(&mut buf);
    strip_ansi(&String::from_utf8(buf).expect("reporter output is valid UTF-8"))
}

// ---------------------------------------------------------------------------
// PrettyReporter — per-scenario emission
// ---------------------------------------------------------------------------

#[test]
fn pretty_default_scenario_header_includes_source_path() {
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("my-scenario", "tests/manual/scenarios/foo/bar.yml", vec![]);
    let out = render(|buf| r.scenario_header(buf, &sc).unwrap());
    assert!(out.contains("my-scenario"));
    assert!(out.contains("at tests/manual/scenarios/foo/bar.yml"));
}

#[test]
fn pretty_default_step_pass_line_with_check_count() {
    let r = PrettyReporter::new(Verbosity::Default);
    let s = step("Clone the repo", "git clone $REMOTE_FOO");
    let assertions = vec![assertion(true, "Exit code", None)];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: None,
        stderr: None,
    };
    let out = render(|buf| {
        r.step_start(buf, 0, 3, &s).unwrap();
        r.step_pass(buf, &report).unwrap();
    });
    assert!(out.starts_with("[1/3] Clone the repo ... ok (1 checks)"));
    // Default verbosity should NOT print per-check icons or captured output.
    assert!(!out.contains("✓ Exit code"));
}

#[test]
fn pretty_verbose_step_pass_emits_check_icons() {
    let r = PrettyReporter::new(Verbosity::Verbose);
    let s = step("Clone the repo", "git clone $REMOTE_FOO");
    let assertions = vec![
        assertion(true, "Exit code: expected 0, got 0", None),
        assertion(true, "Directory exists: clone-target", None),
    ];
    let report = StepReport {
        expanded_command: Some("git clone /tmp/foo"),
        assertions: &assertions,
        stdout: None,
        stderr: None,
    };
    let out = render(|buf| r.step_pass(buf, &report).unwrap());
    assert!(out.contains("ok (2 checks)"));
    assert!(out.contains("$ git clone /tmp/foo"));
    assert!(out.contains("✓ Exit code: expected 0, got 0"));
    assert!(out.contains("✓ Directory exists: clone-target"));
}

#[test]
fn pretty_default_step_fail_shows_failed_assertions_and_capture() {
    let r = PrettyReporter::new(Verbosity::Default);
    let s = step("silent-ok-job log deleted", "daft hooks jobs ...");
    let assertions = vec![
        assertion(
            false,
            "Exit code: expected 0, got 1",
            Some("expected 0, got 1"),
        ),
        assertion(
            false,
            "Output contains \"OK_LOG_DELETED\"",
            Some("expected: OK_LOG_DELETED\nactual:   ASSERTION_FAILED: ..."),
        ),
    ];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: Some("ASSERTION_FAILED: silent-ok-job log was not deleted\n"),
        stderr: None,
    };
    let out = render(|buf| {
        r.step_start(buf, 3, 6, &s).unwrap();
        r.step_fail(buf, &report).unwrap();
    });
    assert!(out.starts_with("[4/6] silent-ok-job log deleted ... FAIL (2 failed)"));
    assert!(out.contains("x Exit code: expected 0, got 1"));
    assert!(out.contains("expected 0, got 1"));
    assert!(out.contains("expected: OK_LOG_DELETED"));
    assert!(out.contains("actual:   ASSERTION_FAILED: ..."));
    assert!(out.contains("--- captured output ---"));
    assert!(out.contains("ASSERTION_FAILED: silent-ok-job log was not deleted"));
}

#[test]
fn pretty_very_verbose_capture_has_no_line_cap() {
    let r = PrettyReporter::new(Verbosity::VeryVerbose);
    let s = step("noisy step", "echo lots");
    let assertions = vec![assertion(true, "Exit code", None)];
    let many_lines: String = (0..40).map(|i| format!("line{i}\n")).collect();
    let report = StepReport {
        expanded_command: Some("echo lots"),
        assertions: &assertions,
        stdout: Some(&many_lines),
        stderr: None,
    };
    let out = render(|buf| r.step_pass(buf, &report).unwrap());
    assert!(out.contains("line0"));
    assert!(out.contains("line39"));
    assert!(!out.contains("more lines truncated"));
}

#[test]
fn pretty_default_capture_truncates_with_hint() {
    let r = PrettyReporter::new(Verbosity::Default);
    let s = step("noisy fail", "echo lots");
    let assertions = vec![assertion(
        false,
        "Exit code: expected 0, got 1",
        Some("expected 0, got 1"),
    )];
    let many_lines: String = (0..40).map(|i| format!("line{i}\n")).collect();
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: Some(&many_lines),
        stderr: None,
    };
    let out = render(|buf| r.step_fail(buf, &report).unwrap());
    assert!(out.contains("line0"));
    assert!(out.contains("line19"));
    assert!(!out.contains("line20"));
    assert!(out.contains("more lines truncated (re-run with -vv for full output)"));
}

#[test]
fn pretty_scenario_block_spacing_attaches_cleanup_to_its_scenario() {
    // The cleanup note should sit flush against the scenario footer; the
    // blank line that separates scenarios belongs to the NEXT scenario's
    // header. Regression test for the spacing fix following #518.
    let r = PrettyReporter::new(Verbosity::Default);
    let sc1 = scenario("first", "tests/manual/scenarios/first.yml", vec![]);
    let sc2 = scenario("second", "tests/manual/scenarios/second.yml", vec![]);
    let out = render(|buf| {
        r.scenario_header(buf, &sc1).unwrap();
        r.scenario_footer(buf, &sc1, ScenarioStatus::Pass).unwrap();
        r.cleanup_note(buf, "Cleaned up test environment.").unwrap();
        r.scenario_header(buf, &sc2).unwrap();
        r.scenario_footer(buf, &sc2, ScenarioStatus::Pass).unwrap();
        r.cleanup_note(buf, "Cleaned up test environment.").unwrap();
    });
    assert!(
        out.contains("✓ first\nCleaned up test environment.\n\nsecond"),
        "expected footer + cleanup tight, blank before next header; got:\n{out}"
    );
}

#[test]
fn pretty_scenario_footer_pass_and_fail() {
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario(
        "hooks:silent-job-logs",
        "tests/manual/scenarios/hooks/silent-job-logs.yml",
        vec![],
    );
    let pass = render(|buf| r.scenario_footer(buf, &sc, ScenarioStatus::Pass).unwrap());
    assert!(pass.starts_with("✓ hooks:silent-job-logs"));
    let fail = render(|buf| r.scenario_footer(buf, &sc, ScenarioStatus::Fail).unwrap());
    assert!(fail.starts_with("✗ hooks:silent-job-logs"));
}

// ---------------------------------------------------------------------------
// PrettyReporter — run_summary
// ---------------------------------------------------------------------------

#[test]
fn pretty_run_summary_includes_failed_scenarios_and_reproduce_block() {
    let r = PrettyReporter::new(Verbosity::Default);
    let source = PathBuf::from("tests/manual/scenarios/hooks/silent-job-logs.yml");
    let failing = FailingStep {
        index: 3,
        total: 6,
        step_name: "silent-ok-job log deleted".to_string(),
        failed_assertions: vec![
            assertion(false, "Exit code: expected 0, got 1", Some("expected 0, got 1")),
            assertion(
                false,
                "Output contains \"OK_LOG_DELETED\"",
                Some("expected: OK_LOG_DELETED\nactual:   ASSERTION_FAILED: silent-ok-job ..."),
            ),
        ],
        captured_output: "ok_logs_found:\n/home/runner/.../silent-ok-job/output.jsonl\nASSERTION_FAILED: silent-ok-job log was not deleted: /home/runner/.../...".to_string(),
    };
    let failed = vec![FailedScenarioRecord {
        name: "hooks:silent-job-logs",
        source: &source,
        reproduce_token: "hooks:silent-job-logs".to_string(),
        failing_step: Some(&failing),
    }];
    let summary = RunSummary {
        scenarios_total: 572,
        scenarios_passed: 570,
        scenarios_failed: 2,
        steps_total: 2193,
        steps_passed: 2191,
        steps_failed: 2,
        duration: Duration::from_secs(64),
        parallel_jobs: Some(4),
        failed,
        errors: vec![],
    };
    let out = render(|buf| r.run_summary(buf, &summary).unwrap());
    assert!(out.contains("Failed scenarios:"));
    assert!(out.contains("✗ hooks:silent-job-logs"));
    assert!(out.contains("tests/manual/scenarios/hooks/silent-job-logs.yml"));
    assert!(out.contains("step 4/6  silent-ok-job log deleted"));
    assert!(out.contains("✗ Exit code: expected 0, got 1"));
    assert!(out.contains("expected: OK_LOG_DELETED"));
    assert!(out.contains("ASSERTION_FAILED: silent-ok-job log was not deleted"));
    assert!(out.contains("Scenarios:  570 passed, 2 failed   (572 total)"));
    assert!(out.contains("Steps:      2191 passed, 2 failed   (2193 total)"));
    assert!(out.contains("Duration:   01:04 (parallel jobs: 4)"));
    assert!(out.contains("Reproduce:"));
    assert!(out.contains("mise run test:manual -- --ci hooks:silent-job-logs"));
}

#[test]
fn pretty_run_summary_clean_when_all_passed() {
    let r = PrettyReporter::new(Verbosity::Default);
    let summary = RunSummary {
        scenarios_total: 5,
        scenarios_passed: 5,
        scenarios_failed: 0,
        steps_total: 20,
        steps_passed: 20,
        steps_failed: 0,
        duration: Duration::from_secs(7),
        parallel_jobs: None,
        failed: vec![],
        errors: vec![],
    };
    let out = render(|buf| r.run_summary(buf, &summary).unwrap());
    assert!(!out.contains("Failed scenarios:"));
    assert!(!out.contains("Reproduce:"));
    assert!(out.contains("Scenarios:  5 passed, 0 failed   (5 total)"));
    assert!(out.contains("Steps:      20 passed, 0 failed   (20 total)"));
    assert!(out.contains("Duration:   00:07"));
    assert!(!out.contains("parallel jobs"));
}

#[test]
fn pretty_run_summary_reports_errors_separately_from_failures() {
    let r = PrettyReporter::new(Verbosity::Default);
    let summary = RunSummary {
        scenarios_total: 1,
        scenarios_passed: 1,
        scenarios_failed: 0,
        steps_total: 3,
        steps_passed: 3,
        steps_failed: 0,
        duration: Duration::from_secs(2),
        parallel_jobs: None,
        failed: vec![],
        errors: vec![ScenarioErrorRecord {
            name: "broken-yaml",
            error: "missing field 'steps' at line 5".to_string(),
        }],
    };
    let out = render(|buf| r.run_summary(buf, &summary).unwrap());
    assert!(out.contains("Errors:"));
    assert!(out.contains("ERROR broken-yaml: missing field 'steps' at line 5"));
}

// ---------------------------------------------------------------------------
// QuietReporter
// ---------------------------------------------------------------------------

#[test]
fn quiet_suppresses_per_step_lines() {
    let r = QuietReporter::new();
    let s = step("Clone the repo", "git clone $REMOTE_FOO");
    let assertions = vec![assertion(true, "Exit code", None)];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: None,
        stderr: None,
    };
    let out = render(|buf| {
        r.step_start(buf, 0, 3, &s).unwrap();
        r.step_pass(buf, &report).unwrap();
    });
    assert!(out.is_empty(), "expected no per-step output, got: {out:?}");
}

#[test]
fn quiet_still_emits_scenario_footer() {
    let r = QuietReporter::new();
    let sc = scenario(
        "hooks:silent-job-logs",
        "tests/manual/scenarios/hooks/silent-job-logs.yml",
        vec![],
    );
    let out = render(|buf| r.scenario_footer(buf, &sc, ScenarioStatus::Pass).unwrap());
    assert!(out.starts_with("✓ hooks:silent-job-logs"));
}

#[test]
fn quiet_run_summary_matches_pretty() {
    let r = QuietReporter::new();
    let summary = RunSummary {
        scenarios_total: 1,
        scenarios_passed: 0,
        scenarios_failed: 1,
        steps_total: 3,
        steps_passed: 2,
        steps_failed: 1,
        duration: Duration::from_secs(5),
        parallel_jobs: Some(1),
        failed: vec![FailedScenarioRecord {
            name: "demo",
            source: Path::new("tests/manual/scenarios/demo.yml"),
            reproduce_token: "demo".to_string(),
            failing_step: None,
        }],
        errors: vec![],
    };
    let out = render(|buf| r.run_summary(buf, &summary).unwrap());
    assert!(out.contains("Failed scenarios:"));
    assert!(out.contains("✗ demo"));
    assert!(out.contains("Scenarios:  0 passed, 1 failed   (1 total)"));
    assert!(out.contains("Reproduce:"));
    assert!(out.contains("mise run test:manual -- --ci demo"));
}
