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
