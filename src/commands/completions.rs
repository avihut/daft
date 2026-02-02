/// Shell completion generation for daft commands
///
/// Generates shell completion scripts for bash, zsh, fish, and fig that provide:
/// - Static completions for flags and options (via clap introspection)
/// - Dynamic completions for branch names (via daft __complete helper)
use anyhow::{Context, Result};
use clap::{Command, CommandFactory, Parser, ValueEnum};
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

    let amazon_q_dir = home.join(".amazon-q/autocomplete");
    let fig_dir = home.join(".fig/autocomplete");

    let install_dir = if amazon_q_dir.is_dir() {
        amazon_q_dir
    } else if fig_dir.is_dir() {
        fig_dir
    } else {
        return; // No autocomplete directory found — nothing to do
    };

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

    let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();

    let mut output = String::new();
    output.push_str(&format!("// {command_name}.js\n"));
    output.push_str("var completionSpec = {\n");
    output.push_str(&format!("  name: \"{command_name}\",\n"));
    output.push_str(&format!(
        "  description: \"{}\",\n",
        escape_js_string(&about)
    ));

    // Generate args
    let positional_args = get_positional_args(&cmd);
    if !positional_args.is_empty() {
        if positional_args.len() == 1 {
            let (arg_name, arg_help, arg_index) = &positional_args[0];
            output.push_str("  args: {\n");
            output.push_str(&format!("    name: \"{arg_name}\",\n"));
            if !arg_help.is_empty() {
                output.push_str(&format!(
                    "    description: \"{}\",\n",
                    escape_js_string(arg_help)
                ));
            }
            if has_branches {
                output.push_str(&fig_generator_block(command_name, *arg_index));
            }
            output.push_str("  },\n");
        } else {
            output.push_str("  args: [\n");
            for (arg_name, arg_help, arg_index) in &positional_args {
                output.push_str("    {\n");
                output.push_str(&format!("      name: \"{arg_name}\",\n"));
                if !arg_help.is_empty() {
                    output.push_str(&format!(
                        "      description: \"{}\",\n",
                        escape_js_string(arg_help)
                    ));
                }
                if has_branches {
                    output.push_str(&fig_generator_block_indented(command_name, *arg_index, 6));
                }
                output.push_str("    },\n");
            }
            output.push_str("  ],\n");
        }
    }

    // Generate options from flags
    let flag_descriptions = get_flag_descriptions(&cmd);
    if !flag_descriptions.is_empty() {
        output.push_str("  options: [\n");
        for (short, long, desc) in &flag_descriptions {
            let description = desc.as_deref().unwrap_or("");
            let mut names = Vec::new();
            if !short.is_empty() {
                names.push(format!("\"{}\"", short));
            }
            if !long.is_empty() {
                names.push(format!("\"{}\"", long));
            }
            if names.len() == 1 {
                output.push_str(&format!(
                    "    {{ name: {}, description: \"{}\" }},\n",
                    names[0],
                    escape_js_string(description)
                ));
            } else {
                output.push_str(&format!(
                    "    {{ name: [{}], description: \"{}\" }},\n",
                    names.join(", "),
                    escape_js_string(description)
                ));
            }
        }
        output.push_str("  ],\n");
    }

    output.push_str("};\n");
    output.push_str("module.exports = completionSpec;\n");

    Ok(output)
}

/// Generate a Fig alias spec that loads another command's spec
fn generate_fig_alias_string(alias: &str, command_name: &str) -> String {
    let mut output = String::new();
    output.push_str(&format!("// {alias}.js\n"));
    output.push_str("var completionSpec = {\n");
    output.push_str(&format!("  name: \"{alias}\",\n"));
    output.push_str(&format!("  loadSpec: \"{command_name}\",\n"));
    output.push_str("};\n");
    output.push_str("module.exports = completionSpec;\n");
    output
}

/// Generate the daft.js umbrella spec with subcommands
fn generate_fig_daft_spec() -> Result<String> {
    let mut output = String::new();
    output.push_str("// daft.js\n");
    output.push_str("var completionSpec = {\n");
    output.push_str("  name: \"daft\",\n");
    output.push_str("  description: \"Git Extensions Toolkit\",\n");
    output.push_str("  subcommands: [\n");

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

    for (name, desc) in &daft_subcommands {
        output.push_str(&format!(
            "    {{ name: \"{name}\", description: \"{desc}\" }},\n"
        ));
    }

    // Worktree commands accessible via daft worktree-*
    for command in COMMANDS {
        let subcommand_name = command.trim_start_matches("git-");
        let cmd = get_command_for_name(command);
        let about = cmd
            .and_then(|c| c.get_about().map(|a| a.to_string()))
            .unwrap_or_default();
        output.push_str(&format!(
            "    {{ name: \"{subcommand_name}\", loadSpec: \"{command}\" }},\n"
        ));
        // Suppress unused variable warning
        let _ = about;
    }

    output.push_str("  ],\n");
    output.push_str("  options: [\n");
    output.push_str(
        "    { name: [\"--version\", \"-V\"], description: \"Print version information\" },\n",
    );
    output.push_str("    { name: [\"--help\", \"-h\"], description: \"Print help\" },\n");
    output.push_str("  ],\n");
    output.push_str("};\n");
    output.push_str("module.exports = completionSpec;\n");

    Ok(output)
}

/// Generate a git-daft.js spec that loads the daft spec
fn generate_fig_git_daft_spec() -> String {
    let mut output = String::new();
    output.push_str("// git-daft.js\n");
    output.push_str("var completionSpec = {\n");
    output.push_str("  name: \"git-daft\",\n");
    output.push_str("  loadSpec: \"daft\",\n");
    output.push_str("};\n");
    output.push_str("module.exports = completionSpec;\n");
    output
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

/// Generate a Fig generators block for dynamic completion
fn fig_generator_block(command_name: &str, position: usize) -> String {
    let mut s = String::new();
    s.push_str("    generators: {\n");
    s.push_str(&format!(
        "      script: [\"daft\", \"__complete\", \"{command_name}\", \"\""
    ));
    if position > 1 {
        s.push_str(&format!(", \"--position\", \"{position}\""));
    }
    s.push_str("],\n");
    s.push_str("      splitOn: \"\\n\",\n");
    s.push_str("    },\n");
    s
}

/// Generate a Fig generators block with custom indentation
fn fig_generator_block_indented(command_name: &str, position: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut s = String::new();
    s.push_str(&format!("{pad}generators: {{\n"));
    s.push_str(&format!(
        "{pad}  script: [\"daft\", \"__complete\", \"{command_name}\", \"\""
    ));
    if position > 1 {
        s.push_str(&format!(", \"--position\", \"{position}\""));
    }
    s.push_str("],\n");
    s.push_str(&format!("{pad}  splitOn: \"\\n\",\n"));
    s.push_str(&format!("{pad}}},\n"));
    s
}

/// Escape a string for use in JavaScript string literals
fn escape_js_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
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

    // Prefer ~/.amazon-q/autocomplete/ if it exists, else ~/.fig/autocomplete/, else create ~/.amazon-q/autocomplete/
    let amazon_q_dir = home.join(".amazon-q/autocomplete");
    let fig_dir = home.join(".fig/autocomplete");

    let install_dir = if amazon_q_dir.exists() {
        amazon_q_dir
    } else if fig_dir.exists() {
        fig_dir
    } else {
        amazon_q_dir
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
