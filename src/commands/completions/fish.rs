use super::{
    VERB_ALIAS_GROUPS, allows_path_completion, emit_formats_for, get_command_for_name,
    get_flag_descriptions, uses_fetch_on_miss, uses_rich_completions,
};
use anyhow::{Context, Result};

/// Generate fish completion string
pub(super) fn generate_fish_completion_string(command_name: &str) -> Result<String> {
    // Rich completion commands get awk-reshuffled descriptions.
    if uses_rich_completions(command_name) {
        return generate_fish_rich_completion(command_name);
    }

    let mut output = String::new();
    // daft-start still uses simple branch-prefix patterns (not rich).
    let has_branches = command_name == "daft-start";

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

    // Value completions for -b / --branch flag (clone only)
    if command_name == "git-worktree-clone" {
        output.push_str("\n# Static value completions for -b / --branch\n");
        output.push_str(&format!(
            "complete -c {} -s b -l branch -x -a 'HEAD @' -d 'Branch to check out (HEAD or @ for default)'\n",
            command_name
        ));
    }

    // Value completions for --layout flag
    let has_layout = matches!(command_name, "git-worktree-clone" | "git-worktree-init");
    if has_layout {
        output.push_str(&format!(
            "\n# Layout name completions for --layout\ncomplete -c {} -l layout -x -a \"(daft __complete layout-value '' 2>/dev/null)\"\n",
            command_name
        ));
    }

    // Value completions for --skip-hooks flag (selector vocabulary from daft.yml)
    let has_skip_hooks = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-clone"
            | "git-worktree-flow-adopt"
            | "daft-go"
            | "daft-start"
    );
    if has_skip_hooks {
        output.push_str(&format!(
            "\n# Skip-hooks selector completions for --skip-hooks\ncomplete -c {} -l skip-hooks -x -a \"(daft __complete skip-hooks-value '' 2>/dev/null)\"\n",
            command_name
        ));
    }

    // Value completions for --columns flag
    let has_columns = matches!(
        command_name,
        "git-worktree-list" | "git-worktree-sync" | "git-worktree-prune"
    );
    if has_columns {
        output.push_str("\n# Column name completions for --columns\n");
        let columns = [
            ("annotation", "Annotation markers"),
            ("branch", "Branch name"),
            ("path", "Worktree path"),
            ("size", "Disk size of worktree"),
            ("base", "Ahead/behind base branch"),
            ("changes", "Local changes"),
            ("remote", "Ahead/behind remote"),
            ("age", "Branch age"),
            ("owner", "Branch owner"),
            ("hash", "Commit hash"),
            ("last-commit", "Last commit"),
        ];
        for (name, desc) in &columns {
            output.push_str(&format!(
                "complete -c {} -l columns -x -a '{} +{} -{}' -d '{}'\n",
                command_name, name, name, name, desc
            ));
        }

        output.push_str("\n# Sort column completions for --sort\n");
        let sort_columns = [
            ("branch", "Sort by branch name"),
            ("path", "Sort by worktree path"),
            ("size", "Sort by disk size"),
            ("base", "Sort by total base divergence"),
            ("changes", "Sort by total local changes"),
            ("remote", "Sort by total remote divergence"),
            ("age", "Sort by branch age"),
            ("owner", "Sort by branch owner"),
            ("hash", "Sort by commit hash"),
            (
                "activity",
                "Sort by overall activity (commits + uncommitted)",
            ),
            ("commit", "Sort by last commit time only"),
        ];
        for (name, desc) in &sort_columns {
            output.push_str(&format!(
                "complete -c {} -l sort -x -a '{} +{} -{}' -d '{}'\n",
                command_name, name, name, name, desc
            ));
        }
    }

    // Format value completions for --format (emit-enabled commands only)
    if let Some(formats) = emit_formats_for(command_name) {
        let format_list = formats.join(" ");
        output.push_str(&format!(
            "\n# Format value completions for --format\ncomplete -c {command_name} -l format -x -a '{format_list}'\n"
        ));
        if is_git_command {
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {git_subcommand}' -l format -x -a '{format_list}'\n"
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

/// Generate a fish completion script with rich grouped output for any command
/// that uses the `name\tgroup\tdescription` protocol.
///
/// Reshuffles the tab-separated output into fish's `name\tdescription` format
/// where the description reads `<age> · <group>`.
fn generate_fish_rich_completion(command_name: &str) -> Result<String> {
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {command_name}"))?;
    let flag_descriptions = get_flag_descriptions(&cmd);

    let fetch_flag = if uses_fetch_on_miss(command_name) {
        " --fetch-on-miss"
    } else {
        ""
    };

    // Extract git subcommand name for dual registration (git-* commands only)
    let git_subcommand = command_name.trim_start_matches("git-");
    let is_git_command = command_name.starts_with("git-");

    let mut output = String::new();
    // Dynamic branch completion with grouped output
    output.push_str("# Dynamic branch name completion\n");
    // Worktree lines: name[*?]\tworktree\tage\tauthor\tpath (5 fields)
    // Local/remote: name\tgroup\tage\tauthor (4 fields)
    // Fish display: clean_name\t[*?] age · author · path (worktree) or name\tage · author
    // Strip *? from name for completion value — these are invalid in git branch names.
    let awk_body = "{{c=$1; sub(/[*?]+$/,\\\"\\\",c); s=substr($1,length(c)+1); if (NF>=5) printf \\\"%s\\t%s %s · %s · %s\\n\\\",c,s,$3,$4,$5; else printf \\\"%s\\t%s %s · %s\\n\\\",c,s,$3,$4}}";
    output.push_str(&format!(
        "complete -c {command_name} -f -a \"(daft __complete {command_name} (commandline -ct) --position 1{fetch_flag} 2>/dev/null | awk -F'\\t' '{awk_body}')\"\n",
    ));
    // Git subcommand invocation (git worktree-checkout) — only for git-* commands
    if is_git_command {
        output.push_str(&format!(
            "complete -c git -n '__fish_seen_subcommand_from {git_subcommand}' -f -a \"(daft __complete {command_name} (commandline -ct) --position 1{fetch_flag} 2>/dev/null | awk -F'\\t' '{awk_body}')\"\n",
        ));
    }
    // Path-accepting commands (daft-remove, daft-rename) also offer directory
    // completion so worktrees can be removed by path inside or outside a repo.
    if allows_path_completion(command_name) {
        output.push_str(&format!(
            "complete -c {command_name} -a \"(__fish_complete_directories (commandline -ct))\"\n",
        ));
        if is_git_command {
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {git_subcommand}' -a \"(__fish_complete_directories (commandline -ct))\"\n",
            ));
        }
    }
    // Rich commands that also carry --skip-hooks (checkout, go) complete its
    // selector vocabulary from daft.yml.
    if matches!(command_name, "git-worktree-checkout" | "daft-go") {
        output.push_str(&format!(
            "complete -c {command_name} -l skip-hooks -x -a \"(daft __complete skip-hooks-value '' 2>/dev/null)\"\n",
        ));
        if is_git_command {
            output.push_str(&format!(
                "complete -c git -n '__fish_seen_subcommand_from {git_subcommand}' -l skip-hooks -x -a \"(daft __complete skip-hooks-value '' 2>/dev/null)\"\n",
            ));
        }
    }

    output.push('\n');
    output.push_str("# Static flag completions (extracted from clap)\n");

    // Flag completions (from clap introspection)
    for (short, long, desc) in flag_descriptions {
        let description = desc.unwrap_or_default();
        if !short.is_empty() && !long.is_empty() {
            let short_char = short.trim_start_matches('-');
            let long_name = long.trim_start_matches("--");
            output.push_str(&format!(
                "complete -c {command_name} -s {short_char} -l {long_name} -d '{description}'\n"
            ));
        } else if !long.is_empty() {
            let long_name = long.trim_start_matches("--");
            output.push_str(&format!(
                "complete -c {command_name} -l {long_name} -d '{description}'\n"
            ));
        } else if !short.is_empty() {
            let short_char = short.trim_start_matches('-');
            output.push_str(&format!(
                "complete -c {command_name} -s {short_char} -d '{description}'\n"
            ));
        }
    }

    // Register completions for shortcut aliases (wraps the full command)
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!(
                "complete -c {} -w {command_name}\n",
                shortcut.alias
            ));
        }
    }

    Ok(output)
}

const DAFT_FISH_COMPLETIONS: &str = r#"# daft subcommand completions
complete -c daft -f
# `-C <path>` is a top-level option (issue #519). Fish completes by short
# letter regardless of position, so a single -c registration with -xa for
# directory value completion is enough — no need to thread it through every
# __fish_seen_subcommand_from condition below.
complete -c daft -s C -d 'Run as if started in <path>' -xa '(__fish_complete_directories)'
complete -c daft -n '__fish_use_subcommand' -s V -l version -d 'Print version information'
complete -c daft -n '__fish_use_subcommand' -s h -l help -d 'Print help'
complete -c daft -n '__fish_use_subcommand' -a 'hooks' -d 'Manage lifecycle hooks'
complete -c daft -n '__fish_use_subcommand' -a 'shell-init' -d 'Generate shell wrappers'
complete -c daft -n '__fish_use_subcommand' -a 'activate' -d 'Activate daft in this shell'
complete -c daft -n '__fish_use_subcommand' -a 'multi-remote' -d 'Multi-remote management'
complete -c daft -n '__fish_use_subcommand' -a 'release-notes' -d 'Generate release notes'
complete -c daft -n '__fish_use_subcommand' -a 'doctor' -d 'Check installation'
complete -c daft -n '__fish_use_subcommand' -a 'layout' -d 'Manage worktree layouts'
complete -c daft -n '__fish_use_subcommand' -a 'clone' -d 'Clone repo into worktree layout'
complete -c daft -n '__fish_use_subcommand' -a 'init' -d 'Init new repo in worktree layout'
complete -c daft -n '__fish_use_subcommand' -a 'install' -d 'Install a starter daft.yml in the current worktree'
complete -c daft -n '__fish_use_subcommand' -a 'go' -d 'Open existing branch worktree'
complete -c daft -n '__fish_use_subcommand' -a 'start' -d 'Create new branch worktree'
complete -c daft -n '__fish_use_subcommand' -a 'carry' -d 'Transfer uncommitted changes'
complete -c daft -n '__fish_use_subcommand' -a 'exec' -d 'Run a command across one or more worktrees'
complete -c daft -n '__fish_use_subcommand' -a 'update' -d 'Update worktree branches'
complete -c daft -n '__fish_use_subcommand' -a 'prune' -d 'Remove stale worktrees'
complete -c daft -n '__fish_use_subcommand' -a 'rename' -d 'Rename branch and move worktree'
complete -c daft -n '__fish_use_subcommand' -a 'remove' -d 'Delete branch and worktree'
complete -c daft -n '__fish_use_subcommand' -a 'adopt' -d 'Convert repo to worktree layout'
complete -c daft -n '__fish_use_subcommand' -a 'sync' -d 'Synchronize worktrees with remote'
complete -c daft -n '__fish_use_subcommand' -a 'list' -d 'List worktrees with status'
complete -c daft -n '__fish_use_subcommand' -a 'merge' -d 'Merge branches across worktrees'
complete -c daft -n '__fish_use_subcommand' -a 'worktree-merge' -d 'Merge branches across worktrees'
complete -c daft -n '__fish_use_subcommand' -a 'eject' -d 'Convert back to traditional layout'
complete -c daft -n '__fish_use_subcommand' -a 'config' -d 'Configure daft settings'
complete -c daft -n '__fish_use_subcommand' -a 'shared' -d 'Manage shared files across worktrees'
complete -c daft -n '__fish_use_subcommand' -a 'repo' -d 'Repository-level operations'
complete -c daft -n '__fish_use_subcommand' -a 'file' -d 'Manage YAML config files'
complete -c daft -n '__fish_seen_subcommand_from go' -f -a "(daft __complete daft-go (commandline -ct) --position 1 --fetch-on-miss 2>/dev/null | awk -F'\t' '{c=$1; sub(/[*?]+$/,\"\",c); s=substr($1,length(c)+1); if (NF>=5) printf \"%s\t%s %s · %s · %s\n\",c,s,$3,$4,$5; else printf \"%s\t%s %s · %s\n\",c,s,$3,$4}')"
complete -c daft -n '__fish_seen_subcommand_from start' -f -a "(daft __complete daft-start '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from carry' -f -a "(daft __complete git-worktree-carry (commandline -ct) --position 1 2>/dev/null | awk -F'\t' '{c=$1; sub(/[*?]+$/,\"\",c); s=substr($1,length(c)+1); if (NF>=5) printf \"%s\t%s %s · %s · %s\n\",c,s,$3,$4,$5; else printf \"%s\t%s %s · %s\n\",c,s,$3,$4}')"
complete -c daft -n '__fish_seen_subcommand_from exec' -f -a "(daft __complete git-worktree-exec (commandline -ct) --position 1 2>/dev/null | awk -F'\t' '{c=$1; sub(/[*?]+$/,\"\",c); s=substr($1,length(c)+1); if (NF>=5) printf \"%s\t%s %s · %s · %s\n\",c,s,$3,$4,$5; else printf \"%s\t%s %s · %s\n\",c,s,$3,$4}')"
complete -c daft -n '__fish_seen_subcommand_from update' -f -a "(daft __complete git-worktree-fetch (commandline -ct) --position 1 2>/dev/null | awk -F'\t' '{c=$1; sub(/[*?]+$/,\"\",c); s=substr($1,length(c)+1); if (NF>=5) printf \"%s\t%s %s · %s · %s\n\",c,s,$3,$4,$5; else printf \"%s\t%s %s · %s\n\",c,s,$3,$4}')"
complete -c daft -n '__fish_seen_subcommand_from remove' -f -a "(daft __complete daft-remove (commandline -ct) --position 1 2>/dev/null | awk -F'\t' '{c=$1; sub(/[*?]+$/,\"\",c); s=substr($1,length(c)+1); if (NF>=5) printf \"%s\t%s %s · %s · %s\n\",c,s,$3,$4,$5; else printf \"%s\t%s %s · %s\n\",c,s,$3,$4}')"
complete -c daft -n '__fish_seen_subcommand_from remove' -a "(__fish_complete_directories (commandline -ct))"
complete -c daft -n '__fish_seen_subcommand_from rename' -f -a "(daft __complete daft-rename (commandline -ct) --position 1 2>/dev/null | awk -F'\t' '{c=$1; sub(/[*?]+$/,\"\",c); s=substr($1,length(c)+1); if (NF>=5) printf \"%s\t%s %s · %s · %s\n\",c,s,$3,$4,$5; else printf \"%s\t%s %s · %s\n\",c,s,$3,$4}')"
complete -c daft -n '__fish_seen_subcommand_from rename' -a "(__fish_complete_directories (commandline -ct))"
complete -c daft -n '__fish_seen_subcommand_from layout; and not __fish_seen_subcommand_from default list show transform' -f -a 'default list show transform'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from show' -F
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -f -a "(daft __complete layout-transform '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l force -s f -d 'Force transform even with uncommitted changes'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l dry-run -d 'Show plan without executing'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l include -r -d 'Also relocate non-conforming worktree'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l include-all -d 'Relocate all non-conforming worktrees'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from default' -f -a "(daft __complete layout-default '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from default' -l reset -d 'Reset to built-in default'
complete -c daft -n '__fish_seen_subcommand_from multi-remote; and not __fish_seen_subcommand_from enable disable status set-default move' -f -a 'enable disable status set-default move'
# repo: subcommands
complete -c daft -n '__fish_seen_subcommand_from repo; and not __fish_seen_subcommand_from add info install list remove' -f -a 'add' -d 'Register a repository in the repo catalog'
complete -c daft -n '__fish_seen_subcommand_from repo; and not __fish_seen_subcommand_from add info install list remove' -f -a 'info' -d "Show a repository's catalog entry"
complete -c daft -n '__fish_seen_subcommand_from repo; and not __fish_seen_subcommand_from add info install list remove' -f -a 'install' -d 'Install a starter daft.yml in the current worktree'
complete -c daft -n '__fish_seen_subcommand_from repo; and not __fish_seen_subcommand_from add info install list remove' -f -a 'list' -d 'List repositories in the repo catalog'
complete -c daft -n '__fish_seen_subcommand_from repo; and not __fish_seen_subcommand_from add info install list remove' -f -a 'remove' -d 'Remove a repository, including all worktrees'
# repo add: path completion + flags
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from add' -a "(__fish_complete_directories (commandline -ct))"
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from add' -l name -r -d 'Catalog name for the repo'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from add' -s q -l quiet -d 'Suppress progress reporting'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from add' -s v -l verbose -d 'Show detailed progress'
# repo info: repo-name completion + flags
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from info' -f -a "(daft __complete repo-name (commandline -ct) 2>/dev/null | cut -f1)"
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from info' -l format -r -d 'Output format'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from info' -l template -r -d 'Tera template string'
# repo list: flags
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from list' -s a -l all -d 'Include removed repositories'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from list' -l format -r -d 'Output format'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from list' -l template -r -d 'Tera template string'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from list' -l no-headers -d 'Omit header row (tsv/csv only)'
# repo install: flags
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from install' -s q -l quiet -d 'Suppress progress reporting'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from install' -s v -l verbose -d 'Show detailed progress'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from install' -l git-exclude -d 'Add /daft.yml to .git/info/exclude without prompting'
# repo remove: path completion + flags
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from remove' -F
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from remove' -s y -l force -d 'Skip the confirmation prompt'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from remove' -l dry-run -d 'Print what would be removed without touching anything'
complete -c daft -n '__fish_seen_subcommand_from repo; and __fish_seen_subcommand_from remove' -s v -l verbose -d 'Increase verbosity'
complete -c daft -n '__fish_seen_subcommand_from config; and not __fish_seen_subcommand_from remote-sync' -f -a 'remote-sync'
# file: subcommands
complete -c daft -n '__fish_seen_subcommand_from file; and not __fish_seen_subcommand_from merge' -f -a 'merge' -d 'Merge a source daft.yml into a target daft.yml'
# file merge: file completion + flags
complete -c daft -n '__fish_seen_subcommand_from file; and __fish_seen_subcommand_from merge' -F
complete -c daft -n '__fish_seen_subcommand_from file; and __fish_seen_subcommand_from merge' -l keep-source -d 'Keep the source file after merging'
complete -c daft -n '__fish_seen_subcommand_from file; and __fish_seen_subcommand_from merge' -s y -l yes -d 'Skip confirmation prompt when target is untracked'
complete -c daft -n '__fish_seen_subcommand_from hooks; and not __fish_seen_subcommand_from trust prompt deny status migrate install validate dump run jobs' -f -a 'trust prompt deny status migrate install validate dump run jobs'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -f -a "(daft __complete hooks-run '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l job -d 'Run only the named job' -r -f -a "(set -l hook (commandline -opc | string match -rv '^-' | tail -n1); DAFT_COMPLETE_HOOK=\$hook daft __complete hooks-run-job '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l tag -d 'Run only jobs with this tag'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l dry-run -d 'Preview what would run'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -s v -l verbose -d 'Show verbose output'
# hooks: also allow path completion alongside subcommands
complete -c daft -n '__fish_seen_subcommand_from hooks; and not __fish_seen_subcommand_from trust prompt deny status migrate install validate dump run jobs' -F
# hooks status: path + flags
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from status' -F
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from status' -s s -l short -d 'Show compact one-line summary'
# hooks trust: sub-subcommands + path + flags
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from trust; and not __fish_seen_subcommand_from list reset prune' -f -a 'list reset prune'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from trust' -F
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from trust' -s f -l force -d 'Do not ask for confirmation'
# hooks prompt/deny: path + flags
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from prompt deny' -F
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from prompt deny' -s f -l force -d 'Do not ask for confirmation'
# hooks migrate: flags
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from migrate' -l dry-run -d 'Preview renames without making changes'
# hooks jobs: sub-subcommands and flags
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -f -a 'logs cancel retry prune'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs' -l all -d 'Show jobs from all worktrees'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -l format -x -a 'json ndjson tsv csv yaml toon markdown' -d 'Output format'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -l template -r -d 'Tera template string'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -l no-headers -d 'Omit header row (tsv/csv)'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel' -l inv -d 'Invocation ID prefix'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l hook -d 'Force hook name interpretation'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l inv -d 'Force invocation prefix interpretation'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l job -d 'Force job name interpretation'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l worktree -r -d 'Retry from specific worktree' -f -a "(daft __complete hooks-jobs-retry-worktree (commandline -ct) 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -l cwd -d 'Override working directory'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -l worktree -r -d 'Filter by worktree' -f -a "(daft __complete hooks-jobs-worktree (commandline -ct) 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -l status -r -d 'Filter by job status' -f -a "failed completed running cancelled skipped"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and not __fish_seen_subcommand_from logs cancel retry prune' -l hook -r -d 'Filter by hook type' -f -a "(daft __complete hooks-jobs-hook-filter (commandline -ct) 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from logs cancel' -f -a "(daft __complete hooks-jobs-job (commandline -ct) 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from jobs; and __fish_seen_subcommand_from retry' -f -a "(daft __complete hooks-jobs-retry (commandline -ct) 2>/dev/null)"
# shared: subcommands
complete -c daft -n '__fish_seen_subcommand_from shared; and not __fish_seen_subcommand_from add link manage materialize remove status sync' -f -a 'add link manage materialize remove status sync'
# shared add: file completion + --declare
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from add' -F
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from add' -l declare -d 'Declare without collecting'
# shared remove: shared files + --delete
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from remove' -f -a "(daft __complete shared-files '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from remove' -l delete -d 'Delete everywhere instead of materializing'
# shared link/materialize: shared files, then worktree names + --override
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from link materialize' -f -a "(daft __complete shared-files '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from link materialize' -f -a "(daft __complete shared-worktrees '' 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from link materialize' -l override -d 'Replace even if local differs'
# --format value completions (emit-enabled subcommands)
complete -c daft -n '__fish_seen_subcommand_from list' -l format -x -a 'json ndjson tsv csv yaml toon markdown'
complete -c daft -n '__fish_seen_subcommand_from release-notes' -l format -x -a 'json yaml toon markdown'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from trust; and __fish_seen_subcommand_from list' -l format -x -a 'json ndjson tsv csv yaml toon markdown'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from list' -l format -x -a 'json ndjson tsv csv yaml toon markdown'
complete -c daft -n '__fish_seen_subcommand_from shared; and __fish_seen_subcommand_from status' -l format -x -a 'json ndjson tsv csv yaml toon markdown'
complete -c daft -n '__fish_seen_subcommand_from multi-remote; and __fish_seen_subcommand_from status' -l format -x -a 'json yaml toon markdown'
complete -c daft -n '__fish_seen_subcommand_from hooks; and __fish_seen_subcommand_from run' -l format -x -a 'json yaml toon markdown'
# merge: flags + branch completion for source/target
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -f -a "(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null)"
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l into -x -a "(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null)" -d 'Target worktree/branch for the merge'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l abort -d 'Abort an in-progress merge'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l continue -d 'Continue an in-progress merge'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l quit -d 'Quit an in-progress merge without resetting the index'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l adopt-target -d 'Adopt an ephemeral worktree for ref-only non-FF merges'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l no-adopt-target -d 'Refuse non-FF merges against target without a worktree'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s y -l yes -d 'Auto-accept interactive prompts'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s r -l remove-branch -d 'Remove the source worktree and delete the source branch'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l keep-branch -d 'Explicit keep — for canceling a config-set cleanup default'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l set-default -d 'Persist the invocation\'s style + cleanup as repo defaults'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s m -r -d 'Commit message for the merge commit'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s F -l file -r -d 'Read commit message from FILE'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l edit -d 'Launch editor for merge commit message'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l no-edit -d 'Accept auto-generated merge commit message'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l cleanup -x -a 'default scissors strip verbatim whitespace' -d 'Commit message cleanup mode'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l merge -d 'Explicit merge style — always create a merge commit (default)'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l squash -d 'Squash style — collapse source commits into one squash commit'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l rebase -d 'Rebase style — replay source onto target, fast-forward'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l rebase-merge -d 'Rebase-merge style — rebase source onto target, then merge commit'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l commit -d 'Automatically create the merge commit'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l no-commit -d 'Leave the merge staged without committing'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l signoff -d 'Add Signed-off-by trailer'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l no-signoff -d 'Explicitly disable signoff'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s s -l strategy -x -a 'ours recursive resolve octopus subtree' -d 'Merge strategy to use'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s X -l strategy-option -r -d 'Strategy-specific option'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s S -l gpg-sign -d 'GPG-sign the merge commit'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l no-gpg-sign -d 'Do not GPG-sign the merge commit'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l verify-signatures -d 'Verify source signature'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l no-verify-signatures -d 'Do not verify source signature'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l allow-unrelated-histories -d 'Allow merging histories with no common ancestor'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -l stat -d 'Show a diffstat at end of merge'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s n -l no-stat -d 'Suppress the diffstat'
complete -c daft -n '__fish_seen_subcommand_from merge worktree-merge' -s v -l verbose -d 'Show detailed progress'
# list --merging flag
complete -c daft -n '__fish_seen_subcommand_from list' -l merging -d 'Only show worktrees with an in-progress merge'
complete -c git-daft -w daft
"#;
