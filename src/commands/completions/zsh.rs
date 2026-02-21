use super::{extract_flags, get_command_for_name};
use anyhow::{Context, Result};

/// Generate zsh completion string
pub(super) fn generate_zsh_completion_string(command_name: &str) -> Result<String> {
    let mut output = String::new();
    let has_branches = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-checkout-branch"
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

pub(super) const DAFT_ZSH_COMPLETIONS: &str = r#"# daft subcommand completions
_daft() {
    local curword="${words[$CURRENT]}"

    # hooks run: dynamic hook type and job name completion
    if (( CURRENT >= 4 )) && [[ "$words[2]" == "hooks" && "$words[3]" == "run" ]]; then
        local prev="$words[$((CURRENT-1))]"
        if [[ "$prev" == "--job" ]]; then
            local hook_type="" i
            for ((i=4; i<CURRENT; i++)); do
                if [[ "$words[$i]" != -* ]]; then
                    hook_type="$words[$i]"
                    break
                fi
            done
            if [[ -n "$hook_type" ]]; then
                local -a job_specs
                local line
                while IFS= read -r line; do
                    if [[ "$line" == *$'\t'* ]]; then
                        local jname="${line%%	*}"
                        local jdesc="${line#*	}"
                        job_specs+=("${jname}:${jdesc}")
                    else
                        job_specs+=("${line}")
                    fi
                done < <(DAFT_COMPLETE_HOOK="$hook_type" daft __complete hooks-run-job "$curword" 2>/dev/null)
                _describe 'job' job_specs
            fi
            return
        fi
        [[ "$prev" == "--tag" ]] && return
        if [[ "$curword" == -* ]]; then
            compadd -- --job --tag --dry-run -h --help
            return
        fi
        local -a hooks
        hooks=(${(f)"$(daft __complete hooks-run "$curword" 2>/dev/null)"})
        compadd -a hooks
        return
    fi

    # hooks: complete subcommands
    if (( CURRENT == 3 )) && [[ "$words[2]" == "hooks" ]]; then
        compadd trust prompt deny status migrate install validate dump run
        return
    fi

    # top-level: complete daft subcommands
    if (( CURRENT == 2 )); then
        compadd hooks shell-init completions setup branch multi-remote release-notes doctor
        return
    fi
}
compdef _daft daft
compdef _daft git-daft
"#;
