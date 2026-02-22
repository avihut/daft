#!/bin/bash

# Simple integration tests to validate the testing framework and basic functionality

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic dependencies
test_simple_dependencies() {
    # Test required commands exist
    assert_command_success "which git" "Git should be available" || return 1
    assert_command_success "which awk" "AWK should be available" || return 1
    assert_command_success "which basename" "basename should be available" || return 1
    
    return 0
}

# Test file operations
test_simple_file_operations() {
    # Test file creation and detection
    echo "test content" > "test-file.txt"
    assert_file_exists "test-file.txt" || return 1
    
    # Test directory creation and detection
    mkdir -p "test-dir"
    assert_directory_exists "test-dir" || return 1
    
    # Clean up
    rm -f "test-file.txt"
    rm -rf "test-dir"
    
    return 0
}

# Test Rust binary availability
test_simple_rust_binaries() {
    # Test that all Rust binaries are available
    local binaries=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-prune")
    
    for binary in "${binaries[@]}"; do
        assert_command_success "command -v $binary" "Binary $binary should be available" || return 1
    done
    
    return 0
}

# Test command help functionality
test_simple_help_commands() {
    # Test help commands
    assert_command_help "git-worktree-init" || return 1
    assert_command_help "git-worktree-clone" || return 1
    
    return 0
}

# Test git-worktree-init bare mode (this should work)
test_simple_init_bare() {
    # Test bare init
    git-worktree-init --bare simple-bare-repo || return 1
    
    # Verify structure
    assert_directory_exists "simple-bare-repo" || return 1
    assert_directory_exists "simple-bare-repo/.git" || return 1
    assert_git_repository "simple-bare-repo/.git" || return 1
    
    # Should NOT have worktree
    if [[ -d "simple-bare-repo/master" ]]; then
        log_error "Bare mode should not create worktree"
        return 1
    fi
    
    return 0
}

# Run simple tests
run_simple_integration_tests() {
    log "Running simple integration validation tests..."
    
    run_test "simple_dependencies" "test_simple_dependencies"
    run_test "simple_file_operations" "test_simple_file_operations"
    run_test "simple_rust_binaries" "test_simple_rust_binaries"
    run_test "simple_help_commands" "test_simple_help_commands"
    run_test "simple_init_bare" "test_simple_init_bare"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_simple_integration_tests
    print_summary
fi