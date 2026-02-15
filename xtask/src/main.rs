//! xtask - Development automation tasks for daft
//!
//! This binary provides development-time tasks that don't need to be
//! included in the distributed binary.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use clap_mangen::Man;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

/// Available daft commands that need man pages
const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-checkout-branch",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
    "git-worktree-flow-adopt",
    "git-worktree-flow-eject",
    "daft-doctor",
    "daft-release-notes",
];

/// A matrix entry defines a configuration variant for integration tests
struct MatrixEntry {
    name: &'static str,
    /// Git config key=value pairs to set at global scope before running tests
    config: &'static [(&'static str, &'static str)],
}

const MATRIX: &[MatrixEntry] = &[
    MatrixEntry {
        name: "default",
        config: &[],
    },
    MatrixEntry {
        name: "gitoxide",
        config: &[("daft.experimental.gitoxide", "true")],
    },
];

/// Get the clap Command for a given command name
fn get_command_for_name(command_name: &str) -> Option<clap::Command> {
    use clap::CommandFactory;
    match command_name {
        "git-worktree-clone" => Some(daft::commands::clone::Args::command()),
        "git-worktree-init" => Some(daft::commands::init::Args::command()),
        "git-worktree-checkout" => Some(daft::commands::checkout::Args::command()),
        "git-worktree-checkout-branch" => Some(daft::commands::checkout_branch::Args::command()),
        "git-worktree-prune" => Some(daft::commands::prune::Args::command()),
        "git-worktree-carry" => Some(daft::commands::carry::Args::command()),
        "git-worktree-fetch" => Some(daft::commands::fetch::Args::command()),
        "git-worktree-flow-adopt" => Some(daft::commands::flow_adopt::Args::command()),
        "git-worktree-flow-eject" => Some(daft::commands::flow_eject::Args::command()),
        "daft-doctor" => Some(daft::commands::doctor::Args::command()),
        "daft-release-notes" => Some(daft::commands::release_notes::Args::command()),
        _ => None,
    }
}

/// Command clusters for "See Also" links in CLI docs
fn related_commands(command_name: &str) -> Vec<&'static str> {
    match command_name {
        // Setup cluster
        "git-worktree-clone" => vec![
            "git-worktree-init",
            "git-worktree-checkout",
            "git-worktree-flow-adopt",
        ],
        "git-worktree-init" => vec![
            "git-worktree-clone",
            "git-worktree-checkout-branch",
            "git-worktree-flow-adopt",
        ],
        "git-worktree-flow-adopt" => vec![
            "git-worktree-clone",
            "git-worktree-init",
            "git-worktree-flow-eject",
        ],
        // Branching cluster
        "git-worktree-checkout" => vec!["git-worktree-checkout-branch", "git-worktree-carry"],
        "git-worktree-checkout-branch" => vec!["git-worktree-checkout", "git-worktree-carry"],
        // Maintenance cluster
        "git-worktree-prune" => vec!["git-worktree-fetch", "git-worktree-flow-eject"],
        "git-worktree-fetch" => vec!["git-worktree-prune", "git-worktree-carry"],
        "git-worktree-carry" => vec![
            "git-worktree-checkout",
            "git-worktree-checkout-branch",
            "git-worktree-fetch",
        ],
        "git-worktree-flow-eject" => vec![
            "git-worktree-flow-adopt",
            "git-worktree-prune",
            "git-worktree-clone",
        ],
        // Config cluster
        "daft-doctor" => vec!["git-worktree-clone", "git-worktree-init"],
        "daft-release-notes" => vec![],
        _ => vec![],
    }
}

#[derive(Parser)]
#[command(name = "xtask")]
#[command(about = "Development automation tasks for daft")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate man pages for daft commands
    GenMan {
        /// Output directory for man pages
        #[arg(long, default_value = "man")]
        output_dir: PathBuf,

        /// Specific command to generate man page for (default: all commands)
        #[arg(long)]
        command: Option<String>,
    },

    /// Generate CLI reference markdown docs for daft commands
    GenCliDocs {
        /// Output directory for CLI docs
        #[arg(long, default_value = "docs/cli")]
        output_dir: PathBuf,

        /// Specific command to generate docs for (default: all commands)
        #[arg(long)]
        command: Option<String>,
    },

    /// Run integration tests across a matrix of configurations
    TestMatrix {
        /// Run only specific matrix entries (can be repeated)
        #[arg(long)]
        entry: Vec<String>,

        /// List available matrix entries and exit
        #[arg(long)]
        list: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenMan {
            output_dir,
            command,
        } => generate_man_pages(&output_dir, command.as_deref()),
        Commands::GenCliDocs {
            output_dir,
            command,
        } => generate_cli_docs(&output_dir, command.as_deref()),
        Commands::TestMatrix { entry, list } => run_test_matrix(&entry, list),
    }
}

/// Generate man pages and write to a directory
fn generate_man_pages(output_dir: &PathBuf, command: Option<&str>) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let commands_to_generate: Vec<&str> = if let Some(cmd) = command {
        vec![cmd]
    } else {
        COMMANDS.to_vec()
    };

    for command_name in commands_to_generate {
        let cmd = get_command_for_name(command_name)
            .with_context(|| format!("Unknown command: {command_name}"))?;

        let man = Man::new(cmd);
        let mut buffer = Vec::new();
        man.render(&mut buffer)?;

        let filename = format!("{command_name}.1");
        let file_path = output_dir.join(&filename);

        fs::write(&file_path, &buffer)
            .with_context(|| format!("Failed to write man page: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    eprintln!("\nMan pages generated in: {}", output_dir.display());
    Ok(())
}

/// Generate CLI reference markdown docs and write to a directory
fn generate_cli_docs(output_dir: &PathBuf, command: Option<&str>) -> Result<()> {
    fs::create_dir_all(output_dir).with_context(|| {
        format!(
            "Failed to create output directory: {}",
            output_dir.display()
        )
    })?;

    let commands_to_generate: Vec<&str> = if let Some(cmd) = command {
        vec![cmd]
    } else {
        COMMANDS.to_vec()
    };

    for command_name in commands_to_generate {
        let cmd = get_command_for_name(command_name)
            .with_context(|| format!("Unknown command: {command_name}"))?;

        let markdown = render_command_markdown(command_name, &cmd);

        let filename = format!("{command_name}.md");
        let file_path = output_dir.join(&filename);

        fs::write(&file_path, &markdown)
            .with_context(|| format!("Failed to write CLI doc: {}", file_path.display()))?;

        eprintln!("Generated: {}", file_path.display());
    }

    eprintln!("\nCLI docs generated in: {}", output_dir.display());
    Ok(())
}

/// Render a clap Command to a markdown CLI reference page.
fn render_command_markdown(command_name: &str, cmd: &clap::Command) -> String {
    let mut md = String::new();

    let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();

    let long_about = cmd
        .get_long_about()
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Display name: convert "git-worktree-clone" → "git worktree-clone" for git commands,
    // "daft-doctor" → "daft doctor" for daft commands
    let display_name = if let Some(suffix) = command_name.strip_prefix("git-") {
        format!("git {suffix}")
    } else if let Some(suffix) = command_name.strip_prefix("daft-") {
        format!("daft {suffix}")
    } else {
        command_name.to_string()
    };

    // Frontmatter
    md.push_str("---\n");
    md.push_str(&format!("title: {command_name}\n"));
    md.push_str(&format!("description: {about}\n"));
    md.push_str("---\n\n");

    // Title
    md.push_str(&format!("# {display_name}\n\n"));
    md.push_str(&format!("{about}\n\n"));

    // Description
    let description = long_about.trim();
    if !description.is_empty() {
        md.push_str("## Description\n\n");
        md.push_str(description);
        md.push_str("\n\n");
    }

    // Usage line
    md.push_str("## Usage\n\n");
    md.push_str("```\n");
    md.push_str(&build_usage_string(command_name, cmd, &display_name));
    md.push_str("\n```\n\n");

    // Positional arguments
    let positionals: Vec<_> = cmd
        .get_arguments()
        .filter(|a| a.is_positional() && a.get_id() != "version" && a.get_id() != "help")
        .collect();

    if !positionals.is_empty() {
        md.push_str("## Arguments\n\n");
        md.push_str("| Argument | Description | Required |\n");
        md.push_str("|----------|-------------|----------|\n");

        for arg in &positionals {
            let id = arg.get_id().as_str();
            let value_name = arg
                .get_value_names()
                .and_then(|v| v.first().map(|s| s.to_string()))
                .unwrap_or_else(|| id.to_uppercase());

            let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();

            let required = if arg.is_required_set() { "Yes" } else { "No" };

            md.push_str(&format!("| `<{value_name}>` | {help} | {required} |\n"));
        }
        md.push('\n');
    }

    // Options (non-positional arguments)
    let options: Vec<_> = cmd
        .get_arguments()
        .filter(|a| !a.is_positional() && a.get_id() != "version" && a.get_id() != "help")
        .collect();

    if !options.is_empty() {
        md.push_str("## Options\n\n");
        md.push_str("| Option | Description | Default |\n");
        md.push_str("|--------|-------------|----------|\n");

        for arg in &options {
            let mut opt_str = String::new();
            if let Some(short) = arg.get_short() {
                opt_str.push_str(&format!("-{short}"));
            }
            if let Some(long) = arg.get_long() {
                if !opt_str.is_empty() {
                    opt_str.push_str(", ");
                }
                opt_str.push_str(&format!("--{long}"));
            }

            // Add value name if the option takes a value (skip for boolean flags)
            let is_bool_flag = matches!(
                arg.get_action(),
                clap::ArgAction::SetTrue | clap::ArgAction::SetFalse | clap::ArgAction::Count
            );
            if !is_bool_flag {
                if let Some(value_names) = arg.get_value_names() {
                    if !value_names.is_empty() {
                        let name = &value_names[0];
                        opt_str.push_str(&format!(" <{name}>"));
                    }
                }
            }

            let help = arg.get_help().map(|s| s.to_string()).unwrap_or_default();

            let defaults: Vec<_> = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().to_string())
                .collect();
            let default_str = if defaults.is_empty() {
                String::new()
            } else {
                format!("`{}`", defaults.join(", "))
            };

            md.push_str(&format!("| `{opt_str}` | {help} | {default_str} |\n"));
        }
        md.push('\n');
    }

    // Global options
    md.push_str("## Global Options\n\n");
    md.push_str("| Option | Description |\n");
    md.push_str("|--------|-------------|\n");
    md.push_str("| `-h`, `--help` | Print help information |\n");
    md.push_str("| `-V`, `--version` | Print version information |\n");
    md.push('\n');

    // See Also
    let related = related_commands(command_name);
    if !related.is_empty() {
        md.push_str("## See Also\n\n");
        for related_cmd in &related {
            md.push_str(&format!("- [{related_cmd}](./{related_cmd}.md)\n"));
        }
        md.push('\n');
    }

    md
}

/// Build the usage string for a command.
fn build_usage_string(command_name: &str, cmd: &clap::Command, display_name: &str) -> String {
    let mut parts = vec![display_name.to_string()];

    // Check if there are any non-positional, non-built-in options
    let has_options = cmd
        .get_arguments()
        .any(|a| !a.is_positional() && a.get_id() != "version" && a.get_id() != "help");

    if has_options {
        parts.push("[OPTIONS]".to_string());
    }

    // Add positional arguments
    for arg in cmd.get_arguments() {
        if !arg.is_positional() || arg.get_id() == "version" || arg.get_id() == "help" {
            continue;
        }

        let value_name = arg
            .get_value_names()
            .and_then(|v| v.first().map(|s| s.to_string()))
            .unwrap_or_else(|| arg.get_id().as_str().to_uppercase());

        if arg.is_required_set() {
            parts.push(format!("<{value_name}>"));
        } else {
            parts.push(format!("[{value_name}]"));
        }
    }

    // Check for trailing var arg (like fetch's -- PULL_ARGS)
    let _ = command_name; // suppress unused warning; reserved for future use

    parts.join(" ")
}

/// Run integration tests across a matrix of configurations
fn run_test_matrix(entries: &[String], list: bool) -> Result<()> {
    if list {
        println!("Available matrix entries:");
        for entry in MATRIX {
            if entry.config.is_empty() {
                println!("  {:<12} (no extra config)", entry.name);
            } else {
                let pairs: Vec<String> = entry
                    .config
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                println!("  {:<12} {}", entry.name, pairs.join(", "));
            }
        }
        return Ok(());
    }

    // Select entries: all if none specified, otherwise filter
    let selected: Vec<&MatrixEntry> = if entries.is_empty() {
        MATRIX.iter().collect()
    } else {
        let mut selected = Vec::new();
        for name in entries {
            let entry = MATRIX
                .iter()
                .find(|e| e.name == name.as_str())
                .with_context(|| {
                    let available: Vec<&str> = MATRIX.iter().map(|e| e.name).collect();
                    format!(
                        "Unknown matrix entry: '{}'. Available: {}",
                        name,
                        available.join(", ")
                    )
                })?;
            selected.push(entry);
        }
        selected
    };

    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask should be inside project root")
        .to_path_buf();

    let test_script = project_root.join("tests/integration/test_all.sh");
    if !test_script.exists() {
        bail!("Test script not found: {}", test_script.display());
    }

    struct EntryResult {
        name: String,
        success: bool,
        elapsed: std::time::Duration,
    }

    let mut results: Vec<EntryResult> = Vec::new();

    for entry in &selected {
        println!();
        println!("=============================================");
        println!("  Matrix entry: {}", entry.name);
        if !entry.config.is_empty() {
            for (k, v) in entry.config {
                println!("    {k} = {v}");
            }
        }
        println!("=============================================");
        println!();

        // Create a temp file for GIT_CONFIG_GLOBAL
        let config_path =
            std::env::temp_dir().join(format!("daft-test-matrix-{}.gitconfig", entry.name));
        fs::write(&config_path, "")
            .with_context(|| format!("Failed to create temp config: {}", config_path.display()))?;

        // Write config values using git config --file
        for (key, value) in entry.config {
            let status = Command::new("git")
                .args(["config", "--file"])
                .arg(&config_path)
                .args([key, value])
                .status()
                .with_context(|| format!("Failed to run git config for {key}={value}"))?;
            if !status.success() {
                bail!("git config --file failed for {key}={value}");
            }
        }

        let start = Instant::now();

        let status = Command::new("bash")
            .arg(&test_script)
            .env("GIT_CONFIG_GLOBAL", &config_path)
            .current_dir(&project_root)
            .status()
            .with_context(|| format!("Failed to invoke test script for entry '{}'", entry.name))?;

        let elapsed = start.elapsed();

        results.push(EntryResult {
            name: entry.name.to_string(),
            success: status.success(),
            elapsed,
        });

        // Clean up temp config
        let _ = fs::remove_file(&config_path);
    }

    // Print summary table
    println!();
    println!("=========================================================");
    println!("  Test Matrix Summary");
    println!("=========================================================");
    for result in &results {
        let status_str = if result.success { "PASS" } else { "FAIL" };
        println!(
            "  {:<12} {} ({:.1}s)",
            result.name,
            status_str,
            result.elapsed.as_secs_f64()
        );
    }
    println!("=========================================================");

    let failed: Vec<&EntryResult> = results.iter().filter(|r| !r.success).collect();
    if failed.is_empty() {
        println!("All matrix entries passed!");
        Ok(())
    } else {
        let names: Vec<&str> = failed.iter().map(|r| r.name.as_str()).collect();
        bail!("Matrix entries failed: {}", names.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_commands_have_valid_handlers() {
        for command_name in COMMANDS {
            assert!(
                get_command_for_name(command_name).is_some(),
                "Command '{}' has no handler",
                command_name
            );
        }
    }

    #[test]
    fn test_unknown_command_returns_none() {
        assert!(get_command_for_name("unknown-command").is_none());
    }

    #[test]
    fn test_man_page_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-man");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating a single man page
        generate_man_pages(&temp_dir, Some("git-worktree-clone")).unwrap();

        let man_file = temp_dir.join("git-worktree-clone.1");
        assert!(man_file.exists(), "Man page was not generated");

        let content = fs::read_to_string(&man_file).unwrap();
        assert!(content.contains(".TH"), "Man page missing .TH header");

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_all_man_pages_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-all-man");
        let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous test
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating all man pages
        generate_man_pages(&temp_dir, None).unwrap();

        // Verify all expected man pages exist
        for command_name in COMMANDS {
            let man_file = temp_dir.join(format!("{command_name}.1"));
            assert!(
                man_file.exists(),
                "Man page for '{}' was not generated",
                command_name
            );

            let content = fs::read_to_string(&man_file).unwrap();
            assert!(
                content.contains(".TH"),
                "Man page for '{}' missing .TH header",
                command_name
            );
        }

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cli_docs_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-cli-docs");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating a single CLI doc
        generate_cli_docs(&temp_dir, Some("git-worktree-clone")).unwrap();

        let doc_file = temp_dir.join("git-worktree-clone.md");
        assert!(doc_file.exists(), "CLI doc was not generated");

        let content = fs::read_to_string(&doc_file).unwrap();
        assert!(content.contains("---"), "CLI doc missing frontmatter");
        assert!(
            content.contains("# git worktree-clone"),
            "CLI doc missing title"
        );
        assert!(
            content.contains("## Usage"),
            "CLI doc missing Usage section"
        );
        assert!(
            content.contains("## Options"),
            "CLI doc missing Options section"
        );
        assert!(
            content.contains("## See Also"),
            "CLI doc missing See Also section"
        );

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_all_cli_docs_generation() {
        let temp_dir = std::env::temp_dir().join("xtask-test-all-cli-docs");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        // Test generating all CLI docs
        generate_cli_docs(&temp_dir, None).unwrap();

        // Verify all expected CLI docs exist
        for command_name in COMMANDS {
            let doc_file = temp_dir.join(format!("{command_name}.md"));
            assert!(
                doc_file.exists(),
                "CLI doc for '{}' was not generated",
                command_name
            );

            let content = fs::read_to_string(&doc_file).unwrap();
            assert!(
                content.contains("---"),
                "CLI doc for '{}' missing frontmatter",
                command_name
            );
            assert!(
                content.contains("## Usage"),
                "CLI doc for '{}' missing Usage section",
                command_name
            );
        }

        // Clean up
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_related_commands_returns_entries() {
        let related = related_commands("git-worktree-clone");
        assert!(!related.is_empty());
        assert!(related.contains(&"git-worktree-init"));
    }

    #[test]
    fn test_related_commands_unknown_returns_empty() {
        let related = related_commands("unknown-command");
        assert!(related.is_empty());
    }

    #[test]
    fn test_matrix_entries_have_unique_names() {
        let mut names: Vec<&str> = MATRIX.iter().map(|e| e.name).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            original_len,
            "Matrix entries must have unique names"
        );
    }

    #[test]
    fn test_matrix_has_default_entry() {
        assert!(
            MATRIX.iter().any(|e| e.name == "default"),
            "Matrix must have a 'default' entry"
        );
    }

    #[test]
    fn test_matrix_list_does_not_error() {
        // --list should succeed without running any tests
        run_test_matrix(&[], true).unwrap();
    }

    #[test]
    fn test_matrix_unknown_entry_errors() {
        let result = run_test_matrix(&["nonexistent".to_string()], false);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Unknown matrix entry"),
            "Error should mention unknown entry, got: {err}"
        );
    }
}
