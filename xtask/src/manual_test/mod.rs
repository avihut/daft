pub mod env;
pub mod interactive;
pub mod repo_gen;
pub mod runner;
pub mod schema;

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Register a Ctrl+C handler that cleans up the active test environment.
///
/// Returns a shared handle that the run loop updates with the current sandbox
/// path. On SIGINT the handler removes that directory and exits.
fn setup_cleanup_handler(keep: bool) -> Arc<Mutex<Option<PathBuf>>> {
    let cleanup_path: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let handler_path = Arc::clone(&cleanup_path);

    ctrlc::set_handler(move || {
        // Restore terminal state in case interactive mode left raw mode on.
        let _ = crossterm::terminal::disable_raw_mode();
        eprintln!();
        if !keep {
            if let Ok(guard) = handler_path.lock() {
                if let Some(dir) = guard.as_ref() {
                    let _ = std::fs::remove_dir_all(dir);
                    eprintln!("Cleaned up test environment.");
                }
            }
        }
        std::process::exit(130); // 128 + SIGINT(2)
    })
    .ok();

    cleanup_path
}

pub fn run(
    scenarios: Vec<PathBuf>,
    no_interactive: bool,
    verbose: bool,
    step: Option<usize>,
    loop_count: Option<usize>,
    keep: bool,
    setup_only: bool,
    list: bool,
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

    if loop_count.is_some() && step.is_none() {
        anyhow::bail!("--loop-count requires --step");
    }

    let scenario_files = if scenarios.is_empty() {
        discover_scenarios(&scenarios_dir)?
    } else {
        resolve_scenario_paths(&scenarios, &scenarios_dir)?
    };

    if scenario_files.is_empty() {
        anyhow::bail!("No scenario files found in {}", scenarios_dir.display());
    }

    let is_interactive = !no_interactive && std::io::stdin().is_terminal();
    let cleanup_path = setup_cleanup_handler(keep);

    let mut total_scenarios = 0usize;
    let mut total_steps = 0usize;
    let mut total_passed = 0usize;
    let mut total_failed = 0usize;
    let mut failed_scenarios: Vec<String> = Vec::new();

    eprintln!();

    for path in &scenario_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read scenario: {}", path.display()))?;
        let raw: schema::RawScenario = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse scenario: {}", path.display()))?;
        let repos = resolve_repos(raw.repos, &fixtures_dir)
            .with_context(|| format!("Failed to resolve repos in: {}", path.display()))?;
        let scenario = schema::Scenario {
            name: raw.name,
            description: raw.description,
            repos,
            env: raw.env,
            steps: raw.steps,
        };

        let mut test_env = env::TestEnv::create(&scenario, &project_root)?;

        // Register for cleanup on Ctrl+C.
        if let Ok(mut guard) = cleanup_path.lock() {
            *guard = Some(test_env.base_dir.clone());
        }

        // Generate repos from specs.
        for repo_spec in &scenario.repos {
            repo_gen::generate_repo(repo_spec, &test_env.remotes_dir)?;
            test_env.register_remote(&repo_spec.name);
        }
        test_env.create_template()?;

        if setup_only {
            // Run steps up to --step N (or all steps if not specified).
            let run_until = step
                .unwrap_or(scenario.steps.len())
                .min(scenario.steps.len());
            for (i, s) in scenario.steps.iter().take(run_until).enumerate() {
                eprint!(
                    "{} {} ... ",
                    daft::styles::blue(&format!("[{}/{}]", i + 1, run_until)),
                    &s.name
                );
                runner::execute_step(s, &test_env, true)?;
                eprintln!("{}", daft::styles::green("ok"));
            }
            eprintln!();
            eprintln!("Test environment ready at: {}", test_env.work_dir.display());
            // Print work dir to stdout for shell wrapper to capture for cd.
            println!("{}", test_env.work_dir.display());
            // Don't clean up — the point is to keep the env for manual use.
            if let Ok(mut guard) = cleanup_path.lock() {
                *guard = None;
            }
            continue;
        }

        let result = if is_interactive {
            interactive::run_interactive(&scenario, &test_env, step, loop_count, verbose)?;
            None
        } else {
            Some(runner::run_non_interactive(&scenario, &test_env, verbose)?)
        };

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

        // Clear cleanup path after successful cleanup.
        if let Ok(mut guard) = cleanup_path.lock() {
            *guard = None;
        }

        if let Some(sr) = result {
            total_scenarios += 1;
            total_steps += sr.steps;
            total_passed += sr.passed;
            total_failed += sr.failed;
            if sr.failed > 0 {
                failed_scenarios.push(scenario.name.clone());
            }
        }
    }

    // Print overall summary for non-interactive runs.
    if !is_interactive && scenario_files.len() > 0 {
        use daft::styles;

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
        if !failed_scenarios.is_empty() {
            for name in &failed_scenarios {
                eprintln!("{} {}", styles::red("x"), name);
            }
        }
        eprintln!();

        if total_failed > 0 {
            anyhow::bail!("{total_failed} step(s) failed across {total_scenarios} scenarios");
        }
    }

    Ok(())
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
    use daft::styles;

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
