//! Interactive step-through UI for the manual test framework.
//!
//! Presents each step to the user, waits for a keypress, executes the command,
//! displays assertion results, and offers re-run / reset / quit controls.

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    terminal,
};
use daft::styles;

use super::env::TestEnv;
use super::runner::{self, AssertionResult};
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
            return Ok(code);
        }
    }
    // _guard dropped here, disabling raw mode
}

// ---------------------------------------------------------------------------
// UI helpers
// ---------------------------------------------------------------------------

fn print_scenario_header(scenario: &Scenario) {
    let step_count = scenario.steps.len();
    let desc = scenario.description.as_deref().unwrap_or("");
    eprintln!();
    eprintln!(
        "  {} ({})",
        styles::bold(&scenario.name),
        styles::dim(&format!("{step_count} steps"))
    );
    if !desc.is_empty() {
        eprintln!("  {}", styles::dim(desc));
    }
    eprintln!("  {}", "\u{2500}".repeat(40));
}

fn print_step_header(index: usize, total: usize, step: &super::schema::Step, env: &TestEnv) {
    let expanded = env.expand_vars(&step.run);
    eprintln!();
    eprintln!(
        "  {} {}",
        styles::dim(&format!("[{}/{}]", index + 1, total)),
        styles::bold(&step.name)
    );
    eprintln!("       {}", styles::dim(&format!("$ {expanded}")));
}

fn print_assertion_results(results: &[AssertionResult]) {
    if results.is_empty() {
        return;
    }
    eprintln!();
    eprintln!("       Checks:");
    for r in results {
        let icon = if r.passed {
            styles::green("PASS")
        } else {
            styles::red("FAIL")
        };
        eprintln!("         [{}] {}", icon, r.label);
        if let Some(detail) = &r.detail {
            eprintln!("              {}", styles::dim(detail));
        }
    }
}

fn print_prompt(msg: &str) {
    eprintln!();
    eprintln!("       {}", styles::dim(msg));
}

// ---------------------------------------------------------------------------
// Interactive runner
// ---------------------------------------------------------------------------

/// Run a scenario interactively, pausing between steps for user input.
///
/// - `start_step`: 1-based index to jump to (prior steps run silently).
/// - `loop_count`: when set (requires `start_step`), re-runs the target step
///   this many times, resetting the environment between iterations.
pub fn run_interactive(
    scenario: &Scenario,
    env: &TestEnv,
    start_step: Option<usize>,
    loop_count: Option<usize>,
) -> Result<()> {
    let total = scenario.steps.len();
    print_scenario_header(scenario);

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
            eprintln!("  {} iteration {}/{}", styles::dim("---"), iteration, count);

            // Run prerequisite steps silently.
            for i in 0..step_index {
                runner::execute_step(&scenario.steps[i], env)?;
            }

            // Run target step.
            let step = &scenario.steps[step_index];
            print_step_header(step_index, total, step, env);
            let result = runner::execute_step(step, env)?;
            print_assertion_results(&result.assertions);

            if iteration < count {
                print_prompt("Press [Enter] for next iteration, [q] quit");
                if let KeyCode::Char('q') = wait_for_key()? {
                    eprintln!();
                    eprintln!("  Quit.");
                    return Ok(());
                }
                env.reset()?;
            }
        }

        print_summary(scenario, total);
        return Ok(());
    }

    // --- Normal interactive mode ---

    // Silently run steps before start_index.
    if start_index > 0 {
        eprintln!(
            "  {}",
            styles::dim(&format!("Skipping to step {}...", start_index + 1))
        );
        for i in 0..start_index.min(total) {
            runner::execute_step(&scenario.steps[i], env)?;
        }
    }

    let mut current = start_index;
    'outer: while current < total {
        let step = &scenario.steps[current];
        print_step_header(current, total, step, env);

        // Pre-run prompt.
        print_prompt("Press [Enter] to run, [s] skip, [q] quit");
        match wait_for_key()? {
            KeyCode::Enter | KeyCode::Char(' ') => { /* proceed to execute */ }
            KeyCode::Char('s') => {
                eprintln!("       {}", styles::dim("(skipped)"));
                current += 1;
                continue;
            }
            KeyCode::Char('q') => {
                eprintln!();
                eprintln!("  Quit.");
                return Ok(());
            }
            _ => {
                // Unrecognised key — treat as "run".
            }
        }

        // Execute the step (may loop for re-runs).
        loop {
            let result = runner::execute_step(step, env)?;
            print_assertion_results(&result.assertions);

            // Post-run prompt.
            print_prompt("Press [Enter] next, [r] re-run, [R] reset all, [q] quit");
            match wait_for_key()? {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    current += 1;
                    break;
                }
                KeyCode::Char('r') => {
                    // Re-run same step.
                    eprintln!("       {}", styles::dim("(re-running...)"));
                    continue;
                }
                KeyCode::Char('R') => {
                    eprintln!("       {}", styles::dim("(resetting environment...)"));
                    env.reset()?;
                    current = 0;
                    continue 'outer;
                }
                KeyCode::Char('q') => {
                    eprintln!();
                    eprintln!("  Quit.");
                    return Ok(());
                }
                _ => {
                    // Unrecognised key — treat as "next".
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
        "  {} {} ({} steps completed)",
        styles::green("Done:"),
        styles::bold(&scenario.name),
        total,
    );
}
