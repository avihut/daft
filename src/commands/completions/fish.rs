use super::{get_command_for_name, get_flag_descriptions, VERB_ALIAS_GROUPS};
use anyhow::{Context, Result};

/// Generate fish completion string
pub(super) fn generate_fish_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-carry"
            | "git-worktree-fetch"
            | "daft-go"
            | "daft-start"
            | "daft-remove"
            | "daft-rename"
    );

    // Extract git subcommand name for dual registration (git-* commands only)
    let git_subcommand = command_name.trim_start_matches("git-");
    let is_git_command = command_name.starts_with("git-");

    // Branch completions for both direct and git subcommand invocation
    if has_branches {
        output.push_str("# Dynamic branch name completion\n");
        // Direct invocation (git-worktree-checkout or daft-remove)
        output.push_str(&format!(
            "complete -c {} -f -a \"(daft __complete {} '')\"\n",
            command_name, command_name
        ));
        // Git subcommand invocation (git worktree-checkout) — only for git-* commands
        if is_git_command {
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {}' -f -a \"(daft __complete {} '')\"\n",
                git_subcommand, command_name
            ));
        }
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
            // Git subcommand invocation (git-* commands only)
            if is_git_command {
                output.push_str(&format!(
                    "complete -c git -n '__fish_seen_subcommand_from {}' -s {short_char} -l {long_name} -d '{description}'\n",
                    git_subcommand
                ));
            }
        } else if !long.is_empty() {
            // Long form only
            let long_name = long.trim_start_matches("--");
            output.push_str(&format!(
                "complete -c {command_name} -l {long_name} -d '{description}'\n"
            ));
            if is_git_command {
                output.push_str(&format!(
                    "complete -c git -n '__fish_seen_subcommand_from {}' -l {long_name} -d '{description}'\n",
                    git_subcommand
                ));
            }
        } else if !short.is_empty() {
            // Short form only (rare)
            let short_char = short.trim_start_matches('-');
            output.push_str(&format!(
                "complete -c {command_name} -s {short_char} -d '{description}'\n"
            ));
            if is_git_command {
                output.push_str(&format!(
                    "complete -c git -n '__fish_seen_subcommand_from {}' -s {short_char} -d '{description}'\n",
                    git_subcommand
                ));
            }
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

/// Generate complete daft fish completions including verb alias flag completions.
pub(super) fn generate_daft_fish_completions() -> String {
    let mut output = DAFT_FISH_COMPLETIONS.to_string();
    output.push('\n');
    output.push_str(&generate_verb_alias_flag_completions());
    output
}

/// Generate fish flag completions for verb aliases (go, start, carry, update)
/// by introspecting the underlying command's clap definition.
fn generate_verb_alias_flag_completions() -> String {
    let mut output = String::new();
    output.push_str("# Flag completions for verb aliases\n");

    for (verbs, command) in VERB_ALIAS_GROUPS {
        let cmd = match get_command_for_name(command) {
            Some(cmd) => cmd,
            None => continue,
        };
        let condition = format!("__fish_seen_subcommand_from {}", verbs.join(" "));
        let flag_descriptions = get_flag_descriptions(&cmd);

        for (short, long, desc) in flag_descriptions {
            let description = desc.unwrap_or_default();

            if !short.is_empty() && !long.is_empty() {
                let short_char = short.trim_start_matches('-');
                let long_name = long.trim_start_matches("--");
                output.push_str(&format!(
                    "complete -c daft -n '{condition}' -s {short_char} -l {long_name} -d '{description}'\n"
                ));
            } else if !long.is_empty() {
                let long_name = long.trim_start_matches("--");
                output.push_str(&format!(
                    "complete -c daft -n '{condition}' -l {long_name} -d '{description}'\n"
                ));
            } else if !short.is_empty() {
                let short_char = short.trim_start_matches('-');
                output.push_str(&format!(
                    "complete -c daft -n '{condition}' -s {short_char} -d '{description}'\n"
                ));
            }
        }
    }

    output
}

const DAFT_FISH_COMPLETIONS: &str = r#"# daft subcommand completions
complete -c daft -f
complete -c daft -n '__fish_use_subcommand' -a 'hooks' -d 'Manage lifecycle hooks'
complete -c daft -n '__fish_use_subcommand' -a 'shell-init' -d 'Generate shell wrappers'
complete -c daft -n '__fish_use_subcommand' -a 'completions' -d 'Generate completions'
complete -c daft -n '__fish_use_subcommand' -a 'setup' -d 'Setup and configuration'
complete -c daft -n '__fish_use_subcommand' -a 'multi-remote' -d 'Multi-remote management'
complete -c daft -n '__fish_use_subcommand' -a 'release-notes' -d 'Generate release notes'
complete -c daft -n '__fish_use_subcommand' -a 'doctor' -d 'Check installation'
complete -c daft -n '__fish_use_subcommand' -a 'clone' -d 'Clone repo into worktree layout'
complete -c daft -n '__fish_use_subcommand' -a 'init' -d 'Init new repo in worktree layout'
complete -c daft -n '__fish_use_subcommand' -a 'go' -d 'Open existing branch worktree'
complete -c daft -n '__fish_use_subcommand' -a 'start' -d 'Create new branch worktree'
complete -c daft -n '__fish_use_subcommand' -a 'carry' -d 'Transfer uncommitted changes'
complete -c daft -n '__fish_use_subcommand' -a 'update' -d 'Update worktree branches'
complete -c daft -n '__fish_use_subcommand' -a 'prune' -d 'Remove stale worktrees'
complete -c daft -n '__fish_use_subcommand' -a 'rename' -d 'Rename branch and move worktree'
complete -c daft -n '__fish_use_subcommand' -a 'remove' -d 'Delete branch and worktree'
complete -c daft -n '__fish_use_subcommand' -a 'adopt' -d 'Convert repo to worktree layout'
complete -c daft -n '__fish_use_subcommand' -a 'sync' -d 'Synchronize worktrees with remote'
complete -c daft -n '__fish_use_subcommand' -a 'eject' -d 'Convert back to traditional layout'
complete -c daft -n '__fish_seen_subcommand_from go' -f -a "(daft __complete daft-go '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from start' -f -a "(daft __complete daft-start '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from carry update' -f -a "(daft __complete git-worktree-checkout '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from remove' -f -a "(daft __complete daft-remove '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from rename' -f -a "(daft __complete daft-rename '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from multi-remote; and not __fish_seen_subcommand_from enable disable status set-default move' -f -a 'enable disable status set-default move'
complete -c daft -n '__fish_seen_subcommand_from hooks; and not __fish_seen_subcommand_from trust prompt deny status migrate install validate dump run' -f -a 'trust prompt deny status migrate install validate dump run'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -f -a "(daft __complete hooks-run '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l job -d 'Run only the named job' -r -f -a "(set -l hook (commandline -opc | string match -rv '^-' | tail -n1); DAFT_COMPLETE_HOOK=\$hook daft __complete hooks-run-job '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l tag -d 'Run only jobs with this tag'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l dry-run -d 'Preview what would run'
complete -c git-daft -w daft
"#;
