use super::{
    allows_path_completion, command_has_repo_flag, emit_formats_for, extract_flags,
    get_command_for_name, uses_fetch_on_miss, uses_rich_completions,
};
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

    // Value completion for --repo flag (catalog repo names)
    if command_has_repo_flag(command_name) {
        output.push_str("    # Catalog repo-name completion for --repo\n");
        output.push_str("    if [[ \"$prev\" == \"--repo\" ]]; then\n");
        output.push_str("        local repos\n");
        output.push_str(
            "        repos=$(daft __complete repo-name \"$cur\" 2>/dev/null | cut -f1)\n",
        );
        output.push_str("        COMPREPLY=( $(compgen -W \"$repos\" -- \"$cur\") )\n");
        output.push_str("        return 0\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    // Value completion for --skip-hooks flag (selector vocabulary from daft.yml)
    let has_skip_hooks = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-clone"
            | "git-worktree-flow-adopt"
            | "daft-go"
            | "daft-start"
    );
    if has_skip_hooks {
        output.push_str("    # Skip-hooks selector completion for --skip-hooks\n");
        output.push_str("    if [[ \"$prev\" == \"--skip-hooks\" ]]; then\n");
        output.push_str("        local selectors\n");
        output.push_str(
            "        selectors=$(daft __complete skip-hooks-value \"$cur\" 2>/dev/null | cut -f1)\n",
        );
        output.push_str("        COMPREPLY=( $(compgen -W \"$selectors\" -- \"$cur\") )\n");
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

    // Value completion for --format flag (emit-enabled commands only)
    if let Some(formats) = emit_formats_for(command_name) {
        let format_list = formats.join(" ");
        output.push_str("    # Format value completion for --format\n");
        output.push_str("    if [[ \"$prev\" == \"--format\" ]]; then\n");
        output.push_str(&format!(
            "        COMPREPLY=( $(compgen -W \"{format_list}\" -- \"$cur\") )\n"
        ));
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
    // Path-accepting commands (daft-remove, daft-rename) also offer directory
    // completion. When the user types a path-like prefix (./, ../, /, ~/) we
    // skip the dynamic branch source entirely; otherwise paths are appended
    // alongside any branch matches so both worlds work in one keystroke.
    //
    // Both branches use `mapfile -t` (bash 4+, already required by
    // `_init_completion`) so directory names containing spaces/tabs/newlines
    // arrive as a single COMPREPLY entry rather than being word-split on $IFS.
    let path_pre = if allows_path_completion(command_name) {
        r#"    case "$cur" in
        /*|./*|../*|~/*|~)
            mapfile -t COMPREPLY < <(compgen -d -- "$cur")
            compopt -o filenames 2>/dev/null || true
            return 0
            ;;
    esac
"#
    } else {
        ""
    };
    let path_post = if allows_path_completion(command_name) {
        r#"    local dirs
    dirs=$(compgen -d -- "$cur")
    if [[ -n "$dirs" ]]; then
        while IFS=$'\n' read -r d; do
            COMPREPLY+=("$d")
        done <<< "$dirs"
        compopt -o filenames 2>/dev/null || true
    fi
"#
    } else {
        ""
    };

    // Value completion for --repo flag (catalog repo names)
    let repo_flag_pre = if command_has_repo_flag(command_name) {
        r#"    if [[ "$prev" == "--repo" ]]; then
        local repos
        repos=$(daft __complete repo-name "$cur" 2>/dev/null | cut -f1)
        COMPREPLY=( $(compgen -W "$repos" -- "$cur") )
        return 0
    fi

"#
    } else {
        ""
    };

    // daft-go's position-2 completion (branches of the repo at position 1)
    // needs the first positional; pass it via env — the __complete protocol
    // only carries the current word.
    let env_prefix = if command_name == "daft-go" {
        r#"DAFT_COMPLETE_GO_FIRST="${words[1]}" "#
    } else {
        ""
    };

    // Rich commands that also carry --skip-hooks (checkout, go) complete its
    // selector vocabulary when the previous word is the flag.
    let skip_hooks_pre = if matches!(command_name, "git-worktree-checkout" | "daft-go") {
        r#"    if [[ "$prev" == "--skip-hooks" ]]; then
        local selectors
        selectors=$(daft __complete skip-hooks-value "$cur" 2>/dev/null | cut -f1)
        COMPREPLY=( $(compgen -W "$selectors" -- "$cur") )
        return 0
    fi

"#
    } else {
        ""
    };

    let mut output = format!(
        r#"_{func_name}() {{
    local cur prev words cword
    _init_completion || return

{repo_flag_pre}{skip_hooks_pre}    if [[ "$cur" == -* ]]; then
        local flags="{flags_joined}"
        COMPREPLY=( $(compgen -W "$flags" -- "$cur") )
        return 0
    fi

{path_pre}    local raw
    raw=$({env_prefix}daft __complete {command_name} "$cur" --position "$cword"{fetch_flag} 2>/dev/null | cut -f1)
    if [[ -n "$raw" ]]; then
        COMPREPLY=( $(compgen -W "$raw" -- "$cur") )
        compopt -o nosort 2>/dev/null || true
    fi
{path_post}    if [[ ${{#COMPREPLY[@]}} -gt 0 ]]; then
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

    # `-C <path>` is a top-level option (issue #519). If the previous token is
    # `-C`, complete directories for its value and stop.
    if [[ "$prev" == "-C" ]]; then
        COMPREPLY=( $(compgen -d -- "$cur") )
        return 0
    fi

    # Strip leading `-C <path>` pairs from words/cword so the rest of the
    # completion logic sees argv as if `-C` weren't there. This keeps every
    # subsequent `${words[1]}` / `cword -eq N` branch correct regardless of
    # how many `-C` flags precede the subcommand.
    local __daft_skip=1
    while [[ "${words[$__daft_skip]}" == "-C" && $((__daft_skip + 1)) -le ${#words[@]} ]]; do
        __daft_skip=$((__daft_skip + 2))
    done
    if [[ $__daft_skip -gt 1 ]]; then
        words=("${words[0]}" "${words[@]:$__daft_skip}")
        cword=$((cword - (__daft_skip - 1)))
        if [[ $cword -lt 1 ]]; then cword=1; fi
    fi

    # --format value completion (emit-enabled subcommand paths)
    if [[ "$prev" == "--format" ]]; then
        local _fmt_path="" _fmt_i _fmt_w
        for ((_fmt_i=1; _fmt_i<cword; _fmt_i++)); do
            _fmt_w="${words[$_fmt_i]}"
            [[ "$_fmt_w" == -* ]] && break
            if [[ -z "$_fmt_path" ]]; then
                _fmt_path="$_fmt_w"
            else
                _fmt_path="$_fmt_path $_fmt_w"
            fi
        done
        case "$_fmt_path" in
            list|worktree-list|"hooks trust list"|"hooks jobs"|"layout list"|"shared status")
                COMPREPLY=( $(compgen -W "json ndjson tsv csv yaml toon markdown" -- "$cur") )
                return 0
                ;;
            release-notes|"multi-remote status"|"hooks run")
                COMPREPLY=( $(compgen -W "json yaml toon markdown" -- "$cur") )
                return 0
                ;;
        esac
    fi

    # hooks: subcommand and argument completion
    if [[ $cword -ge 2 && "${words[1]}" == "hooks" ]]; then
        # hooks subcommand completion (position 2)
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "trust prompt deny status migrate install validate dump run jobs" -- "$cur") )
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
            jobs)
                if [[ $cword -eq 3 ]]; then
                    # Flag prefix → emit listing-form flags; otherwise the
                    # subcommands. The cword > 3 branch below only runs once
                    # a subcommand has been chosen.
                    if [[ "$cur" == -* ]]; then
                        COMPREPLY=( $(compgen -W "--all --format --template --no-headers --worktree --status --hook -h --help" -- "$cur") )
                    else
                        COMPREPLY=( $(compgen -W "logs cancel retry prune" -- "$cur") )
                    fi
                    return 0
                fi
                case "${words[3]}" in
                    logs|cancel)
                        if [[ "$cur" == -* ]]; then
                            COMPREPLY=( $(compgen -W "--inv -h --help" -- "$cur") )
                            return 0
                        fi
                        # Lines are `KIND\t<value>\t<display>`. Bash only
                        # uses the bare value, so strip KIND then take the
                        # first remaining tab-separated field.
                        local completions
                        completions=$(daft __complete hooks-jobs-job "$cur" 2>/dev/null)
                        if [[ -n "$completions" ]]; then
                            while IFS=$'\n' read -r line; do
                                local rest="${line#*	}"
                                local val="${rest%%	*}"
                                COMPREPLY+=( "$val" )
                            done <<< "$completions"
                        fi
                        return 0
                        ;;
                    retry)
                        if [[ "${prev}" == "--worktree" ]]; then
                            local completions
                            completions=$(daft __complete hooks-jobs-retry-worktree "$cur" 2>/dev/null)
                            if [[ -n "$completions" ]]; then
                                while IFS=$'\n' read -r line; do
                                    local val="${line%%	*}"
                                    COMPREPLY+=("$val")
                                done <<< "$completions"
                            fi
                            return 0
                        fi
                        if [[ "$cur" == -* ]]; then
                            COMPREPLY=( $(compgen -W "--hook --inv --job --worktree --cwd -h --help" -- "$cur") )
                            return 0
                        fi
                        local completions
                        completions=$(daft __complete hooks-jobs-retry "$cur" 2>/dev/null)
                        if [[ -n "$completions" ]]; then
                            while IFS=$'\n' read -r line; do
                                local val="${line%%	*}"
                                COMPREPLY+=("$val")
                            done <<< "$completions"
                        fi
                        return 0
                        ;;
                esac
                if [[ "${prev}" == "--worktree" ]]; then
                    local completions
                    completions=$(daft __complete hooks-jobs-worktree "$cur" 2>/dev/null)
                    if [[ -n "$completions" ]]; then
                        while IFS=$'\n' read -r line; do
                            local val="${line%%	*}"
                            COMPREPLY+=("$val")
                        done <<< "$completions"
                    fi
                    return 0
                fi
                if [[ "${prev}" == "--status" ]]; then
                    COMPREPLY=( $(compgen -W "failed completed running cancelled skipped" -- "$cur") )
                    return 0
                fi
                if [[ "${prev}" == "--hook" ]]; then
                    local completions
                    completions=$(daft __complete hooks-jobs-hook-filter "$cur" 2>/dev/null)
                    if [[ -n "$completions" ]]; then
                        while IFS=$'\n' read -r line; do
                            local val="${line%%	*}"
                            COMPREPLY+=("$val")
                        done <<< "$completions"
                    fi
                    return 0
                fi
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--all --format --template --no-headers --worktree --status --hook -h --help" -- "$cur") )
                    return 0
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

    # repo: complete subcommands and arguments
    if [[ $cword -ge 2 && "${words[1]}" == "repo" ]]; then
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "add info install list remove" -- "$cur") )
            return 0
        fi
        case "${words[2]}" in
            add)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--name -q --quiet -v --verbose -h --help" -- "$cur") )
                    return 0
                fi
                COMPREPLY=( $(compgen -d -- "$cur") )
                return 0
                ;;
            info)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--format --template --no-headers -h --help" -- "$cur") )
                    return 0
                fi
                local repos
                repos=$(daft __complete repo-name "$cur" 2>/dev/null | cut -f1)
                COMPREPLY=( $(compgen -W "$repos" -- "$cur") )
                return 0
                ;;
            install)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-q --quiet -v --verbose --git-exclude -h --help" -- "$cur") )
                fi
                return 0
                ;;
            list)
                if [[ "$prev" == "--columns" ]]; then
                    local columns="annotation name worktrees layout branch path size remote"
                    local prefixed=""
                    for c in $columns; do prefixed="$prefixed $c +$c -$c"; done
                    COMPREPLY=( $(compgen -W "$prefixed" -- "$cur") )
                    return 0
                fi
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-a --all --columns --format --template --no-headers -q --quiet -h --help" -- "$cur") )
                fi
                return 0
                ;;
            remove)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-y --force --dry-run -v --verbose -h --help" -- "$cur") )
                    return 0
                fi
                COMPREPLY=( $(compgen -d -- "$cur") )
                return 0
                ;;
        esac
        return 0
    fi

    # config: complete subcommands
    if [[ $cword -eq 2 && "${words[1]}" == "config" ]]; then
        COMPREPLY=( $(compgen -W "remote-sync" -- "$cur") )
        return 0
    fi

    # file: complete subcommands and arguments
    if [[ "${words[1]}" == "file" ]]; then
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "merge" -- "$cur") )
            return 0
        fi
        case "${words[2]}" in
            merge)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--keep-source -y --yes -h --help" -- "$cur") )
                    return 0
                fi
                COMPREPLY=( $(compgen -f -- "$cur") )
                return 0
                ;;
        esac
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

    # merge: flag + branch completion (inline; not auto-generated from COMMANDS)
    if [[ $cword -ge 2 && ( "${words[1]}" == "merge" || "${words[1]}" == "worktree-merge" ) ]]; then
        # --into takes a branch value
        if [[ "$prev" == "--into" ]]; then
            local branches
            branches=$(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null)
            COMPREPLY=( $(compgen -W "$branches" -- "$cur") )
            return 0
        fi
        # --cleanup mode values
        if [[ "$prev" == "--cleanup" ]]; then
            COMPREPLY=( $(compgen -W "default scissors strip verbatim whitespace" -- "$cur") )
            return 0
        fi
        # --strategy / -s values
        if [[ "$prev" == "--strategy" || "$prev" == "-s" ]]; then
            COMPREPLY=( $(compgen -W "ours recursive resolve octopus subtree" -- "$cur") )
            return 0
        fi
        if [[ "$cur" == -* ]]; then
            local flags="--into --abort --continue --quit --adopt-target --no-adopt-target -y --yes --merge --squash --rebase --rebase-merge -r --remove-branch --keep-branch --set-default -m -F --file --edit --no-edit --cleanup --commit --no-commit --signoff --no-signoff -s --strategy -X --strategy-option -S --gpg-sign --no-gpg-sign --verify-signatures --no-verify-signatures --allow-unrelated-histories --stat -n --no-stat -v --verbose -h --help -V --version"
            COMPREPLY=( $(compgen -W "$flags" -- "$cur") )
            return 0
        fi
        # Positional source/target: branch names
        local branches
        branches=$(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null)
        COMPREPLY=( $(compgen -W "$branches" -- "$cur") )
        return 0
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
            exec)
                COMP_WORDS=("git-worktree-exec" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_exec
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
            COMPREPLY=( $(compgen -W "--version -V --help -h -C" -- "$cur") )
        else
            COMPREPLY=( $(compgen -W "activate hooks shell-init multi-remote release-notes doctor layout shared config file repo clone init install go start carry exec update list prune rename sync remove merge worktree-merge adopt eject" -- "$cur") )
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
