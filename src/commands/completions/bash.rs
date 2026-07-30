use super::{
    allows_path_completion, command_has_hooks_flag, command_has_repo_flag,
    command_has_repo_positional, command_has_worktree_from_flag, emit_formats_for, extract_flags,
    get_command_for_name, repo_flag_capture, uses_fetch_on_miss, uses_rich_completions,
    value_taking_flags,
};
use anyhow::{Context, Result};

/// The `--hooks <MODE>` value block, shared by the plain and rich generators.
///
/// Both spell the current and previous words the same way (`$cur` / `$prev`,
/// from `_init_completion`) at the same indent, so one string serves both — a
/// second copy is how the two drift. The mode list is read from
/// [`HookMode::variants`](crate::hooks::HookMode::variants) rather than spelled
/// out; bash has no per-candidate description, so the glosses from
/// `HookMode::describe` stay behind in zsh and Fig.
fn hooks_mode_block() -> String {
    let modes = crate::hooks::HookMode::variants().join(" ");
    format!(
        r#"    # Hook-mode value completion for --hooks
    if [[ "$prev" == "--hooks" ]]; then
        COMPREPLY=( $(compgen -W "{modes}" -- "$cur") )
        return 0
    fi

"#
    )
}

/// Generate bash completion string
pub(super) fn generate_bash_completion_string(command_name: &str) -> Result<String> {
    // Rich completion commands get cut -f1 + nosort to preserve group ordering.
    if uses_rich_completions(command_name) {
        return Ok(generate_bash_rich_completion(command_name));
    }

    let mut output = String::new();

    let func_name = command_name.replace('-', "_");

    output.push_str(&format!("_{func_name}() {{\n"));
    output.push_str("    local cur prev words cword\n");
    output.push_str("    _init_completion || return\n");
    output.push('\n');

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
        output.push_str("    # Catalog repo-name completion for --repo. The helper already\n");
        output
            .push_str("    # case-folds the prefix, so fill COMPREPLY directly — a `compgen -W`\n");
        output.push_str("    # re-filter is case-sensitive and would drop the folded matches.\n");
        output.push_str("    if [[ \"$prev\" == \"--repo\" ]]; then\n");
        output.push_str("        local -a repos\n");
        output.push_str(
            "        mapfile -t repos < <(daft __complete repo-name \"$cur\" 2>/dev/null | cut -f1)\n",
        );
        output.push_str("        COMPREPLY=( \"${repos[@]}\" )\n");
        output.push_str("        return 0\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    // Value completion for --skip-hooks flag (selector vocabulary from daft.yml)
    let has_skip_hooks = matches!(
        command_name,
        "git-worktree-checkout" | "git-worktree-clone" | "daft-go" | "daft-start"
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

    // Value completion for --hooks flag (the run's hook execution mode)
    if command_has_hooks_flag(command_name) {
        output.push_str(&hooks_mode_block());
    }

    // Value completion for --columns flag
    let has_columns = matches!(
        command_name,
        "git-worktree-list" | "git-worktree-sync" | "git-worktree-prune"
    );
    if has_columns {
        output.push_str("    # Column name completion for --columns\n");
        output.push_str("    if [[ \"$prev\" == \"--columns\" ]]; then\n");
        // `status` is list-only: sync/prune pin their own operation-progress
        // Status column and reject the token.
        let columns = if command_name == "git-worktree-list" {
            "annotation status name path size base changes remote pr age owner hash last-commit"
        } else {
            "annotation name path size base changes remote pr age owner hash last-commit"
        };
        output.push_str(&format!("        local columns=\"{columns}\"\n"));
        output.push_str("        local prefixed=\"\"\n");
        output.push_str("        for c in $columns; do prefixed=\"$prefixed $c +$c -$c\"; done\n");
        output.push_str("        COMPREPLY=( $(compgen -W \"$prefixed\" -- \"$cur\") )\n");
        output.push_str("        return 0\n");
        output.push_str("    fi\n");
        output.push('\n');
        output.push_str("    # Sort column completion for --sort\n");
        output.push_str("    if [[ \"$prev\" == \"--sort\" ]]; then\n");
        output.push_str("        local cols=\"name path size base changes remote age owner hash activity commit\"\n");
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

    // Positional repo-name completion (daft list [<repo>]). Placed after
    // every value-flag prev block (each returns on match) so flag values
    // never receive repo names; --template and --stat have no prev block,
    // so they are excluded explicitly.
    if command_has_repo_positional(command_name) {
        output.push_str("    # Positional cataloged-repo completion (case-insensitive: the\n");
        output
            .push_str("    # helper case-folds, so fill COMPREPLY directly instead of through a\n");
        output.push_str("    # case-sensitive `compgen -W` re-filter).\n");
        output.push_str(
            "    if [[ \"$cur\" != -* && \"$prev\" != \"--template\" && \"$prev\" != \"--stat\" ]]; then\n",
        );
        output.push_str("        local -a repos\n");
        output.push_str(
            "        mapfile -t repos < <(daft __complete repo-name \"$cur\" 2>/dev/null | cut -f1)\n",
        );
        output.push_str("        if [[ ${#repos[@]} -gt 0 ]]; then\n");
        output.push_str("            COMPREPLY=( \"${repos[@]}\" )\n");
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

/// Generate a bash completion script with rich grouped output for any command
/// that uses the `name\tgroup\tdescription` protocol.
fn generate_bash_rich_completion(command_name: &str) -> String {
    let cmd = get_command_for_name(command_name)
        .unwrap_or_else(|| panic!("Unknown rich-completion command: {command_name}"));
    let (all_flags, _, _) = extract_flags(&cmd);
    let flags_joined = all_flags.join(" ");

    let func_name = command_name.replace('-', "_");
    // checkout/go complete pr:<n>/mr:<n> forge targets; bash's COMP_WORDBREAKS
    // splits words at ':', so keep it in $cur (-n :) and strip the colon
    // prefix from COMPREPLY afterwards (__ltrim_colon_completions) — the
    // standard bash-completion idiom for colon-bearing candidates. When the
    // sole surviving candidate is a bare pr:/mr: syntax token, suppress the
    // trailing space so the accepted token stays glued to the number the
    // user types next.
    // `_init_completion` splits the current word on COMP_WORDBREAKS by default.
    // Two candidate chars must stay glued into $cur: ':' for pr:/mr: forge
    // targets (stripped afterward via __ltrim_colon_completions), and '=' so
    // `--repo=<name>` arrives as one word for the positional scan (#749) instead
    // of splitting into `--repo` `=` `<name>` and mis-capturing the value.
    // Exclude whichever apply — the set is additive because `daft-go` needs both.
    let takes_forge_targets = matches!(command_name, "git-worktree-checkout" | "daft-go");
    let mut wordbreak_keep = String::new();
    if takes_forge_targets {
        wordbreak_keep.push(':');
    }
    if command_has_repo_flag(command_name) {
        wordbreak_keep.push('=');
    }
    let init_completion = if wordbreak_keep.is_empty() {
        "_init_completion || return".to_string()
    } else {
        format!("_init_completion -n {wordbreak_keep} || return")
    };
    let ltrim_post = if takes_forge_targets {
        "        declare -F __ltrim_colon_completions >/dev/null && __ltrim_colon_completions \"$cur\"\n        if [[ ${#COMPREPLY[@]} -eq 1 && ( \"${COMPREPLY[0]}\" == \"pr:\" || \"${COMPREPLY[0]}\" == \"mr:\" ) ]]; then\n            compopt -o nospace 2>/dev/null || true\n        fi\n"
    } else {
        ""
    };
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
        // The helper case-folds the prefix; fill COMPREPLY directly rather
        // than through a case-sensitive `compgen -W` re-filter.
        r#"    if [[ "$prev" == "--repo" ]]; then
        local -a repos
        mapfile -t repos < <(daft __complete repo-name "$cur" 2>/dev/null | cut -f1)
        COMPREPLY=( "${repos[@]}" )
        return 0
    fi

"#
    } else {
        ""
    };

    // Value completion for --from (worktree names, same source as the
    // positional). Sits before the `-*` flag branch like --repo does, so
    // `daft warm --from <TAB>` answers with worktrees instead of flags.
    let from_flag_pre = if command_has_worktree_from_flag(command_name) {
        format!(
            r#"    if [[ "$prev" == "--from" ]]; then
        local __wts
        __wts=$(daft __complete {command_name} "$cur" --position 1 2>/dev/null | cut -f1)
        COMPREPLY=( $(compgen -W "$__wts" -- "$cur") )
        return 0
    fi

"#
        )
    } else {
        String::new()
    };

    // daft-go and daft-start complete later positions against the repo named
    // at position 1; pass it via env — the __complete protocol only carries
    // the current word. `$__first` is the first *positional*, not words[1],
    // so a leading flag can't masquerade as the repo name.
    // Additive, not either/or: a command can need both hand-offs, and
    // dropping one here silently breaks that slot's completion.
    let mut env_prefix = String::new();
    match command_name {
        "daft-go" => env_prefix.push_str(r#"DAFT_COMPLETE_GO_FIRST="$__first" "#),
        "daft-start" => env_prefix.push_str(r#"DAFT_COMPLETE_START_FIRST="$__first" "#),
        _ => {}
    }
    // Commands that take --repo forward its value so slots after it complete
    // against the *target* repo rather than the caller's (#749).
    if command_has_repo_flag(command_name) {
        env_prefix.push_str(r#"DAFT_COMPLETE_REPO_FLAG="$__repo" "#);
    }

    // Captured in the same scan that counts positionals: the flag may sit
    // anywhere before the cursor, in either spelling. Reading `__i + 1` is safe
    // because the value-skip below has not advanced past it yet. zsh uses the
    // byte-identical snippet, so it lives in one shared helper (#749).
    let (repo_decl, repo_capture) = repo_flag_capture(command_name);

    // Positional slots are counted, not taken from $cword: flags and their
    // values sit in `words` too, so `daft start -q <TAB>` is still slot 1.
    let value_flags = value_taking_flags(&cmd).join(" ");
    let position_pre = format!(
        r#"    local -a __posargs=()
    local __i=1 __w
{repo_decl}    while [[ $__i -lt $cword ]]; do
        __w="${{words[$__i]}}"
        case "$__w" in
            --) ;;
            -*)
{repo_capture}                case " {value_flags} " in
                    *" ${{__w%%=*}} "*)
                        [[ "$__w" == *=* ]] || __i=$((__i + 1))
                        ;;
                esac
                ;;
            *) __posargs+=("$__w") ;;
        esac
        __i=$((__i + 1))
    done
    local __position=$(( ${{#__posargs[@]}} + 1 ))
    local __first="${{__posargs[0]:-}}"

"#
    );

    // Rich commands that also carry --skip-hooks (checkout, go, start)
    // complete its selector vocabulary when the previous word is the flag.
    let skip_hooks_pre = if matches!(
        command_name,
        "git-worktree-checkout" | "daft-go" | "daft-start"
    ) {
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

    // …and its `--hooks` mode, from the same shared block the plain generator
    // emits. Both sit before the `-*` branch: a flag value is not a flag.
    let hooks_pre = if command_has_hooks_flag(command_name) {
        hooks_mode_block()
    } else {
        String::new()
    };

    let mut output = format!(
        r#"_{func_name}() {{
    local cur prev words cword
    {init_completion}

{repo_flag_pre}{from_flag_pre}{skip_hooks_pre}{hooks_pre}    if [[ "$cur" == -* ]]; then
        local flags="{flags_joined}"
        COMPREPLY=( $(compgen -W "$flags" -- "$cur") )
        return 0
    fi

{path_pre}{position_pre}    local raw
    raw=$({env_prefix}daft __complete {command_name} "$cur" --position "$__position"{fetch_flag} 2>/dev/null | cut -f1)
    if [[ -n "$raw" ]]; then
        COMPREPLY=( $(compgen -W "$raw" -- "$cur") )
        compopt -o nosort 2>/dev/null || true
{ltrim_post}    fi
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

# Catalog repo names first, then directories. Every `daft repo` verb whose
# positional is a repo takes both spellings (`repo info api`, `repo remove
# ./old-repo`, `repo link ~/src/api`), so they share one implementation: the
# mapfile/compgen pair has a bash-4 floor and empty-array quirks worth fixing
# in one place rather than in each arm.
__daft_complete_repo_or_dir() {
    local cur="$1"
    local -a repos dirs
    mapfile -t repos < <(daft __complete repo-name "$cur" 2>/dev/null | cut -f1)
    mapfile -t dirs < <(compgen -d -- "$cur")
    COMPREPLY=( "${repos[@]}" "${dirs[@]}" )
    compopt -o filenames 2>/dev/null || true
}

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
            list|worktree-list|"hooks trust list"|"hooks jobs"|"layout list"|"shared status"|"config list")
                COMPREPLY=( $(compgen -W "json ndjson tsv csv yaml toon markdown" -- "$cur") )
                return 0
                ;;
            # The config read and write verbs emit one document rather than
            # rows, so the row formats are not among their options. The trailing
            # glob is because three of them take a key before the flag.
            release-notes|"multi-remote status"|"hooks run"|"config get"*|"config set"*|"config unset"*)
                COMPREPLY=( $(compgen -W "json yaml toon markdown" -- "$cur") )
                return 0
                ;;
        esac
    fi

    # hooks: subcommand and argument completion
    if [[ $cword -ge 2 && "${words[1]}" == "hooks" ]]; then
        # hooks subcommand completion (position 2)
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "trust prompt deny status migrate add validate dump run jobs" -- "$cur") )
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
                if [[ "${words[2]}" == "transform" ]]; then
                    case "$prev" in
                        --pivot)
                            local pivots
                            pivots=$(daft __complete layout-pivot "$cur" 2>/dev/null | cut -f1)
                            COMPREPLY=( $(compgen -W "$pivots" -- "$cur") )
                            return 0
                            ;;
                        --as|--include)
                            return 0
                            ;;
                    esac
                fi
                if [[ "$cur" == -* ]]; then
                    if [[ "${words[2]}" == "transform" ]]; then
                        COMPREPLY=( $(compgen -W "--dry-run --as --pivot --include --include-all -y --yes -q --quiet -v --verbose -h --help" -- "$cur") )
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
            COMPREPLY=( $(compgen -W "add info install link list move remove rename unlink" -- "$cur") )
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
                # Catalog repo names first, then directories (`repo info .`,
                # a subdirectory, or any worktree resolves to its repo).
                __daft_complete_repo_or_dir "$cur"
                return 0
                ;;
            install)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-q --quiet -v --verbose --git-exclude -h --help" -- "$cur") )
                fi
                return 0
                ;;
            link)
                if [[ "$prev" == "--name" || "$prev" == "--kind" ]]; then
                    return 0
                fi
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--name --kind -h --help" -- "$cur") )
                    return 0
                fi
                __daft_complete_repo_or_dir "$cur"
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
                    COMPREPLY=( $(compgen -W "-a --all -w --worktrees --columns --format --template --no-headers -q --quiet -h --help" -- "$cur") )
                fi
                return 0
                ;;
            move)
                if [[ "$prev" == "--name" ]]; then
                    return 0
                fi
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--name --dry-run -q --quiet -v --verbose -h --help" -- "$cur") )
                    return 0
                fi
                # First positional is the repo (catalog names, then dirs for a
                # repo daft has never operated in); the second is a directory.
                if [[ $cword -eq 3 ]]; then
                    local -a repos dirs
                    mapfile -t repos < <(daft __complete repo-name "$cur" 2>/dev/null | cut -f1)
                    mapfile -t dirs < <(compgen -d -- "$cur")
                    COMPREPLY=( "${repos[@]}" "${dirs[@]}" )
                    compopt -o filenames 2>/dev/null || true
                    return 0
                fi
                COMPREPLY=( $(compgen -d -- "$cur") )
                compopt -o filenames 2>/dev/null || true
                return 0
                ;;
            rename)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-q --quiet -v --verbose -h --help" -- "$cur") )
                    return 0
                fi
                # Only the first positional is completable: the second is a
                # new name nothing knows yet.
                if [[ $cword -eq 3 ]]; then
                    mapfile -t COMPREPLY < <(daft __complete repo-name "$cur" 2>/dev/null | cut -f1)
                fi
                return 0
                ;;
            remove)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--purge -y --force --dry-run -v --verbose -h --help" -- "$cur") )
                    return 0
                fi
                # Catalog repo names first, then directories — the positional
                # takes either (`repo remove api`, `repo remove ./old-repo`),
                # same treatment as `repo info`.
                __daft_complete_repo_or_dir "$cur"
                return 0
                ;;
            unlink)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "-h --help" -- "$cur") )
                    return 0
                fi
                local -a labels
                mapfile -t labels < <(daft __complete relation-label "$cur" 2>/dev/null)
                COMPREPLY=( "${labels[@]}" )
                return 0
                ;;
        esac
        return 0
    fi

    # skill: complete subcommands and arguments
    if [[ $cword -ge 2 && "${words[1]}" == "skill" ]]; then
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "install uninstall show" -- "$cur") )
            return 0
        fi
        case "${words[2]}" in
            install|uninstall)
                if [[ "$prev" == "--dir" ]]; then
                    COMPREPLY=( $(compgen -d -- "$cur") )
                    return 0
                fi
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--project --dir -q --quiet -v --verbose -h --help" -- "$cur") )
                fi
                return 0
                ;;
            show)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--no-pager -h --help" -- "$cur") )
                fi
                return 0
                ;;
        esac
        return 0
    fi

    # config: subcommands, then registry keys and the values they accept.
    # The cword guard matters: while `config` is itself the word being
    # completed it already sits in words[1], so testing the word alone claims
    # the completion, matches no case arm below, and returns nothing at all --
    # `daft config<TAB>` would stop completing the verb it is spelling.
    if [[ "${words[1]}" == "config" && $cword -ge 2 ]]; then
        if [[ $cword -eq 2 ]]; then
            COMPREPLY=( $(compgen -W "get list set unset" -- "$cur") )
            return 0
        fi
        local config_sub="${words[2]}"
        # --category takes a value, and the categories are a const table, so
        # this answers without opening anything.
        if [[ "${words[cword-1]}" == "--category" ]]; then
            local __cfg_cats=()
            mapfile -t __cfg_cats < <(daft __complete config-category "$cur" 2>/dev/null | cut -f1)
            COMPREPLY=( $(compgen -W "${__cfg_cats[*]}" -- "$cur") )
            return 0
        fi
        case "$config_sub" in
            get|set|unset)
                if [[ "$cur" == -* ]]; then
                    if [[ "$config_sub" == "get" ]]; then
                        COMPREPLY=( $(compgen -W "--origin --global --local --format --template --no-headers -h --help" -- "$cur") )
                    else
                        COMPREPLY=( $(compgen -W "--global --local --format --template --no-headers -h --help" -- "$cur") )
                    fi
                    return 0
                fi
                # Count positionals after the verb: the key slot and the value
                # slot complete against different things, and a flag anywhere
                # before the cursor must not shift them.
                local __cfg_pos=0 __cfg_key="" __cfg_i
                for (( __cfg_i = 3; __cfg_i < cword; __cfg_i++ )); do
                    [[ "${words[__cfg_i]}" == -* ]] && continue
                    (( __cfg_pos++ ))
                    [[ $__cfg_pos -eq 1 ]] && __cfg_key="${words[__cfg_i]}"
                done
                local __cfg_out=()
                if [[ $__cfg_pos -eq 0 ]]; then
                    mapfile -t __cfg_out < <(daft __complete config-key "$cur" 2>/dev/null | cut -f1)
                elif [[ "$config_sub" == "set" && $__cfg_pos -eq 1 ]]; then
                    mapfile -t __cfg_out < <(DAFT_COMPLETE_CONFIG_KEY="$__cfg_key" daft __complete config-value "$cur" 2>/dev/null | cut -f1)
                else
                    return 0
                fi
                COMPREPLY=( $(compgen -W "${__cfg_out[*]}" -- "$cur") )
                return 0
                ;;
            list)
                if [[ "$cur" == -* ]]; then
                    COMPREPLY=( $(compgen -W "--modified --category --global --local --format --template --no-headers -h --help" -- "$cur") )
                fi
                return 0
                ;;
        esac
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
        # --hooks mode values. Spelled out, not read from HookMode::variants():
        # this const does not interpolate. `hooks_flag_offers_every_mode_in_every_shell`
        # is what keeps the literal honest.
        if [[ "$prev" == "--hooks" ]]; then
            COMPREPLY=( $(compgen -W "auto foreground background off" -- "$cur") )
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
            local flags="--into --abort --continue --quit --adopt-target --no-adopt-target -y --yes --merge --squash --rebase --rebase-merge -r --remove-branch --keep-branch --set-default -m -F --file --edit --no-edit --cleanup --commit --no-commit --signoff --no-signoff -s --strategy -X --strategy-option -S --gpg-sign --no-gpg-sign --verify-signatures --no-verify-signatures --allow-unrelated-histories --stat -n --no-stat --ff-only --no-ff-only --source-worktree --hooks --skip-hooks --skip-tag --only-tag --format --template --no-headers -v --verbose -h --help -V --version"
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
            run)
                COMP_WORDS=("daft-run" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_run
                return 0
                ;;
            env)
                COMP_WORDS=("daft-env" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _daft_env
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
            push)
                COMP_WORDS=("git-worktree-push" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_push
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
            warm)
                COMP_WORDS=("git-worktree-warm" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_warm
                return 0
                ;;
        esac
    fi

    # top-level: complete daft subcommands and flags
    if [[ $cword -eq 1 ]]; then
        if [[ "$cur" == -* ]]; then
            COMPREPLY=( $(compgen -W "--version -V --help -h -C" -- "$cur") )
        else
            COMPREPLY=( $(compgen -W "activate hooks shell-init multi-remote release-notes doctor layout shared config file repo skill clone init install go start carry exec run env warm update list prune rename sync push remove merge worktree-merge" -- "$cur") )
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
