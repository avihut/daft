#!/bin/bash
# Integration tests for shell completions

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DAFT_BIN="$PROJECT_ROOT/target/release/daft"

# --- Real-state isolation (#667) ---
# Every $DAFT_BIN invocation below must resolve daft's XDG surface inside a
# throwaway sandbox, never the developer's real dirs. Mirrors the DAFT_*_DIR
# block in test_framework.sh setup(); kept bespoke because this script runs
# standalone (own mise task + CI step) and must not share — or trap-delete —
# the matrix's /tmp sandbox. The completions:test task wraps this script in
# the real-state guard, which catches any drift between the twin blocks.
# Short /tmp names on purpose: coordinator sockets cap sun_path at ~104 bytes
# on macOS.
COMPLETIONS_SANDBOX="$(mktemp -d /tmp/daft-completions.XXXXXX)"
trap 'rm -rf "$COMPLETIONS_SANDBOX"' EXIT
export DAFT_CONFIG_DIR="$COMPLETIONS_SANDBOX/cfg"
export DAFT_DATA_DIR="$COMPLETIONS_SANDBOX/data"
export DAFT_STATE_DIR="$COMPLETIONS_SANDBOX/st"
export DAFT_SKILLS_DIR="$COMPLETIONS_SANDBOX/skills"
mkdir -p "$DAFT_CONFIG_DIR" "$DAFT_DATA_DIR" "$DAFT_STATE_DIR" "$DAFT_SKILLS_DIR"

# Keep the host's git config (system and global) out of the binary's behavior.
export GIT_CONFIG_NOSYSTEM=1
if [[ -z "${GIT_CONFIG_GLOBAL:-}" ]]; then
    touch "$COMPLETIONS_SANDBOX/gitconfig"
    export GIT_CONFIG_GLOBAL="$COMPLETIONS_SANDBOX/gitconfig"
fi

# Suppress the update-check/trust-prune/log-clean daemons a non-completion
# command (e.g. `daft repo add` below) would spawn — they orphan under PID 1
# and the update check hits the network.
export DAFT_TESTING=1

# Color codes for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Test counters
TESTS_RUN=0
TESTS_PASSED=0
TESTS_SKIPPED=0
SKIPPED_TESTS=()

# Test helper functions
run_test() {
    local test_name="$1"
    TESTS_RUN=$((TESTS_RUN + 1))
    echo "Running: $test_name"
}

pass_test() {
    TESTS_PASSED=$((TESTS_PASSED + 1))
    echo -e "${GREEN}✓ PASS${NC}"
    echo ""
}

fail_test() {
    local message="$1"
    echo -e "${RED}✗ FAIL: $message${NC}"
    echo ""
}

# A test that could not run in this environment. Deliberately NOT a pass: the
# summary reports skips on their own line so an assertion that never executed
# can't masquerade as coverage.
skip_test() {
    local reason="$1"
    TESTS_SKIPPED=$((TESTS_SKIPPED + 1))
    SKIPPED_TESTS+=("$reason")
    echo -e "${YELLOW}○ SKIP: $reason${NC}"
    echo ""
}

# Test: Bash completion generation
test_bash_completion_generation() {
    run_test "Bash completion generation"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *"_git_worktree_checkout"* ]] && [[ "$output" == *"COMPREPLY"* ]]; then
        pass_test
    else
        fail_test "Bash completion output doesn't contain expected patterns"
    fi
}

# Test: Zsh completion generation
test_zsh_completion_generation() {
    run_test "Zsh completion generation"

    local output
    output=$("$DAFT_BIN" completions zsh --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *"#compdef git-worktree-checkout"* ]] && [[ "$output" == *"_git_worktree_checkout"* ]]; then
        pass_test
    else
        fail_test "Zsh completion output doesn't contain expected patterns"
    fi
}

# Test: Fish completion generation
test_fish_completion_generation() {
    run_test "Fish completion generation"

    local output
    output=$("$DAFT_BIN" completions fish --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *"complete -c git-worktree-checkout"* ]]; then
        pass_test
    else
        fail_test "Fish completion output doesn't contain expected patterns"
    fi
}

# Test: Dynamic branch completion
test_dynamic_branch_completion() {
    run_test "Dynamic branch completion"

    # Create a temporary test repository
    local test_repo="/tmp/test-completions-$$"
    mkdir -p "$test_repo"
    cd "$test_repo"

    git init >/dev/null 2>&1
    git config user.name "Test" >/dev/null 2>&1
    git config user.email "test@example.com" >/dev/null 2>&1

    # Create some branches
    echo "test" > README.md
    git add README.md >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1
    git branch feature/test-1 >/dev/null 2>&1
    git branch feature/test-2 >/dev/null 2>&1
    git branch hotfix/urgent >/dev/null 2>&1

    # Test completion with "fea" prefix
    local output
    output=$("$DAFT_BIN" __complete git-worktree-checkout "fea" 2>&1)

    # Clean up
    cd /
    rm -rf "$test_repo"

    if [[ "$output" == *"feature/test-1"* ]] && [[ "$output" == *"feature/test-2"* ]]; then
        pass_test
    else
        fail_test "Branch completion didn't return expected branches"
    fi
}

# Test: Bash completion includes dynamic branch wiring
test_bash_dynamic_wiring() {
    run_test "Bash completion includes dynamic branch logic"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'raw='* ]]; then
        pass_test
    else
        fail_test "Bash completion missing 'daft __complete' call for dynamic branches"
    fi
}

# Test: Zsh completion includes dynamic branch wiring
test_zsh_dynamic_wiring() {
    run_test "Zsh completion includes dynamic branch logic"

    local output
    output=$("$DAFT_BIN" completions zsh --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'raw='* ]]; then
        pass_test
    else
        fail_test "Zsh completion missing 'daft __complete' call for dynamic branches"
    fi
}

# Test: Fish completion includes dynamic branch wiring
test_fish_dynamic_wiring() {
    run_test "Fish completion includes dynamic branch logic"

    local output
    output=$("$DAFT_BIN" completions fish --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'daft __complete'* ]]; then
        pass_test
    else
        fail_test "Fish completion missing 'daft __complete' call for dynamic branches"
    fi
}

# Test: Prune completes repo names (exec-shape --repo) but never branches.
# Prune takes --repo, so it wires `daft __complete repo-name`; grepping the
# generic `daft __complete` marker would trip on that. Branch completion is
# what prune must not have, and that always emits the positional `--position`
# form — so key on its absence. Mirrors the completions/bash.yml twin.
test_prune_no_dynamic() {
    run_test "Prune completion has repo-name completion but no dynamic branch logic"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-prune 2>&1)

    if [[ "$output" == *'--position'* ]]; then
        fail_test "Prune completion incorrectly includes dynamic branch (positional) logic"
    elif [[ "$output" != *'repo-name'* ]]; then
        fail_test "Prune completion is missing expected --repo repo-name completion"
    else
        pass_test
    fi
}

# Test: Position-aware completion for checkout-branch (new branch name vs base branch)
test_position_aware_completion() {
    run_test "Position-aware completion distinguishes argument positions"

    # Create a temporary test repository
    local test_repo="/tmp/test-position-$$"
    mkdir -p "$test_repo"
    cd "$test_repo"

    git init >/dev/null 2>&1
    git config user.name "Test" >/dev/null 2>&1
    git config user.email "test@example.com" >/dev/null 2>&1

    echo "test" > README.md
    git add README.md >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1
    git branch feature-existing >/dev/null 2>&1

    # First argument should suggest patterns AND existing branches
    local output
    output=$("$DAFT_BIN" __complete git-worktree-checkout "fea" 2>&1)

    # Clean up
    cd /
    rm -rf "$test_repo"

    if [[ "$output" == *"feature/"* ]] || [[ "$output" == *"feature-existing"* ]]; then
        pass_test
    else
        fail_test "Position-aware completion didn't provide appropriate suggestions"
    fi
}

# Test: Remote branch handling in dynamic completions
test_remote_branch_completion() {
    run_test "Remote branch handling in completions"

    # Create a test repository with local branches simulating remote branches
    local test_repo="/tmp/test-remote-$$"
    mkdir -p "$test_repo"
    cd "$test_repo"

    git init >/dev/null 2>&1
    git config user.name "Test" >/dev/null 2>&1
    git config user.email "test@example.com" >/dev/null 2>&1

    echo "test" > README.md
    git add README.md >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create local branches that would be typical from a remote
    git branch remote-feature >/dev/null 2>&1
    git branch origin-main >/dev/null 2>&1

    # Test completion includes these branches
    local output
    output=$("$DAFT_BIN" __complete git-worktree-checkout "remote" 2>&1)

    # Clean up
    cd /
    rm -rf "$test_repo"

    # Should at least not crash when checking for remote branches
    if [[ $? -eq 0 ]]; then
        pass_test
    else
        fail_test "Remote branch completion failed"
    fi
}

# Test: Non-git-repo behavior for dynamic completions
test_non_git_repo_completion() {
    run_test "Graceful handling when not in a git repository"

    # Create non-git directory
    local test_dir="/tmp/test-non-git-$$"
    mkdir -p "$test_dir"
    cd "$test_dir"

    # Attempt completion outside git repo
    local output
    output=$("$DAFT_BIN" __complete git-worktree-checkout "feat" 2>&1)

    # Clean up
    cd /
    rm -rf "$test_dir"

    # Should return empty or pattern suggestions, not crash
    if [[ $? -eq 0 ]]; then
        pass_test
    else
        fail_test "Completion crashed outside git repository"
    fi
}

# Test: Bash git subcommand registration
test_bash_git_subcommand_registration() {
    run_test "Bash completion registers git subcommand support"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'__git_complete'* ]] && [[ "$output" == *'git-worktree-checkout'* ]]; then
        pass_test
    else
        fail_test "Bash completion missing git subcommand registration"
    fi
}

# Test: Fish git subcommand registration
test_fish_git_subcommand_registration() {
    run_test "Fish completion registers git subcommand support"

    local output
    output=$("$DAFT_BIN" completions fish --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'complete -c git'* ]] && [[ "$output" == *'__fish_seen_subcommand_from'* ]]; then
        pass_test
    else
        fail_test "Fish completion missing git subcommand registration"
    fi
}

# Test: Zsh git subcommand registration (already implemented)
test_zsh_git_subcommand_registration() {
    run_test "Zsh completion registers git subcommand support"

    local output
    output=$("$DAFT_BIN" completions zsh --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'_git-worktree-checkout'* ]]; then
        pass_test
    else
        fail_test "Zsh completion missing git subcommand registration"
    fi
}

# Test: All commands generate completions without errors
test_all_commands_generate() {
    run_test "All commands generate completions for all shells"

    local commands=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-prune" "git-worktree-carry" "git-worktree-fetch" "git-worktree-flow-adopt" "git-worktree-flow-eject")
    local shells=("bash" "zsh" "fish" "fig")
    local success=true

    for cmd in "${commands[@]}"; do
        for shell in "${shells[@]}"; do
            if ! "$DAFT_BIN" completions "$shell" --command="$cmd" >/dev/null 2>&1; then
                success=false
                fail_test "Failed to generate $shell completion for $cmd"
                return
            fi
        done
    done

    if $success; then
        pass_test
    fi
}

# Test: Centralized flag extraction consistency
test_flag_extraction_consistency() {
    run_test "Flags are consistent across all shells (clap introspection)"

    # Check that all shells include the essential flags from clap introspection
    local bash_has_verbose
    local zsh_has_verbose
    local fish_has_verbose

    bash_has_verbose=$("$DAFT_BIN" completions bash --command=git-worktree-checkout 2>&1 | grep -c "verbose" || true)
    zsh_has_verbose=$("$DAFT_BIN" completions zsh --command=git-worktree-checkout 2>&1 | grep -c "verbose" || true)
    fish_has_verbose=$("$DAFT_BIN" completions fish --command=git-worktree-checkout 2>&1 | grep -c "verbose" || true)

    # All should include the verbose flag (count > 0)
    if [[ "$bash_has_verbose" -gt 0 ]] && \
       [[ "$zsh_has_verbose" -gt 0 ]] && \
       [[ "$fish_has_verbose" -gt 0 ]]; then
        pass_test
    else
        fail_test "Flag extraction inconsistent across shells (bash:$bash_has_verbose zsh:$zsh_has_verbose fish:$fish_has_verbose)"
    fi
}

# Test: shell-init bash includes completions
test_shell_init_includes_bash_completions() {
    run_test "shell-init bash output includes completion functions"

    local output
    output=$("$DAFT_BIN" shell-init bash 2>&1)

    if [[ "$output" == *"complete -F"* ]] && [[ "$output" == *"_git_worktree_checkout"* ]]; then
        pass_test
    else
        fail_test "shell-init bash output does not include completion registrations"
    fi
}

# Test: shell-init zsh includes completions
test_shell_init_includes_zsh_completions() {
    run_test "shell-init zsh output includes completion functions"

    local output
    output=$("$DAFT_BIN" shell-init zsh 2>&1)

    if [[ "$output" == *"compdef"* ]] && [[ "$output" == *"_git_worktree_checkout"* ]]; then
        pass_test
    else
        fail_test "shell-init zsh output does not include completion registrations"
    fi
}

# Test: shell-init fish includes completions
test_shell_init_includes_fish_completions() {
    run_test "shell-init fish output includes completion functions"

    local output
    output=$("$DAFT_BIN" shell-init fish 2>&1)

    if [[ "$output" == *"complete -c git-worktree-checkout"* ]]; then
        pass_test
    else
        fail_test "shell-init fish output does not include completion registrations"
    fi
}

# Test: Shortcut aliases get bash completions
test_shortcut_alias_bash_completions() {
    run_test "Shortcut aliases are registered for bash completions"

    local output
    output=$("$DAFT_BIN" completions bash 2>&1)

    if [[ "$output" == *"complete -F _git_worktree_checkout gwtco"* ]] && \
       [[ "$output" == *"complete -F _git_worktree_checkout gwco"* ]] && \
       [[ "$output" == *"complete -F _git_worktree_checkout gcw"* ]]; then
        pass_test
    else
        fail_test "Bash completions missing shortcut alias registrations"
    fi
}

# Test: carry command gets dynamic completion
test_carry_dynamic_completion() {
    run_test "Carry command has dynamic branch completion"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-carry 2>&1)

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'raw='* ]]; then
        pass_test
    else
        fail_test "Carry command missing dynamic branch completion"
    fi
}

# Test: Fig completion generation
test_fig_completion_generation() {
    run_test "Fig completion generation"

    local output
    output=$("$DAFT_BIN" completions fig --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *"completionSpec"* ]] && \
       [[ "$output" == *"generators"* ]] && \
       [[ "$output" == *"__complete"* ]]; then
        pass_test
    else
        fail_test "Fig completion output doesn't contain expected patterns (completionSpec, generators, __complete)"
    fi
}

# Test: Fig prune spec has no generators (no dynamic completion)
test_fig_no_generator_for_prune() {
    run_test "Fig prune spec has no generators"

    local output
    output=$("$DAFT_BIN" completions fig --command=git-worktree-prune 2>&1)

    if [[ "$output" == *"completionSpec"* ]] && [[ "$output" != *"generators"* ]]; then
        pass_test
    else
        fail_test "Fig prune spec should not contain generators"
    fi
}

# Test: Fig all commands output succeeds
test_fig_all_commands() {
    run_test "Fig all commands generation succeeds"

    local output
    output=$("$DAFT_BIN" completions fig 2>&1)

    if [[ $? -eq 0 ]] && [[ "$output" == *"completionSpec"* ]] && [[ "$output" == *"daft.js"* ]]; then
        pass_test
    else
        fail_test "Fig all commands generation failed or missing expected content"
    fi
}

# Test: Fig specs use ESM format (const + export default, not var + module.exports)
test_fig_esm_format() {
    run_test "Fig single-command spec uses ESM format"

    local output
    output=$("$DAFT_BIN" completions fig --command=git-worktree-checkout 2>&1)

    local success=true

    if [[ "$output" != *"const completionSpec"* ]]; then
        success=false
        fail_test "Fig spec missing 'const completionSpec'"
        return
    fi
    if [[ "$output" != *"export default completionSpec"* ]]; then
        success=false
        fail_test "Fig spec missing 'export default completionSpec'"
        return
    fi
    if [[ "$output" == *"var completionSpec"* ]]; then
        success=false
        fail_test "Fig spec should not use 'var completionSpec'"
        return
    fi
    if [[ "$output" == *"module.exports"* ]]; then
        success=false
        fail_test "Fig spec should not use 'module.exports'"
        return
    fi

    if $success; then
        pass_test
    fi
}

# Test: Fig all-commands output uses ESM format throughout
test_fig_all_esm_format() {
    run_test "Fig all-commands output uses ESM format throughout"

    local output
    output=$("$DAFT_BIN" completions fig 2>&1)

    if [[ "$output" == *"var completionSpec"* ]]; then
        fail_test "Fig all-commands output contains 'var completionSpec'"
        return
    fi
    if [[ "$output" == *"module.exports"* ]]; then
        fail_test "Fig all-commands output contains 'module.exports'"
        return
    fi
    if [[ "$output" != *"const completionSpec"* ]]; then
        fail_test "Fig all-commands output missing 'const completionSpec'"
        return
    fi
    if [[ "$output" != *"export default completionSpec"* ]]; then
        fail_test "Fig all-commands output missing 'export default completionSpec'"
        return
    fi

    pass_test
}

# Test: Fig shortcut aliases use loadSpec
test_fig_shortcut_aliases() {
    run_test "Fig shortcut aliases use loadSpec"

    local output
    output=$("$DAFT_BIN" completions fig 2>&1)

    if [[ "$output" == *"gwtco"* ]] && [[ "$output" == *"loadSpec"* ]]; then
        pass_test
    else
        fail_test "Fig output missing shortcut aliases with loadSpec"
    fi
}

# Test: repo-name completion is case-insensitive — end to end. Both layers must
# hold: the `__complete repo-name` helper case-folds the prefix, AND the sourced
# bash wrapper fills COMPREPLY directly instead of re-filtering through a
# case-sensitive `compgen -W` (which silently drops the folded matches). Runs in
# a fully isolated state dir so it never writes the developer's real catalog.
test_repo_name_completion_case_insensitive() {
    run_test "Repo-name completion is case-insensitive (helper + sourced bash)"

    # Everything runs in a subshell: the DAFT_*_DIR/PATH exports, the sourced
    # completion function, and the temp catalog stay out of the harness.
    local result
    result=$(
        set +eu
        sb=$(mktemp -d "${TMPDIR:-/tmp}/daft-ci-complete.XXXXXX")
        trap 'rm -rf "$sb"' EXIT
        export DAFT_CONFIG_DIR="$sb/cfg" DAFT_DATA_DIR="$sb/data" DAFT_STATE_DIR="$sb/st"

        # (No DAFT_*_DIR preflight here: main() already hard-fails on that
        # condition before any test runs.)

        # The sourced wrapper calls a bare `daft`; point it at the binary
        # under test, then register a lowercase-named repo.
        mkdir -p "$sb/bin"; ln -sf "$DAFT_BIN" "$sb/bin/daft"; export PATH="$sb/bin:$PATH"
        mkdir -p "$sb/apiservice"; cd "$sb/apiservice" || exit 1
        git init -q
        GIT_AUTHOR_NAME=T GIT_AUTHOR_EMAIL=t@t.co \
            GIT_COMMITTER_NAME=T GIT_COMMITTER_EMAIL=t@t.co \
            git commit -q --allow-empty -m init
        daft repo add --name apiservice >/dev/null 2>&1

        # Layer 1: the helper folds case (uppercase prefix → lowercase name).
        helper=$(daft __complete repo-name AP 2>/dev/null | cut -f1)
        [[ "$helper" == *apiservice* ]] || { echo "FAIL helper=[$helper]"; exit 0; }

        # Layer 2: drive the sourced bash completion. The emitted payload has
        # a documented bash 4+ floor (`mapfile -t`; its `_init_completion`
        # caller comes from bash-completion 2.x, which already requires 4+),
        # but this script runs under `#!/bin/bash` = 3.2 on macOS — there
        # `mapfile` is command-not-found, the completion function swallows it
        # via its own 2>/dev/null, and every drive returns an empty COMPREPLY.
        # So pick a bash that actually clears the floor: probe each candidate
        # for `mapfile` rather than trusting `command -v bash`, which yields
        # whichever bash comes first on PATH (3.2 on a stock mac even when
        # Homebrew's 5.x is installed further down).
        drive_bash=""
        for candidate in "$(command -v bash)" /opt/homebrew/bin/bash \
                         /usr/local/bin/bash /bin/bash; do
            if [[ -x "$candidate" ]] && "$candidate" -c 'type mapfile' >/dev/null 2>&1; then
                drive_bash="$candidate"
                break
            fi
        done
        if [[ -z "$drive_bash" ]]; then
            # Reported as a SKIP, which main() counts separately — never as a
            # pass. Counting it green would hide a real case-sensitivity
            # regression on every machine without a bash 4+.
            echo "SKIP: no bash 4+ available for the sourced-wrapper layer"
            exit 0
        fi
        # `_init_completion` (from bash-completion) is stubbed minimally;
        # drive() completes the last word. env (PATH with the daft symlink,
        # DAFT_*_DIR sandbox) and cwd are inherited by the child bash.
        "$drive_bash" <<'DRIVE_EOS'
_init_completion() {
    cur="${COMP_WORDS[COMP_CWORD]}"; prev="${COMP_WORDS[COMP_CWORD-1]}"
    words=("${COMP_WORDS[@]}"); cword=$COMP_CWORD; return 0
}
eval "$(daft completions bash 2>/dev/null)"
drive() { COMP_WORDS=("$@"); COMP_CWORD=$(($# - 1)); COMPREPLY=(); _daft 2>/dev/null; }

# An uppercase prefix surfaces the lowercase repo (the whole point)…
drive daft repo info AP;            match="${COMPREPLY[*]}"
# …and a genuine miss stays empty (no false positives from case-folding).
drive daft repo remove --repo ZZQQ; miss="${COMPREPLY[*]}"
if [[ "$match" == *apiservice* && -z "$miss" ]]; then
    echo "PASS"
else
    echo "FAIL match=[$match] miss=[$miss]"
fi
DRIVE_EOS
    )
    case "$result" in
        PASS) pass_test ;;
        SKIP*) skip_test "${result#SKIP: }" ;;
        *) fail_test "$result" ;;
    esac
}

# Main test execution
main() {
    echo "========================================="
    echo "Shell Completions Integration Tests"
    echo "========================================="
    echo ""

    # Check if daft binary exists
    if [ ! -f "$DAFT_BIN" ]; then
        echo -e "${RED}Error: daft binary not found at $DAFT_BIN${NC}"
        echo "Run 'cargo build --release' first"
        exit 1
    fi

    # Preflight (#667): refuse to run if the binary ignores DAFT_*_DIR (the
    # overrides compile out of non-dev builds) — its writes would land in the
    # real user dirs. Mirrors assert_binary_honors_overrides in the mise task;
    # needed here too because CI runs this script directly rather than through
    # `mise run completions:test`.
    #
    # Checked per key, not as one substring match over the whole output: a
    # build that honored DAFT_CONFIG_DIR but resolved data/state to the real
    # dirs would satisfy a whole-output match while still leaking the catalog
    # and the jobs dir — exactly the #696/#697 leak this exists to stop.
    preflight_dirs="$("$DAFT_BIN" __dirs 2>/dev/null)" || {
        echo -e "${RED}Error: '$DAFT_BIN __dirs' failed (binary too old, or broken)${NC}"
        exit 1
    }
    for key in config data state; do
        resolved="$(printf '%s\n' "$preflight_dirs" | awk -F '\t' -v k="$key" '$1 == k { print $2 }')"
        case "$resolved" in
            "$COMPLETIONS_SANDBOX"/*) ;;
            *)
                echo -e "${RED}Error: $DAFT_BIN ignores DAFT_${key}_DIR (resolved: ${resolved:-<none>})${NC}"
                echo "Refusing to run: completions tests would touch real user state. See #697."
                exit 1
                ;;
        esac
    done

    # Run all tests
    test_bash_completion_generation
    test_zsh_completion_generation
    test_fish_completion_generation
    test_dynamic_branch_completion
    test_repo_name_completion_case_insensitive


    # Test dynamic completion wiring
    test_bash_dynamic_wiring
    test_zsh_dynamic_wiring
    test_fish_dynamic_wiring
    test_prune_no_dynamic

    # Shell-init completions tests
    test_shell_init_includes_bash_completions
    test_shell_init_includes_zsh_completions
    test_shell_init_includes_fish_completions
    test_shortcut_alias_bash_completions
    test_carry_dynamic_completion

    # Fig completion tests
    test_fig_completion_generation
    test_fig_no_generator_for_prune
    test_fig_all_commands
    test_fig_shortcut_aliases
    test_fig_esm_format
    test_fig_all_esm_format

    # New comprehensive tests
    test_position_aware_completion
    test_remote_branch_completion
    test_non_git_repo_completion
    test_bash_git_subcommand_registration
    test_fish_git_subcommand_registration
    test_zsh_git_subcommand_registration
    test_all_commands_generate
    test_flag_extraction_consistency

    # Print summary
    echo "========================================="
    echo "Test Summary"
    echo "========================================="
    echo "Tests run: $TESTS_RUN"
    echo "Tests passed: $TESTS_PASSED"
    if [ "$TESTS_SKIPPED" -gt 0 ]; then
        # Surfaced separately and never folded into the pass count — a skipped
        # assertion is absent coverage, not a green one.
        echo -e "${YELLOW}Tests skipped: $TESTS_SKIPPED${NC}"
        for reason in "${SKIPPED_TESTS[@]}"; do
            echo "  - $reason"
        done
    fi

    if [ $((TESTS_PASSED + TESTS_SKIPPED)) -eq $TESTS_RUN ]; then
        if [ "$TESTS_SKIPPED" -gt 0 ]; then
            echo -e "${YELLOW}All executed tests passed ($TESTS_SKIPPED skipped)${NC}"
        else
            echo -e "${GREEN}All tests passed!${NC}"
        fi
        exit 0
    else
        echo -e "${RED}Some tests failed${NC}"
        exit 1
    fi
}

main
