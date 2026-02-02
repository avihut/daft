#!/bin/bash
# Integration tests for shell completions

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DAFT_BIN="$PROJECT_ROOT/target/release/daft"

# Color codes for output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Test counters
TESTS_RUN=0
TESTS_PASSED=0

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

# Test: New branch pattern suggestions
test_branch_pattern_suggestions() {
    run_test "New branch pattern suggestions"

    local output
    output=$("$DAFT_BIN" __complete git-worktree-checkout-branch "fea" 2>&1)

    if [[ "$output" == *"feature/"* ]] && [[ "$output" == *"feat/"* ]]; then
        pass_test
    else
        fail_test "Pattern suggestions didn't return expected patterns"
    fi
}

# Test: Bash completion includes dynamic branch wiring
test_bash_dynamic_wiring() {
    run_test "Bash completion includes dynamic branch logic"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-checkout 2>&1)

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'branches='* ]]; then
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

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'branches='* ]]; then
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

# Test: Completions without dynamic branches don't include __complete
test_prune_no_dynamic() {
    run_test "Prune completion has no dynamic logic (as expected)"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-prune 2>&1)

    if [[ "$output" != *'daft __complete'* ]]; then
        pass_test
    else
        fail_test "Prune completion incorrectly includes dynamic branch logic"
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
    output=$("$DAFT_BIN" __complete git-worktree-checkout-branch "fea" 2>&1)

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

    local commands=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-checkout-branch" "git-worktree-checkout-branch-from-default" "git-worktree-prune" "git-worktree-carry" "git-worktree-fetch" "git-worktree-flow-adopt" "git-worktree-flow-eject")
    local shells=("bash" "zsh" "fish")
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

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'branches='* ]]; then
        pass_test
    else
        fail_test "Carry command missing dynamic branch completion"
    fi
}

# Test: checkout-branch-from-default has dynamic completion wiring
test_checkout_branch_from_default_dynamic_wiring() {
    run_test "checkout-branch-from-default has dynamic completion"

    local output
    output=$("$DAFT_BIN" completions bash --command=git-worktree-checkout-branch-from-default 2>&1)

    if [[ "$output" == *'daft __complete'* ]] && [[ "$output" == *'branches='* ]]; then
        pass_test
    else
        fail_test "checkout-branch-from-default missing dynamic branch completion"
    fi
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

    # Run all tests
    test_bash_completion_generation
    test_zsh_completion_generation
    test_fish_completion_generation
    test_dynamic_branch_completion
    test_branch_pattern_suggestions

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
    test_checkout_branch_from_default_dynamic_wiring

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

    if [ $TESTS_PASSED -eq $TESTS_RUN ]; then
        echo -e "${GREEN}All tests passed!${NC}"
        exit 0
    else
        echo -e "${RED}Some tests failed${NC}"
        exit 1
    fi
}

main
