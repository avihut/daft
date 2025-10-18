/// Shell completion generation for daft commands
///
/// Generates shell completion scripts for bash, zsh, and fish that provide:
/// - Static completions for flags and options (via clap_complete)
/// - Dynamic completions for branch names (via daft __complete helper)
use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::{generate, Shell};
use std::io;
use std::path::PathBuf;

/// Available daft commands that need completion scripts
const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-checkout-branch",
    "git-worktree-checkout-branch-from-default",
    "git-worktree-prune",
];

#[derive(Parser)]
#[command(name = "daft-completions")]
#[command(about = "Generate shell completion scripts for daft commands")]
struct Args {
    #[arg(value_enum, help = "Shell to generate completions for")]
    shell: Shell,

    #[arg(
        short,
        long,
        help = "Specific command to generate completions for (default: all commands)"
    )]
    command: Option<String>,

    #[arg(short, long, help = "Install completions to standard shell locations")]
    install: bool,
}

pub fn run() -> Result<()> {
    // When called as a subcommand, skip "daft" and "completions" from args
    let mut args_vec: Vec<String> = std::env::args().collect();

    // If args start with [daft, completions, ...], keep only [daft, ...]
    // to make clap parse correctly
    if args_vec.len() >= 2 && args_vec[1] == "completions" {
        args_vec.remove(1); // Remove "completions", keep "daft" for clap
    }

    let args = Args::parse_from(&args_vec);

    if args.install {
        install_completions(args.shell)?;
    } else if let Some(command) = args.command {
        generate_completion_for_command(&command, args.shell)?;
    } else {
        // Generate for all commands
        for command in COMMANDS {
            generate_completion_for_command(command, args.shell)?;
        }
    }

    Ok(())
}

/// Generate completion script for a specific command
fn generate_completion_for_command(command_name: &str, shell: Shell) -> Result<()> {
    // For now, we'll generate a basic completion structure
    // This will be enhanced with dynamic completion hooks
    match command_name {
        "git-worktree-clone" => {
            generate(
                shell,
                &mut crate::commands::clone::Args::command(),
                command_name,
                &mut io::stdout(),
            );
        }
        "git-worktree-init" => {
            generate(
                shell,
                &mut crate::commands::init::Args::command(),
                command_name,
                &mut io::stdout(),
            );
        }
        "git-worktree-checkout" => {
            generate(
                shell,
                &mut crate::commands::checkout::Args::command(),
                command_name,
                &mut io::stdout(),
            );
        }
        "git-worktree-checkout-branch" => {
            generate(
                shell,
                &mut crate::commands::checkout_branch::Args::command(),
                command_name,
                &mut io::stdout(),
            );
        }
        "git-worktree-checkout-branch-from-default" => {
            generate(
                shell,
                &mut crate::commands::checkout_branch_from_default::Args::command(),
                command_name,
                &mut io::stdout(),
            );
        }
        "git-worktree-prune" => {
            generate(
                shell,
                &mut crate::commands::prune::Args::command(),
                command_name,
                &mut io::stdout(),
            );
        }
        _ => {
            anyhow::bail!("Unknown command: {}", command_name);
        }
    }

    Ok(())
}

/// Install completions to standard shell locations
fn install_completions(shell: Shell) -> Result<()> {
    let install_dir = get_completion_dir(shell)?;

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create completion directory: {:?}", install_dir))?;

    eprintln!("Installing completions to: {:?}", install_dir);

    for command in COMMANDS {
        let filename = get_completion_filename(command, shell);
        let file_path = install_dir.join(&filename);

        eprintln!("  Installing: {}", filename);

        let file = std::fs::File::create(&file_path)
            .with_context(|| format!("Failed to create completion file: {:?}", file_path))?;

        let mut writer = io::BufWriter::new(file);

        match *command {
            "git-worktree-clone" => {
                generate(
                    shell,
                    &mut crate::commands::clone::Args::command(),
                    *command,
                    &mut writer,
                );
            }
            "git-worktree-init" => {
                generate(
                    shell,
                    &mut crate::commands::init::Args::command(),
                    *command,
                    &mut writer,
                );
            }
            "git-worktree-checkout" => {
                generate(
                    shell,
                    &mut crate::commands::checkout::Args::command(),
                    *command,
                    &mut writer,
                );
            }
            "git-worktree-checkout-branch" => {
                generate(
                    shell,
                    &mut crate::commands::checkout_branch::Args::command(),
                    *command,
                    &mut writer,
                );
            }
            "git-worktree-checkout-branch-from-default" => {
                generate(
                    shell,
                    &mut crate::commands::checkout_branch_from_default::Args::command(),
                    *command,
                    &mut writer,
                );
            }
            "git-worktree-prune" => {
                generate(
                    shell,
                    &mut crate::commands::prune::Args::command(),
                    *command,
                    &mut writer,
                );
            }
            _ => unreachable!(),
        }
    }

    eprintln!("\n✓ Completions installed successfully!");
    print_post_install_message(shell)?;

    Ok(())
}

/// Get the standard completion directory for a shell
fn get_completion_dir(shell: Shell) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;

    let dir = match shell {
        Shell::Bash => {
            // Try XDG first, fallback to ~/.bash_completion.d
            let xdg_data = std::env::var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".local/share"));
            xdg_data.join("bash-completion/completions")
        }
        Shell::Zsh => {
            // Use ~/.zfunc as it's commonly added to fpath
            home.join(".zfunc")
        }
        Shell::Fish => {
            // Try XDG first, fallback to ~/.config/fish
            let xdg_config = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".config"));
            xdg_config.join("fish/completions")
        }
        _ => anyhow::bail!("Unsupported shell: {:?}", shell),
    };

    Ok(dir)
}

/// Get the filename for a completion script
fn get_completion_filename(command: &str, shell: Shell) -> String {
    match shell {
        Shell::Bash => command.to_string(),
        Shell::Zsh => format!("_{command}"),
        Shell::Fish => format!("{command}.fish"),
        _ => format!("{command}.{shell:?}").to_lowercase(),
    }
}

/// Print post-installation instructions
fn print_post_install_message(shell: Shell) -> Result<()> {
    match shell {
        Shell::Bash => {
            eprintln!("\nTo activate completions, add this to your ~/.bashrc:");
            eprintln!("  # Enable bash completion");
            eprintln!("  if [ -f ~/.local/share/bash-completion/bash_completion ]; then");
            eprintln!("    . ~/.local/share/bash-completion/bash_completion");
            eprintln!("  fi");
            eprintln!(
                "\nOr install bash-completion via your package manager and restart your shell."
            );
        }
        Shell::Zsh => {
            eprintln!("\nTo activate completions, add this to your ~/.zshrc:");
            eprintln!("  # Add completions directory to fpath");
            eprintln!("  fpath=(~/.zfunc $fpath)");
            eprintln!("  autoload -Uz compinit && compinit");
            eprintln!("\nThen restart your shell or run: source ~/.zshrc");
        }
        Shell::Fish => {
            eprintln!("\nCompletions are automatically loaded by fish.");
            eprintln!("Restart your shell or run: source ~/.config/fish/config.fish");
        }
        _ => {}
    }

    Ok(())
}
