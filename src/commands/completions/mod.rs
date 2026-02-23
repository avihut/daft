/// Shell completion generation for daft commands
///
/// Generates shell completion scripts for bash, zsh, fish, and fig that provide:
/// - Static completions for flags and options (via clap introspection)
/// - Dynamic completions for branch names (via daft __complete helper)
mod bash;
mod fig;
mod fish;
mod zsh;

use anyhow::{Context, Result};
use clap::{Command, CommandFactory, Parser, ValueEnum};
use std::path::PathBuf;

/// Completion targets supported by daft
#[derive(Debug, Clone, ValueEnum)]
pub(super) enum CompletionTarget {
    Bash,
    Zsh,
    Fish,
    Fig,
}

/// Verb aliases that map to underlying git-worktree-* commands.
/// Each entry is (list of verb names, underlying command name).
/// Used by completion generators to offer flag completions for verb aliases.
pub(super) const VERB_ALIAS_GROUPS: &[(&[&str], &str)] = &[
    (&["go", "start"], "git-worktree-checkout"),
    (&["carry"], "git-worktree-carry"),
    (&["fetch"], "git-worktree-fetch"),
];

/// Available daft commands that need completion scripts
pub(super) const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
    "git-worktree-flow-adopt",
    "git-worktree-flow-eject",
];

/// Get the clap Command for a given command name by using CommandFactory
pub(super) fn get_command_for_name(command_name: &str) -> Option<Command> {
    match command_name {
        "git-worktree-clone" => Some(crate::commands::clone::Args::command()),
        "git-worktree-init" => Some(crate::commands::init::Args::command()),
        "git-worktree-checkout" => Some(crate::commands::checkout::Args::command()),
        "git-worktree-prune" => Some(crate::commands::prune::Args::command()),
        "git-worktree-carry" => Some(crate::commands::carry::Args::command()),
        "git-worktree-fetch" => Some(crate::commands::fetch::Args::command()),
        "git-worktree-flow-adopt" => Some(crate::commands::flow_adopt::Args::command()),
        "git-worktree-flow-eject" => Some(crate::commands::flow_eject::Args::command()),
        _ => None,
    }
}

/// Extract flag strings from a clap Command for shell completions
/// Returns a tuple of (short_and_long_flags, short_flags, long_flags)
pub(super) fn extract_flags(cmd: &Command) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut all_flags = Vec::new();
    let mut short_flags = Vec::new();
    let mut long_flags = Vec::new();

    for arg in cmd.get_arguments() {
        // Only process actual flags (not positional arguments)
        if arg.get_short().is_none() && arg.get_long().is_none() {
            continue;
        }

        if let Some(short) = arg.get_short() {
            short_flags.push(format!("-{}", short));
            all_flags.push(format!("-{}", short));
        }

        if let Some(long) = arg.get_long() {
            long_flags.push(format!("--{}", long));
            all_flags.push(format!("--{}", long));
        }
    }

    // Add standard clap-generated flags that may not be in user Args
    // These are always available in clap-generated commands
    if !all_flags.contains(&"-h".to_string()) {
        all_flags.push("-h".to_string());
        short_flags.push("-h".to_string());
    }
    if !all_flags.contains(&"--help".to_string()) {
        all_flags.push("--help".to_string());
        long_flags.push("--help".to_string());
    }
    if !all_flags.contains(&"-V".to_string()) {
        all_flags.push("-V".to_string());
        short_flags.push("-V".to_string());
    }
    if !all_flags.contains(&"--version".to_string()) {
        all_flags.push("--version".to_string());
        long_flags.push("--version".to_string());
    }

    (all_flags, short_flags, long_flags)
}

/// Get formatted flag descriptions for fish/zsh completions
pub(super) fn get_flag_descriptions(cmd: &Command) -> Vec<(String, String, Option<String>)> {
    let mut descriptions = Vec::new();
    let mut has_help = false;
    let mut has_version = false;

    for arg in cmd.get_arguments() {
        // Only process actual flags
        if arg.get_short().is_none() && arg.get_long().is_none() {
            continue;
        }

        let short = arg.get_short().map(|c| format!("-{}", c));
        let long = arg.get_long().map(|s| format!("--{}", s));
        let help = arg
            .get_help()
            .map(|h| h.to_string())
            .unwrap_or_default()
            .replace('\'', "\\'");

        // Track if we've seen help/version flags
        if long.as_deref() == Some("--help") {
            has_help = true;
        }
        if long.as_deref() == Some("--version") {
            has_version = true;
        }

        // Store (short, long, description) tuple
        descriptions.push((
            short.unwrap_or_default(),
            long.unwrap_or_default(),
            if help.is_empty() { None } else { Some(help) },
        ));
    }

    // Add standard clap flags if not already present
    if !has_help {
        descriptions.push((
            "-h".to_string(),
            "--help".to_string(),
            Some("Print help".to_string()),
        ));
    }
    if !has_version {
        descriptions.push((
            "-V".to_string(),
            "--version".to_string(),
            Some("Print version".to_string()),
        ));
    }

    descriptions
}

#[derive(Parser)]
#[command(name = "daft-completions")]
#[command(about = "Generate shell completion scripts for daft commands")]
pub struct Args {
    #[arg(
        value_enum,
        help = "Target to generate completions for (bash, zsh, fish, fig)"
    )]
    target: CompletionTarget,

    #[arg(
        short,
        long,
        help = "Specific command to generate completions for (default: all commands)"
    )]
    command: Option<String>,

    #[arg(short, long, help = "Install completions to standard locations")]
    install: bool,
}

/// Silently install Fig/Amazon Q specs if an autocomplete directory exists.
///
/// Called from `shell-init` so specs stay in sync whenever a shell starts.
/// Only writes if `~/.amazon-q/autocomplete/` or `~/.fig/autocomplete/` already
/// exists on disk, meaning the user has Amazon Q / Kiro / Fig installed.
/// All errors are swallowed — this must never interfere with shell startup.
pub fn maybe_install_fig_specs() {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return,
    };

    // Detect which parent directory exists (~/.amazon-q/ or ~/.fig/)
    // and install to the autocomplete/build/ subdirectory within it.
    // Kiro / Amazon Q loads specs from autocomplete/build/, not autocomplete/.
    let amazon_q_parent = home.join(".amazon-q");
    let fig_parent = home.join(".fig");

    let install_dir = if amazon_q_parent.is_dir() {
        amazon_q_parent.join("autocomplete/build")
    } else if fig_parent.is_dir() {
        fig_parent.join("autocomplete/build")
    } else {
        return; // No Amazon Q / Fig installation found — nothing to do
    };

    // Create the build/ directory if it doesn't exist yet
    if std::fs::create_dir_all(&install_dir).is_err() {
        return;
    }

    // Write each command spec
    for command in COMMANDS {
        if let Ok(content) = fig::generate_fig_completion_string(command) {
            let _ = std::fs::write(install_dir.join(format!("{command}.js")), content);
        }
    }

    // Write shortcut alias specs
    let mut seen_aliases = std::collections::HashSet::new();
    for shortcut in crate::shortcuts::SHORTCUTS {
        if seen_aliases.insert(shortcut.alias) {
            let content = fig::generate_fig_alias_string(shortcut.alias, shortcut.command);
            let _ = std::fs::write(install_dir.join(format!("{}.js", shortcut.alias)), content);
        }
    }

    // Write daft.js umbrella spec
    if let Ok(content) = fig::generate_fig_daft_spec() {
        let _ = std::fs::write(install_dir.join("daft.js"), content);
    }

    // Write git-daft.js spec
    let _ = std::fs::write(
        install_dir.join("git-daft.js"),
        fig::generate_fig_git_daft_spec(),
    );
}

/// Generate all completion scripts as a single string for embedding in shell-init output.
pub fn generate_all_completions(shell_name: &str) -> Result<String> {
    let target = match shell_name {
        "bash" => CompletionTarget::Bash,
        "zsh" => CompletionTarget::Zsh,
        "fish" => CompletionTarget::Fish,
        _ => anyhow::bail!("Unsupported shell: {shell_name}"),
    };

    let mut output = String::new();
    for command in COMMANDS {
        output.push_str(&generate_completion_string_for_command(command, &target)?);
        output.push('\n');
    }

    // Add completions for `daft` subcommands (hooks run, etc.)
    output.push_str(&generate_daft_subcommand_completions(&target));
    output.push('\n');

    Ok(output)
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
        install_completions(&args.target)?;
    } else if let Some(command) = args.command {
        generate_completion_for_command(&command, &args.target)?;
    } else {
        generate_all_output(&args.target)?;
    }

    Ok(())
}

/// Generate all output for a given target (all commands + extras for fig)
fn generate_all_output(target: &CompletionTarget) -> Result<()> {
    match target {
        CompletionTarget::Fig => {
            // Print each command spec with file header
            for command in COMMANDS {
                println!("// File: {command}.js");
                print!("{}", fig::generate_fig_completion_string(command)?);
                println!();
            }

            // Print shortcut alias specs
            let mut seen_aliases = std::collections::HashSet::new();
            for shortcut in crate::shortcuts::SHORTCUTS {
                if seen_aliases.insert(shortcut.alias) {
                    println!("// File: {}.js", shortcut.alias);
                    print!(
                        "{}",
                        fig::generate_fig_alias_string(shortcut.alias, shortcut.command)
                    );
                    println!();
                }
            }

            // Print daft.js umbrella spec
            println!("// File: daft.js");
            print!("{}", fig::generate_fig_daft_spec()?);
            println!();

            // Print git-daft.js spec
            println!("// File: git-daft.js");
            print!("{}", fig::generate_fig_git_daft_spec());
        }
        _ => {
            for command in COMMANDS {
                generate_completion_for_command(command, target)?;
            }
            // Add daft subcommand completions (hooks run, etc.)
            print!("{}", generate_daft_subcommand_completions(target));
        }
    }
    Ok(())
}

/// Generate completion script for a specific command
fn generate_completion_for_command(command_name: &str, target: &CompletionTarget) -> Result<()> {
    print!(
        "{}",
        generate_completion_string_for_command(command_name, target)?
    );
    Ok(())
}

/// Generate completion script as a String
fn generate_completion_string_for_command(
    command_name: &str,
    target: &CompletionTarget,
) -> Result<String> {
    match target {
        CompletionTarget::Bash => bash::generate_bash_completion_string(command_name),
        CompletionTarget::Zsh => zsh::generate_zsh_completion_string(command_name),
        CompletionTarget::Fish => fish::generate_fish_completion_string(command_name),
        CompletionTarget::Fig => fig::generate_fig_completion_string(command_name),
    }
}

/// Generate completions for `daft` subcommands (hooks run, etc.)
fn generate_daft_subcommand_completions(target: &CompletionTarget) -> String {
    match target {
        CompletionTarget::Bash => bash::DAFT_BASH_COMPLETIONS.to_string(),
        CompletionTarget::Zsh => zsh::DAFT_ZSH_COMPLETIONS.to_string(),
        CompletionTarget::Fish => fish::generate_daft_fish_completions(),
        CompletionTarget::Fig => String::new(), // Handled in fig spec
    }
}

/// Install completions to standard locations
fn install_completions(target: &CompletionTarget) -> Result<()> {
    match target {
        CompletionTarget::Fig => fig::install_fig_completions(),
        _ => install_shell_completions(target),
    }
}

/// Install shell completions (bash/zsh/fish) to standard locations
fn install_shell_completions(target: &CompletionTarget) -> Result<()> {
    let install_dir = get_completion_dir(target)?;

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create completion directory: {:?}", install_dir))?;

    eprintln!("Installing completions to: {:?}", install_dir);

    for command in COMMANDS {
        let filename = get_completion_filename(command, target);
        let file_path = install_dir.join(&filename);

        eprintln!("  Installing: {filename}");

        // Generate and write completion file
        std::fs::write(
            &file_path,
            generate_completion_string_for_command(command, target)?,
        )
        .with_context(|| format!("Failed to write completion file: {:?}", file_path))?;
    }

    eprintln!("\n✓ Completions installed successfully!");
    print_post_install_message(target)?;

    Ok(())
}

/// Get the standard completion directory for a shell
fn get_completion_dir(target: &CompletionTarget) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;

    let dir = match target {
        CompletionTarget::Bash => {
            // Try XDG first, fallback to ~/.bash_completion.d
            let xdg_data = std::env::var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".local/share"));
            xdg_data.join("bash-completion/completions")
        }
        CompletionTarget::Zsh => {
            // Use ~/.zfunc as it's commonly added to fpath
            home.join(".zfunc")
        }
        CompletionTarget::Fish => {
            // Try XDG first, fallback to ~/.config/fish
            let xdg_config = std::env::var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.join(".config"));
            xdg_config.join("fish/completions")
        }
        CompletionTarget::Fig => {
            // Handled separately by install_fig_completions
            unreachable!("Fig uses install_fig_completions directly")
        }
    };

    Ok(dir)
}

/// Get the filename for a completion script
fn get_completion_filename(command: &str, target: &CompletionTarget) -> String {
    match target {
        CompletionTarget::Bash => command.to_string(),
        CompletionTarget::Zsh => format!("_{command}"),
        CompletionTarget::Fish => format!("{command}.fish"),
        CompletionTarget::Fig => format!("{command}.js"),
    }
}

/// Print post-installation instructions
fn print_post_install_message(target: &CompletionTarget) -> Result<()> {
    match target {
        CompletionTarget::Bash => {
            eprintln!("\nTo activate completions, add this to your ~/.bashrc:");
            eprintln!("  # Enable bash completion");
            eprintln!("  if [ -f ~/.local/share/bash-completion/bash_completion ]; then");
            eprintln!("    . ~/.local/share/bash-completion/bash_completion");
            eprintln!("  fi");
            eprintln!(
                "\nOr install bash-completion via your package manager and restart your shell."
            );
        }
        CompletionTarget::Zsh => {
            eprintln!("\nTo activate completions, add this to your ~/.zshrc:");
            eprintln!("  # Add completions directory to fpath");
            eprintln!("  fpath=(~/.zfunc $fpath)");
            eprintln!("  autoload -Uz compinit && compinit");
            eprintln!("\nThen restart your shell or run: source ~/.zshrc");
        }
        CompletionTarget::Fish => {
            eprintln!("\nCompletions are automatically loaded by fish.");
            eprintln!("Restart your shell or run: source ~/.config/fish/config.fish");
        }
        CompletionTarget::Fig => {
            // Handled by install_fig_completions
        }
    }

    Ok(())
}
