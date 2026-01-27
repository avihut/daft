/// Shell completion generation for daft commands
///
/// Generates shell completion scripts for bash, zsh, and fish that provide:
/// - Static completions for flags and options (via clap introspection)
/// - Dynamic completions for branch names (via daft __complete helper)
use anyhow::{Context, Result};
use clap::{Command, CommandFactory, Parser};
use clap_complete::Shell;
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

/// Get the clap Command for a given command name by using CommandFactory
fn get_command_for_name(command_name: &str) -> Option<Command> {
    match command_name {
        "git-worktree-clone" => Some(crate::commands::clone::Args::command()),
        "git-worktree-init" => Some(crate::commands::init::Args::command()),
        "git-worktree-checkout" => Some(crate::commands::checkout::Args::command()),
        "git-worktree-checkout-branch" => Some(crate::commands::checkout_branch::Args::command()),
        "git-worktree-checkout-branch-from-default" => {
            Some(crate::commands::checkout_branch_from_default::Args::command())
        }
        "git-worktree-prune" => Some(crate::commands::prune::Args::command()),
        _ => None,
    }
}

/// Extract flag strings from a clap Command for shell completions
/// Returns a tuple of (short_and_long_flags, short_flags, long_flags)
fn extract_flags(cmd: &Command) -> (Vec<String>, Vec<String>, Vec<String>) {
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
fn get_flag_descriptions(cmd: &Command) -> Vec<(String, String, Option<String>)> {
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
    // Generate custom completion scripts with dynamic branch completion
    match shell {
        Shell::Bash => generate_bash_completion(command_name)?,
        Shell::Zsh => generate_zsh_completion(command_name)?,
        Shell::Fish => generate_fish_completion(command_name)?,
        _ => {
            anyhow::bail!("Unsupported shell: {:?}", shell);
        }
    }

    Ok(())
}

/// Generate completion script as a String
fn generate_completion_string_for_command(command_name: &str, shell: Shell) -> Result<String> {
    match shell {
        Shell::Bash => generate_bash_completion_string(command_name),
        Shell::Zsh => generate_zsh_completion_string(command_name),
        Shell::Fish => generate_fish_completion_string(command_name),
        _ => anyhow::bail!("Unsupported shell: {:?}", shell),
    }
}

/// Generate bash completion string
fn generate_bash_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout" | "git-worktree-checkout-branch"
    );

    let func_name = command_name.replace('-', "_");

    output.push_str(&format!("_{func_name}() {{\n"));
    output.push_str("    local cur prev words cword\n");
    output.push_str("    _init_completion || return\n");
    output.push('\n');

    if has_branches {
        output.push_str("    # Dynamic branch name completion for positional arguments\n");
        output.push_str(&format!("    if [[ $cword -eq 1 ]] || [[ $cword -eq 2 && \"{}\" == *\"checkout-branch\"* ]]; then\n", command_name));
        output.push_str("        local branches\n");
        output.push_str(&format!(
            "        branches=$(daft __complete \"{}\" \"$cur\" 2>/dev/null)\n",
            command_name
        ));
        output.push_str("        if [[ -n \"$branches\" ]]; then\n");
        output.push_str("            COMPREPLY=( $(compgen -W \"$branches\" -- \"$cur\") )\n");
        output.push_str("            return 0\n");
        output.push_str("        fi\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    output.push_str("    # Static flag completions (extracted from clap)\n");
    output.push_str("    if [[ \"$cur\" == -* ]]; then\n");
    output.push_str("        local flags=\"");

    // Use clap introspection to get flags
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {}", command_name))?;
    let (all_flags, _, _) = extract_flags(&cmd);
    output.push_str(&all_flags.join(" "));

    output.push_str("\"\n");
    output.push_str("        COMPREPLY=( $(compgen -W \"$flags\" -- \"$cur\") )\n");
    output.push_str("        return 0\n");
    output.push_str("    fi\n");
    output.push_str("}\n");
    output.push('\n');

    // Register completion for direct invocation (git-worktree-checkout)
    output.push_str(&format!("complete -F _{func_name} {command_name}\n"));

    // Register completion for git subcommand invocation (git worktree-checkout)
    // Git's bash completion system uses __git_complete for subcommands
    let git_subcommand = command_name.trim_start_matches("git-");
    output.push_str(&format!(
        "# Also register for 'git {}' invocation\n",
        git_subcommand
    ));
    output.push_str("if declare -f __git_complete >/dev/null 2>&1; then\n");
    output.push_str(&format!(
        "    __git_complete git-{} _{}\n",
        git_subcommand, func_name
    ));
    output.push_str("fi\n");

    Ok(output)
}

/// Generate zsh completion string
fn generate_zsh_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout" | "git-worktree-checkout-branch"
    );

    let func_name = command_name.replace('-', "_");

    output.push_str(&format!("#compdef {command_name}\n"));
    output.push('\n');

    // Shared implementation function
    output.push_str(&format!("__{func_name}_impl() {{\n"));
    output.push_str("    local curword\n");
    output.push_str("    curword=\"${words[$CURRENT]}\"\n");
    output.push('\n');

    if has_branches {
        output.push_str("    # Branch completions for non-flag words\n");
        output.push_str("    if [[ $curword != -* ]]; then\n");
        output.push_str("        local -a branches\n");
        output.push_str(&format!(
            "        branches=($(daft __complete \"{}\" \"$curword\" 2>/dev/null))\n",
            command_name
        ));
        output.push_str("        if [[ ${#branches[@]} -gt 0 ]]; then\n");
        output.push_str("            compadd -a branches\n");
        output.push_str("        fi\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    output.push_str("    # Flag completions (extracted from clap)\n");
    output.push_str("    local -a flags\n");
    output.push_str("    flags=(\n");

    // Use clap introspection to get flags
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {}", command_name))?;
    let (all_flags, _, _) = extract_flags(&cmd);

    for flag in all_flags {
        output.push_str(&format!("        '{}'\n", flag));
    }

    output.push_str("    )\n");
    output.push_str("    compadd -a flags\n");
    output.push_str("}\n");
    output.push('\n');

    // Wrapper for direct invocation (git-worktree-checkout)
    output.push_str(&format!("_{func_name}() {{\n"));
    output.push_str(&format!("    __{func_name}_impl\n"));
    output.push_str("}\n");
    output.push('\n');

    // Wrapper for git subcommand invocation (git worktree-checkout)
    // Git's completion system expects _git-<subcommand>
    let git_func_name = format!("_git-{}", command_name.trim_start_matches("git-"));
    output.push_str(&format!("{git_func_name}() {{\n"));
    output.push_str(&format!("    __{func_name}_impl\n"));
    output.push_str("}\n");
    output.push('\n');

    // Register both
    output.push_str(&format!("compdef _{func_name} {command_name}\n"));

    Ok(output)
}

/// Generate fish completion string
fn generate_fish_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout" | "git-worktree-checkout-branch"
    );

    // Extract git subcommand name for dual registration
    let git_subcommand = command_name.trim_start_matches("git-");

    // Branch completions for both direct and git subcommand invocation
    if has_branches {
        output.push_str("# Dynamic branch name completion\n");
        // Direct invocation (git-worktree-checkout)
        output.push_str(&format!(
            "complete -c {} -f -a \"(daft __complete {} '')\"\n",
            command_name, command_name
        ));
        // Git subcommand invocation (git worktree-checkout)
        output.push_str(&format!(
            "complete -c git -n '__fish_seen_subcommand_from {}' -f -a \"(daft __complete {} '')\"\n",
            git_subcommand, command_name
        ));
        output.push('\n');
    }

    output.push_str("# Static flag completions (extracted from clap)\n");

    // Use clap introspection to get flags with descriptions
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {}", command_name))?;
    let flag_descriptions = get_flag_descriptions(&cmd);

    for (short, long, desc) in flag_descriptions {
        // Fish expects separate entries for short and long forms
        let description = desc.unwrap_or_else(|| "".to_string());

        if !short.is_empty() && !long.is_empty() {
            // Both short and long form exist
            let short_char = short.trim_start_matches('-');
            let long_name = long.trim_start_matches("--");
            // Direct invocation
            output.push_str(&format!(
                "complete -c {command_name} -s {short_char} -l {long_name} -d '{description}'\n"
            ));
            // Git subcommand invocation
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {}' -s {short_char} -l {long_name} -d '{description}'\n",
                git_subcommand
            ));
        } else if !long.is_empty() {
            // Long form only
            let long_name = long.trim_start_matches("--");
            output.push_str(&format!(
                "complete -c {command_name} -l {long_name} -d '{description}'\n"
            ));
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {}' -l {long_name} -d '{description}'\n",
                git_subcommand
            ));
        } else if !short.is_empty() {
            // Short form only (rare)
            let short_char = short.trim_start_matches('-');
            output.push_str(&format!(
                "complete -c {command_name} -s {short_char} -d '{description}'\n"
            ));
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {}' -s {short_char} -d '{description}'\n",
                git_subcommand
            ));
        }
    }

    Ok(output)
}

/// Generate bash completion with dynamic branch name support
fn generate_bash_completion(command_name: &str) -> Result<()> {
    print!("{}", generate_bash_completion_string(command_name)?);
    Ok(())
}

/// Generate zsh completion with dynamic branch name support
fn generate_zsh_completion(command_name: &str) -> Result<()> {
    print!("{}", generate_zsh_completion_string(command_name)?);
    Ok(())
}

/// Generate fish completion with dynamic branch name support
fn generate_fish_completion(command_name: &str) -> Result<()> {
    print!("{}", generate_fish_completion_string(command_name)?);
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

        // Generate and write completion file
        std::fs::write(
            &file_path,
            generate_completion_string_for_command(command, shell)?,
        )
        .with_context(|| format!("Failed to write completion file: {:?}", file_path))?;
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
