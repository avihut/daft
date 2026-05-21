//! Interactive step-through UI for the manual test framework.
//!
//! Presents each step to the user, waits for a keypress, executes the command,
//! displays assertion results, and offers re-run / reset / quit controls.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use std::io::Write;
use term_styles as styles;

use super::executor::CommandExecutor;
use super::reporter::{Reporter, StepReport, Verbosity};
use super::runner::{self, AssertionResult};
use super::sandbox::Sandbox;
use super::schema::Scenario;

// ---------------------------------------------------------------------------
// Key handling
// ---------------------------------------------------------------------------

/// RAII guard that enables terminal raw mode on creation and disables it on
/// drop — including during panics.
struct RawModeGuard;

impl RawModeGuard {
    fn enable() -> Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Wait for a single keypress and return the key code.
///
/// Enables raw mode for the duration of the wait and guarantees it is disabled
/// before returning, even on panic (via the [`RawModeGuard`] RAII guard).
fn wait_for_key() -> Result<KeyCode> {
    let _guard = RawModeGuard::enable()?;
    loop {
        if let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read()?
        {
            if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(KeyCode::Char('q'));
            }
            if code == KeyCode::Esc {
                return Ok(KeyCode::Char('q'));
            }
            return Ok(code);
        }
    }
    // _guard dropped here, disabling raw mode
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

fn print_scenario_header(scenario: &Scenario, sandbox: &Sandbox) {
    let desc = scenario.description.as_deref().unwrap_or("");
    let work_dir = &sandbox.work_dir;
    let display_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            work_dir
                .strip_prefix(&cwd)
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_else(|| work_dir.display().to_string());

    eprintln!();
    eprintln!("{}", styles::cyan(&scenario.name));
    if !desc.is_empty() {
        eprintln!("{}", styles::dim(desc));
    }
    eprintln!("{}", styles::dim(&format!("env: {display_path}")));
    eprintln!();
}

fn print_step_header(index: usize, total: usize, step: &super::schema::Step, sandbox: &Sandbox) {
    let expanded = sandbox.expand_vars(&step.run);
    eprintln!();
    // §1 budget: blue isn't a daft color slot. Counter is tertiary scaffolding.
    eprintln!(
        "{} {}",
        styles::dim(&format!("[{}/{}]", index + 1, total)),
        styles::bold(&step.name)
    );
    eprintln!("{}", styles::cyan(&format!("$ {expanded}")));
}

fn print_assertion_results(reporter: &dyn Reporter, results: &[AssertionResult], captured: &str) {
    if results.is_empty() {
        return;
    }
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr);
    let report = StepReport {
        expanded_command: None,
        assertions: results,
        stdout: if captured.is_empty() {
            None
        } else {
            Some(captured)
        },
        stderr: None,
    };
    let all_passed = results.iter().all(|r| r.passed);
    let _ = if all_passed {
        reporter.step_pass(&mut stderr, &report)
    } else {
        reporter.step_fail(&mut stderr, &report)
    };
}

fn print_prompt(msg: &str) {
    eprintln!();
    eprintln!("{}", styles::yellow(msg));
}

// ---------------------------------------------------------------------------
// Interactive runner
// ---------------------------------------------------------------------------

/// Run a scenario interactively, pausing between steps for user input.
///
/// - `start_step`: 1-based index to jump to (prior steps run silently).
/// - `loop_count`: when set (requires `start_step`), re-runs the target step
///   this many times, resetting the environment between iterations.
///
/// Per-step assertion output (the `ok (N checks)` / `FAIL (N failed)` block)
/// is routed through `reporter` so the per-check icons and capture-on-pass
/// honor the `-v` / `-vv` ladder; the scenario header and step preview lines
/// are interactive-specific (they show description + env path + the expanded
/// command) and stay local.
#[allow(clippy::too_many_arguments)]
pub fn run_interactive(
    scenario: &Scenario,
    sandbox: &Sandbox,
    executor: &dyn CommandExecutor,
    reporter: &dyn Reporter,
    _verbosity: Verbosity,
    start_step: Option<usize>,
    loop_count: Option<usize>,
) -> Result<()> {
    let total = scenario.steps.len();
    print_scenario_header(scenario, sandbox);

    // Convert 1-based start_step to 0-based index.
    let start_index = start_step.map(|s| s.saturating_sub(1)).unwrap_or(0);

    // --- Loop mode: re-run a single step N times with reset between each ---
    if let (Some(si), Some(count)) = (start_step, loop_count) {
        let step_index = si.saturating_sub(1);
        if step_index >= total {
            anyhow::bail!("Step {si} is out of range (scenario has {total} steps)");
        }

        for iteration in 1..=count {
            eprintln!();
            eprintln!("{} iteration {}/{}", styles::dim("---"), iteration, count);

            // Run prerequisite steps silently.
            for i in 0..step_index {
                runner::execute_step(&scenario.steps[i], sandbox, executor, false)?;
            }

            // Run target step.
            let step = &scenario.steps[step_index];
            print_step_header(step_index, total, step, sandbox);
            let (exit_code, output) = runner::run_step_command(step, sandbox, executor)?;
            let results = runner::check_step(step, exit_code, sandbox, Some(&output));
            print_assertion_results(reporter, &results, &output);

            if iteration < count {
                print_prompt("[Enter] next iteration, [q] quit");
                if let KeyCode::Char('q') = wait_for_key()? {
                    eprintln!();
                    eprintln!("Quit.");
                    return Ok(());
                }
                sandbox.reset()?;
            }
        }

        print_summary(scenario, total);
        return Ok(());
    }

    // --- Normal interactive mode ---

    // Silently run steps before start_index.
    if start_index > 0 {
        eprintln!(
            "{}",
            styles::dim(&format!("Skipping to step {}...", start_index + 1))
        );
        for i in 0..start_index.min(total) {
            runner::execute_step(&scenario.steps[i], sandbox, executor, false)?;
        }
    }

    let mut current = start_index;
    'outer: while current < total {
        let step = &scenario.steps[current];
        let has_checks = step.expect.is_some();
        print_step_header(current, total, step, sandbox);

        // Pre-run prompt.
        let pre_prompt = if has_checks {
            "[Enter] run, [x] run without checks, [s] skip, [q] quit"
        } else {
            "[Enter] run, [s] skip, [q] quit"
        };
        print_prompt(pre_prompt);

        let run_checks;
        match wait_for_key()? {
            KeyCode::Enter | KeyCode::Char(' ') => {
                run_checks = true;
            }
            KeyCode::Char('x') if has_checks => {
                run_checks = false;
            }
            KeyCode::Char('s') => {
                eprintln!("{}", styles::dim("(skipped)"));
                current += 1;
                continue;
            }
            KeyCode::Char('q') => {
                eprintln!();
                eprintln!("Quit.");
                return Ok(());
            }
            _ => {
                run_checks = true;
            }
        }

        // Execute the command.
        let (mut exit_code, mut captured_output) =
            runner::run_step_command(step, sandbox, executor)?;

        // Run checks if requested — auto-advance on success.
        if run_checks && has_checks {
            let results = runner::check_step(step, exit_code, sandbox, Some(&captured_output));
            let all_passed = results.iter().all(|r| r.passed);
            print_assertion_results(reporter, &results, &captured_output);
            if all_passed {
                current += 1;
                continue;
            }
        }

        // Post-run prompt — shown when checks failed, were skipped, or step has none.
        loop {
            let prompt = if has_checks {
                "[Enter] next, [c] check, [r] re-run, [R] reset, [q] quit"
            } else {
                "[Enter] next, [r] re-run, [R] reset, [q] quit"
            };
            print_prompt(prompt);

            match wait_for_key()? {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    current += 1;
                    break;
                }
                KeyCode::Char('c') if has_checks => {
                    let results =
                        runner::check_step(step, exit_code, sandbox, Some(&captured_output));
                    print_assertion_results(reporter, &results, &captured_output);
                }
                KeyCode::Char('r') => {
                    eprintln!("{}", styles::dim("(re-running...)"));
                    let result = runner::run_step_command(step, sandbox, executor)?;
                    exit_code = result.0;
                    captured_output = result.1;
                }
                KeyCode::Char('R') => {
                    eprintln!("{}", styles::dim("(resetting environment...)"));
                    sandbox.reset()?;
                    current = 0;
                    continue 'outer;
                }
                KeyCode::Char('q') => {
                    eprintln!();
                    eprintln!("Quit.");
                    return Ok(());
                }
                _ => {
                    current += 1;
                    break;
                }
            }
        }
    }

    print_summary(scenario, total);
    Ok(())
}

fn print_summary(scenario: &Scenario, total: usize) {
    eprintln!();
    eprintln!(
        "{} {} ({} steps completed)",
        styles::green("Done:"),
        styles::bold(&scenario.name),
        total,
    );
}
