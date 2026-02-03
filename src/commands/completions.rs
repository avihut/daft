/// Shell completion generation for daft commands
///
/// Generates shell completion scripts for bash, zsh, fish, and fig that provide:
/// - Static completions for flags and options (via clap introspection)
/// - Dynamic completions for branch names (via daft __complete helper)
use anyhow::{Context, Result};
use clap::{Command, CommandFactory, Parser, ValueEnum};
use serde::Serialize;
use std::path::PathBuf;

/// Completion targets supported by daft
#[derive(Debug, Clone, ValueEnum)]
enum CompletionTarget {
    Bash,
    Zsh,
    Fish,
    Fig,
}

/// Available daft commands that need completion scripts
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
        "git-worktree-carry" => Some(crate::commands::carry::Args::command()),
        "git-worktree-fetch" => Some(crate::commands::fetch::Args::command()),
        "git-worktree-flow-adopt" => Some(crate::commands::flow_adopt::Args::command()),
        "git-worktree-flow-eject" => Some(crate::commands::flow_eject::Args::command()),
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
        if let Ok(content) = generate_fig_completion_string(command) {
            let _ = std::fs::write(install_dir.join(format!("{command}.js")), content);
        }
    }

    // Write shortcut alias specs
    let mut seen_aliases = std::collections::HashSet::new();
    for shortcut in crate::shortcuts::SHORTCUTS {
        if seen_aliases.insert(shortcut.alias) {
            let content = generate_fig_alias_string(shortcut.alias, shortcut.command);
            let _ = std::fs::write(install_dir.join(format!("{}.js", shortcut.alias)), content);
        }
    }

    // Write daft.js umbrella spec
    if let Ok(content) = generate_fig_daft_spec() {
        let _ = std::fs::write(install_dir.join("daft.js"), content);
    }

    // Write git-daft.js spec
    let _ = std::fs::write(
        install_dir.join("git-daft.js"),
        generate_fig_git_daft_spec(),
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
                print!("{}", generate_fig_completion_string(command)?);
                println!();
            }

            // Print shortcut alias specs
            let mut seen_aliases = std::collections::HashSet::new();
            for shortcut in crate::shortcuts::SHORTCUTS {
                if seen_aliases.insert(shortcut.alias) {
                    println!("// File: {}.js", shortcut.alias);
                    print!(
                        "{}",
                        generate_fig_alias_string(shortcut.alias, shortcut.command)
                    );
                    println!();
                }
            }

            // Print daft.js umbrella spec
            println!("// File: daft.js");
            print!("{}", generate_fig_daft_spec()?);
            println!();

            // Print git-daft.js spec
            println!("// File: git-daft.js");
            print!("{}", generate_fig_git_daft_spec());
        }
        _ => {
            for command in COMMANDS {
                generate_completion_for_command(command, target)?;
            }
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
        CompletionTarget::Bash => generate_bash_completion_string(command_name),
        CompletionTarget::Zsh => generate_zsh_completion_string(command_name),
        CompletionTarget::Fish => generate_fish_completion_string(command_name),
        CompletionTarget::Fig => generate_fig_completion_string(command_name),
    }
}

/// Generate bash completion string
fn generate_bash_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-checkout-branch"
            | "git-worktree-checkout-branch-from-default"
            | "git-worktree-carry"
            | "git-worktree-fetch"
    );

    let func_name = command_name.replace('-', "_");

    output.push_str(&format!("_{func_name}() {{\n"));
    output.push_str("    local cur prev words cword\n");
    output.push_str("    _init_completion || return\n");
    output.push('\n');

    if has_branches {
        output.push_str("    # Dynamic branch name completion for positional arguments\n");
        output.push_str("    if [[ \"$cur\" != -* ]]; then\n");
        output.push_str("        local branches\n");
        output.push_str(&format!(
            "        branches=$(daft __complete \"{}\" \"$cur\" --position \"$cword\" 2>/dev/null)\n",
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

    // Register completions for shortcut aliases
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!("complete -F _{func_name} {}\n", shortcut.alias));
        }
    }

    Ok(output)
}

/// Generate zsh completion string
fn generate_zsh_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-checkout-branch"
            | "git-worktree-checkout-branch-from-default"
            | "git-worktree-carry"
            | "git-worktree-fetch"
    );

    let func_name = command_name.replace('-', "_");

    output.push_str(&format!("#compdef {command_name}\n"));
    output.push('\n');

    // Shared implementation function
    output.push_str(&format!("__{func_name}_impl() {{\n"));
    output.push_str("    local curword cword\n");
    output.push_str("    curword=\"${words[$CURRENT]}\"\n");
    output.push_str("    cword=$((CURRENT - 1))\n");
    output.push('\n');

    if has_branches {
        output.push_str("    # Branch completions for non-flag words\n");
        output.push_str("    if [[ $curword != -* ]]; then\n");
        output.push_str("        local -a branches\n");
        output.push_str(&format!(
            "        branches=($(daft __complete \"{}\" \"$curword\" --position \"$cword\" 2>/dev/null))\n",
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

    // Register completions for shortcut aliases
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!("compdef _{func_name} {}\n", shortcut.alias));
        }
    }

    Ok(output)
}

/// Generate fish completion string
fn generate_fish_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-checkout-branch"
            | "git-worktree-checkout-branch-from-default"
            | "git-worktree-carry"
            | "git-worktree-fetch"
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

    // Register completions for shortcut aliases (wraps the full command)
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!(
                "complete -c {} -w {}\n",
                shortcut.alias, command_name
            ));
        }
    }

    Ok(output)
}

// ── Fig/Amazon Q serialization types ──────────────────────────────

#[derive(Serialize)]
struct FigSpec {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    args: Option<FigArgs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Vec<FigOption>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    subcommands: Option<Vec<FigSubcommand>>,
    #[serde(rename = "loadSpec", skip_serializing_if = "Option::is_none")]
    load_spec: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum FigArgs {
    Single(FigArg),
    Multiple(Vec<FigArg>),
}

#[derive(Serialize)]
struct FigArg {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generators: Option<FigGenerator>,
}

#[derive(Serialize)]
struct FigGenerator {
    script: Vec<String>,
    #[serde(rename = "splitOn")]
    split_on: String,
}

#[derive(Serialize)]
struct FigOption {
    name: FigName,
    description: String,
}

#[derive(Serialize)]
#[serde(untagged)]
enum FigName {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Serialize)]
struct FigSubcommand {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "loadSpec", skip_serializing_if = "Option::is_none")]
    load_spec: Option<String>,
}

// ── Fig shared helpers ────────────────────────────────────────────

/// Wrap a serialized spec into ESM module format
fn wrap_esm(filename: &str, spec: &impl Serialize) -> Result<String> {
    let json =
        serde_json::to_string_pretty(spec).context("Failed to serialize Fig completion spec")?;
    Ok(format!(
        "// {filename}.js\nconst completionSpec = {json};\nexport default completionSpec;\n"
    ))
}

/// Build a FigGenerator for dynamic branch completion
fn build_fig_generator(command_name: &str, position: usize) -> FigGenerator {
    let mut script = vec![
        "daft".into(),
        "__complete".into(),
        command_name.to_string(),
        String::new(),
    ];
    if position > 1 {
        script.push("--position".into());
        script.push(position.to_string());
    }
    FigGenerator {
        script,
        split_on: "\n".to_string(),
    }
}

/// Generate a Fig/Amazon Q completion spec for a command
fn generate_fig_completion_string(command_name: &str) -> Result<String> {
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {command_name}"))?;

    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-checkout-branch"
            | "git-worktree-checkout-branch-from-default"
            | "git-worktree-carry"
            | "git-worktree-fetch"
    );

    let about = cmd.get_about().map(|a| a.to_string());

    // Build positional args
    let positional_args = get_positional_args(&cmd);
    let args = if positional_args.is_empty() {
        None
    } else {
        let fig_args: Vec<FigArg> = positional_args
            .iter()
            .map(|(name, help, index)| FigArg {
                name: name.clone(),
                description: if help.is_empty() {
                    None
                } else {
                    Some(help.clone())
                },
                generators: if has_branches {
                    Some(build_fig_generator(command_name, *index))
                } else {
                    None
                },
            })
            .collect();
        Some(if fig_args.len() == 1 {
            FigArgs::Single(fig_args.into_iter().next().unwrap())
        } else {
            FigArgs::Multiple(fig_args)
        })
    };

    // Build options from flags
    let flag_descriptions = get_flag_descriptions(&cmd);
    let options: Vec<FigOption> = flag_descriptions
        .into_iter()
        .map(|(short, long, desc)| {
            let name = match (short.is_empty(), long.is_empty()) {
                (false, false) => FigName::Multiple(vec![short, long]),
                (true, false) => FigName::Single(long),
                (false, true) => FigName::Single(short),
                _ => FigName::Single(String::new()),
            };
            FigOption {
                name,
                description: desc.unwrap_or_default(),
            }
        })
        .collect();

    let spec = FigSpec {
        name: command_name.to_string(),
        description: about,
        args,
        options: if options.is_empty() {
            None
        } else {
            Some(options)
        },
        subcommands: None,
        load_spec: None,
    };

    wrap_esm(command_name, &spec)
}

/// Generate a Fig alias spec that loads another command's spec
fn generate_fig_alias_string(alias: &str, command_name: &str) -> String {
    let spec = FigSpec {
        name: alias.to_string(),
        description: None,
        args: None,
        options: None,
        subcommands: None,
        load_spec: Some(command_name.to_string()),
    };
    wrap_esm(alias, &spec).unwrap()
}

/// Generate the daft.js umbrella spec with subcommands
fn generate_fig_daft_spec() -> Result<String> {
    // Daft's own subcommands
    let daft_subcommands = [
        ("shell-init", "Generate shell initialization scripts"),
        ("completions", "Generate shell completion scripts"),
        ("hooks", "Manage lifecycle hooks"),
        ("setup", "Setup and configuration"),
        ("branch", "Branch management utilities"),
        ("multi-remote", "Multi-remote management"),
        ("release-notes", "Generate release notes"),
    ];

    let mut subcommands: Vec<FigSubcommand> = daft_subcommands
        .iter()
        .map(|(name, desc)| FigSubcommand {
            name: name.to_string(),
            description: Some(desc.to_string()),
            load_spec: None,
        })
        .collect();

    // Worktree commands accessible via daft worktree-*
    for command in COMMANDS {
        let subcommand_name = command.trim_start_matches("git-");
        subcommands.push(FigSubcommand {
            name: subcommand_name.to_string(),
            description: None,
            load_spec: Some(command.to_string()),
        });
    }

    let spec = FigSpec {
        name: "daft".to_string(),
        description: Some("Git Extensions Toolkit".to_string()),
        args: None,
        options: Some(vec![
            FigOption {
                name: FigName::Multiple(vec!["--version".to_string(), "-V".to_string()]),
                description: "Print version information".to_string(),
            },
            FigOption {
                name: FigName::Multiple(vec!["--help".to_string(), "-h".to_string()]),
                description: "Print help".to_string(),
            },
        ]),
        subcommands: Some(subcommands),
        load_spec: None,
    };

    wrap_esm("daft", &spec)
}

/// Generate a git-daft.js spec that loads the daft spec
fn generate_fig_git_daft_spec() -> String {
    let spec = FigSpec {
        name: "git-daft".to_string(),
        description: None,
        args: None,
        options: None,
        subcommands: None,
        load_spec: Some("daft".to_string()),
    };
    wrap_esm("git-daft", &spec).unwrap()
}

/// Get positional arguments from a clap Command
/// Returns (name, help_text, position_index) for each positional arg
fn get_positional_args(cmd: &Command) -> Vec<(String, String, usize)> {
    let mut args = Vec::new();
    let mut index = 1;

    for arg in cmd.get_arguments() {
        // Skip flags (have short or long)
        if arg.get_short().is_some() || arg.get_long().is_some() {
            continue;
        }

        let name = arg
            .get_value_names()
            .and_then(|v| v.first())
            .map(|n| n.to_string().to_lowercase().replace('_', "-"))
            .unwrap_or_else(|| arg.get_id().to_string().to_lowercase().replace('_', "-"));

        let help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();

        args.push((name, help, index));
        index += 1;
    }

    args
}

/// Install completions to standard locations
fn install_completions(target: &CompletionTarget) -> Result<()> {
    match target {
        CompletionTarget::Fig => install_fig_completions(),
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

/// Install Fig/Amazon Q completion specs as individual .js files
fn install_fig_completions() -> Result<()> {
    let home = dirs::home_dir().context("Could not determine home directory")?;

    // Kiro / Amazon Q loads specs from autocomplete/build/, not autocomplete/.
    // Detect which parent directory exists, default to ~/.amazon-q/ if neither.
    let amazon_q_parent = home.join(".amazon-q");
    let fig_parent = home.join(".fig");

    let install_dir = if amazon_q_parent.exists() {
        amazon_q_parent.join("autocomplete/build")
    } else if fig_parent.exists() {
        fig_parent.join("autocomplete/build")
    } else {
        amazon_q_parent.join("autocomplete/build")
    };

    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create completion directory: {:?}", install_dir))?;

    eprintln!("Installing Fig/Amazon Q specs to: {:?}", install_dir);

    // Write each command spec
    for command in COMMANDS {
        let filename = format!("{command}.js");
        let file_path = install_dir.join(&filename);
        eprintln!("  Installing: {filename}");
        std::fs::write(&file_path, generate_fig_completion_string(command)?)
            .with_context(|| format!("Failed to write spec file: {:?}", file_path))?;
    }

    // Write shortcut alias specs
    let mut seen_aliases = std::collections::HashSet::new();
    for shortcut in crate::shortcuts::SHORTCUTS {
        if seen_aliases.insert(shortcut.alias) {
            let filename = format!("{}.js", shortcut.alias);
            let file_path = install_dir.join(&filename);
            eprintln!("  Installing: {filename}");
            std::fs::write(
                &file_path,
                generate_fig_alias_string(shortcut.alias, shortcut.command),
            )
            .with_context(|| format!("Failed to write spec file: {:?}", file_path))?;
        }
    }

    // Write daft.js umbrella spec
    let daft_path = install_dir.join("daft.js");
    eprintln!("  Installing: daft.js");
    std::fs::write(&daft_path, generate_fig_daft_spec()?)
        .with_context(|| format!("Failed to write spec file: {:?}", daft_path))?;

    // Write git-daft.js spec
    let git_daft_path = install_dir.join("git-daft.js");
    eprintln!("  Installing: git-daft.js");
    std::fs::write(&git_daft_path, generate_fig_git_daft_spec())
        .with_context(|| format!("Failed to write spec file: {:?}", git_daft_path))?;

    eprintln!("\n✓ Fig/Amazon Q specs installed successfully!");
    eprintln!("\nSpecs will be loaded automatically by Amazon Q / Kiro.");

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: assert a spec string uses ESM format (const + export default)
    /// and does NOT use CommonJS (var + module.exports).
    fn assert_esm_format(spec: &str, label: &str) {
        assert!(
            spec.contains("const completionSpec"),
            "{label}: should use 'const completionSpec'"
        );
        assert!(
            spec.contains("export default completionSpec"),
            "{label}: should use 'export default completionSpec'"
        );
        assert!(
            !spec.contains("var completionSpec"),
            "{label}: should NOT use 'var completionSpec'"
        );
        assert!(
            !spec.contains("module.exports"),
            "{label}: should NOT use 'module.exports'"
        );
    }

    #[test]
    fn fig_command_spec_uses_esm_format() {
        let spec = generate_fig_completion_string("git-worktree-checkout").unwrap();
        assert_esm_format(&spec, "generate_fig_completion_string");
    }

    #[test]
    fn fig_alias_spec_uses_esm_format() {
        let spec = generate_fig_alias_string("gwtco", "git-worktree-checkout");
        assert_esm_format(&spec, "generate_fig_alias_string");
    }

    #[test]
    fn fig_daft_spec_uses_esm_format() {
        let spec = generate_fig_daft_spec().unwrap();
        assert_esm_format(&spec, "generate_fig_daft_spec");
    }

    #[test]
    fn fig_git_daft_spec_uses_esm_format() {
        let spec = generate_fig_git_daft_spec();
        assert_esm_format(&spec, "generate_fig_git_daft_spec");
    }

    #[test]
    fn fig_checkout_spec_has_generators() {
        let spec = generate_fig_completion_string("git-worktree-checkout").unwrap();
        assert!(
            spec.contains("generators"),
            "checkout spec should have generators"
        );
        assert!(
            spec.contains("__complete"),
            "checkout spec should reference __complete"
        );
        assert!(
            spec.contains("splitOn"),
            "checkout spec should have splitOn"
        );
    }

    #[test]
    fn fig_prune_spec_has_no_generators() {
        let spec = generate_fig_completion_string("git-worktree-prune").unwrap();
        assert!(
            !spec.contains("generators"),
            "prune spec should not have generators"
        );
    }

    #[test]
    fn fig_checkout_branch_spec_has_position() {
        let spec = generate_fig_completion_string("git-worktree-checkout-branch").unwrap();
        assert!(
            spec.contains("--position"),
            "checkout-branch spec should have --position for second arg"
        );
    }

    #[test]
    fn fig_alias_spec_has_load_spec() {
        let spec = generate_fig_alias_string("gwtco", "git-worktree-checkout");
        assert!(spec.contains("loadSpec"), "alias spec should have loadSpec");
        assert!(
            spec.contains("git-worktree-checkout"),
            "alias spec should reference target command"
        );
    }

    #[test]
    fn fig_daft_spec_has_subcommands() {
        let spec = generate_fig_daft_spec().unwrap();
        assert!(
            spec.contains("subcommands"),
            "daft spec should have subcommands"
        );
        assert!(
            spec.contains("shell-init"),
            "daft spec should include shell-init subcommand"
        );
        assert!(
            spec.contains("worktree-checkout"),
            "daft spec should include worktree-checkout subcommand"
        );
    }

    #[test]
    fn fig_spec_output_is_valid_json_in_esm() {
        let spec = generate_fig_completion_string("git-worktree-checkout").unwrap();
        // Extract JSON between "const completionSpec = " and ";\nexport"
        let json_start =
            spec.find("const completionSpec = ").unwrap() + "const completionSpec = ".len();
        let json_end = spec.find(";\nexport").unwrap();
        let json_str = &spec[json_start..json_end];
        let parsed: serde_json::Value =
            serde_json::from_str(json_str).expect("Fig spec should contain valid JSON");
        assert!(parsed.is_object(), "Parsed spec should be a JSON object");
        assert_eq!(parsed["name"], "git-worktree-checkout");
    }
}
