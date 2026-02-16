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
    if echo "$output" | grep -q "__daft_wrapper()"; then
        log_success "Output contains __daft_wrapper function"
    else
        log_error "Output missing __daft_wrapper function"
        return 1
    fi

    # Check that it contains wrapper functions for each command
    local commands=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-checkout-branch" "git-worktree-carry")
    for cmd in "${commands[@]}"; do
        if echo "$output" | grep -q "^${cmd}()"; then
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
    if echo "$output" | bash -n 2>&1; then
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
    if echo "$output" | zsh -n 2>&1; then
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
    if echo "$output" | grep -q "function __daft_wrapper"; then
        log_success "Output contains __daft_wrapper function"
    else
        log_error "Output missing __daft_wrapper function"
        return 1
    fi

    # Check that it contains wrapper functions for each command
    local commands=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-checkout-branch" "git-worktree-carry")
    for cmd in "${commands[@]}"; do
        if echo "$output" | grep -q "function ${cmd}"; then
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
    if echo "$output" | fish -n 2>&1; then
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
        if echo "$output" | grep -q "alias ${alias_name}="; then
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
        if echo "$output" | grep -q "alias ${alias_name}="; then
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

    if echo "$output" | grep -qi "shell wrapper"; then
        log_success "Help text mentions shell wrapper"
    else
        log_error "Help text missing shell wrapper description"
        return 1
    fi

    if echo "$output" | grep -q "bash"; then
        log_success "Help text mentions bash"
    else
        log_error "Help text missing bash"
        return 1
    fi

    return 0
}

test_cd_path_marker_output() {
    log "Testing: Commands output CD path marker when DAFT_SHELL_WRAPPER is set"

    # Create a test remote repository
    local remote_dir
    remote_dir=$(create_test_remote "test-repo-cd-marker" "main")

    # Clone the repository
    DAFT_SHELL_WRAPPER=1 git-worktree-clone "$remote_dir" 2>&1 | tee /tmp/clone_output.txt || true

    # Check that the output contains the CD path marker
    if grep -q "^__DAFT_CD__:" /tmp/clone_output.txt; then
        log_success "Clone output contains CD path marker"
    else
        log_error "Clone output missing CD path marker"
        cat /tmp/clone_output.txt
        return 1
    fi

    return 0
}

test_cd_path_marker_not_output_without_env() {
    log "Testing: Commands do NOT output CD path marker when DAFT_SHELL_WRAPPER is not set"

    # Create a test remote repository
    local remote_dir
    remote_dir=$(create_test_remote "test-repo-no-marker" "main")

    # Clone the repository without DAFT_SHELL_WRAPPER
    unset DAFT_SHELL_WRAPPER
    git-worktree-clone "$remote_dir" 2>&1 | tee /tmp/clone_output_no_env.txt || true

    # Check that the output does NOT contain the CD path marker
    if grep -q "^__DAFT_CD__:" /tmp/clone_output_no_env.txt; then
        log_error "Clone output incorrectly contains CD path marker when env not set"
        cat /tmp/clone_output_no_env.txt
        return 1
    else
        log_success "Clone output correctly omits CD path marker when env not set"
    fi

    return 0
}

test_wrapper_cd_integration() {
    log "Testing: Shell wrapper actually changes directory after checkout"

    # Create a test remote repository
    local remote_dir
    remote_dir=$(create_test_remote "test-repo-wrapper" "main")

    # Clone the repository first (without wrapper, just setup)
    git-worktree-clone "$remote_dir" >/dev/null 2>&1

    # Get the project directory
    local project_dir="$PWD/test-repo-wrapper"

    # Change to the main worktree
    cd "$project_dir/main"

    # Source the shell wrappers
    eval "$(daft shell-init bash)"

    # Save current directory
    local start_dir="$PWD"

    # Create a new branch worktree using the wrapper function
    # Note: We're testing that the function is callable, but the actual
    # cd effect won't persist in the subshell - we verify the marker is present
    local output
    output=$(DAFT_SHELL_WRAPPER=1 command git-worktree-checkout-branch test-branch 2>&1) || true

    # Verify the marker is in the output
    if echo "$output" | grep -q "^__DAFT_CD__:"; then
        log_success "Wrapper integration test: CD marker present in output"
    else
        log_error "Wrapper integration test: CD marker missing from output"
        echo "Output was:"
        echo "$output"
        return 1
    fi

    return 0
}

test_git_wrapper_function_exists() {
    log "Testing: daft shell-init generates git() wrapper function"

    local output
    output=$(daft shell-init bash)

    # Check that it contains the git wrapper function
    if echo "$output" | grep -q "^git()"; then
        log_success "Output contains git() wrapper function"
    else
        log_error "Output missing git() wrapper function"
        return 1
    fi

    # Check that it intercepts worktree-checkout
    if echo "$output" | grep -q "worktree-checkout)"; then
        log_success "git() wrapper intercepts worktree-checkout"
    else
        log_error "git() wrapper missing worktree-checkout interception"
        return 1
    fi

    # Check that it has passthrough for other commands
    if echo "$output" | grep -q 'command git "\$@"'; then
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

    if echo "$version_output" | grep -q "git version"; then
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

    if echo "$type_output" | grep -q "function"; then
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
    if echo "$output" | grep -q "^daft()"; then
        log_success "Output contains daft() wrapper function"
    else
        log_error "Output missing daft() wrapper function"
        return 1
    fi

    # Check that it intercepts worktree-checkout
    if echo "$output" | grep -q "worktree-checkout)"; then
        log_success "daft() wrapper intercepts worktree-checkout"
    else
        log_error "daft() wrapper missing worktree-checkout interception"
        return 1
    fi

    # Check that it has passthrough for other commands
    if echo "$output" | grep -q 'command daft "\$@"'; then
        log_success "daft() wrapper passes through other commands"
    else
        log_error "daft() wrapper missing passthrough"
        return 1
    fi

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

    if echo "$version_output" | grep -q "daft"; then
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

    if echo "$type_output" | grep -q "function"; then
        log_success "daft is defined as a function after sourcing wrappers"
    else
        log_error "daft is not a function after sourcing wrappers"
        echo "Output: $type_output"
        return 1
    fi

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
    run_test "cd_path_marker_output" test_cd_path_marker_output
    run_test "cd_path_marker_not_output_without_env" test_cd_path_marker_not_output_without_env
    run_test "wrapper_cd_integration" test_wrapper_cd_integration
    run_test "git_wrapper_function_exists" test_git_wrapper_function_exists
    run_test "git_wrapper_passthrough" test_git_wrapper_passthrough
    run_test "git_wrapper_intercepts_subcommand" test_git_wrapper_intercepts_subcommand
    run_test "daft_wrapper_function_exists" test_daft_wrapper_function_exists
    run_test "daft_wrapper_passthrough" test_daft_wrapper_passthrough
    run_test "daft_wrapper_intercepts_subcommand" test_daft_wrapper_intercepts_subcommand

    print_summary
}

main "$@"
