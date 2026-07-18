use super::{
    allows_path_completion, command_has_repo_flag, command_has_repo_positional, emit_formats_for,
    extract_flags, get_command_for_name, uses_fetch_on_miss, uses_rich_completions,
};
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
    // Value completion for --skip-hooks flag
    let has_skip_hooks = matches!(
        command_name,
        "git-worktree-checkout"
            | "git-worktree-clone"
            | "git-worktree-flow-adopt"
            | "daft-go"
            | "daft-start"
    );

    // Value completion for --repo flag (catalog repo names)
    let has_repo_flag = command_has_repo_flag(command_name);

    // Emit the prev_word variable once if any prev-based completion is needed
    if has_branch_completions || has_layout || has_skip_hooks || has_repo_flag {
        output.push_str("    local prev_word=\"${words[$((CURRENT-1))]}\"\n");
    }

    if has_repo_flag {
        output.push_str("    # Catalog repo-name completion for --repo\n");
        output.push_str("    if [[ \"$prev_word\" == \"--repo\" ]]; then\n");
        output.push_str("        local -a repos\n");
        output.push_str(
            "        repos=( ${(f)\"$(daft __complete repo-name \"$curword\" 2>/dev/null | cut -f1)\"} )\n",
        );
        // The helper case-folds the prefix; a case-insensitive match spec keeps
        // zsh from re-dropping the folded candidates.
        output.push_str(
            "        (( ${#repos} )) && compadd -M 'm:{[:lower:][:upper:]}={[:upper:][:lower:]}' -- \"${repos[@]}\"\n",
        );
        output.push_str("        return\n");
        output.push_str("    fi\n");
        output.push('\n');
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

    if has_skip_hooks {
        output.push_str("    # Skip-hooks selector completion for --skip-hooks\n");
        output.push_str("    if [[ \"$prev_word\" == \"--skip-hooks\" ]]; then\n");
        output.push_str("        local -a selectors\n");
        output.push_str("        selectors=(\"${(@f)$(daft __complete skip-hooks-value \"$curword\" 2>/dev/null | sed 's/\\t/:/')}\")\n");
        output.push_str("        _describe 'skip-hooks selector' selectors\n");
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
        output.push_str("            'pr:Pull/merge request'\n");
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
        output.push_str("            '+pr:Add pull/merge request'\n");
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
        output.push_str("            '-pr:Remove pull/merge request'\n");
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

    // Value completion for --format flag (emit-enabled commands only)
    if let Some(formats) = emit_formats_for(command_name) {
        let format_list = formats.join(" ");
        if !has_branch_completions && !has_layout && !has_columns {
            output.push_str("    local prev_word=\"${words[$((CURRENT-1))]}\"\n");
        }
        output.push_str("    # Format value completion for --format\n");
        output.push_str("    if [[ \"$prev_word\" == \"--format\" ]]; then\n");
        output.push_str("        local -a format_values\n");
        output.push_str(&format!("        format_values=( {format_list} )\n"));
        output.push_str("        compadd -a format_values\n");
        output.push_str("        return\n");
        output.push_str("    fi\n");
        output.push('\n');
    }

    // Positional repo-name completion (daft list [<repo>]). Placed after
    // every value-flag prev block (each returns on match) so flag values
    // never receive repo names; --template and --stat have no prev block,
    // so they are excluded explicitly.
    if command_has_repo_positional(command_name) {
        output.push_str("    # Positional cataloged-repo completion\n");
        output.push_str("    local prev_word=\"${words[$((CURRENT-1))]}\"\n");
        output.push_str(
            "    if [[ \"$curword\" != -* && \"$prev_word\" != \"--template\" && \"$prev_word\" != \"--stat\" ]]; then\n",
        );
        output.push_str("        local -a repos\n");
        output.push_str(
            "        repos=( ${(f)\"$(daft __complete repo-name \"$curword\" 2>/dev/null | cut -f1)\"} )\n",
        );
        // The helper case-folds the prefix; a case-insensitive match spec keeps
        // zsh from re-dropping the folded candidates.
        output.push_str(
            "        (( ${#repos} )) && compadd -M 'm:{[:lower:][:upper:]}={[:upper:][:lower:]}' -- \"${repos[@]}\"\n",
        );
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
    // Path-accepting commands (daft-remove, daft-rename) also offer directory
    // completion. A path-like prefix (./, ../, /, ~/) skips the dynamic branch
    // source entirely; otherwise directories are appended after the branch
    // groups so a single Tab works in both inside-repo and outside-repo cases.
    let path_pre = if allows_path_completion(command_name) {
        r#"    case "$curword" in
        /*|./*|../*|~/*|~)
            _files -/
            return
            ;;
    esac
"#
    } else {
        ""
    };
    let path_post = if allows_path_completion(command_name) {
        "    _files -/\n"
    } else {
        ""
    };
    // Rich commands that also carry --skip-hooks (checkout, go) complete its
    // selector vocabulary when the previous word is the flag.
    let skip_hooks_pre = if matches!(command_name, "git-worktree-checkout" | "daft-go") {
        "    if [[ \"${words[$((CURRENT-1))]}\" == \"--skip-hooks\" ]]; then\n        local -a selectors\n        selectors=(\"${(@f)$(daft __complete skip-hooks-value \"$curword\" 2>/dev/null | sed 's/\\t/:/')}\")\n        _describe 'skip-hooks selector' selectors\n        return\n    fi\n\n"
    } else {
        ""
    };

    // Value completion for --repo flag (catalog repo names)
    let repo_flag_pre = if command_has_repo_flag(command_name) {
        "    if [[ \"${words[$((CURRENT-1))]}\" == \"--repo\" ]]; then\n        local -a repos\n        repos=( ${(f)\"$(daft __complete repo-name \"$curword\" 2>/dev/null | cut -f1)\"} )\n        (( ${#repos} )) && compadd -M 'm:{[:lower:][:upper:]}={[:upper:][:lower:]}' -- \"${repos[@]}\"\n        return\n    fi\n\n"
    } else {
        ""
    };

    // daft-go position 2 completes branches of the repo named at position 1;
    // the __complete protocol only carries the current word, so pass the
    // first positional via env.
    let env_prefix = if command_name == "daft-go" {
        r#"DAFT_COMPLETE_GO_FIRST="$words[2]" "#
    } else {
        ""
    };

    let mut output = format!(
        r#"#compdef {command_name}

__{func_name}_impl() {{
    local curword="${{words[$CURRENT]}}"
    local cword=$((CURRENT - 1))

{repo_flag_pre}{skip_hooks_pre}    if [[ "$curword" == -* ]]; then
        local -a flags
        flags=(
{flags_block}        )
        compadd -a flags
        return
    fi

{path_pre}    local -a raw
    local -a wt_names wt_raw_names wt_ages wt_authors wt_paths
    local -a local_names local_ages local_authors
    local -a remote_names remote_ages remote_authors
    local -a repo_names repo_paths
    local -a forge_names forge_descs forge_tok_names forge_tok_descs
    raw=(${{(f)"$({env_prefix}daft __complete {command_name} "$curword" --position "$cword"{fetch_flag} 2>/dev/null)"}})

    # First pass: collect names and descriptions per group.
    # Worktree lines have 5 fields: name\tworktree\tage\tauthor\tpath
    # Local/remote lines have 4 fields: name\tgroup\tage\tauthor
    # Catalog repo lines have 3 fields: name\trepo\tpath
    # Forge lines have 3 fields: name\tforge\tdescription — the bare pr:/mr:
    # syntax tokens go in their own group so they can complete without a
    # trailing space (the user keeps typing the number after the colon).
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
            repo)
                repo_names+=("$name")
                # desc is the repo's display path
                repo_paths+=("$desc")
                ;;
            forge)
                if [[ "$name" == "pr:" || "$name" == "mr:" ]]; then
                    forge_tok_names+=("$name")
                    forge_tok_descs+=("$desc")
                else
                    forge_names+=("$name")
                    forge_descs+=("$desc")
                fi
                ;;
        esac
    done

    # Second pass: build padded display strings.
    # Worktrees: four columns (name, age, author, path) — uses raw name with indicators.
    # Local/remote: three columns (name, age, author).
    # Catalog repos and forge targets: two columns (name, description).
    local -a wt_display local_display remote_display repo_display forge_display forge_tok_display
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
    for (( i=1; i<=${{#repo_names}}; i++ )); do
        pad=$(( max_len - ${{#repo_names[$i]}} ))
        repo_display+=("${{repo_names[$i]}}${{(l:$pad:: :)}}  ${{repo_paths[$i]}}")
    done
    for (( i=1; i<=${{#forge_names}}; i++ )); do
        pad=$(( max_len - ${{#forge_names[$i]}} ))
        forge_display+=("${{forge_names[$i]}}${{(l:$pad:: :)}}  ${{forge_descs[$i]}}")
    done
    for (( i=1; i<=${{#forge_tok_names}}; i++ )); do
        pad=$(( max_len - ${{#forge_tok_names[$i]}} ))
        forge_tok_display+=("${{forge_tok_names[$i]}}${{(l:$pad:: :)}}  ${{forge_tok_descs[$i]}}")
    done

    # -V preserves group insertion order: worktrees first, then local, then
    # remote, then catalog repos (cross-repo navigation), then forge PR/MR
    # targets. The catalog-repo group matches case-insensitively (a repo-name
    # convenience); branch groups stay case-sensitive, since git refs are. The
    # pr:/mr: syntax tokens complete suffix-free (-S '') so the accepted token
    # stays glued to the number the user types next.
    (( ${{#wt_names}} ))     && compadd -V worktree -l -d wt_display -a wt_names
    (( ${{#local_names}} ))  && compadd -V local -l -d local_display -a local_names
    (( ${{#remote_names}} )) && compadd -V remote -l -d remote_display -a remote_names
    (( ${{#repo_names}} ))   && compadd -M 'm:{{[:lower:][:upper:]}}={{[:upper:][:lower:]}}' -V repo -l -d repo_display -a repo_names
    (( ${{#forge_names}} ))  && compadd -V forge -l -d forge_display -a forge_names
    (( ${{#forge_tok_names}} )) && compadd -V forge-syntax -S '' -l -d forge_tok_display -a forge_tok_names
{path_post}}}

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

    # `-C <path>` is a top-level option (issue #519). If the previous token is
    # `-C`, complete directories for its value and stop.
    local __daft_prev="${words[$((CURRENT-1))]}"
    if [[ "$__daft_prev" == "-C" ]]; then
        _files -/
        return
    fi

    # Strip leading `-C <path>` pairs from words/CURRENT so the rest of the
    # completion logic sees argv as if `-C` weren't there.
    local __daft_skip=2
    while [[ "${words[$__daft_skip]}" == "-C" ]] && (( __daft_skip + 1 <= ${#words} )); do
        __daft_skip=$((__daft_skip + 2))
    done
    if (( __daft_skip > 2 )); then
        words=("${words[1]}" "${(@)words[$__daft_skip,-1]}")
        CURRENT=$((CURRENT - (__daft_skip - 2)))
        if (( CURRENT < 2 )); then CURRENT=2; fi
        curword="${words[$CURRENT]}"
    fi

    # --format value completion (emit-enabled subcommand paths)
    local _fmt_prev="${words[$((CURRENT-1))]}"
    if [[ "$_fmt_prev" == "--format" ]]; then
        local _fmt_path="" _fmt_i _fmt_w
        for ((_fmt_i=2; _fmt_i<CURRENT; _fmt_i++)); do
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
                compadd json ndjson tsv csv yaml toon markdown
                return
                ;;
            release-notes|"multi-remote status"|"hooks run")
                compadd json yaml toon markdown
                return
                ;;
        esac
    fi

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
                    # When the user is typing a flag (`--w<TAB>`), offer the
                    # listing-form flags instead of subcommands — the existing
                    # CURRENT > 4 fall-through (line ~575) only handles the
                    # case where a subcommand has already been chosen.
                    if [[ "$curword" == -* ]]; then
                        compadd -- --all --format --template --no-headers --worktree --status --hook -h --help
                    else
                        compadd logs cancel retry prune
                    fi
                    return
                fi
                case "$words[4]" in
                    logs|cancel)
                        if [[ "$curword" == -* ]]; then
                            compadd -- --inv -h --help
                            return
                        fi
                        # KIND-tagged input: each line is `KIND\t<value>\t<display>`.
                        # Split into per-kind arrays so we can emit one
                        # `compadd -V <kind>` per group (job names cluster
                        # together, then invocations, then worktrees).
                        local -a _job_v _job_d _inv_v _inv_d _wt_v _wt_d
                        local _line _kind _rest _val _disp
                        while IFS='' read -r _line; do
                            _kind="${_line%%$'\t'*}"
                            _rest="${_line#*$'\t'}"
                            _val="${_rest%%$'\t'*}"
                            _disp="${_rest#*$'\t'}"
                            case "$_kind" in
                                JOB) _job_v+=("$_val"); _job_d+=("$_disp") ;;
                                INV) _inv_v+=("$_val"); _inv_d+=("$_disp") ;;
                                WT)  _wt_v+=("$_val");  _wt_d+=("$_disp") ;;
                            esac
                        done < <(daft __complete hooks-jobs-job "$curword" 2>/dev/null)
                        # -V <name> creates an order-preserving group; the
                        # menu shows job names first, then invocations,
                        # then worktrees (matches user's most-likely target).
                        (( ${#_job_v} )) && compadd -V job -l -d _job_d -a _job_v
                        (( ${#_inv_v} )) && compadd -V inv -l -d _inv_d -a _inv_v
                        (( ${#_wt_v}  )) && compadd -V wt  -l -d _wt_d  -a _wt_v
                        return
                        ;;
                    retry)
                        local prev="$words[$((CURRENT-1))]"
                        if [[ "$prev" == "--worktree" ]]; then
                            local -a _vals _descs
                            while IFS='' read -r _line; do
                                _vals+=("${_line%%$'\t'*}")
                                _descs+=("${_line//$'\t'/  }")
                            done < <(daft __complete hooks-jobs-retry-worktree "$curword" 2>/dev/null)
                            compadd -l -d _descs -a _vals
                            return
                        fi
                        if [[ "$curword" == -* ]]; then
                            compadd -- --hook --inv --job --worktree --cwd -h --help
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
                local prev="$words[$((CURRENT-1))]"
                if [[ "$prev" == "--worktree" ]]; then
                    local -a _vals _descs
                    while IFS='' read -r _line; do
                        _vals+=("${_line%%$'\t'*}")
                        _descs+=("${_line//$'\t'/  }")
                    done < <(daft __complete hooks-jobs-worktree "$curword" 2>/dev/null)
                    compadd -l -d _descs -a _vals
                    return
                fi
                if [[ "$prev" == "--status" ]]; then
                    compadd -- failed completed running cancelled skipped
                    return
                fi
                if [[ "$prev" == "--hook" ]]; then
                    local -a _vals _descs
                    while IFS='' read -r _line; do
                        _vals+=("${_line%%$'\t'*}")
                        _descs+=("${_line//$'\t'/  }")
                    done < <(daft __complete hooks-jobs-hook-filter "$curword" 2>/dev/null)
                    compadd -l -d _descs -a _vals
                    return
                fi
                if [[ "$curword" == -* ]]; then
                    compadd -- --all --format --template --no-headers --worktree --status --hook -h --help
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

    # repo: complete subcommands and arguments
    if (( CURRENT >= 3 )) && [[ "$words[2]" == "repo" ]]; then
        if (( CURRENT == 3 )); then
            compadd add info install link list remove unlink
            return
        fi
        case "$words[3]" in
            add)
                if [[ "$curword" == -* ]]; then
                    compadd -- --name -q --quiet -v --verbose -h --help
                    return
                fi
                _files -/
                return
                ;;
            info)
                if [[ "$curword" == -* ]]; then
                    compadd -- --format --template --no-headers -h --help
                    return
                fi
                # Catalog repo names first, then directories (`repo info .`,
                # a subdirectory, or any worktree resolves to its repo).
                local -a repos
                repos=( ${(f)"$(daft __complete repo-name "$curword" 2>/dev/null | cut -f1)"} )
                (( ${#repos} )) && compadd -M 'm:{[:lower:][:upper:]}={[:upper:][:lower:]}' -- "${repos[@]}"
                _files -/
                return
                ;;
            install)
                if [[ "$curword" == -* ]]; then
                    compadd -- -q --quiet -v --verbose --git-exclude -h --help
                fi
                return
                ;;
            link)
                local prev_word="${words[$((CURRENT-1))]}"
                if [[ "$prev_word" == "--name" || "$prev_word" == "--kind" ]]; then
                    return
                fi
                if [[ "$curword" == -* ]]; then
                    compadd -- --name --kind -h --help
                    return
                fi
                local -a repos
                repos=( ${(f)"$(daft __complete repo-name "$curword" 2>/dev/null | cut -f1)"} )
                (( ${#repos} )) && compadd -M 'm:{[:lower:][:upper:]}={[:upper:][:lower:]}' -- "${repos[@]}"
                _files -/
                return
                ;;
            list)
                local prev_word="${words[$((CURRENT-1))]}"
                if [[ "$prev_word" == "--columns" ]]; then
                    local -a column_values
                    column_values=(
                        'annotation:Current repo marker'
                        'name:Catalog name'
                        'worktrees:Worktree count'
                        'layout:Worktree layout'
                        'branch:Default branch'
                        'path:Repository path'
                        'size:Disk size of repository'
                        'remote:Remote URL'
                        '+annotation:Add current repo marker'
                        '+name:Add catalog name'
                        '+worktrees:Add worktree count'
                        '+layout:Add worktree layout'
                        '+branch:Add default branch'
                        '+path:Add repository path'
                        '+size:Add disk size of repository'
                        '+remote:Add remote URL'
                        '-annotation:Remove current repo marker'
                        '-name:Remove catalog name'
                        '-worktrees:Remove worktree count'
                        '-layout:Remove worktree layout'
                        '-branch:Remove default branch'
                        '-path:Remove repository path'
                        '-size:Remove disk size of repository'
                        '-remote:Remove remote URL'
                    )
                    _describe 'column' column_values
                    return
                fi
                if [[ "$curword" == -* ]]; then
                    compadd -- -a --all -w --worktrees --columns --format --template --no-headers -q --quiet -h --help
                fi
                return
                ;;
            remove)
                local prev_word="${words[$((CURRENT-1))]}"
                if [[ "$prev_word" == "--repo" ]]; then
                    local -a repos
                    repos=( ${(f)"$(daft __complete repo-name "$curword" 2>/dev/null | cut -f1)"} )
                    (( ${#repos} )) && compadd -M 'm:{[:lower:][:upper:]}={[:upper:][:lower:]}' -- "${repos[@]}"
                    return
                fi
                if [[ "$curword" == -* ]]; then
                    compadd -- --repo --keep-files -y --force --dry-run -v --verbose -h --help
                    return
                fi
                _files -/
                return
                ;;
            unlink)
                if [[ "$curword" == -* ]]; then
                    compadd -- -h --help
                    return
                fi
                local -a labels
                labels=( ${(f)"$(daft __complete relation-label "$curword" 2>/dev/null)"} )
                (( ${#labels} )) && compadd -- "${labels[@]}"
                return
                ;;
        esac
        return
    fi

    # skill: complete subcommands and arguments
    if (( CURRENT >= 3 )) && [[ "$words[2]" == "skill" ]]; then
        if (( CURRENT == 3 )); then
            compadd install uninstall show
            return
        fi
        case "$words[3]" in
            install|uninstall)
                local prev_word="${words[$((CURRENT-1))]}"
                if [[ "$prev_word" == "--dir" ]]; then
                    _files -/
                    return
                fi
                if [[ "$curword" == -* ]]; then
                    compadd -- --project --dir -q --quiet -v --verbose -h --help
                fi
                return
                ;;
            show)
                if [[ "$curword" == -* ]]; then
                    compadd -- --no-pager -h --help
                fi
                return
                ;;
        esac
        return
    fi

    # config: complete subcommands
    if (( CURRENT == 3 )) && [[ "$words[2]" == "config" ]]; then
        compadd remote-sync
        return
    fi

    # file: complete subcommands and arguments
    if [[ "$words[2]" == "file" ]]; then
        if (( CURRENT == 3 )); then
            compadd merge
            return
        fi
        if [[ "$words[3]" == "merge" ]]; then
            if [[ "$curword" == -* ]]; then
                compadd -- --keep-source -y --yes -h --help
            else
                _files
            fi
            return
        fi
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

    # merge: flag + branch completion (inline; not auto-generated from COMMANDS)
    if (( CURRENT >= 3 )) && { [[ "$words[2]" == "merge" ]] || [[ "$words[2]" == "worktree-merge" ]] }; then
        local prev_word="${words[$((CURRENT-1))]}"
        # --into takes a branch value
        if [[ "$prev_word" == "--into" ]]; then
            local -a branches
            branches=(${(f)"$(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null)"})
            compadd -a branches
            return
        fi
        # --cleanup values
        if [[ "$prev_word" == "--cleanup" ]]; then
            compadd default scissors strip verbatim whitespace
            return
        fi
        # --strategy / -s values
        if [[ "$prev_word" == "--strategy" || "$prev_word" == "-s" ]]; then
            compadd ours recursive resolve octopus subtree
            return
        fi
        if [[ "$curword" == -* ]]; then
            local -a merge_flags
            merge_flags=(
                '--into' '--abort' '--continue' '--quit'
                '--adopt-target' '--no-adopt-target'
                '-y' '--yes'
                '--merge' '--squash' '--rebase' '--rebase-merge'
                '-r' '--remove-branch' '--keep-branch'
                '--set-default'
                '-m' '-F' '--file' '--edit' '--no-edit' '--cleanup'
                '--commit' '--no-commit'
                '--signoff' '--no-signoff'
                '-s' '--strategy' '-X' '--strategy-option'
                '-S' '--gpg-sign' '--no-gpg-sign'
                '--verify-signatures' '--no-verify-signatures'
                '--allow-unrelated-histories'
                '--stat' '-n' '--no-stat'
                '-v' '--verbose' '-h' '--help' '-V' '--version'
            )
            compadd -a merge_flags
            return
        fi
        # Positional source/target: branch names
        local -a branches
        branches=(${(f)"$(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes 2>/dev/null)"})
        compadd -a branches
        return
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
            exec)
                words=("git-worktree-exec" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_exec_impl
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
            compadd -- --version -V --help -h -C
        else
            compadd activate hooks shell-init multi-remote release-notes doctor layout shared \
                    config file repo skill clone init install go start carry exec update list prune rename sync remove \
                    merge worktree-merge adopt eject
        fi
        return
    fi
}
compdef _daft daft
compdef _daft git-daft
"#;
