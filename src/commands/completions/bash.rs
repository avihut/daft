use super::{extract_flags, get_command_for_name};
use anyhow::{Context, Result};

/// Generate bash completion string
pub(super) fn generate_bash_completion_string(command_name: &str) -> Result<String> {
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

pub(super) const DAFT_BASH_COMPLETIONS: &str = r#"# daft subcommand completions
_daft() {
    local cur prev words cword
    _init_completion || return

    # hooks run: dynamic hook type and job name completion
    if [[ $cword -ge 3 && "${words[1]}" == "hooks" && "${words[2]}" == "run" ]]; then
        # --job: complete job names for the given hook type
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
            COMPREPLY=( $(compgen -W "--job --tag --dry-run -h --help" -- "$cur") )
            return 0
        fi
        local hooks
        hooks=$(daft __complete hooks-run "$cur" 2>/dev/null)
        COMPREPLY=( $(compgen -W "$hooks" -- "$cur") )
        return 0
    fi

    # hooks: complete subcommands
    if [[ $cword -eq 2 && "${words[1]}" == "hooks" ]]; then
        COMPREPLY=( $(compgen -W "trust prompt deny status migrate install validate dump run" -- "$cur") )
        return 0
    fi

    # multi-remote: complete subcommands
    if [[ $cword -eq 2 && "${words[1]}" == "multi-remote" ]]; then
        COMPREPLY=( $(compgen -W "enable disable status set-default move" -- "$cur") )
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
                COMP_WORDS=("git-sync" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_sync
                return 0
                ;;
            remove)
                COMP_WORDS=("daft-remove" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_remove
                return 0
                ;;
        esac
    fi

    # top-level: complete daft subcommands
    if [[ $cword -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "hooks shell-init completions setup multi-remote release-notes doctor clone init go start carry update prune rename sync remove adopt eject" -- "$cur") )
        return 0
    fi
}
complete -F _daft daft
complete -F _daft git-daft
if declare -f __git_complete >/dev/null 2>&1; then
    __git_complete git-daft _daft
fi
"#;
