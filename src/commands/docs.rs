/// Documentation command for `git daft`
///
/// Shows daft commands in git-style help format, dynamically extracting
/// descriptions from clap command definitions.
use anyhow::Result;
use clap::{Command, CommandFactory};
use std::path::Path;

use crate::commands::{
    carry, checkout, clone, completions, doctor, fetch, flow_adopt, flow_eject, hooks, init,
    multi_remote, prune, release_notes, shell_init, shortcuts, worktree_branch,
};

/// A category of commands with a title and list of commands.
struct CommandCategory {
    title: &'static str,
    commands: Vec<CommandEntry>,
}

/// A single command entry with its display name and clap Command.
struct CommandEntry {
    display_name: &'static str,
    command: Command,
}

/// Get all command categories with their commands.
fn get_command_categories() -> Vec<CommandCategory> {
    vec![
        CommandCategory {
            title: "start a worktree-based repository",
            commands: vec![
                CommandEntry {
                    display_name: "worktree-clone",
                    command: clone::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-init",
                    command: init::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-flow-adopt",
                    command: flow_adopt::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "work on branches (each branch gets its own directory)",
            commands: vec![CommandEntry {
                display_name: "worktree-checkout",
                command: checkout::Args::command(),
            }],
        },
        CommandCategory {
            title: "share changes across worktrees",
            commands: vec![CommandEntry {
                display_name: "worktree-carry",
                command: carry::Args::command(),
            }],
        },
        CommandCategory {
            title: "maintain your worktrees",
            commands: vec![
                CommandEntry {
                    display_name: "worktree-branch",
                    command: worktree_branch::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-prune",
                    command: prune::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-fetch",
                    command: fetch::Args::command(),
                },
                CommandEntry {
                    display_name: "worktree-flow-eject",
                    command: flow_eject::Args::command(),
                },
            ],
        },
        CommandCategory {
            title: "manage daft configuration",
            commands: vec![
                CommandEntry {
                    display_name: "daft hooks",
                    command: hooks::Args::command(),
                },
                CommandEntry {
                    display_name: "daft multi-remote",
                    command: multi_remote::Args::command(),
                },
                CommandEntry {
                    display_name: "daft setup shortcuts",
                    command: shortcuts::Args::command(),
                },
                CommandEntry {
                    display_name: "daft shell-init",
                    command: shell_init::Args::command(),
                },
                CommandEntry {
                    display_name: "daft doctor",
                    command: doctor::Args::command(),
                },
                CommandEntry {
                    display_name: "daft completions",
                    command: completions::Args::command(),
                },
                CommandEntry {
                    display_name: "daft release-notes",
                    command: release_notes::Args::command(),
                },
            ],
        },
    ]
}

/// Extract the short description (about) from a clap Command.
fn get_about(cmd: &Command) -> String {
    cmd.get_about()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "(no description)".to_string())
}

/// Calculate the maximum display name length for proper alignment.
fn max_display_name_len(categories: &[CommandCategory]) -> usize {
    categories
        .iter()
        .flat_map(|cat| cat.commands.iter())
        .map(|entry| entry.display_name.len())
        .max()
        .unwrap_or(20)
}

pub fn run() -> Result<()> {
    // Detect how we were invoked
    let program_path = std::env::args()
        .next()
        .unwrap_or_else(|| "daft".to_string());
    let program_name = Path::new(&program_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("daft");

    // Determine primary/secondary invocation style based on how we were called
    let (primary, secondary) = if program_name == "git-daft" {
        ("git", "daft")
    } else {
        ("daft", "git")
    };

    println!("usage: daft <command> [<args>]");
    println!("   or: {primary} worktree-<command> [<args>]");
    println!("   or: {secondary} worktree-<command> [<args>]");

    println!();
    println!("These are common daft commands used in various situations:");

    let categories = get_command_categories();
    let max_len = max_display_name_len(&categories);

    for category in &categories {
        println!();
        println!("{}", category.title);

        for entry in &category.commands {
            let about = get_about(&entry.command);
            // Pad the display name for alignment
            println!(
                "   {:width$}   {}",
                entry.display_name,
                about,
                width = max_len
            );
        }
    }

    println!();
    println!("short aliases (daft <verb>)");
    println!("   go <branch>          Check out an existing branch worktree");
    println!("   start <branch>       Create a new branch worktree (-b)");
    println!("   clone, init, carry, update, prune, remove, adopt, eject");

    println!();
    println!("'{primary} worktree-<command> --help' to read about a specific command.");
    println!("See https://github.com/avihut/daft for documentation.");

    Ok(())
}
