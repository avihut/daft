pub mod daft_executor;
pub mod executor;
pub mod interactive;
pub mod repo_gen;
pub mod reporter;
pub mod runner;
pub mod sandbox;
pub mod schema;

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Pick a base directory for a scenario sandbox.
///
/// Honors the `DAFT_MANUAL_TEST_BASE` env var as an opt-in override for
/// managed test directories (e.g. a ramdisk under `sandbox/test/`). When set,
/// the path is `<base>/<slug>` where the slug is the lowercased scenario name
/// — callers using `--jobs > 1` must ensure scenario names are unique.
/// Otherwise falls back to a unique `/tmp/daft-manual-test-*` path allocated
/// by [`sandbox::alloc_default_base_dir`].
///
/// Lives at this layer (not in `sandbox::`) so the runner core stays free of
/// daft-named env vars; renaming `DAFT_MANUAL_TEST_BASE` is a concern for the
/// eventual runner spin-out.
fn pick_sandbox_base_dir(scenario: &schema::Scenario) -> Result<PathBuf> {
    if let Ok(base) = std::env::var("DAFT_MANUAL_TEST_BASE") {
        let slug = scenario.name.to_lowercase().replace(' ', "-");
        return Ok(PathBuf::from(base).join(slug));
    }
    sandbox::alloc_default_base_dir()
}

/// Shared registry of live sandbox directories.
///
/// Workers register their sandbox path on creation and unregister on cleanup;
/// the Ctrl+C handler drains the set and removes every live sandbox. Using a
/// generic `HashSet<PathBuf>` keeps the scheduler decoupled from any sandbox
/// path prefix, which preserves the runner's spin-out story.
type CleanupSet = Arc<Mutex<HashSet<PathBuf>>>;

/// RAII guard that keeps the live-sandbox registry in sync.
///
/// Inserted into the cleanup set on construction and removed on drop, so
/// early returns, `?`-propagated errors, and panics inside a worker all
/// leave the registry consistent.
struct CleanupGuard {
    set: CleanupSet,
    path: PathBuf,
}

impl CleanupGuard {
    fn new(set: CleanupSet, path: PathBuf) -> Self {
        if let Ok(mut g) = set.lock() {
            g.insert(path.clone());
        }
        Self { set, path }
    }
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = self.set.lock() {
            g.remove(&self.path);
        }
    }
}

/// Register a Ctrl+C handler that cleans up every live test environment.
///
/// Returns a shared registry the run loop populates as scenarios start and
/// clears as they finish. On SIGINT the handler drains the set in a loop —
/// `process::exit(130)` aborts worker threads mid-flight without running
/// their `Drop` impls, so a single drain can miss workers that registered
/// after the snapshot. Re-draining with a short sleep between passes catches
/// those late registrations until the set stays empty.
fn setup_cleanup_handler(keep: bool) -> CleanupSet {
    let set: CleanupSet = Arc::new(Mutex::new(HashSet::new()));
    let handler_set = Arc::clone(&set);

    ctrlc::set_handler(move || {
        // Restore terminal state in case interactive mode left raw mode on.
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!();
        if !keep {
            // Workers in flight fall into two camps when SIGINT fires:
            //
            // 1. Past `CleanupGuard::new` — their `base_dir` is in the set.
            //    Drain captures it; we `rm -rf` to remove whatever they
            //    built so far. Their subprocesses (`git`, `cp`) may still
            //    be running and may RECREATE entries at the same path
            //    between our `rm` and `process::exit` below, which is why
            //    we re-rm in a short loop while holding the lock.
            // 2. About to call `CleanupGuard::new` — they block at
            //    `set.lock()` because we hold the lock to process exit, then
            //    die with the process. They never touch disk, so no leak.
            //
            // Holding the lock for the whole sequence prevents new
            // registrations, so the `known` set captures the entire universe
            // of paths that could possibly still have on-disk presence. The
            // re-rm loop is bounded so the handler always finishes ahead of
            // the main thread's rayon-iteration completion (which would
            // otherwise short-circuit the closure via the natural anyhow
            // bail with a non-130 exit code).
            let mut g = match handler_set.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let known: HashSet<PathBuf> = g.drain().collect();
            for _ in 0..4 {
                for dir in &known {
                    let _ = std::fs::remove_dir_all(dir);
                }
                if known.iter().all(|p| !p.exists()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            if !known.is_empty() {
                eprintln!("Cleaned up {} test environment(s).", known.len());
            }
            // `g` stays in scope through process::exit — the lock is never
            // released, which prevents any racing worker from registering a
            // new sandbox between our drain and the process dying.
            let _hold = g;
        }
        std::process::exit(130); // 128 + SIGINT(2)
    })
    .ok();

    set
}

/// Aggregated result of one scenario, produced by a parallel worker.
struct ScenarioOutcome {
    /// Position of the scenario in the input list; used to order both output
    /// and stats independently of completion order.
    index: usize,
    /// Display name from the YAML (or the file path if parsing failed before
    /// the name could be read).
    name: String,
    /// Source YAML path. Empty when execution short-circuited before
    /// `load_scenario` ran successfully.
    source: PathBuf,
    /// `None` when execution short-circuited before stats were computed
    /// (parse/setup error or panic); `Some(_)` after a normal run.
    result: Option<runner::ScenarioResult>,
    /// Captured stderr-style output, replayed verbatim once all workers
    /// finish.
    output: Vec<u8>,
    /// Fatal error, if any (parse/setup failure or panic).
    error: Option<anyhow::Error>,
}

/// Read-only context shared across parallel workers.
struct RunContext<'a> {
    project_root: &'a Path,
    fixtures_dir: &'a Path,
    reporter: &'a dyn reporter::Reporter,
    keep: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    scenarios: Vec<PathBuf>,
    no_interactive: bool,
    verbosity: reporter::Verbosity,
    step: Option<usize>,
    loop_count: Option<usize>,
    keep: bool,
    setup_only: bool,
    list: bool,
    show: bool,
    checks: bool,
    jobs: usize,
) -> Result<()> {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask should be inside project root")
        .to_path_buf();
    let scenarios_dir = project_root.join("tests/manual/scenarios");
    let fixtures_dir = project_root.join("tests/manual/fixtures/repos");

    if list {
        return list_scenarios(&scenarios_dir);
    }

    if show {
        let scenario_files = if scenarios.is_empty() {
            anyhow::bail!("--show requires a scenario name");
        } else {
            resolve_scenario_paths(&scenarios, &scenarios_dir)?
        };
        for path in &scenario_files {
            show_scenario(path, checks)?;
        }
        return Ok(());
    }

    if loop_count.is_some() && step.is_none() {
        anyhow::bail!("--loop-count requires --step");
    }

    let scenario_files = if scenarios.is_empty() {
        discover_scenarios_recursive(&scenarios_dir)?
            .into_iter()
            .map(|(_, path)| path)
            .collect()
    } else {
        resolve_scenario_paths(&scenarios, &scenarios_dir)?
    };

    if scenario_files.is_empty() {
        anyhow::bail!("No scenario files found in {}", scenarios_dir.display());
    }

    let is_interactive = !no_interactive && std::io::stdin().is_terminal();

    if jobs > 1 {
        if is_interactive {
            anyhow::bail!(
                "--jobs/--parallel is only supported in non-interactive mode (pass --ci or run from a non-TTY)"
            );
        }
        if setup_only {
            anyhow::bail!("--jobs/--parallel is incompatible with --setup-only");
        }
    }

    let cleanup_set = setup_cleanup_handler(keep);

    // No leading eprintln! here — the pretty reporter's scenario_header
    // owns the inter-scenario blank line, including the one before the very
    // first scenario.

    let reporter = reporter::reporter_for(verbosity);

    // Interactive and --setup-only stay on the streaming serial path. Both
    // have semantics — TTY ownership for interactive, `println!` of work_dir
    // for shell capture in setup-only — that don't fit the buffered worker
    // model used by the parallel scheduler.
    if is_interactive || setup_only {
        return run_serial(
            &scenario_files,
            &project_root,
            &fixtures_dir,
            &cleanup_set,
            reporter.as_ref(),
            verbosity,
            step,
            loop_count,
            keep,
            setup_only,
            is_interactive,
        );
    }

    // Non-interactive CI path — always goes through the parallel scheduler,
    // even at `jobs == 1` (a 1-thread rayon pool). Output is buffered per
    // scenario and flushed in input order.
    run_parallel(
        &scenario_files,
        &scenarios_dir,
        &project_root,
        &fixtures_dir,
        &cleanup_set,
        reporter.as_ref(),
        keep,
        jobs,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_parallel(
    scenario_files: &[PathBuf],
    scenarios_dir: &Path,
    project_root: &Path,
    fixtures_dir: &Path,
    cleanup_set: &CleanupSet,
    reporter: &dyn reporter::Reporter,
    keep: bool,
    jobs: usize,
) -> Result<()> {
    let ctx = RunContext {
        project_root,
        fixtures_dir,
        reporter,
        keep,
    };

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .context("building rayon thread pool")?;

    let run_start = std::time::Instant::now();
    let mut outcomes: Vec<ScenarioOutcome> = pool.install(|| {
        scenario_files
            .par_iter()
            .enumerate()
            .map(|(idx, path)| run_one_scenario(idx, path, &ctx, cleanup_set))
            .collect()
    });
    let duration = run_start.elapsed();

    // Restore input order before printing and aggregating, so output and
    // stats are deterministic regardless of completion order.
    outcomes.sort_by_key(|o| o.index);

    let stderr = std::io::stderr();
    {
        let mut lock = stderr.lock();
        for o in &outcomes {
            lock.write_all(&o.output)?;
        }
    }

    let stats = aggregate_outcomes(&outcomes, scenarios_dir, duration, Some(jobs));

    let total_failed = stats.summary.steps_failed;
    let error_count = stats.summary.errors.len();
    {
        let mut lock = stderr.lock();
        reporter.run_summary(&mut lock, &stats.summary)?;
    }

    if error_count > 0 {
        anyhow::bail!("{} scenario(s) hit a fatal error", error_count);
    }
    if total_failed > 0 {
        anyhow::bail!(
            "{total_failed} step(s) failed across {} scenarios",
            stats.summary.scenarios_total
        );
    }

    Ok(())
}

/// Aggregated counters + summary records derived from a sorted outcome list.
///
/// Output borrows lifetimes from the `outcomes` slice, so callers must keep
/// the slice alive until the summary is consumed.
struct OutcomeStats<'a> {
    summary: reporter::RunSummary<'a>,
}

fn aggregate_outcomes<'a>(
    outcomes: &'a [ScenarioOutcome],
    scenarios_dir: &Path,
    duration: std::time::Duration,
    parallel_jobs: Option<usize>,
) -> OutcomeStats<'a> {
    let mut scenarios_total = 0usize;
    let mut scenarios_passed = 0usize;
    let mut scenarios_failed = 0usize;
    let mut steps_total = 0usize;
    let mut steps_passed = 0usize;
    let mut steps_failed = 0usize;
    let mut failed = Vec::new();
    let mut errors = Vec::new();

    for o in outcomes {
        match (&o.result, &o.error) {
            (Some(sr), _) => {
                scenarios_total += 1;
                steps_total += sr.steps;
                steps_passed += sr.passed;
                steps_failed += sr.failed;
                if sr.failed > 0 {
                    scenarios_failed += 1;
                    let display_path = o
                        .source
                        .strip_prefix(scenarios_dir)
                        .unwrap_or(&o.source)
                        .display()
                        .to_string();
                    failed.push(reporter::FailedScenarioRecord {
                        name: o.name.as_str(),
                        display_path,
                        reproduce_token: reporter::reproduce_token(&o.source, scenarios_dir),
                        duration: sr.duration,
                        failing_step: sr.failing_step.as_ref(),
                    });
                } else {
                    scenarios_passed += 1;
                }
            }
            (None, Some(err)) => errors.push(reporter::ScenarioErrorRecord {
                name: o.name.as_str(),
                error: format!("{err:#}"),
            }),
            (None, None) => {}
        }
    }

    OutcomeStats {
        summary: reporter::RunSummary {
            scenarios_total,
            scenarios_passed,
            scenarios_failed,
            steps_total,
            steps_passed,
            steps_failed,
            duration,
            parallel_jobs,
            failed,
            errors,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_serial(
    scenario_files: &[PathBuf],
    project_root: &Path,
    fixtures_dir: &Path,
    cleanup_set: &CleanupSet,
    reporter: &dyn reporter::Reporter,
    verbosity: reporter::Verbosity,
    step: Option<usize>,
    loop_count: Option<usize>,
    keep: bool,
    setup_only: bool,
    is_interactive: bool,
) -> Result<()> {
    for path in scenario_files {
        let scenario = load_scenario(path, fixtures_dir)?;

        // Register the sandbox path before touching disk so a SIGINT during
        // `Sandbox::create_at` still leaves a tracked path the cleanup handler
        // can `rm -rf`.
        let base_dir = pick_sandbox_base_dir(&scenario)?;
        let _guard = CleanupGuard::new(Arc::clone(cleanup_set), base_dir.clone());
        let mut sb = sandbox::Sandbox::create_at(&scenario, base_dir, keep || setup_only)?;
        let executor = daft_executor::DaftCommandExecutor::new_for_sandbox(&mut sb, project_root)?;

        for repo_spec in &scenario.repos {
            repo_gen::generate_repo(repo_spec, &sb.remotes_dir)?;
            sb.register_remote(&repo_spec.name);
        }
        sb.create_template()?;

        if setup_only {
            let run_until = step
                .unwrap_or(scenario.steps.len())
                .min(scenario.steps.len());
            for (i, s) in scenario.steps.iter().take(run_until).enumerate() {
                eprint!(
                    "{} {} ... ",
                    term_styles::blue(&format!("[{}/{}]", i + 1, run_until)),
                    &s.name
                );
                runner::execute_step(s, &sb, &executor, true)?;
                eprintln!("{}", term_styles::green("ok"));
            }
            eprintln!();
            eprintln!("Test environment ready at: {}", sb.work_dir.display());
            // Print work dir to stdout for shell wrapper to capture for cd.
            println!("{}", sb.work_dir.display());
            continue;
        }

        if is_interactive {
            interactive::run_interactive(
                &scenario, &sb, &executor, reporter, verbosity, step, loop_count,
            )?;
        }

        let mut stderr = std::io::stderr().lock();
        if keep {
            reporter.cleanup_note(
                &mut stderr,
                &format!("Test environment kept at: {}", sb.base_dir.display()),
            )?;
        } else {
            match sb.cleanup() {
                Ok(()) => reporter.cleanup_note(&mut stderr, "Cleaned up test environment.")?,
                Err(e) => {
                    reporter.cleanup_note(&mut stderr, &format!("Warning: cleanup failed: {e}"))?
                }
            }
        }
    }

    Ok(())
}

fn load_scenario(path: &Path, fixtures_dir: &Path) -> Result<schema::Scenario> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read scenario: {}", path.display()))?;
    let raw: schema::RawScenario = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse scenario: {}", path.display()))?;
    let repos = resolve_repos(raw.repos, fixtures_dir)
        .with_context(|| format!("Failed to resolve repos in: {}", path.display()))?;
    // Canonicalize so `reproduce_token` can reliably strip the absolute
    // scenarios_dir prefix regardless of how the path was spelled on the
    // command line (relative, absolute, `..`-laden). Fall back to the raw
    // path if canonicalize fails for any reason — we'd rather report a
    // funny-looking reproduce token than refuse to load the scenario.
    let source_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let step_lines = extract_step_lines(&content);
    let mut steps = raw.steps;
    for (i, step) in steps.iter_mut().enumerate() {
        step.line = step_lines.get(i).copied();
    }
    Ok(schema::Scenario {
        name: raw.name,
        description: raw.description,
        repos,
        env: raw.env,
        steps,
        source_path,
    })
}

/// Extract 1-indexed line numbers for each step in the scenario YAML.
///
/// Walks the file once. Locates the top-level `steps:` key, captures its
/// indent column, then records every subsequent line whose trimmed prefix is
/// `- name:` at indent strictly greater than the steps block. Stops when a
/// non-blank, non-comment line returns to the steps-block indent (start of
/// the next top-level key).
///
/// Pragmatic, not a YAML parser — we own every scenario file, the schema
/// requires `name:` on every step, and `- name:` does not appear at the
/// steps-list indent anywhere else (it can appear deeper, inside `repos:`
/// blocks for example, but the indent check rules those out). If the scan
/// returns fewer lines than the parsed step count, trailing steps get
/// `line: None` — better than panicking on a YAML the parser already
/// accepted.
fn extract_step_lines(yaml_text: &str) -> Vec<usize> {
    let mut step_lines = Vec::new();
    let mut steps_indent: Option<usize> = None;

    for (idx, line) in yaml_text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();

        match steps_indent {
            None => {
                // Looking for the `steps:` block. Top-level only — accept any
                // indent (canonical scenarios use 0, but be flexible).
                if trimmed == "steps:" || trimmed.starts_with("steps:") {
                    let rest = trimmed.trim_start_matches("steps:").trim();
                    if rest.is_empty() {
                        steps_indent = Some(indent);
                    }
                }
            }
            Some(block_indent) => {
                if indent <= block_indent {
                    // Returned to top level — done with the steps block.
                    break;
                }
                if trimmed.starts_with("- name:") {
                    step_lines.push(idx + 1);
                }
            }
        }
    }

    step_lines
}

fn run_one_scenario(
    index: usize,
    path: &Path,
    ctx: &RunContext<'_>,
    cleanup_set: &CleanupSet,
) -> ScenarioOutcome {
    let scenario = match load_scenario(path, ctx.fixtures_dir) {
        Ok(s) => s,
        Err(e) => {
            return ScenarioOutcome {
                index,
                name: path.display().to_string(),
                source: path.to_path_buf(),
                result: None,
                output: Vec::new(),
                error: Some(e),
            };
        }
    };

    let name = scenario.name.clone();
    let source = scenario.source_path.clone();

    // `catch_unwind` so a single panicking scenario doesn't poison the rayon
    // pool — the worker reports the panic and the pool keeps draining the
    // remaining scenarios.
    let work = std::panic::catch_unwind(AssertUnwindSafe(|| {
        run_one_scenario_inner(&scenario, ctx, cleanup_set)
    }));

    match work {
        Ok(Ok((sr, buf))) => ScenarioOutcome {
            index,
            name,
            source,
            result: Some(sr),
            output: buf,
            error: None,
        },
        Ok(Err(err)) => ScenarioOutcome {
            index,
            name,
            source,
            result: None,
            output: Vec::new(),
            error: Some(err),
        },
        Err(payload) => {
            let msg = panic_payload_to_string(&payload);
            ScenarioOutcome {
                index,
                name,
                source,
                result: None,
                output: Vec::new(),
                error: Some(anyhow::anyhow!("scenario panicked: {msg}")),
            }
        }
    }
}

fn run_one_scenario_inner(
    scenario: &schema::Scenario,
    ctx: &RunContext<'_>,
    cleanup_set: &CleanupSet,
) -> Result<(runner::ScenarioResult, Vec<u8>)> {
    // Pre-register the sandbox path so a SIGINT during create_at still leaves
    // a tracked path the cleanup handler can `rm -rf`. See [`setup_cleanup_handler`]
    // for the matching drain-loop logic.
    let base_dir = pick_sandbox_base_dir(scenario)?;
    let _guard = CleanupGuard::new(Arc::clone(cleanup_set), base_dir.clone());
    let mut sb = sandbox::Sandbox::create_at(scenario, base_dir, ctx.keep)?;
    let executor = daft_executor::DaftCommandExecutor::new_for_sandbox(&mut sb, ctx.project_root)?;

    for repo_spec in &scenario.repos {
        repo_gen::generate_repo(repo_spec, &sb.remotes_dir)?;
        sb.register_remote(&repo_spec.name);
    }
    sb.create_template()?;

    let mut buf: Vec<u8> = Vec::new();
    let result = runner::run_non_interactive(scenario, &sb, &executor, ctx.reporter, &mut buf)?;

    // Opt-in per-scenario timing for the bench harness. Lines are
    // grep-friendly and live inside the scenario's buffered output so they
    // print in input order alongside the scenario's own report. Reuses the
    // duration the runner already tracks so we have a single source of truth.
    if std::env::var_os("DAFT_MANUAL_TEST_EMIT_TIMING").is_some() {
        writeln!(
            &mut buf,
            "[bench] scenario={:?} elapsed_ms={}",
            scenario.name,
            result.duration.as_millis()
        )?;
    }

    if ctx.keep {
        ctx.reporter.cleanup_note(
            &mut buf,
            &format!("Test environment kept at: {}", sb.base_dir.display()),
        )?;
    } else {
        match sb.cleanup() {
            // Suppress the "Cleaned up..." chatter on green scenarios — the
            // cleanup still happens, but the line was noise on the happy path.
            // Failures keep it so the failure-detail block visibly attaches to
            // its scenario rather than running into the next one.
            Ok(()) if result.failed == 0 => {}
            Ok(()) => ctx
                .reporter
                .cleanup_note(&mut buf, "Cleaned up test environment.")?,
            Err(e) => ctx
                .reporter
                .cleanup_note(&mut buf, &format!("Warning: cleanup failed: {e}"))?,
        }
    }

    Ok((result, buf))
}

fn panic_payload_to_string(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

/// Resolve scenario arguments to full paths.
///
/// Each argument can be:
/// - A full/relative file path (used as-is if it exists)
/// - A directory path (all scenarios in that directory)
/// - A namespaced name like `exec:checkout-single` (resolved to `exec/checkout-single.yml`)
/// - A bare name like `clone-basic` (resolved in top-level, then searched recursively)
fn resolve_scenario_paths(args: &[PathBuf], scenarios_dir: &PathBuf) -> Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();
    for arg in args {
        if arg.exists() && arg.is_dir() {
            resolved.extend(discover_scenarios(&arg.to_path_buf())?);
            continue;
        }
        if arg.exists() {
            resolved.push(arg.clone());
            continue;
        }

        let arg_str = arg.to_string_lossy();

        // Namespaced: "exec:checkout-single" → "exec/checkout-single.yml"
        if arg_str.contains(':') {
            let parts: Vec<&str> = arg_str.splitn(2, ':').collect();
            let (namespace, name) = (parts[0], parts[1]);
            let dir = scenarios_dir.join(namespace);
            if let Some(path) = find_scenario_file(&dir, name) {
                resolved.push(path);
                continue;
            }
            anyhow::bail!(
                "Scenario not found: '{arg_str}'\n  Looked in: {}",
                dir.display()
            );
        }

        // Bare name: try top-level first.
        let stem = arg.file_stem().unwrap_or(arg.as_os_str()).to_string_lossy();
        if let Some(path) = find_scenario_file(scenarios_dir, &stem) {
            resolved.push(path);
            continue;
        }

        // Fallback: search subdirectories recursively.
        if let Some(path) = find_scenario_recursive(scenarios_dir, &stem)? {
            resolved.push(path);
            continue;
        }

        anyhow::bail!(
            "Scenario not found: '{}'\n  Looked in: {} (and subdirectories)",
            arg.display(),
            scenarios_dir.display()
        );
    }
    Ok(resolved)
}

/// Try to find `<name>.yml` or `<name>.yaml` in the given directory.
fn find_scenario_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let yml = dir.join(format!("{name}.yml"));
    if yml.exists() {
        return Some(yml);
    }
    let yaml = dir.join(format!("{name}.yaml"));
    if yaml.exists() {
        return Some(yaml);
    }
    None
}

/// Recursively search subdirectories for a scenario by stem name.
fn find_scenario_recursive(dir: &Path, name: &str) -> Result<Option<PathBuf>> {
    if !dir.exists() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_scenario_file(&path, name) {
                return Ok(Some(found));
            }
            // Only one level deep for now.
        }
    }
    Ok(None)
}

/// Discover all `.yml` and `.yaml` files in the scenarios directory.
fn discover_scenarios(dir: &PathBuf) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut scenarios = Vec::new();
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading scenarios dir: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "yml" || ext == "yaml" {
                scenarios.push(path);
            }
        }
    }
    scenarios.sort();
    Ok(scenarios)
}

/// Resolve repo entries — inline specs pass through, fixture references are
/// loaded from the fixtures directory with `{{NAME}}` substitution.
fn resolve_repos(
    repos: Vec<schema::RepoEntry>,
    fixtures_dir: &Path,
) -> Result<Vec<schema::RepoSpec>> {
    let mut resolved = Vec::new();
    for entry in repos {
        match entry {
            schema::RepoEntry::Inline(spec) => resolved.push(spec),
            schema::RepoEntry::Fixture(fixture_ref) => {
                let fixture_path = fixtures_dir
                    .join(&fixture_ref.use_fixture)
                    .with_extension("yml");
                let raw_yaml = std::fs::read_to_string(&fixture_path).with_context(|| {
                    format!(
                        "reading fixture '{}' for repo '{}'",
                        fixture_ref.use_fixture, fixture_ref.name
                    )
                })?;
                let substituted = raw_yaml.replace("{{NAME}}", &fixture_ref.name);
                let fixture: schema::RepoFixture = serde_yaml::from_str(&substituted)
                    .with_context(|| {
                        format!(
                            "parsing fixture '{}' for repo '{}'",
                            fixture_ref.use_fixture, fixture_ref.name
                        )
                    })?;
                resolved.push(schema::RepoSpec {
                    name: fixture_ref.name,
                    default_branch: fixture.default_branch,
                    branches: fixture.branches,
                    daft_yml: fixture.daft_yml,
                    hook_scripts: fixture.hook_scripts,
                });
            }
        }
    }
    Ok(resolved)
}

/// List available scenarios with their name and description.
fn list_scenarios(dir: &PathBuf) -> Result<()> {
    use term_styles as styles;

    let scenarios = discover_scenarios_recursive(dir)?;
    if scenarios.is_empty() {
        eprintln!("No scenarios found in {}", dir.display());
        return Ok(());
    }

    eprintln!();
    eprintln!("  {}", styles::bold("Available scenarios:"));
    eprintln!();

    for (qualified_name, path) in &scenarios {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if let Ok(scenario) = serde_yaml::from_str::<schema::RawScenario>(&content) {
            let desc = scenario.description.as_deref().unwrap_or("");
            eprintln!(
                "  {} {}",
                styles::bold(qualified_name),
                styles::dim(&format!("- {}", scenario.name))
            );
            if !desc.is_empty() {
                eprintln!("    {}", styles::dim(desc));
            }
        }
    }
    eprintln!();

    Ok(())
}

/// Print a human-readable summary of a scenario without executing anything.
fn show_scenario(path: &Path, checks: bool) -> Result<()> {
    use term_styles as styles;

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read scenario: {}", path.display()))?;
    let raw: schema::RawScenario = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse scenario: {}", path.display()))?;

    // Header: scenario name + description
    eprintln!();
    eprintln!("{}", styles::bold(&raw.name));
    if let Some(desc) = &raw.description {
        eprintln!("  {}", styles::dim(desc));
    }
    eprintln!();

    // Steps
    for (i, step) in raw.steps.iter().enumerate() {
        eprintln!("  {}. {}", styles::blue(&(i + 1).to_string()), &step.name);

        // Print each line of the run command (multi-line commands get indented)
        let run_trimmed = step.run.trim();
        for (j, line) in run_trimmed.lines().enumerate() {
            if j == 0 {
                eprintln!("     {} {}", styles::dim("$"), line);
            } else {
                eprintln!("       {}", line);
            }
        }

        // Print cwd if set
        if let Some(cwd) = &step.cwd {
            eprintln!("     {}", styles::dim(&format!("cwd: {cwd}")));
        }

        // Print expectations if --checks is set
        if checks {
            if let Some(expect) = &step.expect {
                show_expectations(expect);
            }
        }

        eprintln!();
    }

    Ok(())
}

/// Format and print expectation checks for a step.
fn show_expectations(expect: &schema::Expectations) {
    use term_styles as styles;

    let mut lines: Vec<String> = Vec::new();

    if let Some(code) = expect.exit_code {
        lines.push(format!("exit_code: {code}"));
    }
    for dir in &expect.dirs_exist {
        lines.push(format!("dir exists: {dir}"));
    }
    for file in &expect.files_exist {
        lines.push(format!("file exists: {file}"));
    }
    for file in &expect.files_not_exist {
        lines.push(format!("file not exists: {file}"));
    }
    for fc in &expect.file_contains {
        lines.push(format!("file contains: {} => \"{}\"", fc.path, fc.content));
    }
    for fc in &expect.file_not_contains {
        lines.push(format!(
            "file not contains: {} => \"{}\"",
            fc.path, fc.content
        ));
    }
    for s in &expect.output_contains {
        lines.push(format!("output contains: \"{s}\""));
    }
    for s in &expect.output_not_contains {
        lines.push(format!("output not contains: \"{s}\""));
    }
    for wt in &expect.is_git_worktree {
        lines.push(format!("is worktree: {} (branch: {})", wt.dir, wt.branch));
    }
    for bc in &expect.branch_exists {
        lines.push(format!("branch exists: {} in {}", bc.branch, bc.repo));
    }

    if !lines.is_empty() {
        eprintln!("     {}", styles::dim("checks:"));
        for line in &lines {
            eprintln!("       {} {}", styles::dim("-"), styles::dim(line));
        }
    }
}

/// Discover all scenarios recursively, returning `(qualified_name, path)` pairs.
///
/// Top-level scenarios get bare names (e.g., `clone-basic`).
/// Subdirectory scenarios get namespaced names (e.g., `exec:checkout-single`).
fn discover_scenarios_recursive(dir: &PathBuf) -> Result<Vec<(String, PathBuf)>> {
    let mut scenarios = Vec::new();

    if !dir.exists() {
        return Ok(scenarios);
    }

    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading scenarios dir: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Subdirectory — namespace its scenarios.
            let namespace = entry.file_name().to_string_lossy().into_owned();
            let sub_scenarios = discover_scenarios(&path)?;
            for sub_path in sub_scenarios {
                let stem = sub_path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                scenarios.push((format!("{namespace}:{stem}"), sub_path));
            }
        } else if let Some(ext) = path.extension() {
            if ext == "yml" || ext == "yaml" {
                let stem = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                scenarios.push((stem, path));
            }
        }
    }
    scenarios.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(scenarios)
}

#[cfg(test)]
mod extract_step_lines_tests {
    use super::extract_step_lines;

    #[test]
    fn returns_each_step_start_line() {
        let yaml = "\
name: example
steps:
  - name: first
    run: \"true\"
  - name: second
    run: \"true\"
  - name: third
    run: \"true\"
";
        let lines = extract_step_lines(yaml);
        // Line 1: `name: example`. Line 2: `steps:`. Step `- name:` lines are 3, 5, 7.
        assert_eq!(lines, vec![3, 5, 7]);
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let yaml = "\
name: example

# explanatory comment

steps:
  # first step
  - name: first
    run: \"true\"

  - name: second
    run: \"true\"
";
        let lines = extract_step_lines(yaml);
        assert_eq!(lines, vec![7, 10]);
    }

    #[test]
    fn ignores_nested_name_keys_inside_repos_block() {
        // Phase 3.2's text scan must not confuse `- name:` entries in the
        // `repos:` block with step entries. The indent check (and the
        // `steps:` anchor) is what protects us.
        let yaml = "\
name: scenario
repos:
  - name: my-repo
    use_fixture: standard-remote
  - name: other-repo
    use_fixture: standard-remote
steps:
  - name: only-step
    run: \"true\"
";
        let lines = extract_step_lines(yaml);
        assert_eq!(lines, vec![8]);
    }

    #[test]
    fn stops_at_end_of_steps_block() {
        // A trailing top-level key after `steps:` must not get scanned for
        // pseudo-steps.
        let yaml = "\
name: scenario
steps:
  - name: only-step
    run: \"true\"
env:
  FOO: bar
";
        let lines = extract_step_lines(yaml);
        assert_eq!(lines, vec![3]);
    }

    #[test]
    fn returns_empty_for_scenarios_without_steps_block() {
        // Defensive: if the YAML somehow parses but has no `steps:` key,
        // the scan yields an empty Vec and every Step gets `line: None`.
        let yaml = "name: malformed\n";
        assert!(extract_step_lines(yaml).is_empty());
    }
}

#[cfg(test)]
mod cleanup_guard_tests {
    use super::*;

    fn empty_set() -> CleanupSet {
        Arc::new(Mutex::new(HashSet::new()))
    }

    #[test]
    fn guard_registers_on_new_and_unregisters_on_drop() {
        let set = empty_set();
        let path = PathBuf::from("/tmp/daft-manual-test-fake");

        {
            let _g = CleanupGuard::new(Arc::clone(&set), path.clone());
            assert!(set.lock().unwrap().contains(&path));
        }

        assert!(
            !set.lock().unwrap().contains(&path),
            "guard should have removed path on drop"
        );
    }

    #[test]
    fn guard_tracks_multiple_concurrent_sandboxes() {
        let set = empty_set();
        let p1 = PathBuf::from("/tmp/daft-manual-test-a");
        let p2 = PathBuf::from("/tmp/daft-manual-test-b");

        let g1 = CleanupGuard::new(Arc::clone(&set), p1.clone());
        let g2 = CleanupGuard::new(Arc::clone(&set), p2.clone());
        assert_eq!(set.lock().unwrap().len(), 2);

        drop(g1);
        assert!(set.lock().unwrap().contains(&p2));
        assert!(!set.lock().unwrap().contains(&p1));

        drop(g2);
        assert!(set.lock().unwrap().is_empty());
    }

    #[test]
    fn guard_drop_during_panic_still_unregisters() {
        let set = empty_set();
        let path = PathBuf::from("/tmp/daft-manual-test-panicky");

        let result = std::panic::catch_unwind({
            let set = Arc::clone(&set);
            let path = path.clone();
            move || {
                let _g = CleanupGuard::new(set, path);
                panic!("worker exploded");
            }
        });

        assert!(result.is_err());
        assert!(
            !set.lock().unwrap().contains(&path),
            "guard must unregister even when the worker panics"
        );
    }
}
