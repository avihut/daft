//! xtask - Development automation tasks for daft
//!
//! This binary provides development-time tasks that don't need to be
//! included in the distributed binary.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use clap_mangen::Man;
use std::fs;
use std::path::PathBuf;

/// Available daft commands that need man pages
const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-checkout-branch",
    "git-worktree-checkout-branch-from-default",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
    "git-worktree-flow-adopt",
    "git-worktree-flow-eject",
    "daft-doctor",
    "daft-release-notes",
];

/// Get the clap Command for a given command name
fn get_command_for_name(command_name: &str) -> Option<clap::Command> {
    use clap::CommandFactory;
    match command_name {
        "git-worktree-clone" => Some(daft::commands::clone::Args::command()),
        "git-worktree-init" => Some(daft::commands::init::Args::command()),
        "git-worktree-checkout" => Some(daft::commands::checkout::Args::command()),
        "git-worktree-checkout-branch" => Some(daft::commands::checkout_branch::Args::command()),
        "git-worktree-checkout-branch-from-default" => {
            Some(daft::commands::checkout_branch_from_default::Args::command())
        }
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::GenMan {
            output_dir,
            command,
        } => generate_man_pages(&output_dir, command.as_deref()),
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
}
