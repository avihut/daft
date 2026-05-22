//! Reporter unit tests. ANSI escapes are stripped from captured output so
//! assertions remain readable; format-only tweaks to terminal styles don't
//! cascade into churn here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::pretty::PrettyReporter;
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
        line: None,
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
fn pretty_verbose_scenario_header_includes_source_path() {
    // §6: scenario header (cyan name + dim path) ships from `-v` upward.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let sc = scenario("my-scenario", "tests/manual/scenarios/foo/bar.yml", vec![]);
    let out = render(|buf| r.scenario_header(buf, &sc).unwrap());
    assert!(out.contains("my-scenario"));
    assert!(out.contains("at tests/manual/scenarios/foo/bar.yml"));
}

#[test]
fn pretty_default_scenario_header_emits_nothing() {
    // §6: at `-q` / `Default`, the entire scenario block collapses to one
    // footer line — the cyan name + dim path live at `-v`.
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("my-scenario", "tests/manual/scenarios/foo/bar.yml", vec![]);
    let out = render(|buf| r.scenario_header(buf, &sc).unwrap());
    assert!(out.is_empty(), "expected no header output, got: {out:?}");
}

#[test]
fn pretty_verbose_step_pass_line_with_check_count() {
    // §6: per-step lines ship from `-v` upward.
    let r = PrettyReporter::new(Verbosity::Verbose);
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
    // §6 indent ladder: step opening line at col 2 (Layer 2 sits under
    // Layer 1 scenario header at col 0).
    assert!(out.starts_with("  [1/3] Clone the repo ... ok (1 checks)"));
    // `-v` should NOT print per-check icons or expanded command — those move
    // up to `-vv`.
    assert!(!out.contains("✓ Exit code"));
    assert!(!out.contains("$ "));
}

#[test]
fn pretty_default_step_pass_emits_nothing() {
    // §6: per-step lines are suppressed at `-q` / `Default`.
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
    assert!(out.is_empty(), "expected no per-step output, got: {out:?}");
}

#[test]
fn pretty_very_verbose_step_block_styling_per_layer() {
    // §6 `-vv` callout: at `-vv` each Layer carries a distinct treatment so
    // the four-layer block remains legible. This test pins all of them:
    //   Layer 2 `[N/M]` counter → plain cyan (`\x1b[36m`) — structural
    //                              anchor at the step boundary
    //   Layer 2 step name → bold bright purple (`\x1b[1m\x1b[95m`) —
    //                       Layer-2 anchor against the body content below
    //   Layer 3 `$ command` body → blue (`\x1b[94m`)
    //   Layer 3 `✓ check` label → default fg (no dim wrap)
    //   Layer 4 capture stream label + body → dim (`\x1b[2m`)
    let r = PrettyReporter::new(Verbosity::VeryVerbose);
    let s = step("Clone the repo", "git clone /tmp/foo");
    let assertions = vec![assertion(true, "Exit code: expected 0, got 0", None)];
    let report = StepReport {
        expanded_command: Some("git clone /tmp/foo"),
        assertions: &assertions,
        stdout: Some("clone progress line\n"),
        stderr: None,
    };
    let mut buf = Vec::new();
    r.step_start(&mut buf, 0, 3, &s).unwrap();
    r.step_pass(&mut buf, &report).unwrap();
    let raw = String::from_utf8(buf).expect("reporter output is valid UTF-8");

    // `[N/M]` counter is plain cyan (structural anchor — same hue family as
    // the bold-cyan scenario header).
    assert!(
        raw.contains("\x1b[36m[1/3]\x1b[0m"),
        "[N/M] counter not cyan; got: {raw:?}",
    );
    // Step name is bold bright purple — Layer-2 anchor.
    assert!(
        raw.contains("\x1b[1m\x1b[95mClone the repo\x1b[0m"),
        "step name not bold-bright-purple at -vv; got: {raw:?}",
    );
    // `$ command` body is blue.
    assert!(
        raw.contains("\x1b[94m$ git clone /tmp/foo\x1b[0m"),
        "$ command not blue at -vv; got: {raw:?}",
    );
    // Check label is default fg — must NOT be wrapped in dim.
    let check_line = raw
        .lines()
        .find(|l| l.contains("Exit code: expected 0, got 0"))
        .expect("check label line emitted");
    assert!(
        !check_line.contains("\x1b[2m"),
        "check label wrapped in dim; got: {check_line:?}",
    );
    // Capture stream label + body line are BOTH dim — color would compete
    // with the step-identity signal above (see §6 -vv callout).
    let stream_label_line = raw
        .lines()
        .find(|l| l.contains("stdout") && !l.contains("Clone"))
        .expect("stdout stream label emitted");
    assert!(
        stream_label_line.contains("\x1b[2m"),
        "stream label not dim; got: {stream_label_line:?}",
    );
    let body_line = raw
        .lines()
        .find(|l| l.contains("clone progress line"))
        .expect("capture body line emitted");
    assert!(
        body_line.contains("\x1b[2m"),
        "capture body not dim; got: {body_line:?}",
    );
}

#[test]
fn pretty_verbose_step_line_has_cyan_counter_and_purple_name() {
    // §6: step opening line pairs a cyan `[N/M]` counter (structural anchor
    // at the step boundary) with a bright-purple step name (step identity).
    // At `-v` the name is plain bright purple — bold is reserved for `-vv`
    // where the step line needs extra anchor weight against Layer 3/4
    // content.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let s = step("Clone the repo", "git clone /tmp/foo");
    let mut buf = Vec::new();
    r.step_start(&mut buf, 0, 3, &s).unwrap();
    let raw = String::from_utf8(buf).unwrap();
    assert!(
        raw.contains("\x1b[36m[1/3]\x1b[0m"),
        "[N/M] counter not cyan at -v; got: {raw:?}",
    );
    assert!(
        raw.contains("\x1b[95mClone the repo\x1b[0m"),
        "step name not bright purple at -v; got: {raw:?}",
    );
    assert!(
        !raw.contains("\x1b[1m"),
        "step name must not be bold at -v; got: {raw:?}",
    );
}

#[test]
fn pretty_very_verbose_step_pass_emits_check_icons() {
    // §6: per-check `✓` icons + `$ expanded-command` ship only at `-vv`.
    let r = PrettyReporter::new(Verbosity::VeryVerbose);
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
fn pretty_verbose_step_pass_omits_check_icons_and_expanded_command() {
    // Regression guard: today's `-v` shows the `ok (N checks)` outcome but
    // NOT per-check icons or expanded commands — those moved to `-vv`.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let s = step("Clone the repo", "git clone $REMOTE_FOO");
    let assertions = vec![assertion(true, "Exit code", None)];
    let report = StepReport {
        expanded_command: Some("git clone /tmp/foo"),
        assertions: &assertions,
        stdout: None,
        stderr: None,
    };
    let out = render(|buf| {
        r.step_start(buf, 0, 3, &s).unwrap();
        r.step_pass(buf, &report).unwrap();
    });
    assert!(out.contains("ok (1 checks)"));
    assert!(!out.contains("✓ Exit code"));
    assert!(!out.contains("$ git clone /tmp/foo"));
}

#[test]
fn pretty_default_step_fail_emits_nothing() {
    // §6: at `-q` / `Default`, fail-step inline detail is deferred to the
    // end-of-run failures block. Inline emission ships from `-v` upward.
    let r = PrettyReporter::new(Verbosity::Default);
    let assertions = vec![assertion(
        false,
        "Exit code: expected 0, got 1",
        Some("expected 0, got 1"),
    )];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: Some("noise\n"),
        stderr: None,
    };
    let out = render(|buf| r.step_fail(buf, &report).unwrap());
    assert!(
        out.is_empty(),
        "expected no inline step_fail output, got: {out:?}"
    );
}

#[test]
fn pretty_verbose_step_fail_shows_failed_assertions_and_capture() {
    let r = PrettyReporter::new(Verbosity::Verbose);
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
    // §6 indent ladder: step header at col 2 (Layer 2).
    assert!(out.starts_with("  [4/6] silent-ok-job log deleted ... FAIL (2 failed)"));
    // Iconography (§3 of reporter/CLAUDE.md): assertion failures use `✗`, not `x`.
    assert!(out.contains("✗ Exit code: expected 0, got 1"));
    assert!(!out.contains("x Exit code:"));
    assert!(out.contains("expected 0, got 1"));
    assert!(out.contains("expected: OK_LOG_DELETED"));
    assert!(out.contains("actual:   ASSERTION_FAILED: ..."));
    // §6 `-vv` callout: capture-stream label dropped the `--- {label} ---`
    // decoration in favor of plain `stdout` / `stderr` (indent carries the
    // framing). The label still appears just without the dashes.
    assert!(out.contains("stdout"));
    assert!(!out.contains("--- stdout ---"));
    assert!(out.contains("ASSERTION_FAILED: silent-ok-job log was not deleted"));
    // stderr was empty for this fixture — no stderr block should appear.
    assert!(!out.contains("\n      stderr\n"));
    assert!(!out.contains("--- stderr ---"));
}

#[test]
fn pretty_step_fail_renders_stdout_and_stderr_in_separate_blocks() {
    // Phase 1.4: per-step captured output is split into `--- stdout ---` and
    // `--- stderr ---` blocks so the reader can immediately tell which stream
    // the noise came from. §6: inline per-step ships at `-v` upward.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let s = step("noisy fail", "do-it");
    let assertions = vec![assertion(
        false,
        "Exit code: expected 0, got 1",
        Some("expected 0, got 1"),
    )];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: Some("normal-output-line\n"),
        stderr: Some("warning: something\nfatal: it broke\n"),
    };
    let out = render(|buf| {
        r.step_start(buf, 0, 1, &s).unwrap();
        r.step_fail(buf, &report).unwrap();
    });
    // §6 `-vv` callout: stream label is plain `stdout` / `stderr` (no
    // `--- {label} ---` decoration). Find by the label content on its own
    // line at the Layer-4-header indent (col 6).
    let stdout_idx = out.find("      stdout\n").expect("stdout block emitted");
    let stderr_idx = out.find("      stderr\n").expect("stderr block emitted");
    assert!(stdout_idx < stderr_idx, "stdout block precedes stderr");
    assert!(out.contains("normal-output-line"));
    assert!(out.contains("warning: something"));
    assert!(out.contains("fatal: it broke"));
}

#[test]
fn pretty_step_fail_emits_detail_lines_without_dim_wrap() {
    // Design language §1: assertion `detail` lines under a failed assertion
    // are the failure payload — secondary, default fg. They must NOT be
    // wrapped in `dim` (`\x1b[2m`), which on most terminals collapses any
    // embedded color (the diff labels) into muddy grey-X.
    // §6: inline per-step ships at `-v` upward.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let s = step("output-check", "echo something");
    let assertions = vec![assertion(
        false,
        "Output contains \"WANTED\"",
        Some("expected: WANTED\nactual:   got_something_else"),
    )];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: None,
        stderr: None,
    };
    let mut buf = Vec::new();
    r.step_fail(&mut buf, &report).unwrap();
    let raw = String::from_utf8(buf).expect("reporter output is valid UTF-8");
    assert!(raw.contains("expected: WANTED"));
    assert!(raw.contains("actual:   got_something_else"));
    // The dim sequence is `\x1b[2m`. It must not appear around the detail
    // lines (it can still appear earlier in the output for the FAIL count
    // suffix, so we extract the line containing "expected:" and assert that
    // specific line is unwrapped).
    let expected_line = raw
        .lines()
        .find(|l| l.contains("expected: WANTED"))
        .expect("output has a line containing expected: WANTED");
    assert!(
        !expected_line.contains("\x1b[2m"),
        "detail line is wrapped in dim, will render as muddy grey on color: {expected_line:?}",
    );
}

#[test]
fn format_diff_detail_uses_bold_color_labels() {
    // Design language §1 + §2: `expected:` is bold green (accent), `actual:`
    // is bold red (accent). The bold attribute (`\x1b[1m`) carries the
    // accent weight; without it the colored label is too thin to scan in
    // a stretch of default-fg text.
    use crate::manual_test::runner;
    let r = runner::check_output_contains("got something else", "WANTED");
    assert!(!r.passed);
    let detail = r.detail.expect("failed check has detail");
    // Bold + green for the expected label, bold + red for the actual label,
    // each terminated by a full reset (no FG-only reset trick anymore — the
    // reporter no longer dims around these lines).
    assert!(
        detail.contains("\x1b[1m\x1b[32mexpected\x1b[0m:"),
        "expected: label not bold green; got: {detail:?}",
    );
    assert!(
        detail.contains("\x1b[1m\x1b[31mactual\x1b[0m:"),
        "actual: label not bold red; got: {detail:?}",
    );
}

#[test]
fn pretty_step_fail_omits_stream_block_when_empty() {
    // §6: inline per-step ships at `-v` upward.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let s = step("stderr-only fail", "do-it");
    let assertions = vec![assertion(
        false,
        "Exit code: expected 0, got 1",
        Some("expected 0, got 1"),
    )];
    let report = StepReport {
        expanded_command: None,
        assertions: &assertions,
        stdout: Some("   \n"), // whitespace-only — trims to empty
        stderr: Some("real stderr content\n"),
    };
    let out = render(|buf| r.step_fail(buf, &report).unwrap());
    // §6 `-vv` callout: stream labels are plain `stdout` / `stderr` (no
    // `--- {label} ---`). Match the label-on-its-own-line at the
    // Layer-4-header indent.
    assert!(!out.contains("      stdout\n"));
    assert!(out.contains("      stderr\n"));
    assert!(out.contains("real stderr content"));
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
fn pretty_verbose_capture_truncates_with_hint() {
    // §6: capture cap is 20 lines at `-v`, uncapped at `-vv`.
    let r = PrettyReporter::new(Verbosity::Verbose);
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
fn pretty_verbose_scenario_block_spacing_attaches_cleanup_to_its_scenario() {
    // The cleanup note should sit flush against the scenario footer; the
    // blank line that separates scenarios belongs to the NEXT scenario's
    // header. Regression test for the spacing fix following #518.
    // §6: scenario header (and the blank above it) ships at `-v` upward.
    let r = PrettyReporter::new(Verbosity::Verbose);
    let sc1 = scenario("first", "tests/manual/scenarios/first.yml", vec![]);
    let sc2 = scenario("second", "tests/manual/scenarios/second.yml", vec![]);
    let out = render(|buf| {
        r.scenario_header(buf, &sc1).unwrap();
        r.scenario_footer(buf, &sc1, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap();
        r.cleanup_note(buf, "Cleaned up test environment.").unwrap();
        r.scenario_header(buf, &sc2).unwrap();
        r.scenario_footer(buf, &sc2, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap();
        r.cleanup_note(buf, "Cleaned up test environment.").unwrap();
    });
    assert!(
        out.contains("✓ first\nCleaned up test environment.\n\nsecond"),
        "expected footer + cleanup tight, blank before next header; got:\n{out}"
    );
}

#[test]
fn pretty_default_scenarios_pack_dense_no_blanks_between() {
    // At `Default`, scenario_header emits nothing — successive scenarios
    // pack tight as a stream of footer lines. No blank separators.
    let r = PrettyReporter::new(Verbosity::Default);
    let sc1 = scenario("first", "tests/manual/scenarios/first.yml", vec![]);
    let sc2 = scenario("second", "tests/manual/scenarios/second.yml", vec![]);
    let out = render(|buf| {
        r.scenario_header(buf, &sc1).unwrap();
        r.scenario_footer(buf, &sc1, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap();
        r.scenario_header(buf, &sc2).unwrap();
        r.scenario_footer(buf, &sc2, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap();
    });
    // Two footer lines back-to-back, no blank in between.
    let line = out.trim_end_matches('\n');
    assert_eq!(line, "✓ first\n✓ second");
}

#[test]
fn pretty_scenario_footer_pass_and_fail() {
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario(
        "hooks:silent-job-logs",
        "tests/manual/scenarios/hooks/silent-job-logs.yml",
        vec![],
    );
    let pass = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap()
    });
    assert!(pass.starts_with("✓ hooks:silent-job-logs"));
    let fail = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Fail, Duration::ZERO)
            .unwrap()
    });
    assert!(fail.starts_with("✗ hooks:silent-job-logs"));
}

#[test]
fn pretty_scenario_footer_pass_is_quiet_fail_is_loud() {
    // §4 pass-quiet/fail-loud: the pass footer must NOT carry bold (`\x1b[1m`).
    // Plain green on a tiny `✓` glyph is the entire pass signal — anything
    // more turns a 252-scenario green run into a wall of chrome. The fail
    // footer continues to stack signals: bold + red on the icon+name span.
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("demo", "tests/manual/scenarios/demo.yml", vec![]);
    let mut pass_buf = Vec::new();
    r.scenario_footer(&mut pass_buf, &sc, ScenarioStatus::Pass, Duration::ZERO)
        .unwrap();
    let pass_raw = String::from_utf8(pass_buf).unwrap();
    assert!(
        !pass_raw.contains("\x1b[1m"),
        "pass footer must not be bold; got: {pass_raw:?}",
    );
    assert!(
        pass_raw.contains("\x1b[32m✓\x1b[0m"),
        "pass icon must be plain green; got: {pass_raw:?}",
    );

    let mut fail_buf = Vec::new();
    r.scenario_footer(&mut fail_buf, &sc, ScenarioStatus::Fail, Duration::ZERO)
        .unwrap();
    let fail_raw = String::from_utf8(fail_buf).unwrap();
    assert!(
        fail_raw.contains("\x1b[1m\x1b[31m"),
        "fail footer prefix must be bold red; got: {fail_raw:?}",
    );
}

// ---------------------------------------------------------------------------
// PrettyReporter — run_summary
// ---------------------------------------------------------------------------

#[test]
fn pretty_footer_renders_duration_in_ms() {
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("fast", "tests/manual/scenarios/fast.yml", vec![]);
    let out = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Pass, Duration::from_millis(142))
            .unwrap()
    });
    assert!(out.contains("✓ fast"));
    assert!(out.contains("142ms"));
    // Fast scenarios must not carry the (slow) annotation.
    assert!(!out.contains("(slow)"));
}

#[test]
fn pretty_footer_renders_seconds_with_decimal() {
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("medium", "tests/manual/scenarios/medium.yml", vec![]);
    let out = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Pass, Duration::from_millis(2_300))
            .unwrap()
    });
    assert!(out.contains("2.3s"));
    assert!(!out.contains("(slow)"));
}

#[test]
fn pretty_footer_marks_slow_scenarios() {
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("slowpoke", "tests/manual/scenarios/slowpoke.yml", vec![]);
    let out = render(|buf| {
        // SLOW_THRESHOLD is 5s — anything at or beyond gets the annotation.
        r.scenario_footer(buf, &sc, ScenarioStatus::Pass, Duration::from_secs(7))
            .unwrap()
    });
    assert!(out.contains("7.0s"));
    assert!(out.contains("(slow)"));
}

#[test]
fn pretty_footer_suppresses_suffix_for_zero_duration() {
    // Interactive runner passes Duration::ZERO because wall-clock would
    // include time the user spent at the prompt — meaningless as a perf
    // signal, so the reporter omits the duration entirely.
    let r = PrettyReporter::new(Verbosity::Default);
    let sc = scenario("interactive", "tests/manual/scenarios/i.yml", vec![]);
    let out = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap()
    });
    // The line should be just "✓ interactive\n" — no trailing duration.
    let line = out.trim_end_matches('\n');
    assert_eq!(line, "✓ interactive");
}

#[test]
fn pretty_quiet_fail_footer_renders_duration_and_slow_annotation() {
    // §6: at `-q` the pass footer is suppressed, but the FAIL footer still
    // surfaces — and it carries the same `(slow)` annotation as the pretty
    // tiers when the scenario crossed the SLOW_THRESHOLD.
    let r = PrettyReporter::new(Verbosity::Quiet);
    let sc = scenario("slow-quiet", "tests/manual/scenarios/sq.yml", vec![]);
    let out = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Fail, Duration::from_secs(6))
            .unwrap()
    });
    assert!(out.contains("6.0s"));
    assert!(out.contains("(slow)"));
}

#[test]
fn pretty_run_summary_includes_failed_scenarios_and_reproduce_block() {
    let r = PrettyReporter::new(Verbosity::Default);
    let failing = FailingStep {
        index: 3,
        total: 6,
        step_name: "silent-ok-job log deleted".to_string(),
        line: Some(23),
        failed_assertions: vec![
            assertion(false, "Exit code: expected 0, got 1", Some("expected 0, got 1")),
            assertion(
                false,
                "Output contains \"OK_LOG_DELETED\"",
                Some("expected: OK_LOG_DELETED\nactual:   ASSERTION_FAILED: silent-ok-job ..."),
            ),
        ],
        captured_stdout: "ok_logs_found:\n/home/runner/.../silent-ok-job/output.jsonl\nASSERTION_FAILED: silent-ok-job log was not deleted: /home/runner/.../...".to_string(),
        captured_stderr: String::new(),
    };
    let failed = vec![FailedScenarioRecord {
        name: "hooks:silent-job-logs",
        display_path: "hooks/silent-job-logs.yml".to_string(),
        reproduce_token: "hooks:silent-job-logs".to_string(),
        duration: Duration::from_millis(842),
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
    // Phase 1.2: banner with rule chars + failure count.
    assert!(out.contains("Failed Scenarios (1)"));
    assert!(out.contains("⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯⎯"));
    // Phase 1.3: numbered entry.
    assert!(out.contains("1) ✗ hooks:silent-job-logs"));
    // S8: display_path is scenarios-dir-relative, not the absolute path.
    assert!(out.contains("hooks/silent-job-logs.yml"));
    assert!(!out.contains("tests/manual/scenarios/hooks/silent-job-logs.yml"));
    // Phase 1.5 + design-language §3 + §2: ❯ marker, dim `step N/M` counter,
    // bold default-fg step name. The strip_ansi helper collapses styling so
    // we assert on the visible character content.
    assert!(out.contains("❯ step 4/6 silent-ok-job log deleted"));
    // Failure-block location pointer: scenarios-relative `path:line` on its
    // own line (terminal-clickable). Replaces the prior basename-based `at
    // file.yml:N` citation that sat at the right of the focal-step line.
    assert!(out.contains("hooks/silent-job-logs.yml:23"));
    // Old `at <basename>:<line>` citation form must not reappear inline.
    assert!(!out.contains("at silent-job-logs.yml:23"));
    assert!(out.contains("✗ Exit code: expected 0, got 1"));
    assert!(out.contains("expected: OK_LOG_DELETED"));
    assert!(out.contains("ASSERTION_FAILED: silent-ok-job log was not deleted"));
    // Captured output is split into stdout/stderr blocks.
    assert!(out.contains("--- stdout ---"));
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
    assert!(!out.contains("Failed Scenarios"));
    assert!(!out.contains("Reproduce:"));
    assert!(out.contains("Scenarios:  5 passed, 0 failed   (5 total)"));
    assert!(out.contains("Steps:      20 passed, 0 failed   (20 total)"));
    assert!(out.contains("Duration:   00:07"));
    assert!(!out.contains("parallel jobs"));
}

#[test]
fn pretty_run_summary_numbers_multiple_failures() {
    let r = PrettyReporter::new(Verbosity::Default);
    let summary = RunSummary {
        scenarios_total: 3,
        scenarios_passed: 1,
        scenarios_failed: 2,
        steps_total: 6,
        steps_passed: 4,
        steps_failed: 2,
        duration: Duration::from_secs(3),
        parallel_jobs: None,
        failed: vec![
            FailedScenarioRecord {
                name: "alpha",
                display_path: "a.yml".to_string(),
                reproduce_token: "a".to_string(),
                duration: Duration::from_millis(120),
                failing_step: None,
            },
            FailedScenarioRecord {
                name: "beta",
                display_path: "b.yml".to_string(),
                reproduce_token: "b".to_string(),
                duration: Duration::from_millis(450),
                failing_step: None,
            },
        ],
        errors: vec![],
    };
    let out = render(|buf| r.run_summary(buf, &summary).unwrap());
    assert!(out.contains("Failed Scenarios (2)"));
    assert!(out.contains("1) ✗ alpha"));
    assert!(out.contains("2) ✗ beta"));
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
// PrettyReporter — Verbosity::Quiet behavior (silent on pass, loud on fail)
// ---------------------------------------------------------------------------

#[test]
fn pretty_quiet_suppresses_per_step_lines() {
    // §6: at `-q`, per-step lines are suppressed (same rule as `Default`).
    let r = PrettyReporter::new(Verbosity::Quiet);
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
fn pretty_quiet_suppresses_pass_footer_but_emits_fail_footer() {
    // §6: at `-q`, the pass footer is suppressed entirely — failures are the
    // only thing that surfaces inline. The FAIL footer + cleanup line still
    // emit at `-q`, and the end-of-run summary block carries the detail.
    let r = PrettyReporter::new(Verbosity::Quiet);
    let sc = scenario(
        "hooks:silent-job-logs",
        "tests/manual/scenarios/hooks/silent-job-logs.yml",
        vec![],
    );
    let pass = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Pass, Duration::ZERO)
            .unwrap()
    });
    assert!(
        pass.is_empty(),
        "expected no pass footer at `-q`, got: {pass:?}",
    );
    let fail = render(|buf| {
        r.scenario_footer(buf, &sc, ScenarioStatus::Fail, Duration::ZERO)
            .unwrap()
    });
    assert!(fail.starts_with("✗ hooks:silent-job-logs"));
}

#[test]
fn pretty_quiet_run_summary_matches_pretty() {
    // §6 + design language: the end-of-run summary block (failed-scenarios +
    // reproduce) emits at every verbosity, including `-q`. It's how `-q`
    // surfaces failures despite the inline stream being silent on pass.
    let r = PrettyReporter::new(Verbosity::Quiet);
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
            display_path: "demo.yml".to_string(),
            reproduce_token: "demo".to_string(),
            duration: Duration::from_secs(1),
            failing_step: None,
        }],
        errors: vec![],
    };
    let out = render(|buf| r.run_summary(buf, &summary).unwrap());
    assert!(out.contains("Failed Scenarios (1)"));
    assert!(out.contains("1) ✗ demo"));
    assert!(out.contains("Scenarios:  0 passed, 1 failed   (1 total)"));
    assert!(out.contains("Reproduce:"));
    assert!(out.contains("mise run test:manual -- --ci demo"));
}

#[test]
fn pretty_quiet_emits_cleanup_on_fail() {
    // CLAUDE.md §6: cleanup line emits on fail at every verbosity (including
    // `-q`). The runner only calls cleanup_note after a failed scenario, so
    // the reporter just needs to emit unconditionally when called.
    let r = PrettyReporter::new(Verbosity::Quiet);
    let out = render(|buf| r.cleanup_note(buf, "Cleaned up test environment.").unwrap());
    assert!(out.contains("Cleaned up test environment."));
}
