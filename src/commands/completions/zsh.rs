use super::{extract_flags, get_command_for_name, uses_fetch_on_miss, uses_rich_completions};
use anyhow::{Context, Result};

/// Generate zsh completion string
pub(super) fn generate_zsh_completion_string(command_name: &str) -> Result<String> {
    // Rich completion commands get the grouped parsing (compadd -V).
    if uses_rich_completions(command_name) {
        return Ok(generate_zsh_rich_completion(command_name));
    }

    let mut output = String::new();
    // daft-start still uses simple branch-prefix patterns (not rich).
    let has_branches = command_name == "daft-start";

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

    // Value completion for -b / --branch flag (clone only)
    let has_branch_completions = command_name == "git-worktree-clone";
    // Value completion for --layout flag
    let has_layout = matches!(command_name, "git-worktree-clone" | "git-worktree-init");

    // Emit the prev_word variable once if any prev-based completion is needed
    if has_branch_completions || has_layout {
        output.push_str("    local prev_word=\"${words[$((CURRENT-1))]}\"\n");
    }

    if has_branch_completions {
        output.push_str("    # Static value completion for -b / --branch\n");
        output.push_str(
            "    if [[ \"$prev_word\" == \"-b\" || \"$prev_word\" == \"--branch\" ]]; then\n",
        );
        output.push_str("        compadd HEAD @\n");
        output.push_str("        return\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    if has_layout {
        output.push_str("    # Layout name completion for --layout\n");
        output.push_str("    if [[ \"$prev_word\" == \"--layout\" ]]; then\n");
        output.push_str("        local -a layouts\n");
        output.push_str("        layouts=(\"${(@f)$(daft __complete layout-value \"$curword\" 2>/dev/null | sed 's/\\t/:/')}\")\n");
        output.push_str("        _describe 'layout' layouts\n");
        output.push_str("        return\n");
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
        output.push_str("    local prev_word=\"${words[$((CURRENT-1))]}\"\n");
        output.push_str("    if [[ \"$prev_word\" == \"--columns\" ]]; then\n");
        output.push_str("        local -a column_values\n");
        output.push_str("        column_values=(\n");
        output.push_str("            'annotation:Annotation markers'\n");
        output.push_str("            'branch:Branch name'\n");
        output.push_str("            'path:Worktree path'\n");
        output.push_str("            'size:Disk size of worktree'\n");
        output.push_str("            'base:Ahead/behind base branch'\n");
        output.push_str("            'changes:Local changes'\n");
        output.push_str("            'remote:Ahead/behind remote'\n");
        output.push_str("            'age:Branch age'\n");
        output.push_str("            'owner:Branch owner'\n");
        output.push_str("            'hash:Commit hash'\n");
        output.push_str("            'last-commit:Last commit'\n");
        output.push_str("            '+annotation:Add annotation markers'\n");
        output.push_str("            '+branch:Add branch name'\n");
        output.push_str("            '+path:Add worktree path'\n");
        output.push_str("            '+size:Add disk size of worktree'\n");
        output.push_str("            '+base:Add ahead/behind base branch'\n");
        output.push_str("            '+changes:Add local changes'\n");
        output.push_str("            '+remote:Add ahead/behind remote'\n");
        output.push_str("            '+age:Add branch age'\n");
        output.push_str("            '+owner:Add branch owner'\n");
        output.push_str("            '+hash:Add commit hash'\n");
        output.push_str("            '+last-commit:Add last commit'\n");
        output.push_str("            '-annotation:Remove annotation markers'\n");
        output.push_str("            '-branch:Remove branch name'\n");
        output.push_str("            '-path:Remove worktree path'\n");
        output.push_str("            '-size:Remove disk size of worktree'\n");
        output.push_str("            '-base:Remove ahead/behind base branch'\n");
        output.push_str("            '-changes:Remove local changes'\n");
        output.push_str("            '-remote:Remove ahead/behind remote'\n");
        output.push_str("            '-age:Remove branch age'\n");
        output.push_str("            '-owner:Remove branch owner'\n");
        output.push_str("            '-hash:Remove commit hash'\n");
        output.push_str("            '-last-commit:Remove last commit'\n");
        output.push_str("        )\n");
        output.push_str("        _describe 'column' column_values\n");
        output.push_str("        return\n");
        output.push_str("    fi\n");
        output.push('\n');
        output.push_str("    # Sort column completion for --sort\n");
        output.push_str("    if [[ \"$prev_word\" == \"--sort\" ]]; then\n");
        output.push_str("        local -a sort_values\n");
        output.push_str("        sort_values=(\n");
        output.push_str("            'branch:Sort by branch name'\n");
        output.push_str("            'path:Sort by worktree path'\n");
        output.push_str("            'size:Sort by disk size'\n");
        output.push_str("            'base:Sort by total base divergence'\n");
        output.push_str("            'changes:Sort by total local changes'\n");
        output.push_str("            'remote:Sort by total remote divergence'\n");
        output.push_str("            'age:Sort by branch age'\n");
        output.push_str("            'owner:Sort by branch owner'\n");
        output.push_str("            'hash:Sort by commit hash'\n");
        output
            .push_str("            'activity:Sort by overall activity (commits + uncommitted)'\n");
        output.push_str("            'commit:Sort by last commit time only'\n");
        output.push_str("            '+branch:Sort by branch name ascending'\n");
        output.push_str("            '+path:Sort by worktree path ascending'\n");
        output.push_str("            '+size:Sort by disk size ascending'\n");
        output.push_str("            '+base:Sort by total base divergence ascending'\n");
        output.push_str("            '+changes:Sort by total local changes ascending'\n");
        output.push_str("            '+remote:Sort by total remote divergence ascending'\n");
        output.push_str("            '+age:Sort by branch age ascending'\n");
        output.push_str("            '+owner:Sort by branch owner ascending'\n");
        output.push_str("            '+hash:Sort by commit hash ascending'\n");
        output.push_str("            '+activity:Sort by overall activity ascending'\n");
        output.push_str("            '+commit:Sort by last commit time ascending'\n");
        output.push_str("            '-branch:Sort by branch name descending'\n");
        output.push_str("            '-path:Sort by worktree path descending'\n");
        output.push_str("            '-size:Sort by disk size descending'\n");
        output.push_str("            '-base:Sort by total base divergence descending'\n");
        output.push_str("            '-changes:Sort by total local changes descending'\n");
        output.push_str("            '-remote:Sort by total remote divergence descending'\n");
        output.push_str("            '-age:Sort by branch age descending'\n");
        output.push_str("            '-owner:Sort by branch owner descending'\n");
        output.push_str("            '-hash:Sort by commit hash descending'\n");
        output.push_str("            '-activity:Sort by overall activity descending'\n");
        output.push_str("            '-commit:Sort by last commit time descending'\n");
        output.push_str("        )\n");
        output.push_str("        _describe 'sort' sort_values\n");
        output.push_str("        return\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    output.push_str("    # Flag completions (extracted from clap)\n");
    output.push_str("    if [[ \"$curword\" == -* ]]; then\n");
    output.push_str("        local -a flags\n");
    output.push_str("        flags=(\n");

    // Use clap introspection to get flags
    let cmd =
        get_command_for_name(command_name).context(format!("Unknown command: {}", command_name))?;
    let (all_flags, _, _) = extract_flags(&cmd);

    for flag in all_flags {
        output.push_str(&format!("            '{}'\n", flag));
    }

    output.push_str("        )\n");
    output.push_str("        compadd -a flags\n");
    output.push_str("    fi\n");
    output.push_str("}\n");
    output.push('\n');

    // Wrapper for direct invocation (git-worktree-checkout)
    output.push_str(&format!("_{func_name}() {{\n"));
    output.push_str(&format!("    __{func_name}_impl\n"));
    output.push_str("}\n");
    output.push('\n');

    // Wrapper for git subcommand invocation (git worktree-checkout)
    // Git's completion system expects _git-<subcommand>
    // Skip for daft-* commands — they don't need git subcommand style completion
    if command_name.starts_with("git-") {
        let git_func_name = format!("_git-{}", command_name.trim_start_matches("git-"));
        output.push_str(&format!("{git_func_name}() {{\n"));
        output.push_str(&format!("    __{func_name}_impl\n"));
        output.push_str("}\n");
        output.push('\n');
    }

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

/// Generate a zsh completion script with rich grouped output for any command
/// that uses the `name\tgroup\tdescription` protocol.
fn generate_zsh_rich_completion(command_name: &str) -> String {
    let cmd = get_command_for_name(command_name)
        .unwrap_or_else(|| panic!("Unknown rich-completion command: {command_name}"));
    let (all_flags, _, _) = extract_flags(&cmd);
    let flags_block: String = all_flags
        .iter()
        .map(|f| format!("            '{f}'\n"))
        .collect();

    let func_name = command_name.replace('-', "_");
    let fetch_flag = if uses_fetch_on_miss(command_name) {
        " --fetch-on-miss"
    } else {
        ""
    };

    let mut output = format!(
        r#"#compdef {command_name}

__{func_name}_impl() {{
    local curword="${{words[$CURRENT]}}"
    local cword=$((CURRENT - 1))

    if [[ "$curword" == -* ]]; then
        local -a flags
        flags=(
{flags_block}        )
        compadd -a flags
        return
    fi

    local -a raw
    local -a wt_names wt_raw_names wt_ages wt_authors wt_paths
    local -a local_names local_ages local_authors
    local -a remote_names remote_ages remote_authors
    raw=(${{(f)"$(daft __complete {command_name} "$curword" --position "$cword"{fetch_flag} 2>/dev/null)"}})

    # First pass: collect names and descriptions per group.
    # Worktree lines have 5 fields: name\tworktree\tage\tauthor\tpath
    # Local/remote lines have 4 fields: name\tgroup\tage\tauthor
    # Worktree names may have *? dirty indicators — strip for completion
    # value, keep for display. (* and ? are invalid in git branch names.)
    local line name rest group desc max_len=0 len clean_name
    local age_len max_age_len=0 auth_len max_auth_len=0
    for line in "${{raw[@]}}"; do
        name="${{line%%$'\t'*}}"
        rest="${{line#*$'\t'}}"
        group="${{rest%%$'\t'*}}"
        desc="${{rest#*$'\t'}}"
        len=${{#name}}
        (( len > max_len )) && max_len=$len
        case "$group" in
            worktree)
                # Strip *? for completion value, keep raw for display
                clean_name="${{name%%[*?]*}}"
                wt_names+=("$clean_name")
                wt_raw_names+=("$name")
                # desc is "age\tauthor\tpath" — split on tabs
                wt_ages+=("${{desc%%$'\t'*}}")
                local wt_rest="${{desc#*$'\t'}}"
                wt_authors+=("${{wt_rest%%$'\t'*}}")
                wt_paths+=("${{wt_rest#*$'\t'}}")
                age_len=${{#${{desc%%$'\t'*}}}}
                (( age_len > max_age_len )) && max_age_len=$age_len
                auth_len=${{#${{wt_rest%%$'\t'*}}}}
                (( auth_len > max_auth_len )) && max_auth_len=$auth_len
                ;;
            local)
                local_names+=("$name")
                # desc is "age\tauthor"
                local_ages+=("${{desc%%$'\t'*}}")
                local_authors+=("${{desc#*$'\t'}}")
                age_len=${{#${{desc%%$'\t'*}}}}
                (( age_len > max_age_len )) && max_age_len=$age_len
                auth_len=${{#${{desc#*$'\t'}}}}
                (( auth_len > max_auth_len )) && max_auth_len=$auth_len
                ;;
            remote)
                remote_names+=("$name")
                # desc is "age\tauthor"
                remote_ages+=("${{desc%%$'\t'*}}")
                remote_authors+=("${{desc#*$'\t'}}")
                age_len=${{#${{desc%%$'\t'*}}}}
                (( age_len > max_age_len )) && max_age_len=$age_len
                auth_len=${{#${{desc#*$'\t'}}}}
                (( auth_len > max_auth_len )) && max_auth_len=$auth_len
                ;;
        esac
    done

    # Second pass: build padded display strings.
    # Worktrees: four columns (name, age, author, path) — uses raw name with indicators.
    # Local/remote: three columns (name, age, author).
    local -a wt_display local_display remote_display
    local i pad apad authpad
    (( max_len += 2 ))
    (( max_age_len += 2 ))
    (( max_auth_len += 2 ))
    for (( i=1; i<=${{#wt_names}}; i++ )); do
        pad=$(( max_len - ${{#wt_raw_names[$i]}} ))
        apad=$(( max_age_len - ${{#wt_ages[$i]}} ))
        authpad=$(( max_auth_len - ${{#wt_authors[$i]}} ))
        wt_display+=("${{wt_raw_names[$i]}}${{(l:$pad:: :)}}  ${{wt_ages[$i]}}${{(l:$apad:: :)}}  ${{wt_authors[$i]}}${{(l:$authpad:: :)}}  ${{wt_paths[$i]}}")
    done
    for (( i=1; i<=${{#local_names}}; i++ )); do
        pad=$(( max_len - ${{#local_names[$i]}} ))
        apad=$(( max_age_len - ${{#local_ages[$i]}} ))
        authpad=$(( max_auth_len - ${{#local_authors[$i]}} ))
        local_display+=("${{local_names[$i]}}${{(l:$pad:: :)}}  ${{local_ages[$i]}}${{(l:$apad:: :)}}  ${{local_authors[$i]}}")
    done
    for (( i=1; i<=${{#remote_names}}; i++ )); do
        pad=$(( max_len - ${{#remote_names[$i]}} ))
        apad=$(( max_age_len - ${{#remote_ages[$i]}} ))
        authpad=$(( max_auth_len - ${{#remote_authors[$i]}} ))
        remote_display+=("${{remote_names[$i]}}${{(l:$pad:: :)}}  ${{remote_ages[$i]}}${{(l:$apad:: :)}}  ${{remote_authors[$i]}}")
    done

    # -V preserves group insertion order: worktrees first, then local, then remote.
    (( ${{#wt_names}} ))     && compadd -V worktree -l -d wt_display -a wt_names
    (( ${{#local_names}} ))  && compadd -V local -l -d local_display -a local_names
    (( ${{#remote_names}} )) && compadd -V remote -l -d remote_display -a remote_names
}}

_{func_name}() {{
    __{func_name}_impl
}}

compdef _{func_name} {command_name}
"#
    );

    // Wrapper for git subcommand invocation (git worktree-checkout).
    // Git's completion system expects _git-<subcommand>.
    if command_name.starts_with("git-") {
        let git_func_name = format!("_git-{}", command_name.trim_start_matches("git-"));
        output.push_str(&format!("{git_func_name}() {{\n"));
        output.push_str(&format!("    __{func_name}_impl\n"));
        output.push_str("}\n\n");
    }

    // Register completions for shortcut aliases
    for shortcut in crate::shortcuts::SHORTCUTS {
        if shortcut.command == command_name {
            output.push_str(&format!("compdef _{func_name} {}\n", shortcut.alias));
        }
    }

    output
}

pub(super) const DAFT_ZSH_COMPLETIONS: &str = r#"# daft subcommand completions
_daft() {
    local curword="${words[$CURRENT]}"

    # hooks: subcommand and argument completion
    if (( CURRENT >= 3 )) && [[ "$words[2]" == "hooks" ]]; then
        # hooks subcommand completion (position 3)
        if (( CURRENT == 3 )); then
            compadd trust prompt deny status migrate install validate dump run jobs
            _files -/
            return
        fi

        # hooks subcommand arguments (position 4+)
        case "$words[3]" in
            run)
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
                    compadd -- --job --tag --dry-run -v --verbose -h --help
                    return
                fi
                local -a hooks
                hooks=(${(f)"$(daft __complete hooks-run "$curword" 2>/dev/null)"})
                compadd -a hooks
                return
                ;;
            status)
                if [[ "$curword" == -* ]]; then
                    compadd -- -s --short -h --help
                    return
                fi
                _files -/
                return
                ;;
            prompt|deny)
                if [[ "$curword" == -* ]]; then
                    compadd -- -f --force -h --help
                    return
                fi
                _files -/
                return
                ;;
            trust)
                if (( CURRENT == 4 )); then
                    if [[ "$curword" == -* ]]; then
                        compadd -- -f --force -h --help
                        return
                    fi
                    compadd list reset prune
                    _files -/
                    return
                fi
                if (( CURRENT == 5 )) && [[ "$words[4]" == "reset" ]]; then
                    if [[ "$curword" == -* ]]; then
                        compadd -- -f --force -h --help
                        return
                    fi
                    compadd all
                    _files -/
                    return
                fi
                return
                ;;
            migrate)
                if [[ "$curword" == -* ]]; then
                    compadd -- --dry-run -h --help
                fi
                return
                ;;
            jobs)
                if (( CURRENT == 4 )); then
                    compadd logs cancel retry clean
                    return
                fi
                case "$words[4]" in
                    logs|cancel)
                        if [[ "$curword" == -* ]]; then
                            compadd -- --inv -h --help
                            return
                        fi
                        local -a _vals _descs
                        local _line
                        while IFS='' read -r _line; do
                            _vals+=("${_line%%$'\t'*}")
                            _descs+=("${_line//$'\t'/  }")
                        done < <(daft __complete hooks-jobs-job "$curword" 2>/dev/null)
                        compadd -l -d _descs -a _vals
                        return
                        ;;
                    retry)
                        if [[ "$curword" == -* ]]; then
                            compadd -- --hook --inv --job -h --help
                            return
                        fi
                        local -a _vals _descs
                        while IFS='' read -r _line; do
                            _vals+=("${_line%%$'\t'*}")
                            _descs+=("${_line//$'\t'/  }")
                        done < <(daft __complete hooks-jobs-retry "$curword" 2>/dev/null)
                        compadd -l -d _descs -a _vals
                        return
                        ;;
                esac
                if [[ "$curword" == -* ]]; then
                    compadd -- --all --json -h --help
                fi
                return
                ;;
        esac
        return
    fi

    # layout: complete subcommands and arguments
    if (( CURRENT >= 3 )) && [[ "$words[2]" == "layout" ]]; then
        if (( CURRENT == 3 )); then
            compadd default list show transform
            return
        fi
        case "$words[3]" in
            show)
                _files -/
                return
                ;;
            transform|default)
                if [[ "$curword" == -* ]]; then
                    if [[ "$words[3]" == "transform" ]]; then
                        compadd -- --force -f --dry-run --include --include-all -h --help
                    else
                        compadd -- --reset -h --help
                    fi
                    return
                fi
                local -a layouts
                layouts=("${(@f)$(daft __complete layout-$words[3] "$curword" 2>/dev/null | sed 's/\t/:/')}")
                _describe 'layout' layouts
                return
                ;;
        esac
        return
    fi

    # multi-remote: complete subcommands
    if (( CURRENT == 3 )) && [[ "$words[2]" == "multi-remote" ]]; then
        compadd enable disable status set-default move
        return
    fi

    # config: complete subcommands
    if (( CURRENT == 3 )) && [[ "$words[2]" == "config" ]]; then
        compadd remote-sync
        return
    fi

    # shared: complete subcommands and their arguments
    if [[ "$words[2]" == "shared" ]]; then
        if (( CURRENT == 3 )); then
            compadd add link manage materialize remove status sync
            return
        fi
        local shared_sub="$words[3]"
        case "$shared_sub" in
            add)
                if [[ "$curword" == -* ]]; then
                    compadd -- --declare --help -h
                else
                    _files
                fi
                return
                ;;
            remove)
                if [[ "$curword" == -* ]]; then
                    compadd -- --delete --help -h
                else
                    local -a shared_files
                    shared_files=(${(f)"$(daft __complete shared-files "$curword" 2>/dev/null)"})
                    compadd -- $shared_files
                fi
                return
                ;;
            link|materialize)
                if [[ "$curword" == -* ]]; then
                    compadd -- --override --help -h
                elif (( CURRENT == 4 )); then
                    local -a shared_files
                    shared_files=(${(f)"$(daft __complete shared-files "$curword" 2>/dev/null)"})
                    compadd -- $shared_files
                elif (( CURRENT == 5 )); then
                    local -a worktrees
                    worktrees=(${(f)"$(daft __complete shared-worktrees "$curword" 2>/dev/null)"})
                    compadd -- $worktrees
                fi
                return
                ;;
            status|sync)
                if [[ "$curword" == -* ]]; then
                    compadd -- --help -h
                fi
                return
                ;;
        esac
    fi

    # verb aliases: delegate to underlying command completions
    if (( CURRENT >= 3 )); then
        case "$words[2]" in
            go)
                words=("daft-go" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __daft_go_impl
                return
                ;;
            start)
                words=("daft-start" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __daft_start_impl
                return
                ;;
            carry)
                words=("git-worktree-carry" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_carry_impl
                return
                ;;
            update)
                words=("git-worktree-fetch" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_fetch_impl
                return
                ;;
            rename)
                words=("daft-rename" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __daft_rename_impl
                return
                ;;
            sync)
                words=("git-worktree-sync" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_sync_impl
                return
                ;;
            remove)
                words=("daft-remove" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __daft_remove_impl
                return
                ;;
            list)
                words=("git-worktree-list" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_list_impl
                return
                ;;
            prune)
                words=("git-worktree-prune" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_prune_impl
                return
                ;;
            clone)
                words=("git-worktree-clone" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_clone_impl
                return
                ;;
            init)
                words=("git-worktree-init" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_init_impl
                return
                ;;
        esac
    fi

    # top-level: complete daft subcommands and flags
    if (( CURRENT == 2 )); then
        if [[ "$curword" == -* ]]; then
            compadd -- --version -V --help -h
        else
            compadd hooks shell-init setup multi-remote release-notes doctor layout shared \
                    config clone init go start carry update list prune rename sync remove adopt eject
        fi
        return
    fi
}
compdef _daft daft
compdef _daft git-daft
"#;
