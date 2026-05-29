pub mod cow_copy;
pub mod daft_executor;
pub mod executor;
pub mod fixture_cache;
pub mod interactive;
pub mod progress;
pub mod repo_gen;
pub mod reporter;
pub mod runner;
pub mod sandbox;
pub mod schema;

use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::io::{IsTerminal, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Cheap scenario metadata extracted by [`peek_scenario_metadata`].
/// Used purely for column-width sizing in the progress region and
/// footer — full YAML parsing remains the worker's job.
#[derive(Debug, Default, Clone)]
pub(crate) struct ScenarioMeta {
    /// Top-level `name:` value, with surrounding quotes stripped.
    pub name: Option<String>,
    /// Step names in declaration order. Empty if the `steps:` block
    /// couldn't be parsed.
    pub step_names: Vec<String>,
}

/// Cheaply extract a scenario's display name and step names from its
/// YAML without a full parse. Scans the file once with the same
/// indent-aware logic as `extract_step_lines` so it doesn't confuse
/// `- name:` keys inside the `repos:` block with step entries.
///
/// Returns `Some` whenever the file is readable — the caller decides
/// what to do with empty fields. Conservative on quoting (strips `"`
/// and `'`).
pub(crate) fn peek_scenario_metadata(path: &Path) -> Option<ScenarioMeta> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut meta = ScenarioMeta::default();
    let mut steps_indent: Option<usize> = None;
    let mut name_locked = false;

    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - trimmed.len();

        // Top-level `name:` only — once we descend into a block, stop
        // looking. This avoids matching the runner-output fixture's
        // `name:` inside an embedded YAML literal.
        if !name_locked && indent == 0 {
            if let Some(rest) = trimmed.strip_prefix("name:") {
                let value = rest.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    meta.name = Some(value.to_string());
                    name_locked = true;
                    continue;
                }
            }
        }

        match steps_indent {
            None => {
                if (trimmed == "steps:" || trimmed.starts_with("steps:"))
                    && trimmed.trim_start_matches("steps:").trim().is_empty()
                {
                    steps_indent = Some(indent);
                }
            }
            Some(block_indent) => {
                if indent <= block_indent {
                    // Returned to a same-or-shallower indent — end of
                    // steps block.
                    break;
                }
                if let Some(rest) = trimmed.strip_prefix("- name:") {
                    let value = rest.trim().trim_matches('"').trim_matches('\'');
                    if !value.is_empty() {
                        meta.step_names.push(value.to_string());
                    }
                }
            }
        }
    }

    Some(meta)
}

/// Compute the widest `done/total` step counter across the discovered
/// set, in chars. Used by the live progress region to pad each worker
/// row's step counter so the time counter to its right lands at a stable
/// column across in-flight rows. The widest counter for a `T`-step
/// scenario is `T/T`, so its width is `2 * digit_count(T) + 1`.
fn max_step_counter_width(metas: &[ScenarioMeta]) -> usize {
    metas
        .iter()
        .map(|m| {
            let d = digit_count(m.step_names.len());
            2 * d + 1
        })
        .max()
        .unwrap_or(0)
}

// Twin of `digit_count` in `progress/indicatif_sink.rs` (which takes `u64`
// to match indicatif's counters). Keep the two in sync if the formula
// changes.
fn digit_count(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n.ilog10() as usize) + 1
    }
}

/// Pick a base directory for a scenario sandbox.
///
/// Honors the `DAFT_MANUAL_TEST_BASE` env var as an opt-in override for
/// managed test directories (e.g. a ramdisk under `sandbox/test/`). When set,
/// the path is `<base>/<slug>` where the slug is derived from the scenario's
/// source path (`<parent-dir>-<file-stem>`), making it unique even when two
/// scenarios share the same `name:` field — which they do today, see
/// `layout/transform-matrix-nested-to-sibling.yml` and
/// `layout/transform-nested-to-sibling.yml`, both named "Transform nested to
/// sibling". Otherwise falls back to a unique `/tmp/daft-manual-test-*` path
/// allocated by [`sandbox::alloc_default_base_dir`].
///
/// Lives at this layer (not in `sandbox::`) so the runner core stays free of
/// daft-named env vars; renaming `DAFT_MANUAL_TEST_BASE` is a concern for the
/// eventual runner spin-out.
fn pick_sandbox_base_dir(scenario: &schema::Scenario) -> Result<PathBuf> {
    if let Ok(base) = std::env::var("DAFT_MANUAL_TEST_BASE") {
        return Ok(PathBuf::from(base).join(scenario_base_slug(scenario)));
    }
    sandbox::alloc_default_base_dir()
}

/// Allocate the run-wide fixture-cache root directory.
///
/// When `DAFT_MANUAL_TEST_BASE` is set, the cache lives under that root with
/// a leading-underscore prefix so it sorts above the per-scenario sandbox
/// directories in a listing and is visually distinct. The PID suffix makes
/// two concurrent runs against the same base happy. Otherwise the cache
/// gets its own timestamp-PID-counter path under `/tmp`, mirroring the
/// shape of [`sandbox::alloc_default_base_dir`].
///
/// Lives in this layer (not in `sandbox::`) for the same reason as
/// [`pick_sandbox_base_dir`]: the runner's daft-named env-var awareness
/// stays out of the sandbox module so the spin-out story is clean.
fn alloc_fixture_cache_dir() -> Result<PathBuf> {
    if let Ok(base) = std::env::var("DAFT_MANUAL_TEST_BASE") {
        let pid = std::process::id();
        return Ok(PathBuf::from(base).join(format!("_fixture-cache-{pid}")));
    }
    // Reuse the timestamp-PID-counter slug from the default-base helper but
    // tag the suffix so a stray cache directory is identifiable in `/tmp`.
    // This intentionally burns one `SANDBOX_COUNTER` slot — the returned
    // path is never materialised under that name, only used to compose a
    // sibling-with-suffix path. Accounting-counted sandboxes will run one
    // ahead of the visible scenario count; the trade-off is keeping the
    // uniqueness contract centralised in `alloc_default_base_dir` rather
    // than duplicating the nanos-PID-counter logic here.
    let base = sandbox::alloc_default_base_dir()?;
    let file_name = base
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("daft-manual-test-fixture-cache");
    let parent = base.parent().unwrap_or(Path::new("/tmp"));
    Ok(parent.join(format!("{file_name}-fixture-cache")))
}

/// Build a stable, collision-free path component for a scenario's sandbox
/// directory under `DAFT_MANUAL_TEST_BASE`. Prefers a source-path slug
/// (`<parent-dir>-<file-stem>`) since scenario file paths are unique by
/// construction. Falls back to the scenario name for unit tests that build
/// `Scenario` directly without populating `source_path`.
fn scenario_base_slug(scenario: &schema::Scenario) -> String {
    if !scenario.source_path.as_os_str().is_empty() {
        let parent = scenario
            .source_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("");
        let stem = scenario
            .source_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        // Both pieces feed into `shell_safe_slug` so any path-component
        // surprise (rare but possible — e.g. branch-named scenario files)
        // still produces a safe path.
        return shell_safe_slug(&format!("{parent}-{stem}"));
    }
    shell_safe_slug(&scenario.name)
}

/// Normalise a scenario name into a directory component safe to embed in a
/// `bash -c "cd <path> && …"` step. Scenarios contain parentheses, slashes,
/// colons, and other shell-active characters in their human-readable names
/// (e.g. `"Transform sanitizes branch slashes (contained to contained-flat)"`).
/// Default `/tmp/daft-manual-test-*` sandboxes never see these because the
/// path is timestamp/PID based, but any caller that points
/// `DAFT_MANUAL_TEST_BASE` somewhere stable and joins the scenario name
/// (`#512`'s RAM-disk task is the first such caller in tree) needs the
/// component scrubbed.
///
/// Rules: lower-case, lossy ASCII; any character outside `[a-z0-9._]` becomes
/// `-`; runs of `-` collapse to a single `-`; leading and trailing `-` are
/// trimmed. Empty input yields `unnamed-scenario` so the caller still gets a
/// valid path component.
fn shell_safe_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        let safe = matches!(ch, 'a'..='z' | '0'..='9' | '.' | '_');
        if safe {
            out.push(ch);
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unnamed-scenario".to_string()
    } else {
        trimmed.to_string()
    }
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

/// Register a Ctrl+C handler with two-press semantics.
///
/// **First press (soft cancel):** sets the shared [`progress::InterruptFlag`]
/// and prints a short banner. The handler returns without exiting; the
/// parallel scheduler's workers observe the flag between steps and bail with
/// [`reporter::ScenarioStatus::Cancelled`], their `Sandbox::drop` cleans up
/// each sandbox naturally, and the main thread reaches the run's end in
/// `run_parallel` and falls through to the `process::exit(130)` at the
/// bottom of `run`.
///
/// **Second press (hard exit):** the handler short-circuits to the legacy
/// emergency-cleanup path — `rm -rf` every registered sandbox and
/// `process::exit(130)`. This is the escape hatch when a worker is stuck
/// inside a slow subprocess (e.g. a long git clone) and can't react to the
/// interrupt flag in a reasonable time.
///
/// Returns a shared registry the run loop populates as scenarios start and
/// clears as they finish. The registry is read on the hard-exit path; under
/// soft cancel it stays in sync via the workers' own `CleanupGuard` drops.
fn setup_cleanup_handler(keep: bool, interrupt: progress::InterruptFlag) -> CleanupSet {
    let set: CleanupSet = Arc::new(Mutex::new(HashSet::new()));
    let handler_set = Arc::clone(&set);

    ctrlc::set_handler(move || {
        let already_interrupted = interrupt.set();
        if !already_interrupted {
            // First press: soft cancel. Workers see the flag between steps,
            // bail with `Cancelled`, and their `Sandbox::drop` cleans up.
            // No emergency cleanup, no `process::exit` — let the run wind
            // down naturally so the cancelled count and final summary
            // print correctly.
            //
            // Deliberately silent: any `eprintln!` here pushes the bar's
            // current state into scrollback as "ghost" rows. Feedback that
            // cancellation registered comes through two channels instead:
            //   1. The bar's summary message gains a `(cancelling)` suffix
            //      via `update_summary_msg` reading the flag (the bar's
            //      steady_tick refreshes ~10/s).
            //   2. The orchestrator calls `notify_cancelling` on the sink
            //      to force the suffix to appear on the very next tick
            //      rather than waiting for a worker to bail.
            // `disable_raw_mode` stays — it's idempotent and writes nothing
            // to the terminal.
            let _ = crossterm::terminal::disable_raw_mode();
            return;
        }

        // Second press: hard exit. Restore terminal, drain the cleanup
        // registry under the lock, and force-exit with 130.
        //
        // Workers in flight fall into two camps when we hard-exit:
        //
        // 1. Past `CleanupGuard::new` — their `base_dir` is in the set.
        //    Drain captures it; we `rm -rf` to remove whatever they
        //    built so far. Their subprocesses (`git`, `cp`) may still
        //    be running and may RECREATE entries at the same path
        //    between our `rm` and `process::exit` below, which is why
        //    we re-rm in a short loop while holding the lock.
        // 2. About to call `CleanupGuard::new` — they block at
        //    `set.lock()` because we hold the lock to process exit,
        //    then die with the process. They never touch disk, so no
        //    leak.
        //
        // Holding the lock for the whole sequence prevents new
        // registrations, so the `known` set captures the entire universe
        // of paths that could possibly still have on-disk presence.
        let _ = crossterm::terminal::disable_raw_mode();
        // Collapse the live progress region first — removing every bar so its
        // 200 ms steady tick can't repaint over the clear while the cleanup loop
        // below runs (the stranded live-totals-line bug). This is the path the
        // cooperative teardown can't reach: when every worker is wedged in a slow
        // subprocess, no completion fires `notify_cancelling`, so the region is
        // still up when this second-press handler runs. `finalize_region_for_exit`
        // returns the totals "end screen" (completion-map + passed/failed/
        // cancelled breakdown, no live `running` count) to print where the live
        // line was; empty on a non-TTY run or if a soft cancel already tore the
        // region down.
        let totals = progress::finalize_region_for_exit();
        eprintln!();
        for line in &totals {
            eprintln!("{line}");
        }
        if !totals.is_empty() {
            eprintln!();
        }
        eprintln!("{}", term_styles::dim("Forced exit. Cleaning up..."));
        if !keep {
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
    /// Run-wide cache of pre-generated fixture remotes. Each
    /// `RepoSpec` carrying `fixture_source = Some(...)` is materialised by
    /// `cow_copy`-ing from the cache into the scenario sandbox; inline
    /// specs still go through [`repo_gen::generate_repo`].
    fixture_cache: &'a fixture_cache::FixtureCache,
    reporter: &'a dyn reporter::Reporter,
    progress: &'a dyn progress::ProgressSink,
    /// Cooperative cancellation flag. Workers check between steps inside
    /// `run_non_interactive`; on transition false→true they bail with
    /// [`reporter::ScenarioStatus::Cancelled`].
    interrupt: &'a progress::InterruptFlag,
    keep: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    scenarios: Vec<PathBuf>,
    interactive: bool,
    verbosity: reporter::Verbosity,
    step: Option<usize>,
    loop_count: Option<usize>,
    keep: bool,
    setup_only: bool,
    list: bool,
    show: bool,
    checks: bool,
    jobs: usize,
    jobs_explicit: bool,
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

    if loop_count.is_some() && !interactive {
        anyhow::bail!("--loop-count requires --interactive / -i");
    }
    if step.is_some() && !interactive && !setup_only {
        anyhow::bail!("--step requires --interactive / -i (or --setup-only)");
    }
    if loop_count.is_some() && step.is_none() {
        anyhow::bail!("--loop-count requires --step");
    }

    // `scenarios_with_labels` carries `(label, path)` pairs in alphabetical
    // order — the user-visible ordering preserved for the non-TTY summary.
    // `dispatch_order` (computed below) is the *parallel-dispatch* permutation
    // that breaks the alphabetical clustering for rayon's chunked partition.
    let scenarios_with_labels: Vec<(String, PathBuf)> = if scenarios.is_empty() {
        discover_scenarios_recursive(&scenarios_dir)?
    } else {
        resolve_scenario_paths(&scenarios, &scenarios_dir)?
            .into_iter()
            .map(|p| {
                let label = label_from_path(&p, &scenarios_dir);
                (label, p)
            })
            .collect()
    };

    if scenarios_with_labels.is_empty() {
        anyhow::bail!("No scenario files found in {}", scenarios_dir.display());
    }

    let labels: Vec<String> = scenarios_with_labels
        .iter()
        .map(|(l, _)| l.clone())
        .collect();
    let scenario_files: Vec<PathBuf> = scenarios_with_labels.into_iter().map(|(_, p)| p).collect();
    let dispatch_order = interleave_by_namespace(&labels);

    // Explicit opt-in: the flag is the whole signal. TTY detection used to
    // route mode here, but after #554 the runner is automatic-first; users
    // who want step-through pass `-i`. TTY detection still gates the live
    // progress region inside `run_parallel` (see reporter CLAUDE.md §8) —
    // that's a rendering concern, not a routing one.
    let is_interactive = interactive;

    // Only bail when the user *explicitly* asked for parallel (via `--jobs`,
    // `DAFT_MANUAL_TEST_JOBS`, or `--parallel`) and the mode forbids it. The
    // auto-default also picks `jobs > 1`, but interactive / --setup-only runs
    // should just silently fall through to run_serial in that case rather than
    // erroring on a default the user didn't choose.
    if jobs_explicit && jobs > 1 {
        if is_interactive {
            anyhow::bail!("--jobs/--parallel is incompatible with --interactive / -i");
        }
        if setup_only {
            anyhow::bail!("--jobs/--parallel is incompatible with --setup-only");
        }
    }

    // Single shared interrupt flag plumbed through:
    //   1. SIGINT handler (sets it on first Ctrl+C),
    //   2. workers in `run_non_interactive` (bail mid-scenario when set),
    //   3. `IndicatifProgressSink` (colors the cancelled segment),
    //   4. the orchestrator's exit-code decision below.
    // One source of truth keeps the runner's "is this cancelled?" signal
    // consistent across all three subsystems.
    let interrupt = progress::InterruptFlag::new();
    let cleanup_set = setup_cleanup_handler(keep, interrupt.clone());

    // Prime the run-wide fixture cache. See [`fixture_cache`] for the
    // shape: walk every scenario file's raw YAML, collect unique
    // `(use_fixture, name)` tuples, generate each into the cache once, then
    // workers `cow_copy` from the cache into their sandboxes instead of
    // regenerating the same fixture for every scenario that references it.
    //
    // Register the cache root with the cleanup set *before* priming so a
    // SIGINT during `generate_repo` still leaves a tracked path the
    // handler can `rm -rf` — matching the same pre-create registration
    // pattern the per-scenario sandbox path uses.
    let fixture_keys = fixture_cache::collect_fixture_keys(&scenario_files);
    let cache_root = alloc_fixture_cache_dir()?;
    if !keep {
        if let Ok(mut g) = cleanup_set.lock() {
            g.insert(cache_root.clone());
        }
    }
    let fixture_cache =
        fixture_cache::FixtureCache::prime(&fixture_keys, &fixtures_dir, cache_root, jobs)?;

    // No leading eprintln! here — the pretty reporter's scenario_header
    // owns the inter-scenario blank line, including the one before the very
    // first scenario.

    // Pre-scan scenario files once to size the live region's step-counter
    // column: pad each worker row's `done/total` step counter so the time
    // column to its right stacks across in-flight rows. (Scenario/step names
    // flow naturally and truncate — they aren't padded.)
    //
    // The scan is `peek_scenario_metadata` — a cheap text scan, not a
    // full YAML parse. ~200ms for 580 files on an SSD.
    let metas: Vec<ScenarioMeta> = scenario_files
        .iter()
        .filter_map(|p| peek_scenario_metadata(p))
        .collect();
    let step_counter_width = max_step_counter_width(&metas);
    let reporter = reporter::reporter_for(verbosity);

    // Interactive and --setup-only stay on the streaming serial path. Both
    // have semantics — TTY ownership for interactive, `println!` of work_dir
    // for shell capture in setup-only — that don't fit the buffered worker
    // model used by the parallel scheduler.
    if is_interactive || setup_only {
        let result = run_serial(
            &scenario_files,
            &project_root,
            &fixtures_dir,
            &fixture_cache,
            &cleanup_set,
            reporter.as_ref(),
            verbosity,
            step,
            loop_count,
            &interrupt,
            keep,
            setup_only,
            is_interactive,
        );
        // Drain the cleanup set — `run_parallel` does this internally
        // (see line ~808's "belt-and-suspenders cleanup" block), but the
        // serial path otherwise leaks the fixture-cache root: per-scenario
        // sandboxes are cleaned up by their CleanupGuards' Drop impls, but
        // the cache root is registered manually with no guard. Without this
        // drain a single `mise run test:manual -- -i <scenario>` leaves a
        // `_fixture-cache-<pid>/` directory behind every time — particularly
        // visible under `DAFT_MANUAL_TEST_BASE` (e.g. the RAM-disk task).
        if !keep {
            if let Ok(mut g) = cleanup_set.lock() {
                for dir in g.drain() {
                    let _ = std::fs::remove_dir_all(&dir);
                }
            }
        }
        if interrupt.is_set() {
            std::process::exit(130);
        }
        return result;
    }

    // Non-interactive CI path — always goes through the parallel scheduler,
    // even at `jobs == 1` (a 1-thread rayon pool). Output is buffered per
    // scenario and flushed in input order.
    //
    // The progress sink shows a pinned live region on TTY; on non-TTY (CI
    // logs, redirected output, `cargo run`) it's a no-op so output stays
    // byte-identical to the pre-progress behavior. CI=… is checked
    // alongside TTY because GitHub Actions et al. flag stderr as a TTY but
    // progress redraws still pollute the logs.
    let show_progress = std::io::stderr().is_terminal()
        && std::env::var_os("NO_PROGRESS").is_none()
        && std::env::var_os("CI").is_none();
    let progress =
        progress::progress_sink_for(show_progress, step_counter_width, jobs, interrupt.clone());
    let result = run_parallel(
        &scenario_files,
        &labels,
        &dispatch_order,
        &scenarios_dir,
        &project_root,
        &fixtures_dir,
        &fixture_cache,
        &cleanup_set,
        reporter.as_ref(),
        progress.as_ref(),
        &interrupt,
        show_progress,
        keep,
        jobs,
        jobs_explicit,
    );

    // SIGINT convention: 128 + signal_number (2 for SIGINT). Bypass
    // anyhow's exit path so the shell sees the conventional code (mise /
    // shells / CI runners specifically test for 130 to distinguish
    // user-cancellation from a test failure).
    if interrupt.is_set() {
        std::process::exit(130);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn run_parallel(
    scenario_files: &[PathBuf],
    labels: &[String],
    dispatch_order: &[usize],
    scenarios_dir: &Path,
    project_root: &Path,
    fixtures_dir: &Path,
    fixture_cache: &fixture_cache::FixtureCache,
    cleanup_set: &CleanupSet,
    reporter: &dyn reporter::Reporter,
    progress: &dyn progress::ProgressSink,
    interrupt: &progress::InterruptFlag,
    show_progress: bool,
    keep: bool,
    jobs: usize,
    jobs_explicit: bool,
) -> Result<()> {
    let ctx = RunContext {
        project_root,
        fixtures_dir,
        fixture_cache,
        reporter,
        progress,
        interrupt,
        keep,
    };

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .context("building rayon thread pool")?;

    // Pre-run banner — shows scenario count + worker count so the user
    // knows up-front what's about to happen, and where the worker count
    // came from. Dim because it's metadata, not data (design language §1).
    // Blank line after to separate from the scenario stream.
    write_run_banner(scenario_files.len(), jobs, jobs_explicit)?;

    progress.run_started(scenario_files.len());

    // Rayon-channel pattern: a producer thread runs the par_iter, sending
    // each completed outcome down a channel. The main thread drains the
    // channel as outcomes arrive.
    //
    // - On TTY (`show_progress == true`), each completed outcome's buffer
    //   streams to stderr immediately via `stream_completed_scenario`,
    //   which suspends the live region so the writes don't tear. Result:
    //   scrollback in completion order, live bar uninterrupted.
    // - On non-TTY (`show_progress == false`), no streaming — outcomes
    //   accumulate and drain in input order at end (byte-identical to the
    //   pre-progress behavior; preserves the CI log contract).
    let (tx, rx) = std::sync::mpsc::channel::<ScenarioOutcome>();
    let mut all_outcomes: Vec<ScenarioOutcome> = Vec::with_capacity(scenario_files.len());

    // Acceptance #1 from #559: dispatch debug print behind an env flag so we
    // can verify per-thread namespace mix without altering normal output.
    // Read once outside the closure — `std::env::var_os` synchronizes on a
    // global lock, no point paying for it per scenario.
    //
    // Suppress on TTY: `eprintln!` from worker threads races indicatif's
    // live-region redraws and garbles the terminal. Pair the env var with
    // `CI=1` or `NO_PROGRESS=1` (both already disable `show_progress`) to
    // see the lines cleanly. If the user set the env on a TTY, warn once so
    // they know why nothing is appearing.
    let debug_dispatch_requested = std::env::var_os("DAFT_MANUAL_TEST_DEBUG_DISPATCH").is_some();
    let debug_dispatch = debug_dispatch_requested && !show_progress;
    if debug_dispatch_requested && show_progress {
        eprintln!(
            "[dispatch] DAFT_MANUAL_TEST_DEBUG_DISPATCH set but TTY progress is on; \
             suppressing per-scenario output. Re-run with CI=1 or NO_PROGRESS=1 to see it.",
        );
    }

    let run_start = std::time::Instant::now();
    std::thread::scope(|s| -> Result<()> {
        let pool_ref = &pool;
        let ctx_ref = &ctx;
        let cleanup_ref = cleanup_set;
        let files_ref = scenario_files;
        let labels_ref = labels;
        let dispatch_ref = dispatch_order;
        s.spawn(move || {
            pool_ref.install(|| {
                // Iterate `dispatch_order` (an interleaved permutation of
                // 0..N) rather than `scenario_files` directly. The carried
                // `display_idx` is the *alphabetical* position — passed
                // through as `ScenarioOutcome.index` so the post-run sort at
                // line ~639 still produces alphabetical non-TTY output.
                dispatch_ref
                    .par_iter()
                    .for_each_with(tx, |tx, &display_idx| {
                        let path = &files_ref[display_idx];
                        if debug_dispatch {
                            eprintln!(
                                "[dispatch] thread={:?} idx={} ns={}",
                                rayon::current_thread_index(),
                                display_idx,
                                namespace_of(&labels_ref[display_idx]),
                            );
                        }
                        let outcome = run_one_scenario(display_idx, path, ctx_ref, cleanup_ref);
                        let _ = tx.send(outcome);
                    });
            });
        });

        // Tracks whether we've already poked the sink so the (cancelling)
        // suffix lands as soon as Ctrl+C is observed. Without this nudge
        // the suffix wouldn't appear until the first worker finishes its
        // current step — which can be seconds — leaving the user wondering
        // if the cancel registered.
        let mut notified_cancelling = false;
        while let Ok(mut outcome) = rx.recv() {
            if !notified_cancelling && interrupt.is_set() {
                progress.notify_cancelling();
                notified_cancelling = true;
            }
            // For the TTY (live-region) path, the worker already printed
            // this scenario's buffer atomically via
            // `progress.complete_scenario` — `multi.println` for each
            // line, then `multi.remove(row)`, all on the worker thread.
            // That sequencing is what keeps in-flight bar rows from
            // getting stranded in scrollback (the cross-thread race that
            // earlier `multi.println`-on-main + `multi.remove`-on-worker
            // designs all hit). Free the buf here so the aggregate
            // doesn't hold the bytes alive needlessly.
            if show_progress {
                outcome.output.clear();
                outcome.output.shrink_to_fit();
            }
            all_outcomes.push(outcome);
        }
        Ok(())
    })?;
    let duration = run_start.elapsed();

    // Input-order sort: required for both deterministic non-TTY drain AND
    // deterministic stats aggregation (FailedScenarioRecord ordering, etc.).
    all_outcomes.sort_by_key(|o| o.index);

    let stderr = std::io::stderr();
    if !show_progress {
        // Non-TTY: drain buffers in input order at end (today's behavior,
        // byte-identical for CI). Cancelled buffers carry the `⊘ name
        // (cancelled)` footer just like pass/fail buffers carry their
        // `✓`/`✗` footers — drain them all uniformly so an automatic-mode
        // log shows which scenarios were in flight at the moment of cancel.
        let mut lock = stderr.lock();
        for o in &all_outcomes {
            lock.write_all(&o.output)?;
        }
    }

    // Belt-and-suspenders cleanup. Workers' `Sandbox::drop` removes each
    // sandbox at scenario end, so under a normal run (and under soft cancel
    // where workers wind down naturally) the registry should be empty
    // here. This catches the edge cases: a worker that panicked between
    // `CleanupGuard::new` and `Sandbox::create_at`, or a directory the OS
    // still reports as existing after `remove_dir_all` returned (NFS
    // quirks, slow async unlinks). The hard-exit SIGINT path has its own
    // `rm -rf` loop; this one is for the natural-end case.
    if !keep {
        if let Ok(mut g) = cleanup_set.lock() {
            for dir in g.drain() {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    let stats = aggregate_outcomes(&all_outcomes, scenarios_dir, duration, Some(jobs));

    let total_failed = stats.summary.steps_failed;
    let error_count = stats.summary.errors.len();
    // Clear the live region first, then write the summary onto a clean
    // canvas. Doing it the other way around (any pause-then-redraw
    // sequence) briefly flashes the bars back on top of the
    // freshly-written summary as the redraw fires.
    progress.run_finished();
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

/// Write the pre-run banner showing scenario count + worker count + whether
/// the worker count was auto-detected. Goes straight to stderr (no Reporter
/// dispatch) — it's framing metadata, not per-scenario content. Singular/
/// plural forms keep the line grammatical for the 1-scenario and 1-worker
/// edge cases.
fn write_run_banner(scenarios_count: usize, jobs: usize, jobs_explicit: bool) -> Result<()> {
    use std::io::Write;
    let s_scen = if scenarios_count == 1 {
        "scenario"
    } else {
        "scenarios"
    };
    let banner = if jobs == 1 {
        format!("Running {scenarios_count} {s_scen} sequentially")
    } else {
        let suffix = if jobs_explicit {
            ""
        } else {
            " (auto-detected)"
        };
        format!("Running {scenarios_count} {s_scen} with {jobs} parallel workers{suffix}")
    };
    let mut stderr = std::io::stderr().lock();
    writeln!(stderr, "{}", term_styles::dim(&banner))?;
    writeln!(stderr)?;
    Ok(())
}

/// Aggregate parallel worker outcomes into a single summary.
///
/// Scenarios that hit a fatal error before running (YAML parse failure,
/// sandbox creation error, captured panic) are accumulated into
/// `summary.errors` and **do not** count toward `scenarios_total /
/// scenarios_passed / scenarios_failed / steps_*`. The stats line
/// describes what actually ran; errored scenarios surface in their own
/// section above it with the underlying error. A run that hits 1 parse
/// error + 9 passing scenarios reads as "9 total" in stats plus a
/// separate `Errors:` block above — by design.
fn aggregate_outcomes<'a>(
    outcomes: &'a [ScenarioOutcome],
    scenarios_dir: &Path,
    duration: std::time::Duration,
    parallel_jobs: Option<usize>,
) -> OutcomeStats<'a> {
    let mut scenarios_total = 0usize;
    let mut scenarios_passed = 0usize;
    let mut scenarios_failed = 0usize;
    let mut scenarios_cancelled = 0usize;
    let mut scenarios_not_run = 0usize;
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
                if sr.cancelled {
                    // Cancelled is its own bucket — distinct from pass/fail.
                    // A cancelled scenario may have had failing steps before
                    // the cancellation; we still bucket it as cancelled
                    // (precedence intentional: the run didn't complete, so
                    // calling it "failed" misrepresents what happened).
                    scenarios_cancelled += 1;
                } else if sr.failed > 0 {
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
            // Both None = the worker observed the interrupt flag and
            // bailed before the scenario became visible to the user
            // (either at the top of `run_one_scenario` or in the race
            // window before `scenario_started`). These don't count as
            // passed / failed / cancelled — the user never saw them as
            // a row. They surface only as `scenarios_not_run`, which the
            // reporter renders as a one-line "did not run" note when > 0.
            (None, None) => scenarios_not_run += 1,
        }
    }

    OutcomeStats {
        summary: reporter::RunSummary {
            scenarios_total,
            scenarios_passed,
            scenarios_failed,
            scenarios_cancelled,
            scenarios_not_run,
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
    fixture_cache: &fixture_cache::FixtureCache,
    cleanup_set: &CleanupSet,
    reporter: &dyn reporter::Reporter,
    verbosity: reporter::Verbosity,
    step: Option<usize>,
    loop_count: Option<usize>,
    interrupt: &progress::InterruptFlag,
    keep: bool,
    setup_only: bool,
    is_interactive: bool,
) -> Result<()> {
    for path in scenario_files {
        // Between-scenario interrupt check. A single scenario in serial mode
        // is its own process, so we don't have the parallel path's
        // cancelled-status tracking — we just stop iterating. The next
        // scenario doesn't start; the run unwinds normally; the caller in
        // `run` sees the interrupt flag and exits 130.
        if interrupt.is_set() {
            break;
        }
        let scenario = load_scenario(path, fixtures_dir)?;

        // Register the sandbox path before touching disk so a SIGINT during
        // `Sandbox::create_at` still leaves a tracked path the cleanup handler
        // can `rm -rf`.
        let base_dir = pick_sandbox_base_dir(&scenario)?;
        let _guard = CleanupGuard::new(Arc::clone(cleanup_set), base_dir.clone());
        let mut sb = sandbox::Sandbox::create_at(&scenario, base_dir, keep || setup_only)?;
        let executor = daft_executor::DaftCommandExecutor::new_for_sandbox(&mut sb, project_root)?;

        for repo_spec in &scenario.repos {
            provision_repo(fixture_cache, repo_spec, &sb.remotes_dir)?;
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
    // After a SIGINT, rayon workers keep pulling scenarios off the queue —
    // each one would be reported as `Cancelled` even though no step ran.
    // That inflates the cancelled count beyond what the user actually saw
    // running. Skip these so cancellation only counts scenarios that were
    // truly in flight when the user pressed Ctrl+C. The outcome carries
    // `result: None, error: None`, which `aggregate_outcomes` already
    // treats as a no-op (its `(None, None) => {}` arm).
    if ctx.interrupt.is_set() {
        return ScenarioOutcome {
            index,
            name: path.display().to_string(),
            source: path.to_path_buf(),
            result: None,
            output: Vec::new(),
            error: None,
        };
    }

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
        run_one_scenario_inner(&scenario, index, ctx, cleanup_set)
    }));

    match work {
        Ok(Ok(Some((sr, buf)))) => ScenarioOutcome {
            index,
            name,
            source,
            result: Some(sr),
            output: buf,
            error: None,
        },
        Ok(Ok(None)) => {
            // Race-window skip: the worker had loaded the scenario and
            // created its sandbox before noticing the interrupt. Same
            // no-op outcome as the early-bail at the top of this
            // function — invisible to the user, invisible to the count.
            ScenarioOutcome {
                index,
                name,
                source,
                result: None,
                output: Vec::new(),
                error: None,
            }
        }
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
    index: usize,
    ctx: &RunContext<'_>,
    cleanup_set: &CleanupSet,
) -> Result<Option<(runner::ScenarioResult, Vec<u8>)>> {
    // Per-phase wall-clock — emitted alongside the existing `elapsed_ms` step
    // duration under `DAFT_MANUAL_TEST_EMIT_TIMING` for the bench harness.
    // Lets a #509 perf investigation attribute per-scenario cost to setup vs
    // fixture gen vs template snapshot vs the step phase itself.
    let setup_started = std::time::Instant::now();
    // Pre-register the sandbox path so a SIGINT during create_at still leaves
    // a tracked path the cleanup handler can `rm -rf`. See [`setup_cleanup_handler`]
    // for the matching drain-loop logic.
    let base_dir = pick_sandbox_base_dir(scenario)?;
    let _guard = CleanupGuard::new(Arc::clone(cleanup_set), base_dir.clone());
    let mut sb = sandbox::Sandbox::create_at(scenario, base_dir, ctx.keep)?;
    let executor = daft_executor::DaftCommandExecutor::new_for_sandbox(&mut sb, ctx.project_root)?;
    let setup_ms = setup_started.elapsed().as_millis();

    let fixture_started = std::time::Instant::now();
    for repo_spec in &scenario.repos {
        provision_repo(ctx.fixture_cache, repo_spec, &sb.remotes_dir)?;
        sb.register_remote(&repo_spec.name);
    }
    let fixture_ms = fixture_started.elapsed().as_millis();

    // The non-interactive path never calls `Sandbox::reset()` (reset is
    // interactive/loop-only — see interactive.rs), so snapshotting
    // `remotes/` -> `remotes-template/` here would be pure dead work
    // (~5.6s core across the suite). Skip it; the interactive setup path
    // (run_interactive) still snapshots. `template_ms` stays in the bench
    // schema (always 0 now) so DAFT_MANUAL_TEST_EMIT_TIMING consumers don't
    // break. See #585.
    let template_ms = 0u128;

    let mut buf: Vec<u8> = Vec::new();
    // `None` = the runner saw the interrupt flag before `scenario_started`
    // fired — the user never saw a row for this scenario, so we count it
    // as "skipped" (invisible, no aggregate count) rather than cancelled.
    // Sandbox drop on this function's exit cleans up the directory we
    // already created.
    let Some(result) = runner::run_non_interactive(
        scenario,
        index,
        &sb,
        &executor,
        ctx.reporter,
        ctx.progress,
        ctx.interrupt,
        &mut buf,
    )?
    else {
        return Ok(None);
    };

    // Opt-in per-scenario timing for the bench harness. Lines are
    // grep-friendly and live inside the scenario's buffered output so they
    // print in input order alongside the scenario's own report. `elapsed_ms`
    // stays compatible with the percentiles script
    // (`benches/scenarios/test_manual_scale.sh`); the per-phase fields
    // (`setup_ms`, `fixture_ms`, `template_ms`) are additional diagnostics
    // for #509 bottleneck attribution.
    if std::env::var_os("DAFT_MANUAL_TEST_EMIT_TIMING").is_some() {
        writeln!(
            &mut buf,
            "[bench] scenario={:?} elapsed_ms={} setup_ms={} fixture_ms={} template_ms={}",
            scenario.name,
            result.duration.as_millis(),
            setup_ms,
            fixture_ms,
            template_ms,
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
            // its scenario rather than running into the next one. Cancelled
            // scenarios suppress it too: the wind-down already streams a `⊘
            // (cancelled)` footer per scenario, and an extra cleanup line on
            // each would just be interrupt-time chatter.
            Ok(()) if result.failed == 0 || result.cancelled => {}
            Ok(()) => ctx
                .reporter
                .cleanup_note(&mut buf, "Cleaned up test environment.")?,
            Err(e) => ctx
                .reporter
                .cleanup_note(&mut buf, &format!("Warning: cleanup failed: {e}"))?,
        }
    }

    // Hand the full buffer to the sink for atomic print-then-remove on
    // the worker thread. This is the load-bearing call that prevents
    // ghost rows: the previous design split it across threads (worker
    // removed the row in `scenario_finished`, main thread printed the
    // footer via `multi.println`) which reordered under load and
    // stranded in-flight rows in scrollback. See the comment on
    // `IndicatifProgressSink::complete_scenario` for the long version.
    //
    // For NoopProgressSink (non-TTY runs), this is a no-op — the
    // orchestrator drains buffers in input order at end-of-run so a
    // sink-side print here would duplicate output.
    let status = if result.cancelled {
        reporter::ScenarioStatus::Cancelled
    } else if result.failed > 0 {
        reporter::ScenarioStatus::Fail
    } else {
        reporter::ScenarioStatus::Pass
    };
    ctx.progress
        .complete_scenario(index, status, result.duration, &buf);

    Ok(Some((result, buf)))
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

/// Extract the namespace portion of a scenario label (`"namespace:stem"` →
/// `"namespace"`, bare `"stem"` → `""`). Top-level scenarios share the empty
/// namespace, which keeps them in their own round-robin bucket below.
fn namespace_of(label: &str) -> &str {
    match label.split_once(':') {
        Some((ns, _)) => ns,
        None => "",
    }
}

/// Compute an interleaved dispatch order over `labels`: returns indices into
/// `labels` arranged so that consecutive items come from different namespaces.
///
/// Rayon's `par_iter` partitions its slice into contiguous per-worker chunks
/// and only work-steals when a worker finishes its chunk. With scenarios sorted
/// alphabetically by `"namespace:stem"`, that puts every `layout:*` scenario
/// on the same worker — and if a heavy namespace lands contiguously on one
/// worker, the tail of the suite is that worker grinding while every other
/// worker is idle. Interleaving by namespace ensures each worker's initial
/// chunk is a mix, so per-namespace cost variance distributes evenly.
///
/// Deterministic by construction: a `BTreeMap` iterates by sorted namespace
/// key, and `VecDeque::pop_front` over input-ordered indices preserves
/// within-namespace order. No RNG.
fn interleave_by_namespace(labels: &[String]) -> Vec<usize> {
    let mut by_namespace: BTreeMap<&str, VecDeque<usize>> = BTreeMap::new();
    for (i, label) in labels.iter().enumerate() {
        by_namespace
            .entry(namespace_of(label))
            .or_default()
            .push_back(i);
    }

    let mut interleaved = Vec::with_capacity(labels.len());
    let mut keys: Vec<&str> = by_namespace.keys().copied().collect();
    while !keys.is_empty() {
        keys.retain(|key| {
            let Some(queue) = by_namespace.get_mut(key) else {
                return false;
            };
            match queue.pop_front() {
                Some(idx) => {
                    interleaved.push(idx);
                    !queue.is_empty()
                }
                None => false,
            }
        });
    }
    interleaved
}

/// Derive a `"namespace:stem"` label from a scenario `PathBuf` for the explicit
/// `--scenarios <name>` path, mirroring how `discover_scenarios_recursive`
/// labels scenarios under `scenarios_dir`. Used so the dispatch interleave can
/// still group user-named scenarios by namespace.
///
/// Assumes the one-level layout `scenarios_dir/<namespace>/<stem>.yml` that
/// `discover_scenarios_recursive` itself enforces. A path nested deeper, e.g.
/// `scenarios_dir/checkout/subdir/scenario.yml`, is labeled with the
/// *immediate* parent (`subdir:scenario`) rather than the top-level namespace
/// (`checkout:...`) — those scenarios would then form their own interleave
/// bucket. Acceptable because the discovery side never produces such layouts;
/// only explicit user paths could.
fn label_from_path(path: &Path, scenarios_dir: &Path) -> String {
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match path.parent() {
        Some(parent) if parent != scenarios_dir => parent
            .file_name()
            .map(|ns| format!("{}:{}", ns.to_string_lossy(), stem))
            .unwrap_or(stem),
        _ => stem,
    }
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
                resolved.push(resolve_fixture_spec(
                    fixtures_dir,
                    &fixture_ref.use_fixture,
                    fixture_ref.name,
                )?);
            }
        }
    }
    Ok(resolved)
}

/// Materialise a single repo into the scenario's `remotes_dir`.
///
/// Fixture-derived specs (`fixture_source = Some(_)`) `cow_copy` from the
/// run-wide cache — O(1) on APFS / reflink-capable Linux, byte-copy fallback
/// elsewhere. Inline specs hit `repo_gen::generate_repo` per scenario since
/// they have no caching identity to share against.
fn provision_repo(
    cache: &fixture_cache::FixtureCache,
    repo_spec: &schema::RepoSpec,
    remotes_dir: &Path,
) -> Result<()> {
    if let Some(use_fixture) = &repo_spec.fixture_source {
        let key = (use_fixture.clone(), repo_spec.name.clone());
        let dst = remotes_dir.join(&repo_spec.name);
        cache.clone_into(&key, &dst)
    } else {
        repo_gen::generate_repo(repo_spec, remotes_dir).map(|_| ())
    }
}

/// Resolve a single `use_fixture` reference into a fully-substituted
/// [`schema::RepoSpec`]. Shared between scenario resolution and the
/// fixture-cache primer so both paths apply identical `{{NAME}}` substitution
/// and identical error messages.
pub(crate) fn resolve_fixture_spec(
    fixtures_dir: &Path,
    use_fixture: &str,
    name: String,
) -> Result<schema::RepoSpec> {
    let fixture_path = fixtures_dir.join(use_fixture).with_extension("yml");
    let raw_yaml = std::fs::read_to_string(&fixture_path)
        .with_context(|| format!("reading fixture '{}' for repo '{}'", use_fixture, name))?;
    let substituted = raw_yaml.replace("{{NAME}}", &name);
    let fixture: schema::RepoFixture = serde_yaml::from_str(&substituted)
        .with_context(|| format!("parsing fixture '{}' for repo '{}'", use_fixture, name))?;
    Ok(schema::RepoSpec {
        name,
        default_branch: fixture.default_branch,
        branches: fixture.branches,
        daft_yml: fixture.daft_yml,
        hook_scripts: fixture.hook_scripts,
        fixture_source: Some(use_fixture.to_string()),
    })
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
mod dispatch_order_tests {
    use super::{interleave_by_namespace, label_from_path, namespace_of};
    use std::path::Path;

    fn labels(strs: &[&str]) -> Vec<String> {
        strs.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn namespace_splits_on_first_colon() {
        assert_eq!(namespace_of("checkout:basic"), "checkout");
        assert_eq!(namespace_of("a:b:c"), "a");
        assert_eq!(namespace_of("bare"), "");
        assert_eq!(namespace_of(""), "");
    }

    #[test]
    fn interleave_round_robins_two_namespaces() {
        let input = labels(&["a:1", "a:2", "a:3", "b:1"]);
        // a, b alternate; b runs out first, then a:2, a:3 trail.
        assert_eq!(interleave_by_namespace(&input), vec![0, 3, 1, 2]);
    }

    #[test]
    fn interleave_round_robins_three_namespaces_uneven() {
        let input = labels(&["a:1", "a:2", "b:1", "c:1", "c:2"]);
        // Round 1: a:1, b:1, c:1. Round 2: a:2, c:2 (b exhausted).
        assert_eq!(interleave_by_namespace(&input), vec![0, 2, 3, 1, 4]);
    }

    #[test]
    fn interleave_single_namespace_is_identity() {
        let input = labels(&["a:1", "a:2", "a:3"]);
        assert_eq!(interleave_by_namespace(&input), vec![0, 1, 2]);
    }

    #[test]
    fn interleave_empty_input_is_empty() {
        assert_eq!(interleave_by_namespace(&[]), Vec::<usize>::new());
    }

    #[test]
    fn interleave_preserves_within_namespace_order() {
        let input = labels(&["a:1", "b:1", "a:2", "b:2", "a:3"]);
        let order = interleave_by_namespace(&input);
        let a_positions: Vec<_> = order
            .iter()
            .enumerate()
            .filter(|(_, &idx)| input[idx].starts_with("a:"))
            .map(|(pos, _)| pos)
            .collect();
        let a_indices: Vec<usize> = a_positions.iter().map(|&p| order[p]).collect();
        assert_eq!(a_indices, vec![0, 2, 4]);
    }

    #[test]
    fn interleave_is_deterministic() {
        let input = labels(&["b:2", "a:1", "c:1", "b:1", "a:2"]);
        assert_eq!(
            interleave_by_namespace(&input),
            interleave_by_namespace(&input)
        );
    }

    #[test]
    fn interleave_bare_scenarios_alternate_with_namespaced() {
        // Bare labels share the "" namespace bucket — they round-robin against
        // namespaced ones one-per-round, not against each other internally.
        let input = labels(&["bare1", "a:1", "bare2", "a:2"]);
        // Round 1: "" -> bare1, "a" -> a:1. Round 2: "" -> bare2, "a" -> a:2.
        assert_eq!(interleave_by_namespace(&input), vec![0, 1, 2, 3]);
    }

    #[test]
    fn label_from_path_uses_parent_dir_as_namespace() {
        let scenarios_dir = Path::new("/repo/tests/manual/scenarios");
        assert_eq!(
            label_from_path(
                Path::new("/repo/tests/manual/scenarios/checkout/basic.yml"),
                scenarios_dir,
            ),
            "checkout:basic"
        );
    }

    #[test]
    fn label_from_path_top_level_returns_bare_stem() {
        let scenarios_dir = Path::new("/repo/tests/manual/scenarios");
        assert_eq!(
            label_from_path(
                Path::new("/repo/tests/manual/scenarios/quickcheck.yml"),
                scenarios_dir,
            ),
            "quickcheck"
        );
    }
}

#[cfg(test)]
mod shell_safe_slug_tests {
    use super::shell_safe_slug;

    #[test]
    fn simple_lowercase_and_dash() {
        assert_eq!(shell_safe_slug("Checkout basic"), "checkout-basic");
    }

    #[test]
    fn parens_become_dashes() {
        // Regression for the #512 ramdisk path: scenario names with `(` and
        // `)` are common, and an unsanitised slug landed inside a `bash -c
        // "cd <path>"` step that failed with `syntax error near unexpected
        // token '('` because bash treats the unquoted `(` as a subshell.
        assert_eq!(
            shell_safe_slug("Owner strategy recency-plurality (default)"),
            "owner-strategy-recency-plurality-default"
        );
    }

    #[test]
    fn collapses_runs_of_dashes() {
        // Three unsafe chars in a row should produce one `-`, not three.
        assert_eq!(shell_safe_slug("a   b"), "a-b");
        assert_eq!(shell_safe_slug("a()b"), "a-b");
    }

    #[test]
    fn trims_leading_and_trailing_dashes() {
        assert_eq!(shell_safe_slug("(wrapped)"), "wrapped");
    }

    #[test]
    fn preserves_safe_punctuation() {
        // Dots and underscores are shell-safe in unquoted paths; preserved.
        assert_eq!(shell_safe_slug("v1.2_release"), "v1.2_release");
    }

    #[test]
    fn empty_yields_unnamed_scenario() {
        // A degenerate name still must produce a valid path component.
        assert_eq!(shell_safe_slug(""), "unnamed-scenario");
        assert_eq!(shell_safe_slug("()"), "unnamed-scenario");
    }

    #[test]
    fn non_ascii_collapses_to_dash() {
        // Lossy ASCII normalisation: each non-ASCII char (e.g. `é`, `✨`)
        // maps to `-` rather than landing untransformed in the path
        // component. Here `é` becomes `-` and collapses with the preceding
        // space, while the ASCII `moji` survives intact and the trailing
        // ` ✨` collapses to a single `-` that's then trimmed.
        assert_eq!(shell_safe_slug("name with émoji ✨"), "name-with-moji");
    }

    #[test]
    fn leading_dot_is_preserved_but_documented() {
        // `.` is in the safe-character set so a YAML scenario whose name
        // started with `.` would produce a hidden directory under
        // `DAFT_MANUAL_TEST_BASE`. No scenarios do this today; the test
        // pins the behaviour so a future audit catches it if it starts to
        // matter.
        assert_eq!(shell_safe_slug(".hidden"), ".hidden");
    }
}

#[cfg(test)]
mod scenario_base_slug_tests {
    use super::{scenario_base_slug, schema};
    use std::path::PathBuf;

    fn scenario_with_path(name: &str, path: &str) -> schema::Scenario {
        schema::Scenario {
            name: name.to_string(),
            description: None,
            repos: vec![],
            env: Default::default(),
            steps: vec![],
            source_path: PathBuf::from(path),
        }
    }

    #[test]
    fn source_path_disambiguates_duplicate_names() {
        // Regression for #512: two scenarios share `name: "Transform nested
        // to sibling"`. The default `/tmp/daft-manual-test-*-PID` base
        // tolerated this because it's timestamp/PID unique. Pointing
        // `DAFT_MANUAL_TEST_BASE` at a stable path (like the RAM disk) ate
        // the duplicate name and produced one collision per parallel pair.
        let a = scenario_with_path(
            "Transform nested to sibling",
            "tests/manual/scenarios/layout/transform-matrix-nested-to-sibling.yml",
        );
        let b = scenario_with_path(
            "Transform nested to sibling",
            "tests/manual/scenarios/layout/transform-nested-to-sibling.yml",
        );
        assert_eq!(
            scenario_base_slug(&a),
            "layout-transform-matrix-nested-to-sibling"
        );
        assert_eq!(scenario_base_slug(&b), "layout-transform-nested-to-sibling");
        assert_ne!(scenario_base_slug(&a), scenario_base_slug(&b));
    }

    #[test]
    fn empty_source_path_falls_back_to_name() {
        // Unit tests that construct `Scenario` directly leave source_path
        // empty (`#[serde(default, skip)]`). Those code paths still need a
        // slug; fall back to the name field via `shell_safe_slug`.
        let s = scenario_with_path("My scenario", "");
        assert_eq!(scenario_base_slug(&s), "my-scenario");
    }

    #[test]
    fn nested_source_path_uses_immediate_parent() {
        // `<parent-dir>-<stem>`, not `<full-path>-<stem>`. Keeps slugs
        // human-readable; deeper nesting (rare today) collides at the
        // parent-dir level but real paths are unique even at that resolution.
        let s = scenario_with_path("Anything", "a/b/c/d.yml");
        assert_eq!(scenario_base_slug(&s), "c-d");
    }

    #[test]
    fn bare_filename_with_no_parent_falls_back_to_stem() {
        // `PathBuf::from("scenario.yml").parent()` is `Some("")` whose
        // `file_name()` is `None`; the parent component then resolves to
        // the empty string and `shell_safe_slug("-scenario")` trims the
        // leading dash. Covers the "path-derived branch is taken but the
        // parent piece is empty" edge.
        let s = scenario_with_path("Doesn't matter", "scenario.yml");
        assert_eq!(scenario_base_slug(&s), "scenario");
    }
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

#[cfg(test)]
mod non_interactive_template_tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    // Serializes the DAFT_MANUAL_TEST_BASE mutation against other env-mutating
    // tests, mirroring the ENV_LOCK pattern in `main.rs`'s resolve_jobs tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// #585 regression: the non-interactive scenario runner
    /// (`run_one_scenario_inner`) must NOT snapshot `remotes/` ->
    /// `remotes-template/`. That snapshot is only consumed by
    /// `Sandbox::reset()`, which runs solely in interactive mode, so taking it
    /// on the parallel path is pure dead work (~5.6s across the suite). Guards
    /// against anyone re-adding `sb.create_template()` to the hot path.
    #[test]
    fn non_interactive_run_does_not_snapshot_template() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let base = tempfile::tempdir().expect("tempdir");
        // Make the per-scenario sandbox base deterministic so we can inspect
        // it after the run (see `pick_sandbox_base_dir`).
        std::env::set_var("DAFT_MANUAL_TEST_BASE", base.path());

        let scenario = schema::Scenario {
            name: "no-template-585".to_string(),
            description: None,
            repos: Vec::new(),
            env: HashMap::new(),
            steps: Vec::new(),
            source_path: PathBuf::new(),
        };

        let fixtures_dir = base.path().join("fixtures");
        std::fs::create_dir_all(&fixtures_dir).expect("fixtures dir");
        let fixture_cache = fixture_cache::FixtureCache::prime(
            &BTreeSet::new(),
            &fixtures_dir,
            base.path().join("cache"),
            1,
        )
        .expect("prime empty fixture cache");

        let reporter = reporter::pretty::PrettyReporter::new(reporter::Verbosity::Default);
        let progress = progress::NoopProgressSink;
        let interrupt = progress::InterruptFlag::new();
        let cleanup_set: CleanupSet = Arc::new(Mutex::new(HashSet::new()));

        let ctx = RunContext {
            project_root: base.path(),
            fixtures_dir: &fixtures_dir,
            fixture_cache: &fixture_cache,
            reporter: &reporter,
            progress: &progress,
            interrupt: &interrupt,
            // Keep the sandbox on disk so we can assert on its contents.
            keep: true,
        };

        run_one_scenario_inner(&scenario, &ctx, &cleanup_set)
            .expect("non-interactive scenario run");

        std::env::remove_var("DAFT_MANUAL_TEST_BASE");

        let sandbox_base = base.path().join(scenario_base_slug(&scenario));
        assert!(
            sandbox_base.join("remotes").is_dir(),
            "the sandbox should have been created (remotes/ present)"
        );
        assert!(
            !sandbox_base.join("remotes-template").exists(),
            "non-interactive run must not snapshot remotes-template/ (#585)"
        );
    }
}

#[cfg(test)]
mod peek_scenario_metadata_tests {
    use super::{max_step_counter_width, peek_scenario_metadata, ScenarioMeta};
    use std::io::Write;

    fn write_yaml(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::Builder::new().suffix(".yml").tempfile().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn meta_from(yaml: &str) -> ScenarioMeta {
        let f = write_yaml(yaml);
        peek_scenario_metadata(f.path()).expect("readable yaml")
    }

    #[test]
    fn peeks_name_and_step_names() {
        let m = meta_from(
            "name: Demo\nsteps:\n  - name: first\n    run: \"true\"\n  - name: second\n    run: \"true\"\n",
        );
        assert_eq!(m.name.as_deref(), Some("Demo"));
        assert_eq!(
            m.step_names,
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn strips_double_and_single_quotes_in_name() {
        let m1 = meta_from("name: \"Quoted scenario\"\nsteps: []\n");
        assert_eq!(m1.name.as_deref(), Some("Quoted scenario"));
        let m2 = meta_from("name: 'Single quoted'\nsteps: []\n");
        assert_eq!(m2.name.as_deref(), Some("Single quoted"));
    }

    #[test]
    fn ignores_repos_block_name_keys() {
        // Same indent-aware scan as extract_step_lines — `- name:`
        // entries inside the `repos:` block are not steps.
        let m = meta_from(
            "name: scenario\nrepos:\n  - name: my-repo\n    use_fixture: standard\nsteps:\n  - name: only-step\n    run: \"true\"\n",
        );
        assert_eq!(m.step_names, vec!["only-step".to_string()]);
    }

    #[test]
    fn returns_default_meta_when_no_name() {
        let m = meta_from("description: nothing here\nsteps: []\n");
        assert_eq!(m.name, None);
        assert!(m.step_names.is_empty());
    }

    #[test]
    fn returns_none_for_unreadable_path() {
        assert!(peek_scenario_metadata(std::path::Path::new("/nonexistent-xyzzy.yml")).is_none());
    }

    #[test]
    fn max_step_counter_width_picks_widest_counter() {
        // Counter width is `2 * digit_count(total) + 1` (the widest counter
        // for a T-step scenario is `T/T`). The 12-step scenario wins:
        // "12/12" = 5 chars, vs "3/3" = 3 for the 3-step one.
        let metas = vec![
            ScenarioMeta {
                name: Some("a".into()),
                step_names: vec!["s".into(); 3],
            },
            ScenarioMeta {
                name: Some("b".into()),
                step_names: vec!["s".into(); 12],
            },
        ];
        assert_eq!(max_step_counter_width(&metas), "12/12".chars().count());
    }

    #[test]
    fn widths_are_zero_on_empty_set() {
        assert_eq!(max_step_counter_width(&[]), 0);
    }
}
