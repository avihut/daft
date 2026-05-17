#!/bin/bash

# Integration tests for daft shell-init command
# Tests shell wrapper generation and cd-into-worktree functionality

set -eo pipefail

# Source the test framework
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/test_framework.sh"

# --- Test Functions ---

test_shell_init_bash_output() {
    log "Testing: daft shell-init bash generates valid output"

    local output
    output=$(daft shell-init bash)

    # Check that it contains the wrapper function
    if grep -q "__daft_wrapper()" <<< "$output"; then
        log_success "Output contains __daft_wrapper function"
    else
        log_error "Output missing __daft_wrapper function"
        return 1
    fi

    # Check that it contains wrapper functions for each command
    local commands=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-carry")
    for cmd in "${commands[@]}"; do
        if grep -q "^${cmd}()" <<< "$output"; then
            log_success "Output contains ${cmd} function"
        else
            log_error "Output missing ${cmd} function"
            return 1
        fi
    done

    return 0
}

test_shell_init_bash_syntax() {
    log "Testing: daft shell-init bash generates valid bash syntax"

    local output
    output=$(daft shell-init bash)

    # Validate bash syntax using bash -n
    if bash -n <<< "$output" 2>&1; then
        log_success "Generated bash code has valid syntax"
        return 0
    else
        log_error "Generated bash code has syntax errors"
        return 1
    fi
}

test_shell_init_zsh_syntax() {
    log "Testing: daft shell-init zsh generates valid zsh syntax"

    # Skip if zsh is not available
    if ! command -v zsh >/dev/null 2>&1; then
        log_warning "zsh not available, skipping syntax validation"
        return 0
    fi

    local output
    output=$(daft shell-init zsh)

    # Validate zsh syntax using zsh -n
    if zsh -n <<< "$output" 2>&1; then
        log_success "Generated zsh code has valid syntax"
        return 0
    else
        log_error "Generated zsh code has syntax errors"
        return 1
    fi
}

test_shell_init_fish_output() {
    log "Testing: daft shell-init fish generates valid output"

    local output
    output=$(daft shell-init fish)

    # Check that it contains the wrapper function
    if grep -q "function __daft_wrapper" <<< "$output"; then
        log_success "Output contains __daft_wrapper function"
    else
        log_error "Output missing __daft_wrapper function"
        return 1
    fi

    # Check that it contains wrapper functions for each command
    local commands=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-carry")
    for cmd in "${commands[@]}"; do
        if grep -q "function ${cmd}" <<< "$output"; then
            log_success "Output contains ${cmd} function"
        else
            log_error "Output missing ${cmd} function"
            return 1
        fi
    done

    return 0
}

test_shell_init_fish_syntax() {
    log "Testing: daft shell-init fish generates valid fish syntax"

    # Skip if fish is not available
    if ! command -v fish >/dev/null 2>&1; then
        log_warning "fish not available, skipping syntax validation"
        return 0
    fi

    local output
    output=$(daft shell-init fish)

    # Validate fish syntax using fish -n
    if fish -n <<< "$output" 2>&1; then
        log_success "Generated fish code has valid syntax"
        return 0
    else
        log_error "Generated fish code has syntax errors"
        return 1
    fi
}

test_shell_init_bash_aliases() {
    log "Testing: daft shell-init bash --aliases includes short aliases"

    local output
    output=$(daft shell-init bash --aliases)

    # Check for short aliases
    local aliases=("gwclone" "gwinit" "gwco" "gwcob" "gwcarry" "gwprune")
    for alias_name in "${aliases[@]}"; do
        if grep -q "alias ${alias_name}=" <<< "$output"; then
            log_success "Output contains ${alias_name} alias"
        else
            log_error "Output missing ${alias_name} alias"
            return 1
        fi
    done

    return 0
}

test_shell_init_fish_aliases() {
    log "Testing: daft shell-init fish --aliases includes short aliases"

    local output
    output=$(daft shell-init fish --aliases)

    # Check for short aliases
    local aliases=("gwclone" "gwinit" "gwco" "gwcob" "gwcarry" "gwprune")
    for alias_name in "${aliases[@]}"; do
        if grep -q "alias ${alias_name}=" <<< "$output"; then
            log_success "Output contains ${alias_name} alias"
        else
            log_error "Output missing ${alias_name} alias"
            return 1
        fi
    done

    return 0
}

test_shell_init_help() {
    log "Testing: daft shell-init --help shows usage"

    local output
    output=$(daft shell-init --help 2>&1)

    if grep -qi "shell wrapper" <<< "$output"; then
        log_success "Help text mentions shell wrapper"
    else
        log_error "Help text missing shell wrapper description"
        return 1
    fi

    if grep -q "bash" <<< "$output"; then
        log_success "Help text mentions bash"
    else
        log_error "Help text missing bash"
        return 1
    fi

    return 0
}

test_cd_file_output() {
    log "Testing: Commands write CD path to temp file when DAFT_CD_FILE is set"

    # Create a test remote repository
    local remote_dir
    remote_dir=$(create_test_remote "test-repo-cd-marker" "main")

    # Create a temp file for CD path
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")

    # Clone the repository with DAFT_CD_FILE set
    DAFT_CD_FILE="$cd_file" git-worktree-clone --layout contained "$remote_dir" 2>&1 || true

    # Check that the temp file is non-empty
    if [ -s "$cd_file" ]; then
        log_success "Clone wrote CD path to temp file"
    else
        log_error "Clone did not write CD path to temp file"
        return 1
    fi

    rm -f "$cd_file"
    return 0
}

test_no_cd_file_without_env() {
    log "Testing: Commands do not leak __DAFT_CD__ marker to stdout"

    # Create a test remote repository
    local remote_dir
    remote_dir=$(create_test_remote "test-repo-no-marker" "main")

    # Clone the repository without DAFT_CD_FILE
    unset DAFT_CD_FILE
    git-worktree-clone --layout contained "$remote_dir" 2>&1 | tee /tmp/clone_output_no_env.txt || true

    # Check that the output does NOT contain the old CD path marker
    if grep -q "^__DAFT_CD__:" /tmp/clone_output_no_env.txt; then
        log_error "Clone output incorrectly contains legacy __DAFT_CD__ marker"
        cat /tmp/clone_output_no_env.txt
        return 1
    else
        log_success "Clone output correctly omits legacy __DAFT_CD__ marker"
    fi

    return 0
}

test_wrapper_cd_integration() {
    log "Testing: Shell wrapper writes CD path to temp file after checkout"

    # Create a test remote repository
    local remote_dir
    remote_dir=$(create_test_remote "test-repo-wrapper" "main")

    # Clone the repository first (without wrapper, just setup)
    git-worktree-clone --layout contained "$remote_dir" >/dev/null 2>&1

    # Get the project directory
    local project_dir="$PWD/test-repo-wrapper"

    # Change to the main worktree
    cd "$project_dir/main"

    # Source the shell wrappers
    eval "$(daft shell-init bash)"

    # Create a temp file for CD path
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")

    # Create a new branch worktree with DAFT_CD_FILE set
    DAFT_CD_FILE="$cd_file" command git-worktree-checkout -b test-branch 2>&1 || true

    # Verify the temp file has content
    if [ -s "$cd_file" ]; then
        log_success "Wrapper integration test: CD path written to temp file"
    else
        log_error "Wrapper integration test: CD path not written to temp file"
        return 1
    fi

    rm -f "$cd_file"
    return 0
}

test_git_wrapper_function_exists() {
    log "Testing: daft shell-init generates git() wrapper function"

    local output
    output=$(daft shell-init bash)

    # Check that it contains the git wrapper function
    if grep -q "^git()" <<< "$output"; then
        log_success "Output contains git() wrapper function"
    else
        log_error "Output missing git() wrapper function"
        return 1
    fi

    # Check that it intercepts worktree-checkout
    if grep -q "worktree-checkout)" <<< "$output"; then
        log_success "git() wrapper intercepts worktree-checkout"
    else
        log_error "git() wrapper missing worktree-checkout interception"
        return 1
    fi

    # Check that it has passthrough for other commands
    if grep -q 'command git "\$@"' <<< "$output"; then
        log_success "git() wrapper passes through other commands"
    else
        log_error "git() wrapper missing passthrough"
        return 1
    fi

    return 0
}

test_git_wrapper_passthrough() {
    log "Testing: git() wrapper passes through regular git commands"

    # Source the shell wrappers in a subshell and test
    local version_output
    version_output=$(bash -c '
        eval "$(daft shell-init bash)"
        git --version
    ' 2>&1)

    if grep -q "git version" <<< "$version_output"; then
        log_success "git --version works through wrapper"
    else
        log_error "git --version failed through wrapper"
        echo "Output: $version_output"
        return 1
    fi

    return 0
}

test_git_wrapper_intercepts_subcommand() {
    log "Testing: git() wrapper intercepts 'git worktree-checkout' subcommand"

    # Source the shell wrappers and check that git is a function
    local type_output
    type_output=$(bash -c '
        eval "$(daft shell-init bash)"
        type git | head -1
    ' 2>&1)

    if grep -q "function" <<< "$type_output"; then
        log_success "git is defined as a function after sourcing wrappers"
    else
        log_error "git is not a function after sourcing wrappers"
        echo "Output: $type_output"
        return 1
    fi

    return 0
}

test_daft_wrapper_function_exists() {
    log "Testing: daft shell-init generates daft() wrapper function"

    local output
    output=$(daft shell-init bash)

    # Check that it contains the daft wrapper function
    if grep -q "^daft()" <<< "$output"; then
        log_success "Output contains daft() wrapper function"
    else
        log_error "Output missing daft() wrapper function"
        return 1
    fi

    # Check that it intercepts worktree-checkout
    if grep -q "worktree-checkout)" <<< "$output"; then
        log_success "daft() wrapper intercepts worktree-checkout"
    else
        log_error "daft() wrapper missing worktree-checkout interception"
        return 1
    fi

    # Passthrough behavior is exercised end-to-end by
    # test_daft_wrapper_passthrough. A grep for `command daft "$@"` here
    # is unreliable because the wrapper now strips top-level `-C <path>`
    # pairs and emits `command daft "${__daft_pre[@]}" "$@"` — and
    # explanatory comments in the wrapper can match a literal-pattern grep
    # by accident.

    return 0
}

test_daft_wrapper_passthrough() {
    log "Testing: daft() wrapper passes through regular daft commands"

    # Source the shell wrappers in a subshell and test
    local version_output
    version_output=$(bash -c '
        eval "$(daft shell-init bash)"
        daft --version
    ' 2>&1)

    if grep -q "daft" <<< "$version_output"; then
        log_success "daft --version works through wrapper"
    else
        log_error "daft --version failed through wrapper"
        echo "Output: $version_output"
        return 1
    fi

    return 0
}

test_daft_wrapper_intercepts_subcommand() {
    log "Testing: daft() wrapper intercepts 'daft worktree-checkout' subcommand"

    # Source the shell wrappers and check that daft is a function
    local type_output
    type_output=$(bash -c '
        eval "$(daft shell-init bash)"
        type daft | head -1
    ' 2>&1)

    if grep -q "function" <<< "$type_output"; then
        log_success "daft is defined as a function after sourcing wrappers"
    else
        log_error "daft is not a function after sourcing wrappers"
        echo "Output: $type_output"
        return 1
    fi

    return 0
}

# Regression test for #380: the wrapper used to cache the daft binary path at
# source time. Replacing the on-disk binary mid-shell would leave wrappers
# running the stale build until the user re-sourced. This test sources the
# wrapper, swaps the resolved-on-PATH binary, and asserts the wrapper picks up
# the new build without re-sourcing.
test_wrapper_resolves_binary_live() {
    log "Testing: shell wrapper resolves daft binary on every invocation (#380)"

    local sandbox="$PWD/wrapper-live"
    rm -rf "$sandbox"
    mkdir -p "$sandbox/bin"

    # Capture the wrapper using the real built daft binary.
    local wrapper_file="$sandbox/wrapper.bash"
    daft shell-init bash > "$wrapper_file"

    # Stage mock v1 binary. The wrapper exec's the resolved binary with
    # argv[0] set to the wrapped command name (e.g. git-worktree-clone), so
    # the mock just needs to print a recognizable marker regardless of args.
    cat > "$sandbox/bin/daft" <<'EOF'
#!/usr/bin/env bash
echo "MOCK_v1"
EOF
    chmod +x "$sandbox/bin/daft"

    # Source wrapper, then prepend the mock dir to PATH so command -v daft
    # inside __daft_find_bin resolves to the mock. Run a wrapped invocation,
    # swap the mock to v2 in place, run again — all in one shell process so
    # the wrapper isn't re-sourced between calls.
    local out
    out=$(SANDBOX="$sandbox" WRAPPER="$wrapper_file" bash -c '
        source "$WRAPPER"
        export PATH="$SANDBOX/bin:$PATH"
        first=$(git-worktree-clone 2>/dev/null)
        cat > "$SANDBOX/bin/daft" <<__V2__
#!/usr/bin/env bash
echo "MOCK_v2"
__V2__
        chmod +x "$SANDBOX/bin/daft"
        second=$(git-worktree-clone 2>/dev/null)
        printf "first=%s second=%s\n" "$first" "$second"
    ' 2>&1) || true

    if [[ "$out" == *"first=MOCK_v1"* && "$out" == *"second=MOCK_v2"* ]]; then
        log_success "Wrapper resolved daft binary live across binary swap"
        return 0
    else
        log_error "Wrapper did not pick up new binary without re-sourcing"
        echo "Output: $out"
        return 1
    fi
}

# Regression test: `daft repo remove` is a subcommand (not a separate
# binary), so the wrapper must wire DAFT_CD_FILE for it the same way it
# does for `daft layout`. Without this, running `daft repo remove .` from
# inside a worktree leaves the user's shell sitting in a deleted dir
# (the binary falls back to `eprintln!("Run cd ...")` because no
# DAFT_CD_FILE was set in env). User-reported field test caught this.
test_daft_repo_wrapper_writes_cd_file() {
    log "Testing: daft repo wrapper sets DAFT_CD_FILE so cd-out works after remove"

    local remote_dir
    remote_dir=$(create_test_remote "test-repo-wrapper-remove" "main")

    # Clone via the actual binary to set up the working repo.
    git-worktree-clone --layout contained "$remote_dir" >/dev/null 2>&1
    local project_root="$PWD/test-repo-wrapper-remove"
    local main_worktree="$project_root/main"
    if [[ ! -d "$main_worktree" ]]; then
        log_error "setup: main worktree not created at $main_worktree"
        return 1
    fi

    # Source the wrapper, cd into the worktree about to be deleted, run
    # `daft repo remove --force`, then verify the wrapper's `cd` happened
    # by reading $PWD afterwards. If the wrapper didn't pass DAFT_CD_FILE,
    # the binary's fallback kicks in and the shell stays in the (now-gone)
    # worktree dir — so `pwd` returns the project_root or fails.
    local out
    out=$(MAIN_WT="$main_worktree" PROJECT_ROOT="$project_root" bash -c '
        eval "$(daft shell-init bash)"
        builtin cd "$MAIN_WT" || exit 11
        # Suppress the binary chatter; we only care about post-cd state.
        daft repo remove --force >/dev/null 2>&1 || true
        # Print the pwd the *wrapper* landed us in.
        builtin pwd
    ' 2>&1) || true

    if [[ -z "$out" ]]; then
        log_error "wrapper produced no pwd output"
        echo "Output: $out"
        return 1
    fi
    # Successful wrapper redirect lands us in some sibling/ancestor dir
    # that is NOT inside the now-deleted project_root.
    if [[ "$out" == "$project_root"* ]]; then
        log_error "wrapper did not cd out of deleted project_root: $out"
        return 1
    fi
    if [[ -d "$project_root" ]]; then
        log_error "project_root not removed: $project_root"
        return 1
    fi

    log_success "daft repo wrapper redirected shell out of deleted project (now in: $out)"
    return 0
}

# Regression for issue #519: the `-C <path>` global flag must work end-to-end
# through the shell wrapper. Two failure modes the wrapper could introduce:
#  1. Binary writes DAFT_CD_FILE relative to its post-`-C` cwd correctly, but
#     the wrapper's `cd` happens before the binary runs and points at the
#     wrong place. (Wrapper doesn't touch DAFT_CD_FILE between
#     binary-exit and `cd`, so this should be fine — but assert it.)
#  2. Binary's `cli::install_and_apply` doesn't trigger for the multicall arm
#     and the chdir never happens — wrapper would then land in the wrong repo.
test_c_flag_cd_redirect_through_wrapper() {
    log "Testing: daft -C <other-repo> checkout through wrapper lands shell in other-repo's worktree"

    local remote_a remote_b
    remote_a=$(create_test_remote "test-c-flag-wrapper-a" "main")
    remote_b=$(create_test_remote "test-c-flag-wrapper-b" "main")

    git-worktree-clone --layout contained "$remote_a" >/dev/null 2>&1
    git-worktree-clone --layout contained "$remote_b" >/dev/null 2>&1
    local repo_a="$PWD/test-c-flag-wrapper-a"
    local repo_b="$PWD/test-c-flag-wrapper-b"

    # From inside repo_a/main, invoke `daft -C <repo_b/main> go develop`.
    # Expected: the wrapper cd's us into repo_b's new worktree, NOT into
    # repo_b/main directly and NOT staying in repo_a.
    local out
    out=$(REPO_A_MAIN="$repo_a/main" REPO_B_MAIN="$repo_b/main" REPO_B="$repo_b" bash -c '
        eval "$(daft shell-init bash)"
        builtin cd "$REPO_A_MAIN" || exit 11
        daft -C "$REPO_B_MAIN" go develop >/dev/null 2>&1 || true
        builtin pwd
    ' 2>&1) || true

    # Resolve symlinks: /tmp -> /private/tmp on macOS, where daft canonicalizes
    # paths in DAFT_CD_FILE but $repo_b stays un-resolved.
    local resolved_repo_b
    resolved_repo_b=$(python3 -c "import os,sys; print(os.path.realpath(sys.argv[1]))" "$repo_b")

    if [[ "$out" != "$resolved_repo_b/develop"* ]]; then
        log_error "wrapper did not cd into the -C target's new worktree"
        log_error "  expected prefix: $resolved_repo_b/develop"
        log_error "  actual pwd:      $out"
        return 1
    fi

    log_success "daft -C through wrapper lands shell at: $out"
    return 0
}

# Regression: trailing `daft -C` (no path argument) through the wrapper must
# emit the same `option requires an argument` error the binary would print,
# and exit 2. Previously the wrapper's `shift 2 || return 2` short-circuited
# silently — exit code was right but the user got no message because the
# binary never ran.
test_c_flag_no_arg_through_wrapper_errors_cleanly() {
    log "Testing: daft -C (no arg) through wrapper exits 2 with error message"

    local output
    output=$(bash -c 'eval "$(daft shell-init bash)"; daft -C' 2>&1)
    local exit_code=$?

    if [[ $exit_code -ne 2 ]]; then
        log_error "Expected exit code 2, got $exit_code"
        log_error "Output: $output"
        return 1
    fi

    if [[ "$output" != *"requires an argument"* ]]; then
        log_error "Expected error message containing 'requires an argument'"
        log_error "Got: $output"
        return 1
    fi

    log_success "daft -C through wrapper: exit 2 with clear error message"
    return 0
}

# Regression for issue #519: -C must work for symlinked-entry invocations
# (git-worktree-*, daft-go, etc.) just like for the multicall `daft` arm.
# The argv-strip is in the SAME place for both paths, so this is mostly a
# smoke test that nothing in the symlink dispatch path bypassed it.
test_c_flag_symlink_entry() {
    log "Testing: git-worktree-list -C <other-repo> succeeds"

    local remote
    remote=$(create_test_remote "test-c-flag-symlink" "main")
    git-worktree-clone --layout contained "$remote" >/dev/null 2>&1
    local repo="$PWD/test-c-flag-symlink"

    # From an unrelated cwd, list via the symlink entry against the repo.
    local out
    out=$(builtin cd "$TEMP_BASE_DIR" && NO_COLOR=1 git-worktree-list -C "$repo/main" 2>&1) || {
        log_error "git-worktree-list -C exited non-zero"
        log_error "output: $out"
        return 1
    }

    if [[ "$out" != *"main"* ]]; then
        log_error "git-worktree-list -C did not list the target repo"
        log_error "output: $out"
        return 1
    fi

    log_success "git-worktree-list -C succeeds: lists target repo's worktrees"
    return 0
}

# --- Main Test Runner ---

main() {
    setup

    echo
    echo "========================================================="
    echo "Running daft shell-init Integration Tests"
    echo "========================================================="
    echo

    run_test "shell_init_bash_output" test_shell_init_bash_output
    run_test "shell_init_bash_syntax" test_shell_init_bash_syntax
    run_test "shell_init_zsh_syntax" test_shell_init_zsh_syntax
    run_test "shell_init_fish_output" test_shell_init_fish_output
    run_test "shell_init_fish_syntax" test_shell_init_fish_syntax
    run_test "shell_init_bash_aliases" test_shell_init_bash_aliases
    run_test "shell_init_fish_aliases" test_shell_init_fish_aliases
    run_test "shell_init_help" test_shell_init_help
    run_test "cd_file_output" test_cd_file_output
    run_test "no_cd_file_without_env" test_no_cd_file_without_env
    run_test "wrapper_cd_integration" test_wrapper_cd_integration
    run_test "git_wrapper_function_exists" test_git_wrapper_function_exists
    run_test "git_wrapper_passthrough" test_git_wrapper_passthrough
    run_test "git_wrapper_intercepts_subcommand" test_git_wrapper_intercepts_subcommand
    run_test "daft_wrapper_function_exists" test_daft_wrapper_function_exists
    run_test "daft_wrapper_passthrough" test_daft_wrapper_passthrough
    run_test "daft_wrapper_intercepts_subcommand" test_daft_wrapper_intercepts_subcommand
    run_test "wrapper_resolves_binary_live" test_wrapper_resolves_binary_live
    run_test "daft_repo_wrapper_writes_cd_file" test_daft_repo_wrapper_writes_cd_file
    run_test "c_flag_cd_redirect_through_wrapper" test_c_flag_cd_redirect_through_wrapper
    run_test "c_flag_no_arg_through_wrapper_errors_cleanly" test_c_flag_no_arg_through_wrapper_errors_cleanly
    run_test "c_flag_symlink_entry" test_c_flag_symlink_entry

    print_summary
}

main "$@"
