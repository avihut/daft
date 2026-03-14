pub mod env;
pub mod interactive;
pub mod repo_gen;
pub mod runner;
pub mod schema;

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

pub fn run(
    scenarios: Vec<PathBuf>,
    no_interactive: bool,
    verbose: bool,
    step: Option<usize>,
    loop_count: Option<usize>,
    keep: bool,
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

        // Generate repos from specs.
        for repo_spec in &scenario.repos {
            repo_gen::generate_repo(repo_spec, &test_env.remotes_dir)?;
            test_env.register_remote(&repo_spec.name);
        }
        test_env.create_template()?;

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
        } else if let Err(e) = test_env.cleanup() {
            eprintln!("  Warning: cleanup failed: {e}");
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
/// - A bare name like `clone-basic` (resolved to `<scenarios_dir>/clone-basic.yml` or `.yaml`)
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
        // Try resolving as a name within scenarios_dir.
        let stem = arg.file_stem().unwrap_or(arg.as_os_str());
        let yml = scenarios_dir.join(format!("{}.yml", stem.to_string_lossy()));
        let yaml = scenarios_dir.join(format!("{}.yaml", stem.to_string_lossy()));
        if yml.exists() {
            resolved.push(yml);
        } else if yaml.exists() {
            resolved.push(yaml);
        } else {
            anyhow::bail!(
                "Scenario not found: '{}'\n  Looked for:\n    {}\n    {}\n    {}",
                arg.display(),
                arg.display(),
                yml.display(),
                yaml.display()
            );
        }
    }
    Ok(resolved)
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

    let scenarios = discover_scenarios(dir)?;
    if scenarios.is_empty() {
        eprintln!("No scenarios found in {}", dir.display());
        return Ok(());
    }

    eprintln!();
    eprintln!("  {}", styles::bold("Available scenarios:"));
    eprintln!();

    for path in &scenarios {
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if let Ok(scenario) = serde_yaml::from_str::<schema::RawScenario>(&content) {
            let desc = scenario.description.as_deref().unwrap_or("");
            let filename = path.file_name().unwrap_or_default().to_string_lossy();
            eprintln!(
                "  {} {}",
                styles::bold(&filename),
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
