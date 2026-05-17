pub mod env;
pub mod interactive;
pub mod repo_gen;
pub mod runner;
pub mod schema;

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    verbose: bool,
    keep: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    scenarios: Vec<PathBuf>,
    no_interactive: bool,
    verbose: bool,
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

    eprintln!();

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
            step,
            loop_count,
            verbose,
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
        &project_root,
        &fixtures_dir,
        &cleanup_set,
        verbose,
        keep,
        jobs,
    )
}

fn run_parallel(
    scenario_files: &[PathBuf],
    project_root: &Path,
    fixtures_dir: &Path,
    cleanup_set: &CleanupSet,
    verbose: bool,
    keep: bool,
    jobs: usize,
) -> Result<()> {
    let ctx = RunContext {
        project_root,
        fixtures_dir,
        verbose,
        keep,
    };

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .context("building rayon thread pool")?;

    let mut outcomes: Vec<ScenarioOutcome> = pool.install(|| {
        scenario_files
            .par_iter()
            .enumerate()
            .map(|(idx, path)| run_one_scenario(idx, path, &ctx, cleanup_set))
            .collect()
    });

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

    let mut total_scenarios = 0usize;
    let mut total_steps = 0usize;
    let mut total_passed = 0usize;
    let mut total_failed = 0usize;
    let mut failed_scenarios: Vec<String> = Vec::new();
    let mut errors: Vec<(String, anyhow::Error)> = Vec::new();

    for o in outcomes {
        match (o.result, o.error) {
            (Some(sr), _) => {
                total_scenarios += 1;
                total_steps += sr.steps;
                total_passed += sr.passed;
                total_failed += sr.failed;
                if sr.failed > 0 {
                    failed_scenarios.push(o.name);
                }
            }
            (None, Some(err)) => errors.push((o.name, err)),
            (None, None) => {}
        }
    }

    use term_styles as styles;

    eprintln!();
    eprintln!(
        "{} scenarios, {} steps, {} passed, {} failed",
        total_scenarios,
        total_steps,
        styles::green(&total_passed.to_string()),
        if total_failed > 0 {
            styles::red(&total_failed.to_string())
        } else {
            "0".into()
        }
    );
    for name in &failed_scenarios {
        eprintln!("{} {}", styles::red("x"), name);
    }
    for (name, err) in &errors {
        eprintln!("{} {}: {err:#}", styles::red("ERROR"), name);
    }
    eprintln!();

    if !errors.is_empty() {
        anyhow::bail!("{} scenario(s) hit a fatal error", errors.len());
    }
    if total_failed > 0 {
        anyhow::bail!("{total_failed} step(s) failed across {total_scenarios} scenarios");
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_serial(
    scenario_files: &[PathBuf],
    project_root: &Path,
    fixtures_dir: &Path,
    cleanup_set: &CleanupSet,
    step: Option<usize>,
    loop_count: Option<usize>,
    verbose: bool,
    keep: bool,
    setup_only: bool,
    is_interactive: bool,
) -> Result<()> {
    for path in scenario_files {
        let scenario = load_scenario(path, fixtures_dir)?;

        // Register the sandbox path before touching disk so a SIGINT during
        // `TestEnv::create_at` still leaves a tracked path the cleanup handler
        // can `rm -rf`.
        let base_dir = env::next_sandbox_base_dir(&scenario)?;
        let _guard = CleanupGuard::new(Arc::clone(cleanup_set), base_dir.clone());
        let mut test_env =
            env::TestEnv::create_at(&scenario, project_root, base_dir, keep || setup_only)?;

        for repo_spec in &scenario.repos {
            repo_gen::generate_repo(repo_spec, &test_env.remotes_dir)?;
            test_env.register_remote(&repo_spec.name);
        }
        test_env.create_template()?;

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
                runner::execute_step(s, &test_env, true)?;
                eprintln!("{}", term_styles::green("ok"));
            }
            eprintln!();
            eprintln!("Test environment ready at: {}", test_env.work_dir.display());
            // Print work dir to stdout for shell wrapper to capture for cd.
            println!("{}", test_env.work_dir.display());
            continue;
        }

        if is_interactive {
            interactive::run_interactive(&scenario, &test_env, step, loop_count, verbose)?;
        }

        if keep {
            eprintln!(
                "  Test environment kept at: {}",
                test_env.base_dir.display()
            );
        } else {
            match test_env.cleanup() {
                Ok(()) => eprintln!("Cleaned up test environment."),
                Err(e) => eprintln!("  Warning: cleanup failed: {e}"),
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
    Ok(schema::Scenario {
        name: raw.name,
        description: raw.description,
        repos,
        env: raw.env,
        steps: raw.steps,
    })
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
                result: None,
                output: Vec::new(),
                error: Some(e),
            };
        }
    };

    let name = scenario.name.clone();

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
            result: Some(sr),
            output: buf,
            error: None,
        },
        Ok(Err(err)) => ScenarioOutcome {
            index,
            name,
            result: None,
            output: Vec::new(),
            error: Some(err),
        },
        Err(payload) => {
            let msg = panic_payload_to_string(&payload);
            ScenarioOutcome {
                index,
                name,
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
    let base_dir = env::next_sandbox_base_dir(scenario)?;
    let _guard = CleanupGuard::new(Arc::clone(cleanup_set), base_dir.clone());
    let mut test_env = env::TestEnv::create_at(scenario, ctx.project_root, base_dir, ctx.keep)?;

    for repo_spec in &scenario.repos {
        repo_gen::generate_repo(repo_spec, &test_env.remotes_dir)?;
        test_env.register_remote(&repo_spec.name);
    }
    test_env.create_template()?;

    let mut buf: Vec<u8> = Vec::new();
    let scenario_start = std::time::Instant::now();
    let result = runner::run_non_interactive(scenario, &test_env, ctx.verbose, &mut buf)?;
    let elapsed_ms = scenario_start.elapsed().as_millis();

    // Opt-in per-scenario timing for the bench harness. Lines are
    // grep-friendly and live inside the scenario's buffered output so they
    // print in input order alongside the scenario's own report.
    if std::env::var_os("DAFT_MANUAL_TEST_EMIT_TIMING").is_some() {
        writeln!(
            &mut buf,
            "[bench] scenario={:?} elapsed_ms={elapsed_ms}",
            scenario.name
        )?;
    }

    if ctx.keep {
        writeln!(
            &mut buf,
            "  Test environment kept at: {}",
            test_env.base_dir.display()
        )?;
    } else {
        match test_env.cleanup() {
            Ok(()) => writeln!(&mut buf, "Cleaned up test environment.")?,
            Err(e) => writeln!(&mut buf, "  Warning: cleanup failed: {e}")?,
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
