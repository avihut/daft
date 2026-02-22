#!/bin/bash

# Master test runner for all daft commands

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Import all test modules
source "$(dirname "${BASH_SOURCE[0]}")/test_clone.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_checkout.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_init.sh"

# Test framework self-tests
test_framework_assertions() {
    # Test successful assertions
    assert_command_success "true" || return 1
    assert_command_failure "false" || return 1
    
    # Create test directory for file/directory assertions
    mkdir -p "test_dir"
    touch "test_dir/test_file"
    
    assert_directory_exists "test_dir" || return 1
    assert_file_exists "test_dir/test_file" || return 1
    
    # Clean up
    rm -rf "test_dir"
    
    return 0
}

# Test remote repository creation
test_remote_repo_creation() {
    local remote_repo=$(create_test_remote "test-remote-creation" "main")
    
    # Verify remote repository was created
    assert_directory_exists "$remote_repo" || return 1
    assert_git_repository "$remote_repo" || return 1
    
    # Verify we can clone from it
    git clone "$remote_repo" "test-clone" >/dev/null 2>&1 || return 1
    assert_directory_exists "test-clone" || return 1
    assert_git_repository "test-clone" || return 1
    assert_file_exists "test-clone/README.md" || return 1
    
    return 0
}

# Integration test: Full workflow
test_full_workflow() {
    # Test complete workflow: init -> checkout branches -> prune
    
    # Step 1: Initialize a new repository
    git worktree-init test-workflow || return 1
    assert_directory_exists "test-workflow" || return 1
    assert_git_worktree "test-workflow/master" "master" || return 1
    
    cd "test-workflow"
    
    # Step 2: Create some commits
    cd "master"
    echo "# Test Workflow" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ..
    
    # Step 3: Create new branches with checkout -b (suppress push errors)
    git worktree-checkout -b feature/test-feature >/dev/null 2>&1 || true
    assert_directory_exists "feature/test-feature" || return 1
    assert_git_worktree "feature/test-feature" "feature/test-feature" || return 1
    
    # Step 4: Create branch from current branch (since we don't have a remote)
    git worktree-checkout -b bugfix/test-bug >/dev/null 2>&1 || true
    assert_directory_exists "bugfix/test-bug" || return 1
    assert_git_worktree "bugfix/test-bug" "bugfix/test-bug" || return 1
    
    # Step 5: Verify all worktrees exist
    local worktree_count=$(git worktree list | wc -l)
    # We expect 4 worktrees: bare repo + 3 working trees
    if [[ $worktree_count -ne 4 ]]; then
        log_error "Expected 4 worktrees, got $worktree_count"
        git worktree list >&2
        return 1
    fi
    
    log_success "Full workflow test completed successfully"
    return 0
}

# Performance test: Large repository simulation
test_performance_basic() {
    # Test performance with a repository that has many files
    git worktree-init perf-test || return 1
    
    cd "perf-test/master"
    
    # Create many files to simulate larger repository
    for i in {1..100}; do
        echo "File $i content" > "file_$i.txt"
    done
    
    git add . >/dev/null 2>&1
    git commit -m "Add many files" >/dev/null 2>&1
    
    cd ..
    
    # Test checkout operations are still fast
    local start_time=$(date +%s)
    # Suppress push errors since there's no remote
    git worktree-checkout -b performance-branch >/dev/null 2>&1 || true
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 10 ]]; then
        log_warning "Performance test took ${duration}s (expected < 10s)"
    else
        log_success "Performance test completed in ${duration}s"
    fi
    
    assert_directory_exists "performance-branch" || return 1
    assert_git_worktree "performance-branch" "performance-branch" || return 1
    
    return 0
}

# Error handling test: Cleanup on failure
test_error_handling() {
    # Test that failed operations don't leave partial state
    
    # Create a repository
    git worktree-init error-test || return 1
    cd "error-test"
    
    # Try to create worktree with invalid branch name (should fail cleanly)
    assert_command_failure "git worktree-checkout nonexistent-branch" "Should fail with nonexistent branch"
    
    # Verify no partial worktree was created
    if [[ -d "nonexistent-branch" ]]; then
        log_error "Partial worktree directory should not exist after failed operation"
        return 1
    fi
    
    log_success "Error handling test passed - no partial state left"
    return 0
}

# Cross-platform compatibility test
test_cross_platform() {
    # Test operations that might behave differently on different platforms
    
    # Test with branch names that might cause issues
    git worktree-init compat-test || return 1
    cd "compat-test"
    
    # Create initial commit so we have something to branch from
    cd "master"
    echo "# Compatibility Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ..
    
    # Test with various branch name formats
    local branch_names=("feature/test" "bugfix-123" "hotfix_urgent" "release-v1.0.0")
    
    for branch in "${branch_names[@]}"; do
        # Suppress push errors since there's no remote
        git worktree-checkout -b "$branch" >/dev/null 2>&1 || true
        assert_directory_exists "$branch" || return 1
        assert_git_worktree "$branch" "$branch" || return 1
    done
    
    log_success "Cross-platform compatibility test passed"
    return 0
}

# Security test: Path traversal prevention
test_security_path_traversal() {
    # Test that malicious paths are handled safely
    
    git worktree-init security-test || return 1
    cd "security-test"
    
    # Test with path traversal attempts (should fail or be sanitized)
    assert_command_failure "git worktree-checkout -b ../../../etc/passwd" "Should fail with path traversal attempt"
    assert_command_failure "git worktree-checkout -b ..\\..\\..\\windows\\system32" "Should fail with Windows path traversal"
    
    # Verify no directories were created outside the repository
    if [[ -d "../../../etc" ]] || [[ -d "..\\..\\..\\windows" ]]; then
        log_error "Path traversal attack succeeded - security vulnerability!"
        return 1
    fi
    
    log_success "Security test passed - path traversal prevented"
    return 0
}

# Run all tests
run_all_tests() {
    log "Running comprehensive test suite for daft..."
    
    # Framework tests
    run_test "framework_assertions" "test_framework_assertions"
    run_test "remote_repo_creation" "test_remote_repo_creation"
    
    # Command-specific tests
    run_clone_tests
    run_checkout_tests
    run_init_tests
    
    # Integration tests
    run_test "full_workflow" "test_full_workflow"
    run_test "performance_basic" "test_performance_basic"
    run_test "error_handling" "test_error_handling"
    run_test "cross_platform" "test_cross_platform"
    run_test "security_path_traversal" "test_security_path_traversal"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_all_tests
    print_summary
    
    # Exit with appropriate code
    if [[ $TESTS_FAILED -eq 0 ]]; then
        exit 0
    else
        exit 1
    fi
fi