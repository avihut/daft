pub mod env;
pub mod interactive;
pub mod repo_gen;
pub mod runner;
pub mod schema;

use anyhow::{Context, Result};
use std::io::IsTerminal;
use std::path::PathBuf;

pub fn run(
    scenarios: Vec<PathBuf>,
    no_interactive: bool,
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

    if list {
        return list_scenarios(&scenarios_dir);
    }

    let scenario_files = if scenarios.is_empty() {
        discover_scenarios(&scenarios_dir)?
    } else {
        scenarios
    };

    if scenario_files.is_empty() {
        anyhow::bail!("No scenario files found in {}", scenarios_dir.display());
    }

    let is_interactive = !no_interactive && std::io::stdin().is_terminal();

    for path in &scenario_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read scenario: {}", path.display()))?;
        let scenario: schema::Scenario = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse scenario: {}", path.display()))?;

        let mut test_env = env::TestEnv::create(&scenario, &project_root)?;

        // Generate repos from specs.
        for repo_spec in &scenario.repos {
            repo_gen::generate_repo(repo_spec, &test_env.remotes_dir)?;
            test_env.register_remote(&repo_spec.name);
        }
        test_env.create_template()?;

        let result = if is_interactive {
            interactive::run_interactive(&scenario, &test_env, step, loop_count)
        } else {
            runner::run_non_interactive(&scenario, &test_env)
        };

        if keep {
            eprintln!(
                "\n  Test environment kept at: {}",
                test_env.base_dir.display()
            );
        } else {
            test_env.cleanup()?;
        }

        result?;
    }

    Ok(())
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
        if let Ok(scenario) = serde_yaml::from_str::<schema::Scenario>(&content) {
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
