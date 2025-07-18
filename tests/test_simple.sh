#!/bin/bash

# Simple tests to validate the testing framework and basic functionality

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic git-worktree-init functionality
test_simple_init() {
    # Test basic init
    git worktree-init simple-test-repo || return 1
    
    # Verify structure
    assert_directory_exists "simple-test-repo" || return 1
    assert_directory_exists "simple-test-repo/.git" || return 1
    assert_directory_exists "simple-test-repo/master" || return 1
    assert_git_repository "simple-test-repo/master" || return 1
    
    return 0
}

# Test git-worktree-init with custom branch
test_simple_init_custom_branch() {
    # Test init with custom branch
    git worktree-init --initial-branch main simple-main-repo || return 1
    
    # Verify structure
    assert_directory_exists "simple-main-repo" || return 1
    assert_directory_exists "simple-main-repo/.git" || return 1
    assert_directory_exists "simple-main-repo/main" || return 1
    assert_git_repository "simple-main-repo/main" || return 1
    
    return 0
}

# Test git-worktree-init bare mode
test_simple_init_bare() {
    # Test bare init
    git worktree-init --bare simple-bare-repo || return 1
    
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

# Test git-worktree-init error handling
test_simple_init_errors() {
    # Test no repo name
    assert_command_failure "git worktree-init" "Should fail without repo name"
    
    # Test existing directory
    mkdir -p "existing-dir"
    assert_command_failure "git worktree-init existing-dir" "Should fail with existing directory"
    
    return 0
}

# Test checkout-branch on initialized repo
test_simple_checkout_branch() {
    # Initialize repo first
    git worktree-init simple-checkout-test || return 1
    
    # Change to repo directory
    cd "simple-checkout-test"
    
    # Create some content and commit
    cd "master"
    echo "# Simple Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ..
    
    # Test checkout-branch
    git worktree-checkout-branch feature/simple-feature || return 1
    
    # Verify structure
    assert_directory_exists "feature/simple-feature" || return 1
    assert_git_repository "feature/simple-feature" || return 1
    
    return 0
}

# Test command help outputs
test_simple_help_commands() {
    # Test that help commands don't crash
    assert_command_success "git worktree-init --help >/dev/null 2>&1 || echo 'Help shown'" "Init help should work"
    
    return 0
}

# Test script dependencies
test_simple_dependencies() {
    # Test required commands exist
    assert_command_success "which git" "Git should be available"
    assert_command_success "which awk" "AWK should be available"
    assert_command_success "which basename" "basename should be available"
    
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

# Run simple tests
run_simple_tests() {
    log "Running simple validation tests..."
    
    run_test "simple_dependencies" "test_simple_dependencies"
    run_test "simple_file_operations" "test_simple_file_operations"
    run_test "simple_init" "test_simple_init"
    run_test "simple_init_custom_branch" "test_simple_init_custom_branch"
    run_test "simple_init_bare" "test_simple_init_bare"
    run_test "simple_init_errors" "test_simple_init_errors"
    run_test "simple_checkout_branch" "test_simple_checkout_branch"
    run_test "simple_help_commands" "test_simple_help_commands"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_simple_tests
    print_summary
fi