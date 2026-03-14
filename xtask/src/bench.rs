//! TUI benchmark runner for integration tests.
//!
//! Displays a live table with spinners and timers while suites run,
//! then shows final results with pass/fail and duration.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Suite definitions
// ---------------------------------------------------------------------------

struct Suite {
    name: &'static str,
    bash_test: Option<&'static str>,
    yaml_dir: &'static str,
}

const SUITES: &[Suite] = &[
    Suite {
        name: "exec",
        bash_test: Some("test_exec.sh"),
        yaml_dir: "exec",
    },
    Suite {
        name: "clone",
        bash_test: Some("test_clone.sh"),
        yaml_dir: "clone",
    },
    Suite {
        name: "init",
        bash_test: Some("test_init.sh"),
        yaml_dir: "init",
    },
    Suite {
        name: "checkout",
        bash_test: Some("test_checkout.sh"),
        yaml_dir: "checkout",
    },
    Suite {
        name: "checkout-branch",
        bash_test: Some("test_checkout_branch.sh"),
        yaml_dir: "checkout-branch",
    },
    Suite {
        name: "prune",
        bash_test: Some("test_prune.sh"),
        yaml_dir: "prune",
    },
    Suite {
        name: "branch-delete",
        bash_test: Some("test_branch_delete.sh"),
        yaml_dir: "branch-delete",
    },
    Suite {
        name: "list",
        bash_test: Some("test_list.sh"),
        yaml_dir: "list",
    },
    Suite {
        name: "fetch",
        bash_test: Some("test_fetch.sh"),
        yaml_dir: "fetch",
    },
    Suite {
        name: "hooks",
        bash_test: Some("test_hooks.sh"),
        yaml_dir: "hooks",
    },
    Suite {
        name: "sync",
        bash_test: Some("test_sync.sh"),
        yaml_dir: "sync",
    },
    Suite {
        name: "config",
        bash_test: Some("test_config.sh"),
        yaml_dir: "config",
    },
    Suite {
        name: "completions",
        bash_test: Some("test_completions.sh"),
        yaml_dir: "completions",
    },
    Suite {
        name: "rename",
        bash_test: Some("test_rename.sh"),
        yaml_dir: "rename",
    },
    // These bash tests are not in test_all.sh (stale expectations).
    Suite {
        name: "shell-init",
        bash_test: None,
        yaml_dir: "shell-init",
    },
    Suite {
        name: "setup",
        bash_test: None,
        yaml_dir: "setup",
    },
];

// ---------------------------------------------------------------------------
// Cell state
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum CellState {
    Pending,
    Running(Instant),
    Passed(Duration),
    Failed(Duration),
    Skipped,
}

// ---------------------------------------------------------------------------
// ANSI helpers
// ---------------------------------------------------------------------------

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";

const SPINNERS: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn format_cell(state: &CellState, spinner_frame: usize) -> String {
    match state {
        CellState::Pending => format!("{DIM}·{RESET}"),
        CellState::Running(start) => {
            let elapsed = start.elapsed().as_secs_f64();
            let spin = SPINNERS[spinner_frame % SPINNERS.len()];
            format!("{CYAN}{spin} {elapsed:.1}s{RESET}")
        }
        CellState::Passed(d) => {
            format!("{GREEN}✓ {:.1}s{RESET}", d.as_secs_f64())
        }
        CellState::Failed(d) => {
            format!("{RED}✗ {:.1}s{RESET}", d.as_secs_f64())
        }
        CellState::Skipped => format!("{DIM}--{RESET}"),
    }
}

fn duration_of(state: &CellState) -> Duration {
    match state {
        CellState::Passed(d) | CellState::Failed(d) => *d,
        _ => Duration::ZERO,
    }
}

// ---------------------------------------------------------------------------
// Table rendering
// ---------------------------------------------------------------------------

struct TableState {
    bash_cells: Vec<CellState>,
    yaml_cells: Vec<CellState>,
    spinner_frame: usize,
}

impl TableState {
    fn new(n: usize) -> Self {
        Self {
            bash_cells: vec![CellState::Pending; n],
            yaml_cells: vec![CellState::Pending; n],
            spinner_frame: 0,
        }
    }

    fn render(&self, suites: &[&Suite]) -> String {
        let mut out = String::new();
        let name_w = 20;
        let col_w = 16;

        // Header
        out.push_str(&format!(
            "\n  {BOLD}{:<name_w$}  {:<col_w$}  {:<col_w$}{RESET}\n",
            "Suite", "Bash", "YAML"
        ));
        out.push_str(&format!(
            "  {DIM}{:─<name_w$}  {:─<col_w$}  {:─<col_w$}{RESET}\n",
            "", "", ""
        ));

        // Rows
        for (i, suite) in suites.iter().enumerate() {
            let bash_str = format_cell(&self.bash_cells[i], self.spinner_frame);
            let yaml_str = format_cell(&self.yaml_cells[i], self.spinner_frame);
            // Pad with invisible spaces to align (ANSI codes don't count for width)
            out.push_str(&format!(
                "  {:<name_w$}  {}{}  {}{}\n",
                suite.name,
                bash_str,
                pad_ansi(&self.bash_cells[i], col_w),
                yaml_str,
                pad_ansi(&self.yaml_cells[i], col_w),
            ));
        }

        // Totals
        let bash_total: Duration = self.bash_cells.iter().map(duration_of).sum();
        let yaml_total: Duration = self.yaml_cells.iter().map(duration_of).sum();

        out.push_str(&format!(
            "  {DIM}{:─<name_w$}  {:─<col_w$}  {:─<col_w$}{RESET}\n",
            "", "", ""
        ));

        let bash_total_str = if bash_total > Duration::ZERO {
            format!("{BOLD}{:.1}s{RESET}", bash_total.as_secs_f64())
        } else {
            format!("{DIM}--{RESET}")
        };
        let yaml_total_str = if yaml_total > Duration::ZERO {
            format!("{BOLD}{:.1}s{RESET}", yaml_total.as_secs_f64())
        } else {
            format!("{DIM}--{RESET}")
        };

        out.push_str(&format!(
            "  {BOLD}{:<name_w$}{RESET}  {}  {}\n",
            "TOTAL", bash_total_str, yaml_total_str
        ));

        out
    }
}

/// Pad after ANSI-colored cell text to maintain column alignment.
fn pad_ansi(state: &CellState, col_w: usize) -> String {
    let visible_len = match state {
        CellState::Pending => 1, // "·"
        CellState::Running(start) => {
            let s = format!("{:.1}s", start.elapsed().as_secs_f64());
            2 + s.len() // "⠋ " + number
        }
        CellState::Passed(d) | CellState::Failed(d) => {
            let s = format!("{:.1}s", d.as_secs_f64());
            2 + s.len() // "✓ " + number
        }
        CellState::Skipped => 2, // "--"
    };
    let padding = col_w.saturating_sub(visible_len);
    " ".repeat(padding)
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

fn draw_table(state: &TableState, suites: &[&Suite], total_lines: &mut usize) {
    let mut stderr = std::io::stderr();
    // Move cursor up to overwrite previous render
    if *total_lines > 0 {
        write!(stderr, "\x1b[{}A", *total_lines).ok();
    }
    let rendered = state.render(suites);
    let lines: Vec<&str> = rendered.lines().collect();
    *total_lines = lines.len();
    for line in &lines {
        // Clear line and write
        write!(stderr, "\x1b[2K{line}\n").ok();
    }
    stderr.flush().ok();
}

fn run_bash_test(project_root: &std::path::Path, bash_file: &str) -> Result<(bool, Duration)> {
    let start = Instant::now();
    let status = Command::new("bash")
        .arg(project_root.join("tests/integration").join(bash_file))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(project_root)
        .status()
        .with_context(|| format!("Failed to run {bash_file}"))?;
    Ok((status.success(), start.elapsed()))
}

fn run_yaml_test(
    project_root: &std::path::Path,
    xtask_bin: &std::path::Path,
    yaml_dir: &str,
) -> Result<(bool, Duration)> {
    let start = Instant::now();
    let scenarios_path = project_root.join("tests/manual/scenarios").join(yaml_dir);
    let status = Command::new(xtask_bin)
        .args(["manual-test", "--ci"])
        .arg(&scenarios_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .current_dir(project_root)
        .status()
        .with_context(|| format!("Failed to run YAML tests for {yaml_dir}"))?;
    Ok((status.success(), start.elapsed()))
}

pub fn run(parallel: bool) -> Result<()> {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask should be inside project root")
        .to_path_buf();

    // Pre-build xtask in release mode
    eprintln!("{YELLOW}Building xtask (release)...{RESET}");
    let build_status = Command::new("cargo")
        .args(["build", "--package", "xtask", "--release", "--quiet"])
        .current_dir(&project_root)
        .status()?;
    if !build_status.success() {
        anyhow::bail!("Failed to build xtask in release mode");
    }

    let xtask_bin = project_root.join("target/release/xtask");
    let suites: Vec<&Suite> = SUITES.iter().collect();
    let n = suites.len();

    let state = Arc::new(Mutex::new(TableState::new(n)));
    let mut total_lines = 0usize;

    // Mark skipped bash cells
    for (i, suite) in suites.iter().enumerate() {
        if suite.bash_test.is_none() {
            state.lock().unwrap().bash_cells[i] = CellState::Skipped;
        }
    }

    // Initial render
    let title = if parallel {
        format!("\n  {BOLD}{CYAN}Integration Benchmark{RESET} {DIM}(parallel){RESET}")
    } else {
        format!("\n  {BOLD}{CYAN}Integration Benchmark{RESET} {DIM}(sequential){RESET}")
    };
    eprintln!("{title}");
    draw_table(&state.lock().unwrap(), &suites, &mut total_lines);

    let overall_start = Instant::now();

    // Spinner thread
    let spinner_state = Arc::clone(&state);
    let spinner_handle = std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(80));
        let mut s = spinner_state.lock().unwrap();
        s.spinner_frame += 1;
        let all_done = s.bash_cells.iter().chain(s.yaml_cells.iter()).all(|c| {
            matches!(
                c,
                CellState::Passed(_) | CellState::Failed(_) | CellState::Skipped
            )
        });
        drop(s);
        if all_done {
            break;
        }
    });

    if parallel {
        run_parallel(&suites, &state, &project_root, &xtask_bin, &mut total_lines)?;
    } else {
        run_sequential(&suites, &state, &project_root, &xtask_bin, &mut total_lines)?;
    }

    // Wait for spinner to notice completion
    let _ = spinner_handle.join();

    // Final render
    draw_table(&state.lock().unwrap(), &suites, &mut total_lines);

    let overall_elapsed = overall_start.elapsed();
    eprintln!(
        "\n  {DIM}Wall time: {:.1}s{RESET}\n",
        overall_elapsed.as_secs_f64()
    );

    // Check for failures
    let s = state.lock().unwrap();
    let any_failed = s
        .bash_cells
        .iter()
        .chain(s.yaml_cells.iter())
        .any(|c| matches!(c, CellState::Failed(_)));

    if any_failed {
        anyhow::bail!("One or more suites failed");
    }

    Ok(())
}

fn run_sequential(
    suites: &[&Suite],
    state: &Arc<Mutex<TableState>>,
    project_root: &std::path::Path,
    xtask_bin: &std::path::Path,
    total_lines: &mut usize,
) -> Result<()> {
    let suites_for_draw: Vec<&Suite> = SUITES.iter().collect();

    for (i, suite) in suites.iter().enumerate() {
        // Bash
        if let Some(bash_file) = suite.bash_test {
            state.lock().unwrap().bash_cells[i] = CellState::Running(Instant::now());
            draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);

            let (success, elapsed) = run_bash_test(project_root, bash_file)?;
            state.lock().unwrap().bash_cells[i] = if success {
                CellState::Passed(elapsed)
            } else {
                CellState::Failed(elapsed)
            };
            draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);
        }

        // YAML
        state.lock().unwrap().yaml_cells[i] = CellState::Running(Instant::now());
        draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);

        let (success, elapsed) = run_yaml_test(project_root, xtask_bin, suite.yaml_dir)?;
        state.lock().unwrap().yaml_cells[i] = if success {
            CellState::Passed(elapsed)
        } else {
            CellState::Failed(elapsed)
        };
        draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);
    }

    Ok(())
}

fn run_parallel(
    suites: &[&Suite],
    state: &Arc<Mutex<TableState>>,
    project_root: &std::path::Path,
    xtask_bin: &std::path::Path,
    total_lines: &mut usize,
) -> Result<()> {
    let suites_for_draw: Vec<&Suite> = SUITES.iter().collect();

    for (i, suite) in suites.iter().enumerate() {
        // Start both bash and YAML concurrently
        let bash_file = suite.bash_test.map(|s| s.to_string());
        let yaml_dir = suite.yaml_dir.to_string();
        let pr = project_root.to_path_buf();
        let xb = xtask_bin.to_path_buf();

        // Mark both as running
        {
            let mut s = state.lock().unwrap();
            if bash_file.is_some() {
                s.bash_cells[i] = CellState::Running(Instant::now());
            }
            s.yaml_cells[i] = CellState::Running(Instant::now());
        }
        draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);

        let bash_state = Arc::clone(state);
        let yaml_state = Arc::clone(state);

        let bash_handle = if let Some(bf) = bash_file {
            let pr2 = pr.clone();
            Some(std::thread::spawn(move || -> Result<()> {
                let (success, elapsed) = run_bash_test(&pr2, &bf)?;
                bash_state.lock().unwrap().bash_cells[i] = if success {
                    CellState::Passed(elapsed)
                } else {
                    CellState::Failed(elapsed)
                };
                Ok(())
            }))
        } else {
            None
        };

        let yaml_handle = std::thread::spawn(move || -> Result<()> {
            let (success, elapsed) = run_yaml_test(&pr, &xb, &yaml_dir)?;
            yaml_state.lock().unwrap().yaml_cells[i] = if success {
                CellState::Passed(elapsed)
            } else {
                CellState::Failed(elapsed)
            };
            Ok(())
        });

        // Wait for both to complete, redrawing periodically
        loop {
            std::thread::sleep(Duration::from_millis(80));
            {
                let mut s = state.lock().unwrap();
                s.spinner_frame += 1;
            }
            draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);

            let bash_done = bash_handle.as_ref().map_or(true, |h| h.is_finished());
            let yaml_done = yaml_handle.is_finished();
            if bash_done && yaml_done {
                break;
            }
        }

        if let Some(h) = bash_handle {
            h.join()
                .map_err(|_| anyhow::anyhow!("bash thread panicked"))??;
        }
        yaml_handle
            .join()
            .map_err(|_| anyhow::anyhow!("yaml thread panicked"))??;

        draw_table(&state.lock().unwrap(), &suites_for_draw, total_lines);
    }

    Ok(())
}
