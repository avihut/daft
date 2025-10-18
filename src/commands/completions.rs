/// Shell completion generation for daft commands
///
/// Generates shell completion scripts for bash, zsh, and fish that provide:
/// - Static completions for flags and options (via clap_complete)
/// - Dynamic completions for branch names (via daft __complete helper)
use anyhow::{Context, Result};
use clap::Parser;
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

    output.push_str("    # Static flag completions\n");
    output.push_str("    if [[ \"$cur\" == -* ]]; then\n");
    output.push_str("        local flags=\"");

    match command_name {
        "git-worktree-clone" => {
            output.push_str("-n --no-checkout -q --quiet -a --all-branches -h --help -V --version")
        }
        "git-worktree-init" => {
            output.push_str("--bare -q --quiet -b --initial-branch -h --help -V --version")
        }
        "git-worktree-checkout" => output.push_str("-v --verbose -h --help -V --version"),
        "git-worktree-checkout-branch" => {
            output.push_str("-q --quiet -v --verbose -h --help -V --version")
        }
        "git-worktree-checkout-branch-from-default" => {
            output.push_str("-v --verbose -h --help -V --version")
        }
        "git-worktree-prune" => output.push_str("-v --verbose -h --help -V --version"),
        _ => {}
    }

    output.push_str("\"\n");
    output.push_str("        COMPREPLY=( $(compgen -W \"$flags\" -- \"$cur\") )\n");
    output.push_str("        return 0\n");
    output.push_str("    fi\n");
    output.push_str("}\n");
    output.push('\n');
    output.push_str(&format!("complete -F _{func_name} {command_name}\n"));

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

    output.push_str("    # Flag completions\n");
    output.push_str("    local -a flags\n");
    output.push_str("    flags=(\n");

    match command_name {
        "git-worktree-clone" => {
            output.push_str("        '-n' '--no-checkout'\n");
            output.push_str("        '-q' '--quiet'\n");
            output.push_str("        '-a' '--all-branches'\n");
            output.push_str("        '-h' '--help'\n");
            output.push_str("        '-V' '--version'\n");
        }
        "git-worktree-init" => {
            output.push_str("        '--bare'\n");
            output.push_str("        '-q' '--quiet'\n");
            output.push_str("        '-b' '--initial-branch'\n");
            output.push_str("        '-h' '--help'\n");
            output.push_str("        '-V' '--version'\n");
        }
        "git-worktree-checkout" => {
            output.push_str("        '-v' '--verbose'\n");
            output.push_str("        '-h' '--help'\n");
            output.push_str("        '-V' '--version'\n");
        }
        "git-worktree-checkout-branch" => {
            output.push_str("        '-q' '--quiet'\n");
            output.push_str("        '-v' '--verbose'\n");
            output.push_str("        '-h' '--help'\n");
            output.push_str("        '-V' '--version'\n");
        }
        "git-worktree-checkout-branch-from-default" => {
            output.push_str("        '-v' '--verbose'\n");
            output.push_str("        '-h' '--help'\n");
            output.push_str("        '-V' '--version'\n");
        }
        "git-worktree-prune" => {
            output.push_str("        '-v' '--verbose'\n");
            output.push_str("        '-h' '--help'\n");
            output.push_str("        '-V' '--version'\n");
        }
        _ => {}
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

    if has_branches {
        output.push_str("# Dynamic branch name completion\n");
        output.push_str(&format!(
            "complete -c {} -f -a \"(daft __complete {} '')\"\n",
            command_name, command_name
        ));
        output.push('\n');
    }

    output.push_str("# Static flag completions\n");

    match command_name {
        "git-worktree-clone" => {
            output.push_str(&format!(
                "complete -c {command_name} -s n -l no-checkout -d 'Only clone the repository'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s q -l quiet -d 'Suppress output'\n"
            ));
            output.push_str(&format!("complete -c {command_name} -s a -l all-branches -d 'Create worktrees for all branches'\n"));
            output.push_str(&format!(
                "complete -c {command_name} -s h -l help -d 'Print help'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s V -l version -d 'Print version'\n"
            ));
        }
        "git-worktree-init" => {
            output.push_str(&format!(
                "complete -c {command_name} -l bare -d 'Only create bare repository'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s q -l quiet -d 'Suppress output'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s b -l initial-branch -d 'Set initial branch' -r\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s h -l help -d 'Print help'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s V -l version -d 'Print version'\n"
            ));
        }
        "git-worktree-checkout" => {
            output.push_str(&format!(
                "complete -c {command_name} -s v -l verbose -d 'Enable verbose output'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s h -l help -d 'Print help'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s V -l version -d 'Print version'\n"
            ));
        }
        "git-worktree-checkout-branch" => {
            output.push_str(&format!(
                "complete -c {command_name} -s q -l quiet -d 'Suppress output'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s v -l verbose -d 'Enable verbose output'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s h -l help -d 'Print help'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s V -l version -d 'Print version'\n"
            ));
        }
        "git-worktree-checkout-branch-from-default" => {
            output.push_str(&format!(
                "complete -c {command_name} -s v -l verbose -d 'Enable verbose output'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s h -l help -d 'Print help'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s V -l version -d 'Print version'\n"
            ));
        }
        "git-worktree-prune" => {
            output.push_str(&format!(
                "complete -c {command_name} -s v -l verbose -d 'Enable verbose output'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s h -l help -d 'Print help'\n"
            ));
            output.push_str(&format!(
                "complete -c {command_name} -s V -l version -d 'Print version'\n"
            ));
        }
        _ => {}
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
