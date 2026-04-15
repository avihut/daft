use super::{extract_flags, get_command_for_name, uses_fetch_on_miss, uses_rich_completions};
use anyhow::{Context, Result};

/// Generate bash completion string
pub(super) fn generate_bash_completion_string(command_name: &str) -> Result<String> {
    // Rich completion commands get cut -f1 + nosort to preserve group ordering.
    if uses_rich_completions(command_name) {
        return Ok(generate_bash_rich_completion(command_name));
    }

    let mut output = String::new();
    // daft-start still uses simple branch-prefix patterns (not rich).
    let has_branches = command_name == "daft-start";

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

    // Value completion for -b / --branch flag (clone only)
    if command_name == "git-worktree-clone" {
        output.push_str("    # Static value completion for -b / --branch\n");
        output.push_str("    if [[ \"$prev\" == \"-b\" || \"$prev\" == \"--branch\" ]]; then\n");
        output.push_str("        COMPREPLY=( $(compgen -W \"HEAD @\" -- \"$cur\") )\n");
        output.push_str("        return 0\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    // Value completion for --layout flag
    let has_layout = matches!(command_name, "git-worktree-clone" | "git-worktree-init");
    if has_layout {
        output.push_str("    # Layout name completion for --layout\n");
        output.push_str("    if [[ \"$prev\" == \"--layout\" ]]; then\n");
        output.push_str("        local layouts\n");
        output.push_str(
            "        layouts=$(daft __complete layout-value \"$cur\" 2>/dev/null | cut -f1)\n",
        );
        output.push_str("        COMPREPLY=( $(compgen -W \"$layouts\" -- \"$cur\") )\n");
        output.push_str("        return 0\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    // Value completion for --columns flag
    let has_columns = matches!(
        command_name,
        "git-worktree-list" | "git-worktree-sync" | "git-worktree-prune"
    );
    if has_columns {
        output.push_str("    # Column name completion for --columns\n");
        output.push_str("    if [[ \"$prev\" == \"--columns\" ]]; then\n");
        output.push_str("        local columns=\"annotation branch path size base changes remote age owner hash last-commit\"\n");
        output.push_str("        local prefixed=\"\"\n");
        output.push_str("        for c in $columns; do prefixed=\"$prefixed $c +$c -$c\"; done\n");
        output.push_str("        COMPREPLY=( $(compgen -W \"$prefixed\" -- \"$cur\") )\n");
        output.push_str("        return 0\n");
        output.push_str("    fi\n");
        output.push('\n');
        output.push_str("    # Sort column completion for --sort\n");
        output.push_str("    if [[ \"$prev\" == \"--sort\" ]]; then\n");
        output.push_str("        local cols=\"branch path size base changes remote age owner hash activity commit\"\n");
        output.push_str("        local prefixed=\"\"\n");
        output.push_str("        for c in $cols; do prefixed=\"$prefixed $c +$c -$c\"; done\n");
        output.push_str("        COMPREPLY=( $(compgen -W \"$prefixed\" -- \"$cur\") )\n");
        output.push_str("        return 0\n");
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
    // Skip for daft-* commands — they don't need git subcommand style completion
    if command_name.starts_with("git-") {
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
    }

    // Register completions for shortcut aliases
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!("complete -F _{func_name} {}\n", shortcut.alias));
        }
    }

    Ok(output)
}

/// Generate a bash completion script with rich grouped output for any command
/// that uses the `name\tgroup\tdescription` protocol.
fn generate_bash_rich_completion(command_name: &str) -> String {
    let cmd = get_command_for_name(command_name)
        .unwrap_or_else(|| panic!("Unknown rich-completion command: {command_name}"));
    let (all_flags, _, _) = extract_flags(&cmd);
    let flags_joined = all_flags.join(" ");

    let func_name = command_name.replace('-', "_");
    let fetch_flag = if uses_fetch_on_miss(command_name) {
        " --fetch-on-miss"
    } else {
        ""
    };

    let mut output = format!(
        r#"_{func_name}() {{
    local cur prev words cword
    _init_completion || return

    if [[ "$cur" == -* ]]; then
        local flags="{flags_joined}"
        COMPREPLY=( $(compgen -W "$flags" -- "$cur") )
        return 0
    fi

    local raw
    raw=$(daft __complete {command_name} "$cur" --position "$cword"{fetch_flag} 2>/dev/null | cut -f1)
    if [[ -n "$raw" ]]; then
        COMPREPLY=( $(compgen -W "$raw" -- "$cur") )
        compopt -o nosort 2>/dev/null || true
        return 0
    fi
}}
complete -F _{func_name} {command_name}
"#
    );

    // Register for git subcommand invocation (git worktree-checkout)
    if command_name.starts_with("git-") {
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
    }

    // Register completions for shortcut aliases
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!("complete -F _{func_name} {}\n", shortcut.alias));
        }
    }

    output
}

pub(super) const DAFT_BASH_COMPLETIONS: &str = r#"# daft subcommand completions
_daft() {
    local cur prev words cword
    _init_completion || return

    # hooks: subcommand and argument completion
    if [[ $cword -ge 2 && "${words[1]}" == "hooks" ]]; then
        # hooks subcommand completion (position 2)
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "trust prompt deny status migrate install validate dump run" -- "$cur") )
            COMPREPLY+=( $(compgen -d -- "$cur") )
            return 0
        fi

        # hooks subcommand arguments (position 3+)
        case "${words[2]}" in
            run)
                if [[ "$prev" == "--job" ]]; then
                    local hook_type="" i
                    for ((i=3; i<cword; i++)); do
                        if [[ "${words[$i]}" != -* ]]; then
                            hook_type="${words[$i]}"
                            break
                        fi
                    done
                    if [[ -n "$hook_type" ]]; then
                        local jobs
                        jobs=$(DAFT_COMPLETE_HOOK="$hook_type" daft __complete hooks-run-job "$cur" 2>/dev/null | cut -f1)
                        COMPREPLY=( $(compgen -W "$jobs" -- "$cur") )
                    fi
                    return 0
                fi
                [[ "$prev" == "--tag" ]] && return 0
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--job --tag --dry-run -v --verbose -h --help" -- "$cur") )
                    return 0
                fi
                local hooks
                hooks=$(daft __complete hooks-run "$cur" 2>/dev/null)
                COMPREPLY=( $(compgen -W "$hooks" -- "$cur") )
                return 0
                ;;
            status)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-s --short -h --help" -- "$cur") )
                    return 0
                fi
                COMPREPLY=( $(compgen -d -- "$cur") )
                return 0
                ;;
            prompt|deny)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-f --force -h --help" -- "$cur") )
                    return 0
                fi
                COMPREPLY=( $(compgen -d -- "$cur") )
                return 0
                ;;
            trust)
                if [[ $cword -eq 3 ]]; then
                    if [[ "$cur" == -* ]]; then
                        COMPREPLY=( $(compgen -W "-f --force -h --help" -- "$cur") )
                        return 0
                    fi
                    COMPREPLY=( $(compgen -W "list reset prune" -- "$cur") )
                    COMPREPLY+=( $(compgen -d -- "$cur") )
                    return 0
                fi
                if [[ $cword -eq 4 && "${words[3]}" == "reset" ]]; then
                    if [[ "$cur" == -* ]]; then
                        COMPREPLY=( $(compgen -W "-f --force -h --help" -- "$cur") )
                        return 0
                    fi
                    COMPREPLY=( $(compgen -W "all" -- "$cur") )
                    COMPREPLY+=( $(compgen -d -- "$cur") )
                    return 0
                fi
                COMPREPLY=( $(compgen -d -- "$cur") )
                return 0
                ;;
            migrate)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--dry-run -h --help" -- "$cur") )
                fi
                return 0
                ;;
        esac
        return 0
    fi

    # layout: complete subcommands and arguments
    if [[ $cword -ge 2 && "${words[1]}" == "layout" ]]; then
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "default list show transform" -- "$cur") )
            return 0
        fi
        case "${words[2]}" in
            show)
                COMPREPLY=( $(compgen -d -- "$cur") )
                return 0
                ;;
            transform|default)
                if [[ "$cur" == -* ]]; then
                    if [[ "${words[2]}" == "transform" ]]; then
                        COMPREPLY=( $(compgen -W "--force -f --dry-run --include --include-all -h --help" -- "$cur") )
                    else
                        COMPREPLY=( $(compgen -W "--reset -h --help" -- "$cur") )
                    fi
                    return 0
                fi
                local layouts
                layouts=$(daft __complete layout-"${words[2]}" "$cur" 2>/dev/null | cut -f1)
                COMPREPLY=( $(compgen -W "$layouts" -- "$cur") )
                return 0
                ;;
        esac
        return 0
    fi

    # multi-remote: complete subcommands
    if [[ $cword -eq 2 && "${words[1]}" == "multi-remote" ]]; then
        COMPREPLY=( $(compgen -W "enable disable status set-default move" -- "$cur") )
        return 0
    fi

    # config: complete subcommands
    if [[ $cword -eq 2 && "${words[1]}" == "config" ]]; then
        COMPREPLY=( $(compgen -W "remote-sync" -- "$cur") )
        return 0
    fi

    # shared: complete subcommands and their arguments
    if [[ "${words[1]}" == "shared" ]]; then
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "add link manage materialize remove status sync" -- "$cur") )
            return 0
        fi
        local shared_sub="${words[2]}"
        case "$shared_sub" in
            add)
                # File completion + --declare flag
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--declare --help -h" -- "$cur") )
                else
                    COMPREPLY=( $(compgen -f -- "$cur") )
                fi
                return 0
                ;;
            remove)
                # Complete from shared files list + --delete flag
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--delete --help -h" -- "$cur") )
                else
                    local shared_files
                    shared_files=$(daft __complete "shared-files" "$cur" 2>/dev/null)
                    COMPREPLY=( $(compgen -W "$shared_files" -- "$cur") )
                fi
                return 0
                ;;
            link|materialize)
                # Position 3: shared file, position 4: worktree name
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--override --help -h" -- "$cur") )
                elif [[ $cword -eq 3 ]]; then
                    local shared_files
                    shared_files=$(daft __complete "shared-files" "$cur" 2>/dev/null)
                    COMPREPLY=( $(compgen -W "$shared_files" -- "$cur") )
                elif [[ $cword -eq 4 ]]; then
                    local worktrees
                    worktrees=$(daft __complete "shared-worktrees" "$cur" 2>/dev/null)
                    COMPREPLY=( $(compgen -W "$worktrees" -- "$cur") )
                fi
                return 0
                ;;
            status|sync)
                # No arguments
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--help -h" -- "$cur") )
                fi
                return 0
                ;;
        esac
    fi

    # verb aliases: delegate to underlying command completions
    if [[ $cword -ge 2 ]]; then
        case "${words[1]}" in
            go)
                COMP_WORDS=("daft-go" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_go
                return 0
                ;;
            start)
                COMP_WORDS=("daft-start" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_start
                return 0
                ;;
            carry)
                COMP_WORDS=("git-worktree-carry" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_carry
                return 0
                ;;
            update)
                COMP_WORDS=("git-worktree-fetch" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_fetch
                return 0
                ;;
            rename)
                COMP_WORDS=("daft-rename" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_rename
                return 0
                ;;
            sync)
                COMP_WORDS=("git-worktree-sync" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_sync
                return 0
                ;;
            remove)
                COMP_WORDS=("daft-remove" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_remove
                return 0
                ;;
            list)
                COMP_WORDS=("git-worktree-list" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_list
                return 0
                ;;
            prune)
                COMP_WORDS=("git-worktree-prune" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_prune
                return 0
                ;;
            clone)
                COMP_WORDS=("git-worktree-clone" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_clone
                return 0
                ;;
            init)
                COMP_WORDS=("git-worktree-init" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_init
                return 0
                ;;
        esac
    fi

    # top-level: complete daft subcommands and flags
    if [[ $cword -eq 1 ]]; then
        if [[ "$cur" == -* ]]; then
            COMPREPLY=( $(compgen -W "--version -V --help -h" -- "$cur") )
        else
            COMPREPLY=( $(compgen -W "hooks shell-init setup multi-remote release-notes doctor layout shared config clone init go start carry update list prune rename sync remove adopt eject" -- "$cur") )
        fi
        return 0
    fi
}
complete -F _daft daft
complete -F _daft git-daft
if declare -f __git_complete >/dev/null 2>&1; then
    __git_complete git-daft _daft
fi
"#;
