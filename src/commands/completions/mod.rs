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
    (&["go"], "daft-go"),
    (&["start"], "daft-start"),
    (&["carry"], "git-worktree-carry"),
    (&["update"], "git-worktree-fetch"),
    (&["remove"], "daft-remove"),
    (&["rename"], "daft-rename"),
    (&["sync"], "git-worktree-sync"),
    (&["list"], "git-worktree-list"),
    (&["prune"], "git-worktree-prune"),
    (&["clone"], "git-worktree-clone"),
    (&["init"], "git-worktree-init"),
    (&["shared"], "daft-shared"),
    (&["exec"], "git-worktree-exec"),
];

/// Available daft commands that need completion scripts
pub(super) const COMMANDS: &[&str] = &[
    "git-worktree-clone",
    "git-worktree-init",
    "git-worktree-checkout",
    "git-worktree-prune",
    "git-worktree-carry",
    "git-worktree-fetch",
    "git-worktree-exec",
    "git-worktree-flow-adopt",
    "git-worktree-flow-eject",
    "git-worktree-list",
    "daft-go",
    "daft-start",
    "daft-remove",
    "daft-rename",
    "git-worktree-sync",
    "daft-shared",
    "daft-install",
    "daft-file",
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
        "git-worktree-exec" => Some(crate::commands::exec::Args::command()),
        "git-worktree-flow-adopt" => Some(crate::commands::flow_adopt::Args::command()),
        "git-worktree-flow-eject" => Some(crate::commands::flow_eject::Args::command()),
        "git-worktree-list" => Some(crate::commands::list::Args::command()),
        "daft-go" => Some(crate::commands::checkout::GoArgs::command()),
        "daft-start" => Some(crate::commands::checkout::StartArgs::command()),
        "daft-remove" => Some(crate::commands::worktree_branch::RemoveArgs::command()),
        "daft-rename" => Some(crate::commands::worktree_branch::RenameArgs::command()),
        "git-worktree-sync" => Some(crate::commands::sync::Args::command()),
        "daft-shared" => Some(crate::commands::shared::Args::command()),
        "daft-install" => Some(crate::commands::install::Args::command()),
        "daft-file" => Some(crate::commands::file::merge::Args::command()),
        _ => None,
    }
}

/// Whether a command uses the rich (grouped, tab-separated) completion
/// protocol with worktree/local/remote groups and metadata.
pub(super) fn uses_rich_completions(command_name: &str) -> bool {
    matches!(
        command_name,
        "daft-go"
            | "git-worktree-checkout"
            | "daft-remove"
            | "daft-rename"
            | "git-worktree-carry"
            | "git-worktree-fetch"
            | "git-worktree-branch"
            | "git-worktree-exec"
    )
}

/// Whether a command should pass `--fetch-on-miss` to `daft __complete`.
pub(super) fn uses_fetch_on_miss(command_name: &str) -> bool {
    matches!(command_name, "daft-go")
}

/// Whether a command carries a `--repo <REPO>` flag whose value completes
/// to catalog repo names (via `daft __complete repo-name`).
pub(super) fn command_has_repo_flag(command_name: &str) -> bool {
    matches!(
        command_name,
        "daft-go"
            | "git-worktree-list"
            | "git-worktree-fetch"
            | "git-worktree-exec"
            | "git-worktree-prune"
    )
}

/// Whether a command's first positional is an optional cataloged-repo name
/// (`daft list [<repo>]`, positional sugar for `--repo`), completed via
/// `daft __complete repo-name`. Per the repo-aware command grammar
/// (CLAUDE.md), only read-only commands with a free positional slot qualify.
pub(super) fn command_has_repo_positional(command_name: &str) -> bool {
    command_name == "git-worktree-list"
}

/// Whether a command accepts worktree paths as positional arguments and should
/// fall back to filesystem directory completion when the dynamic source has
/// nothing to offer (or as an additional candidate set). This keeps the
/// "remove this worktree by path" UX usable both inside a repo (where the user
/// might type `./` or `../`) and outside any repo (where the dynamic source
/// can't return branches at all).
pub(super) fn allows_path_completion(command_name: &str) -> bool {
    matches!(command_name, "daft-remove" | "daft-rename")
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

/// Supported `--format` values for a given command invocation path.
///
/// The path is the canonical invocation — `git-worktree-list`, `release-notes`,
/// `hooks trust list`, `layout list`, `shared status`, `multi-remote status`,
/// `hooks run`. Returns `None` for commands that do not support `--format`.
///
/// This is the single source of truth for shell completion value lists. The
/// underlying support matrix lives in `emit::dispatch::supported_formats`.
pub(super) fn emit_formats_for(command_path: &str) -> Option<Vec<&'static str>> {
    use crate::output::emit::dispatch::supported_formats;
    use crate::output::emit::payload::Shape;

    let shape = match command_path {
        "git-worktree-list" | "list" => Shape::Tabular,
        "release-notes" => Shape::Document,
        "hooks trust list" => Shape::Tabular,
        "hooks jobs" => Shape::Tabular,
        "layout list" => Shape::Tabular,
        "shared status" => Shape::Matrix,
        "multi-remote status" => Shape::Sectioned,
        "hooks run" => Shape::Sectioned,
        _ => return None,
    };
    Some(
        supported_formats(shape)
            .iter()
            .map(|f| f.as_str())
            .collect(),
    )
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
    let mut args_vec: Vec<String> = crate::cli::argv().to_vec();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_daft_go_uses_nosort_and_fetch_on_miss() {
        let script =
            bash::generate_bash_completion_string("daft-go").expect("generator must succeed");
        assert!(
            script.contains("--fetch-on-miss"),
            "daft-go bash completion must pass --fetch-on-miss to daft __complete"
        );
        assert!(
            script.contains("compopt -o nosort"),
            "daft-go bash completion must attempt compopt -o nosort to \
             preserve group ordering"
        );
        assert!(
            script.contains("cut -f1"),
            "daft-go bash completion must strip tab-separated group/desc \
             columns with cut -f1"
        );
    }

    #[test]
    fn zsh_daft_go_uses_compadd_groups_no_headers() {
        let script =
            zsh::generate_zsh_completion_string("daft-go").expect("generator must succeed");
        // Groups use -V for ordering (worktree first)
        assert!(
            script.contains("-V worktree"),
            "daft-go zsh must use -V worktree group"
        );
        assert!(
            script.contains("-V local"),
            "daft-go zsh must use -V local group"
        );
        assert!(
            script.contains("-V remote"),
            "daft-go zsh must use -V remote group"
        );
        // Worktree display has three columns: name, age, path
        assert!(
            script.contains("wt_ages") && script.contains("wt_paths"),
            "daft-go zsh must split worktree desc into age and path"
        );
        // No group headers
        assert!(
            !script.contains("-X "),
            "daft-go must NOT use -X group headers"
        );
        assert!(
            !script.contains("\n    _describe"),
            "daft-go must NOT call _describe (triggers tag-retry 3x repetition)"
        );
    }

    /// `daft list [<repo>]` — the positional is sugar for --repo, so every
    /// shell must offer catalog repo names at the first positional (fig has
    /// its own test beside its generator).
    #[test]
    fn list_repo_positional_completes_catalog_names_in_all_shells() {
        let zsh = zsh::generate_zsh_completion_string("git-worktree-list").expect("zsh gen");
        assert!(
            zsh.contains("daft __complete repo-name")
                && zsh.contains("\"$curword\" != -* && \"$prev_word\" != \"--template\""),
            "zsh list completion must complete the positional repo outside flag values"
        );
        let bash = bash::generate_bash_completion_string("git-worktree-list").expect("bash gen");
        assert!(
            bash.contains("\"$cur\" != -* && \"$prev\" != \"--template\""),
            "bash list completion must complete the positional repo outside flag values"
        );
        let fish = fish::generate_fish_completion_string("git-worktree-list").expect("fish gen");
        assert!(
            fish.contains(
                "complete -c git-worktree-list -n 'test (count (commandline -opc)) -eq 1'"
            ),
            "fish list completion must complete the positional repo at the first token"
        );
        let umbrella = fish::generate_daft_fish_completions();
        assert!(
            umbrella.contains(
                "__fish_seen_subcommand_from list; and test (count (commandline -opc)) -eq 2"
            ),
            "fish umbrella must complete daft list's positional repo"
        );
    }

    /// `daft repo remove --repo <name>` / `--keep-files`: the repo-verb
    /// sections of the umbrella completions are hardcoded per shell, so a
    /// new flag must land in all three (fig has its own spec test).
    #[test]
    fn repo_remove_completes_repo_flag_and_keep_files_in_all_shells() {
        let zsh = zsh::DAFT_ZSH_COMPLETIONS;
        assert!(
            zsh.contains("compadd -- --repo --keep-files -y --force"),
            "zsh repo-remove flag list must include --repo and --keep-files"
        );
        let bash = bash::DAFT_BASH_COMPLETIONS;
        assert!(
            bash.contains("--repo --keep-files -y --force --dry-run"),
            "bash repo-remove flag list must include --repo and --keep-files"
        );
        // Both umbrellas complete the --repo VALUE with catalog names inside
        // the repo section's remove arm (the same `daft __complete repo-name`
        // helper the other repo-aware commands use). Bound the probe to that
        // arm: the umbrella also has a top-level worktree `remove` verb.
        for (shell, script) in [("zsh", zsh), ("bash", bash)] {
            let repo_section = script
                .split("# repo: complete subcommands")
                .nth(1)
                .unwrap_or_else(|| panic!("{shell} umbrella must have a repo section"));
            let remove_arm = repo_section
                .split("remove)")
                .nth(1)
                .unwrap_or_else(|| panic!("{shell} repo section must have a remove arm"));
            let arm = &remove_arm[..remove_arm.find(";;").unwrap_or(remove_arm.len())];
            assert!(
                arm.contains("daft __complete repo-name"),
                "{shell} repo-remove arm must complete --repo values with catalog names"
            );
        }
        let fish = fish::generate_daft_fish_completions();
        assert!(
            fish.contains(
                "__fish_seen_subcommand_from remove' -l repo -x -a \"(daft __complete repo-name"
            ) && fish.contains("__fish_seen_subcommand_from remove' -l keep-files"),
            "fish repo-remove must complete --repo values and offer --keep-files"
        );
    }

    /// Regression: the `daft go` position-1 helper appends a catalog `repo`
    /// group (`name\trepo\tpath`), but the zsh renderer's per-group parser
    /// only knew worktree/local/remote — catalog repos silently vanished
    /// from the menu, and outside a repo (where they are the ONLY entries)
    /// `daft go <TAB>` completed nothing. Fish's shared awk had no 3-field
    /// branch and rendered a dangling `path · ` description.
    #[test]
    fn go_first_arg_renders_the_catalog_repo_group_in_zsh_and_fish() {
        let zsh = zsh::generate_zsh_completion_string("daft-go").expect("zsh gen");
        assert!(
            zsh.contains("repo)") && zsh.contains("repo_paths"),
            "zsh parser must collect repo-group lines"
        );
        assert!(
            zsh.contains("compadd -V repo "),
            "zsh must render the catalog repo group"
        );

        // 3-field repo lines display as name\tpath — no age/author columns.
        let repo_branch = "else printf \\\"%s\\t%s\\n\\\",c,$3";
        let fish = fish::generate_fish_completion_string("daft-go").expect("fish gen");
        assert!(
            fish.contains(repo_branch),
            "fish per-command awk must handle 3-field repo lines"
        );
        assert!(
            fish::generate_daft_fish_completions().contains(repo_branch),
            "fish umbrella awk for `daft go` must handle 3-field repo lines"
        );
    }

    #[test]
    fn zsh_daft_go_passes_fetch_on_miss_flag() {
        let script =
            zsh::generate_zsh_completion_string("daft-go").expect("generator must succeed");
        assert!(
            script.contains("--fetch-on-miss"),
            "daft-go zsh completion must pass --fetch-on-miss to daft __complete"
        );
    }

    #[test]
    fn fish_daft_go_passes_fetch_on_miss_and_awk_reshuffles() {
        let script =
            fish::generate_fish_completion_string("daft-go").expect("generator must succeed");
        assert!(
            script.contains("--fetch-on-miss"),
            "daft-go fish completion must pass --fetch-on-miss"
        );
        assert!(
            script.contains("awk"),
            "daft-go fish completion must reshuffle tab-separated columns via awk"
        );
    }

    #[test]
    fn skip_hooks_value_completion_wired_for_all_flag_carrying_commands() {
        // Every command that exposes --skip-hooks must complete its selector
        // values via `daft __complete skip-hooks-value`, in all three shells.
        // Regression for the rich/non-rich generator split: checkout and go
        // take the rich path, so a value block added only to the non-rich
        // generator silently misses them.
        for cmd in [
            "git-worktree-checkout",   // rich
            "daft-go",                 // rich
            "git-worktree-clone",      // non-rich
            "git-worktree-flow-adopt", // non-rich
            "daft-start",              // non-rich
        ] {
            let bash = bash::generate_bash_completion_string(cmd).expect("bash gen");
            assert!(
                bash.contains("skip-hooks-value"),
                "bash completion for {cmd} must complete --skip-hooks values"
            );
            let zsh = zsh::generate_zsh_completion_string(cmd).expect("zsh gen");
            assert!(
                zsh.contains("skip-hooks-value"),
                "zsh completion for {cmd} must complete --skip-hooks values"
            );
            let fish = fish::generate_fish_completion_string(cmd).expect("fish gen");
            assert!(
                fish.contains("skip-hooks-value"),
                "fish completion for {cmd} must complete --skip-hooks values"
            );
        }
    }

    #[test]
    fn fish_daft_umbrella_passes_fetch_on_miss_for_go() {
        let fish_completions = fish::generate_daft_fish_completions();
        assert!(
            fish_completions.contains("daft __complete daft-go")
                && fish_completions.contains("--fetch-on-miss"),
            "daft go subcommand in umbrella fish completions must pass --fetch-on-miss"
        );
    }

    // zstyle coloring was removed — compadd -V groups don't interact
    // with zsh's tag system, so per-tag zstyle list-colors wouldn't apply.
    // Colors may be revisited via a different mechanism in a follow-up.

    #[test]
    fn zsh_umbrella_delegates_go_to_daft_go_impl() {
        let combined = format!(
            "{}\n{}",
            zsh::generate_zsh_completion_string("daft-go").unwrap(),
            zsh::DAFT_ZSH_COMPLETIONS,
        );
        assert!(
            combined.contains("__daft_go_impl"),
            "zsh umbrella must call __daft_go_impl for the `go` verb alias"
        );
    }

    #[test]
    fn rich_commands_use_compadd_v_groups_in_zsh() {
        // Every rich-completion command should use compadd -V for group ordering.
        let rich_commands = [
            "git-worktree-checkout",
            "git-worktree-carry",
            "git-worktree-fetch",
            "daft-go",
            "daft-remove",
            "daft-rename",
            "git-worktree-exec",
        ];
        for cmd in rich_commands {
            let script = zsh::generate_zsh_completion_string(cmd)
                .unwrap_or_else(|e| panic!("generator must succeed for {cmd}: {e}"));
            assert!(
                script.contains("-V worktree"),
                "{cmd} zsh must use -V worktree group"
            );
        }
    }

    #[test]
    fn only_daft_go_passes_fetch_on_miss_in_all_shells() {
        let non_go_rich = [
            "git-worktree-checkout",
            "git-worktree-carry",
            "git-worktree-fetch",
            "daft-remove",
            "daft-rename",
        ];
        for cmd in non_go_rich {
            let zsh = zsh::generate_zsh_completion_string(cmd)
                .unwrap_or_else(|e| panic!("zsh generator must succeed for {cmd}: {e}"));
            assert!(
                !zsh.contains("--fetch-on-miss"),
                "{cmd} zsh must NOT pass --fetch-on-miss"
            );
            let bash = bash::generate_bash_completion_string(cmd)
                .unwrap_or_else(|e| panic!("bash generator must succeed for {cmd}: {e}"));
            assert!(
                !bash.contains("--fetch-on-miss"),
                "{cmd} bash must NOT pass --fetch-on-miss"
            );
            let fish = fish::generate_fish_completion_string(cmd)
                .unwrap_or_else(|e| panic!("fish generator must succeed for {cmd}: {e}"));
            assert!(
                !fish.contains("--fetch-on-miss"),
                "{cmd} fish must NOT pass --fetch-on-miss"
            );
        }
    }

    #[test]
    fn fish_rich_commands_use_awk_reshuffle() {
        let rich_commands = [
            "git-worktree-checkout",
            "git-worktree-carry",
            "git-worktree-fetch",
            "daft-go",
            "daft-remove",
            "daft-rename",
            "git-worktree-exec",
        ];
        for cmd in rich_commands {
            let script = fish::generate_fish_completion_string(cmd)
                .unwrap_or_else(|e| panic!("fish generator must succeed for {cmd}: {e}"));
            assert!(
                script.contains("awk"),
                "{cmd} fish must reshuffle tab-separated columns via awk"
            );
        }
    }

    #[test]
    fn bash_rich_commands_use_cut_and_nosort() {
        let rich_commands = [
            "git-worktree-checkout",
            "git-worktree-carry",
            "git-worktree-fetch",
            "daft-go",
            "daft-remove",
            "daft-rename",
            "git-worktree-exec",
        ];
        for cmd in rich_commands {
            let script = bash::generate_bash_completion_string(cmd)
                .unwrap_or_else(|e| panic!("bash generator must succeed for {cmd}: {e}"));
            assert!(
                script.contains("cut -f1"),
                "{cmd} bash must strip group/desc columns with cut -f1"
            );
            assert!(
                script.contains("compopt -o nosort"),
                "{cmd} bash must use compopt -o nosort to preserve group ordering"
            );
        }
    }

    #[test]
    fn umbrella_shells_dispatch_exec_verb() {
        let bash = bash::DAFT_BASH_COMPLETIONS;
        assert!(
            bash.contains("exec)"),
            "bash umbrella must dispatch `exec` verb"
        );
        assert!(
            bash.contains("_git_worktree_exec"),
            "bash umbrella must call per-command completer"
        );

        let combined_zsh = format!(
            "{}\n{}",
            zsh::generate_zsh_completion_string("git-worktree-exec").unwrap(),
            zsh::DAFT_ZSH_COMPLETIONS,
        );
        assert!(
            combined_zsh.contains("exec)") || combined_zsh.contains("__git_worktree_exec_impl"),
            "zsh umbrella must dispatch `exec`"
        );

        let fish = fish::generate_daft_fish_completions();
        assert!(
            fish.contains("git-worktree-exec") || fish.contains(" exec "),
            "fish umbrella must reference exec verb"
        );
    }

    #[test]
    fn zsh_gates_flag_completions_on_leading_dash() {
        let commands = [
            "git-worktree-checkout",
            "git-worktree-carry",
            "git-worktree-fetch",
            "daft-go",
            "daft-start",
            "daft-remove",
            "daft-rename",
        ];
        for cmd in commands {
            let script = zsh::generate_zsh_completion_string(cmd)
                .unwrap_or_else(|e| panic!("generator must succeed for {cmd}: {e}"));
            let flags_pos = script.find("compadd -a flags").unwrap_or_else(|| {
                panic!("generated script for {cmd} must contain `compadd -a flags`")
            });
            let guard_pos = script.find("[[ \"$curword\" == -* ]]").unwrap_or_else(|| {
                panic!(
                    "generated script for {cmd} must gate flag completion on a leading dash \
before adding flags (zsh flag-leak regression)"
                )
            });
            assert!(
                guard_pos < flags_pos,
                "flag-gating guard must appear before `compadd -a flags` for {cmd}, \
                 otherwise flags leak into branch completions. \
                 guard_pos={guard_pos} flags_pos={flags_pos}",
            );
        }
    }

    #[test]
    fn emit_formats_for_covers_every_emit_enabled_path() {
        for path in [
            "git-worktree-list",
            "list",
            "release-notes",
            "hooks trust list",
            "hooks jobs",
            "layout list",
            "shared status",
            "multi-remote status",
            "hooks run",
        ] {
            assert!(
                emit_formats_for(path).is_some(),
                "emit_formats_for must return Some for known emit path: {path}"
            );
        }
        assert!(
            emit_formats_for("git-worktree-clone").is_none(),
            "non-emit command must return None"
        );
    }

    #[test]
    fn bash_list_completion_offers_all_tabular_formats() {
        let script = bash::generate_bash_completion_string("git-worktree-list")
            .expect("generator must succeed");
        assert!(
            script.contains("prev\" == \"--format\""),
            "bash list completion must branch on --format as prev word"
        );
        assert!(
            script.contains("\"json ndjson tsv csv yaml toon markdown\""),
            "bash list completion must offer all 7 tabular formats"
        );
    }

    #[test]
    fn zsh_list_completion_offers_all_tabular_formats() {
        let script = zsh::generate_zsh_completion_string("git-worktree-list")
            .expect("generator must succeed");
        assert!(
            script.contains("prev_word\" == \"--format\""),
            "zsh list completion must branch on --format as prev word"
        );
        assert!(
            script.contains("format_values=( json ndjson tsv csv yaml toon markdown )"),
            "zsh list completion must offer all 7 tabular formats"
        );
    }

    #[test]
    fn fish_daft_umbrella_offers_format_per_subcommand_path() {
        let script = fish::generate_daft_fish_completions();
        for (path_condition, formats) in [
            (
                "__fish_seen_subcommand_from list",
                "json ndjson tsv csv yaml toon markdown",
            ),
            (
                "__fish_seen_subcommand_from release-notes",
                "json yaml toon markdown",
            ),
            (
                "__fish_seen_subcommand_from multi-remote; and __fish_seen_subcommand_from status",
                "json yaml toon markdown",
            ),
            (
                "__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from status",
                "json ndjson tsv csv yaml toon markdown",
            ),
        ] {
            let needle = format!("-n '{path_condition}' -l format -x -a '{formats}'");
            assert!(
                script.contains(&needle),
                "fish umbrella must offer --format value completion for path: {path_condition}\n\
                 expected: {needle}"
            );
        }
    }

    #[test]
    fn bash_daft_umbrella_dispatches_format_by_subcommand_path() {
        let script = bash::DAFT_BASH_COMPLETIONS;
        assert!(
            script.contains(
                "list|worktree-list|\"hooks trust list\"|\"hooks jobs\"|\"layout list\"|\"shared status\""
            ),
            "bash umbrella must dispatch tabular/matrix paths to all-7-format list"
        );
        assert!(
            script.contains("release-notes|\"multi-remote status\"|\"hooks run\""),
            "bash umbrella must dispatch document/sectioned paths to 4-format list"
        );
    }

    // ── Path completions for daft-remove and daft-rename ─────────────────
    //
    // Both commands document that positional args may be branch names OR
    // worktree paths. Without path completion in the stubs, Tab on `./` or
    // an absolute prefix produces nothing — and outside a repo the dynamic
    // source returns empty so Tab is dead. These tests pin the stubs that
    // restore directory completion for both inside-repo and outside-repo use.

    #[test]
    fn bash_path_accepting_commands_offer_directory_completion() {
        for cmd in ["daft-remove", "daft-rename"] {
            let script = bash::generate_bash_completion_string(cmd)
                .unwrap_or_else(|e| panic!("bash generator must succeed for {cmd}: {e}"));
            assert!(
                script.contains("compgen -d"),
                "{cmd} bash must offer directory completion via `compgen -d`"
            );
            assert!(
                script.contains("/*|./*|../*|~/*|~"),
                "{cmd} bash must short-circuit to dir completion when prefix is path-like"
            );
            // Both the path-prefix short-circuit and the post-branch fallback
            // must avoid unquoted command substitution into COMPREPLY, which
            // would word-split directory names on $IFS (e.g. `my worktree/`
            // would produce two entries `my` and `worktree/`).
            assert!(
                script.contains("mapfile -t COMPREPLY < <(compgen -d"),
                "{cmd} bash path-prefix branch must use `mapfile -t` to preserve \
                 directory names with whitespace"
            );
            assert!(
                !script.contains("COMPREPLY=( $(compgen -d"),
                "{cmd} bash must NOT assign `COMPREPLY=( $(compgen -d ...) )` \
                 directly — that word-splits dirs containing whitespace"
            );
        }
    }

    #[test]
    fn zsh_path_accepting_commands_offer_directory_completion() {
        for cmd in ["daft-remove", "daft-rename"] {
            let script = zsh::generate_zsh_completion_string(cmd)
                .unwrap_or_else(|e| panic!("zsh generator must succeed for {cmd}: {e}"));
            assert!(
                script.contains("_files -/"),
                "{cmd} zsh must offer directory completion via `_files -/`"
            );
            assert!(
                script.contains("/*|./*|../*|~/*|~"),
                "{cmd} zsh must short-circuit to dir completion when prefix is path-like"
            );
        }
    }

    #[test]
    fn fish_path_accepting_commands_offer_directory_completion() {
        for cmd in ["daft-remove", "daft-rename"] {
            let script = fish::generate_fish_completion_string(cmd)
                .unwrap_or_else(|e| panic!("fish generator must succeed for {cmd}: {e}"));
            assert!(
                script.contains("__fish_complete_directories"),
                "{cmd} fish must offer directory completion via __fish_complete_directories"
            );
        }
    }

    #[test]
    fn fish_daft_umbrella_offers_directory_completion_for_remove_and_rename() {
        let script = fish::generate_daft_fish_completions();
        for verb in ["remove", "rename"] {
            let needle =
                format!("__fish_seen_subcommand_from {verb}' -a \"(__fish_complete_directories");
            assert!(
                script.contains(&needle),
                "fish umbrella must offer dir completion for `daft {verb}`\n\
                 expected to contain: {needle}"
            );
        }
    }

    #[test]
    fn non_path_commands_do_not_offer_directory_completion() {
        // Sanity: only the path-accepting commands enable filesystem
        // completion; other rich commands stay branch-only.
        for cmd in [
            "git-worktree-checkout",
            "git-worktree-carry",
            "git-worktree-fetch",
            "daft-go",
            "git-worktree-exec",
        ] {
            let zsh = zsh::generate_zsh_completion_string(cmd)
                .unwrap_or_else(|e| panic!("zsh generator must succeed for {cmd}: {e}"));
            assert!(
                !zsh.contains("_files -/"),
                "{cmd} zsh must NOT offer directory completion (branches only)"
            );
            let bash = bash::generate_bash_completion_string(cmd)
                .unwrap_or_else(|e| panic!("bash generator must succeed for {cmd}: {e}"));
            assert!(
                !bash.contains("compgen -d"),
                "{cmd} bash must NOT offer directory completion (branches only)"
            );
        }
    }
}
