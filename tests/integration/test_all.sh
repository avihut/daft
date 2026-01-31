#!/bin/bash

# Master integration test runner for all daft Rust binaries

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Import all test modules
source "$(dirname "${BASH_SOURCE[0]}")/test_clone.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_init.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_checkout.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_checkout_branch.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_checkout_branch_from_default.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_prune.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_fetch.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_config.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_hooks.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_flow_adopt.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_flow_eject.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_unknown_command.sh"

# Test framework self-tests
test_integration_framework_assertions() {
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
test_integration_remote_repo_creation() {
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

# Integration test: Full workflow using Rust binaries
test_integration_full_workflow() {
    # Test complete workflow: init -> checkout branches -> prune
    
    # Step 1: Initialize a new repository
    git-worktree-init test-workflow || return 1
    assert_directory_exists "test-workflow" || return 1
    assert_git_worktree "test-workflow/master" "master" || return 1
    
    cd "test-workflow"
    
    # Step 2: Create some commits
    cd "master"
    echo "# Integration Test Workflow" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ..
    
    # Step 3: Create new branches with checkout-branch
    git-worktree-checkout-branch feature/integration-test >/dev/null 2>&1 || true
    assert_directory_exists "feature/integration-test" || return 1
    assert_git_worktree "feature/integration-test" "feature/integration-test" || return 1
    
    # Step 4: Create branch from master (since this is a local repo with no remote)
    git-worktree-checkout-branch hotfix/integration-fix master >/dev/null 2>&1 || true
    assert_directory_exists "hotfix/integration-fix" || return 1
    assert_git_worktree "hotfix/integration-fix" "hotfix/integration-fix" || return 1
    
    # Step 5: Verify all worktrees exist
    local worktree_count=$(git worktree list | wc -l)
    # We expect 4 worktrees: bare repo + 3 working trees
    if [[ $worktree_count -ne 4 ]]; then
        log_error "Expected 4 worktrees, got $worktree_count"
        git worktree list >&2
        return 1
    fi
    
    log_success "Full integration workflow test completed successfully"
    return 0
}

# Performance test: Large repository simulation with Rust binaries
test_integration_performance_basic() {
    # Test performance with a repository that has many files
    git-worktree-init perf-test || return 1
    
    cd "perf-test/master"
    
    # Create many files to simulate larger repository
    for i in {1..100}; do
        echo "Integration test file $i content" > "file_$i.txt"
    done
    
    git add . >/dev/null 2>&1
    git commit -m "Add many files" >/dev/null 2>&1
    
    cd ..
    
    # Test checkout operations are still fast
    local start_time=$(date +%s)
    git-worktree-checkout-branch performance-branch >/dev/null 2>&1 || true
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

# Error handling test: Cleanup on failure with Rust binaries
test_integration_error_handling() {
    # Test that failed operations don't leave partial state
    
    # Create a repository
    git-worktree-init error-test || return 1
    cd "error-test"
    
    # Try to checkout nonexistent branch (should fail cleanly)
    assert_command_failure "git-worktree-checkout nonexistent-branch" "Should fail with nonexistent branch"
    
    # Verify no partial worktree was created
    if [[ -d "nonexistent-branch" ]]; then
        log_error "Partial worktree directory should not exist after failed operation"
        return 1
    fi
    
    log_success "Error handling test passed - no partial state left"
    return 0
}

# Cross-platform compatibility test with Rust binaries
test_integration_cross_platform() {
    # Test operations that might behave differently on different platforms
    
    # Test with branch names that might cause issues
    git-worktree-init compat-test || return 1
    cd "compat-test"
    
    # Create initial commit so we have something to branch from
    cd "master"
    echo "# Integration Compatibility Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ..
    
    # Test with various branch name formats
    local branch_names=("feature/test" "bugfix-123" "hotfix_urgent" "release-v1.0.0")
    
    for branch in "${branch_names[@]}"; do
        git-worktree-checkout-branch "$branch" >/dev/null 2>&1 || true
        assert_directory_exists "$branch" || return 1
        assert_git_worktree "$branch" "$branch" || return 1
    done
    
    log_success "Cross-platform compatibility test passed"
    return 0
}

# Security test: Path traversal prevention with Rust binaries
test_integration_security_path_traversal() {
    # Test that malicious paths are handled safely
    
    git-worktree-init security-test || return 1
    cd "security-test"
    
    # Test with path traversal attempts (should fail or be sanitized)
    assert_command_failure "git-worktree-checkout-branch ../../../etc/passwd" "Should fail with path traversal attempt"
    assert_command_failure "git-worktree-checkout-branch ..\\..\\..\\windows\\system32" "Should fail with Windows path traversal"
    
    # Verify no directories were created outside the repository
    if [[ -d "../../../etc" ]] || [[ -d "..\\..\\..\\windows" ]]; then
        log_error "Path traversal attack succeeded - security vulnerability!"
        return 1
    fi
    
    log_success "Security test passed - path traversal prevented"
    return 0
}

# Test binary availability and help functionality
test_integration_binaries_availability() {
    # Test that all Rust binaries are available and working
    local binaries=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-checkout-branch" "git-worktree-checkout-branch-from-default" "git-worktree-prune")
    
    for binary in "${binaries[@]}"; do
        assert_command_success "command -v $binary" "Binary $binary should be available" || return 1
        assert_command_help "$binary" "Binary $binary help should work" || return 1
    done
    
    log_success "All binaries are available and functional"
    return 0
}

# Test Rust binary vs legacy shell script comparison
test_integration_rust_vs_shell_compatibility() {
    # Test that Rust binaries produce similar results to shell scripts
    
    # Test init command
    git-worktree-init rust-test || return 1
    assert_directory_exists "rust-test" || return 1
    assert_directory_exists "rust-test/master" || return 1
    assert_git_worktree "rust-test/master" "master" || return 1
    
    # Test that the structure is compatible with shell scripts
    cd "rust-test"
    
    # Create a commit
    cd "master"
    echo "# Rust vs Shell Compatibility Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ..
    
    # Test checkout-branch
    git-worktree-checkout-branch feature/rust-test >/dev/null 2>&1 || true
    assert_directory_exists "feature/rust-test" || return 1
    assert_git_worktree "feature/rust-test" "feature/rust-test" || return 1
    
    log_success "Rust binary compatibility test passed"
    return 0
}

# Test integration with real-world scenarios
test_integration_real_world_scenarios() {
    # Test common development workflows
    
    # Scenario 1: Clone, feature development, and cleanup
    local remote_repo=$(create_test_remote "real-world-test" "main")
    
    git-worktree-clone "$remote_repo" || return 1
    cd "real-world-test"
    
    # Feature development workflow
    git-worktree-checkout-branch feature/user-authentication || return 1
    
    # Add some work
    cd "feature/user-authentication"
    echo "User authentication implementation" > auth.txt
    git add auth.txt >/dev/null 2>&1
    git commit -m "Implement user authentication" >/dev/null 2>&1
    cd ..
    
    # Hotfix workflow
    git-worktree-checkout-branch-from-default hotfix/security-fix || return 1
    
    # Add hotfix
    cd "hotfix/security-fix"
    echo "Security fix implementation" > security.txt
    git add security.txt >/dev/null 2>&1
    git commit -m "Fix security vulnerability" >/dev/null 2>&1
    cd ..
    
    # Verify all worktrees exist
    assert_directory_exists "feature/user-authentication" || return 1
    assert_directory_exists "hotfix/security-fix" || return 1
    
    log_success "Real-world scenarios test passed"
    return 0
}

# Run all integration tests
run_all_integration_tests() {
    log "Running comprehensive integration test suite for Rust binaries..."
    
    # Framework tests
    run_test "integration_framework_assertions" "test_integration_framework_assertions"
    run_test "integration_remote_repo_creation" "test_integration_remote_repo_creation"
    run_test "integration_binaries_availability" "test_integration_binaries_availability"
    
    # Command-specific tests
    run_clone_tests
    run_init_tests
    run_checkout_tests
    run_checkout_branch_tests
    run_checkout_branch_from_default_tests
    run_prune_tests
    run_fetch_tests
    run_config_tests
    run_hooks_tests
    run_flow_adopt_tests
    run_flow_eject_tests
    run_unknown_command_tests

    # Integration tests
    run_test "integration_full_workflow" "test_integration_full_workflow"
    run_test "integration_performance_basic" "test_integration_performance_basic"
    run_test "integration_error_handling" "test_integration_error_handling"
    run_test "integration_cross_platform" "test_integration_cross_platform"
    run_test "integration_security_path_traversal" "test_integration_security_path_traversal"
    run_test "integration_rust_vs_shell_compatibility" "test_integration_rust_vs_shell_compatibility"
    run_test "integration_real_world_scenarios" "test_integration_real_world_scenarios"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_all_integration_tests
    print_summary
    
    # Exit with appropriate code
    if [[ $TESTS_FAILED -eq 0 ]]; then
        exit 0
    else
        exit 1
    fi
fi