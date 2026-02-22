#!/bin/bash

# Integration tests for unknown command error handling

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test that `daft nonexistent` prints error and exits with code 1
test_unknown_command_daft_error() {
    local output
    output=$(daft nonexistent 2>&1)
    local exit_code=$?

    if [[ $exit_code -ne 1 ]]; then
        log_error "Expected exit code 1, got $exit_code"
        return 1
    fi

    if [[ "$output" != *"'nonexistent' is not a daft command"* ]]; then
        log_error "Expected error message about unknown command, got: $output"
        return 1
    fi

    if [[ "$output" != *"See 'daft --help'"* ]]; then
        log_error "Expected help hint in error message, got: $output"
        return 1
    fi

    return 0
}

# Test that `daft steup` suggests "setup"
test_unknown_command_daft_suggestion() {
    local output
    output=$(daft steup 2>&1)
    local exit_code=$?

    if [[ $exit_code -ne 1 ]]; then
        log_error "Expected exit code 1, got $exit_code"
        return 1
    fi

    if [[ "$output" != *"setup"* ]]; then
        log_error "Expected 'setup' suggestion, got: $output"
        return 1
    fi

    if [[ "$output" != *"The most similar command"* ]]; then
        log_error "Expected suggestion header, got: $output"
        return 1
    fi

    return 0
}

# Test that `git-daft nonexistent` prints error and exits with code 1
test_unknown_command_git_daft_error() {
    local output
    output=$(git-daft nonexistent 2>&1)
    local exit_code=$?

    if [[ $exit_code -ne 1 ]]; then
        log_error "Expected exit code 1, got $exit_code"
        return 1
    fi

    if [[ "$output" != *"'nonexistent' is not a git daft command"* ]]; then
        log_error "Expected error message about unknown git daft command, got: $output"
        return 1
    fi

    return 0
}

# Test that `daft` (no args) still shows help and exits with code 0
test_unknown_command_daft_no_args() {
    daft >/dev/null 2>&1
    local exit_code=$?

    if [[ $exit_code -ne 0 ]]; then
        log_error "Expected exit code 0 for 'daft' with no args, got $exit_code"
        return 1
    fi

    return 0
}

# Test that `git-daft` (no args) still shows help and exits with code 0
test_unknown_command_git_daft_no_args() {
    git-daft >/dev/null 2>&1
    local exit_code=$?

    if [[ $exit_code -ne 0 ]]; then
        log_error "Expected exit code 0 for 'git-daft' with no args, got $exit_code"
        return 1
    fi

    return 0
}

# Test that completely unrelated input gives error but no suggestions
test_unknown_command_no_suggestions() {
    local output
    output=$(daft completely-unrelated-xyzzy 2>&1)
    local exit_code=$?

    if [[ $exit_code -ne 1 ]]; then
        log_error "Expected exit code 1, got $exit_code"
        return 1
    fi

    if [[ "$output" == *"The most similar command"* ]]; then
        log_error "Expected no suggestions for completely unrelated input, got: $output"
        return 1
    fi

    return 0
}

# Run all unknown command tests
run_unknown_command_tests() {
    log "Running unknown command error handling tests..."

    run_test "unknown_command_daft_error" "test_unknown_command_daft_error"
    run_test "unknown_command_daft_suggestion" "test_unknown_command_daft_suggestion"
    run_test "unknown_command_git_daft_error" "test_unknown_command_git_daft_error"
    run_test "unknown_command_daft_no_args" "test_unknown_command_daft_no_args"
    run_test "unknown_command_git_daft_no_args" "test_unknown_command_git_daft_no_args"
    run_test "unknown_command_no_suggestions" "test_unknown_command_no_suggestions"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_unknown_command_tests
    print_summary
fi
