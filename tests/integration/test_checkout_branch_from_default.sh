#!/bin/bash

# Integration tests for git-worktree-checkout-branch-from-default Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic checkout-branch-from-default functionality
test_checkout_branch_from_default_basic() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default"
    
    # Switch to develop branch to make sure we're not on default
    git-worktree-checkout develop || return 1
    cd "develop"
    
    # Test checkout-branch-from-default (should create branch from main, not develop)
    git-worktree-checkout-branch-from-default feature/from-default || return 1
    
    # Verify structure
    assert_directory_exists "../feature/from-default" || return 1
    assert_git_worktree "../feature/from-default" "feature/from-default" || return 1
    
    return 0
}

# Test checkout-branch-from-default with different default branch
test_checkout_branch_from_default_develop() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-develop" "develop")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-develop"
    
    # Test checkout-branch-from-default with develop as default
    git-worktree-checkout-branch-from-default feature/from-develop || return 1
    
    # Verify structure
    assert_directory_exists "feature/from-develop" || return 1
    assert_git_worktree "feature/from-develop" "feature/from-develop" || return 1
    
    return 0
}

# Test checkout-branch-from-default from subdirectory
test_checkout_branch_from_default_subdir() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-subdir" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-subdir"
    
    # Create a subdirectory and test from there
    mkdir -p "main/subdir/deeper"
    cd "main/subdir/deeper"
    
    # Test checkout-branch-from-default from deep subdirectory
    git-worktree-checkout-branch-from-default feature/from-subdir || return 1
    
    # Verify structure (should be created at repository root)
    assert_directory_exists "../../../feature/from-subdir" || return 1
    assert_git_worktree "../../../feature/from-subdir" "feature/from-subdir" || return 1
    
    return 0
}

# Test checkout-branch-from-default error handling
test_checkout_branch_from_default_errors() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-errors" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-errors"
    
    # Test checkout-branch-from-default with no branch name
    assert_command_failure "git-worktree-checkout-branch-from-default" "Should fail without branch name"
    
    # Test checkout-branch-from-default with invalid branch name
    assert_command_failure "git-worktree-checkout-branch-from-default 'invalid branch name'" "Should fail with invalid branch name"
    
    # Test checkout-branch-from-default with existing branch
    git-worktree-checkout-branch-from-default feature/test || return 1
    assert_command_failure "git-worktree-checkout-branch-from-default feature/test" "Should fail with existing branch"
    
    return 0
}

# Test checkout-branch-from-default with various branch naming conventions
test_checkout_branch_from_default_naming() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-naming" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-naming"
    
    # Test various branch naming conventions
    local branch_names=("feature/user-auth" "bugfix-123" "hotfix_urgent" "release-v1.0.0" "chore/update-deps")
    
    for branch in "${branch_names[@]}"; do
        git-worktree-checkout-branch-from-default "$branch" || return 1
        assert_directory_exists "$branch" || return 1
        assert_git_worktree "$branch" "$branch" || return 1
    done
    
    return 0
}

# Test checkout-branch-from-default with direnv integration
test_checkout_branch_from_default_direnv() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-direnv" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-direnv"
    
    # Create a branch with .envrc
    git-worktree-checkout-branch-from-default feature/with-envrc || return 1
    
    # Add .envrc file
    echo "export TEST_VAR=feature_value" > "feature/with-envrc/.envrc"
    
    # The binary should handle direnv gracefully
    assert_directory_exists "feature/with-envrc" || return 1
    assert_file_exists "feature/with-envrc/.envrc" || return 1
    
    return 0
}

# Test checkout-branch-from-default outside git repository
test_checkout_branch_from_default_outside_repo() {
    # Test checkout-branch-from-default command outside git repository
    assert_command_failure "git-worktree-checkout-branch-from-default some-branch" "Should fail outside git repository"
    
    return 0
}

# Test checkout-branch-from-default help functionality
test_checkout_branch_from_default_help() {
    # Test help commands
    assert_command_help "git-worktree-checkout-branch-from-default" || return 1
    assert_command_version "git-worktree-checkout-branch-from-default" || return 1
    
    return 0
}

# Test checkout-branch-from-default with modified default branch
test_checkout_branch_from_default_modified() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-modified" "main")
    
    # Add commits to default branch
    local temp_clone="$TEMP_BASE_DIR/temp_modified_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        echo "Modified default branch" > modified.txt
        git add modified.txt >/dev/null 2>&1
        git commit -m "Modify default branch" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-modified"
    
    # Create branch from modified default
    git-worktree-checkout-branch-from-default feature/from-modified || return 1
    
    # Verify structure and content
    assert_directory_exists "feature/from-modified" || return 1
    assert_git_worktree "feature/from-modified" "feature/from-modified" || return 1
    assert_file_exists "feature/from-modified/modified.txt" || return 1
    
    return 0
}

# Test checkout-branch-from-default performance
test_checkout_branch_from_default_performance() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-perf" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-perf"
    
    # Test checkout-branch-from-default performance
    local start_time=$(date +%s)
    git-worktree-checkout-branch-from-default feature/performance-test || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 10 ]]; then
        log_warning "Checkout-branch-from-default performance test took ${duration}s (expected < 10s)"
    else
        log_success "Checkout-branch-from-default performance test completed in ${duration}s"
    fi
    
    # Verify structure
    assert_directory_exists "feature/performance-test" || return 1
    assert_git_worktree "feature/performance-test" "feature/performance-test" || return 1
    
    return 0
}

# Test checkout-branch-from-default with large repository
test_checkout_branch_from_default_large_repo() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-large" "main")
    
    # Add many files to the repository
    local temp_clone="$TEMP_BASE_DIR/temp_large_default_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        # Create many files on main branch
        for i in {1..50}; do
            echo "Large repo test file $i" > "large_file_$i.txt"
        done
        git add . >/dev/null 2>&1
        git commit -m "Add many files to main" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-large"
    
    # Test checkout-branch-from-default with large repository
    git-worktree-checkout-branch-from-default feature/large-test || return 1
    
    # Verify structure and some files
    assert_directory_exists "feature/large-test" || return 1
    assert_file_exists "feature/large-test/large_file_1.txt" || return 1
    assert_file_exists "feature/large-test/large_file_50.txt" || return 1
    
    return 0
}

# Test checkout-branch-from-default with remote updates
test_checkout_branch_from_default_remote_updates() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-remote" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-remote"
    
    # Update remote default branch
    local temp_clone="$TEMP_BASE_DIR/temp_remote_update_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        echo "Remote update" > remote_update.txt
        git add remote_update.txt >/dev/null 2>&1
        git commit -m "Update remote default branch" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Fetch updates
    git fetch origin >/dev/null 2>&1
    
    # Create branch from updated default
    git-worktree-checkout-branch-from-default feature/from-updated || return 1
    
    # Verify structure and content
    assert_directory_exists "feature/from-updated" || return 1
    assert_git_worktree "feature/from-updated" "feature/from-updated" || return 1
    assert_file_exists "feature/from-updated/remote_update.txt" || return 1
    
    return 0
}

# Test checkout-branch-from-default security - path traversal prevention
test_checkout_branch_from_default_security() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-from-default-security" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-from-default-security"
    
    # Test that path traversal attempts are handled safely
    assert_command_failure "git-worktree-checkout-branch-from-default ../../../etc/passwd" "Should fail with path traversal attempt"
    assert_command_failure "git-worktree-checkout-branch-from-default ..\\..\\..\\windows\\system32" "Should fail with Windows path traversal"
    
    # Verify no directories were created outside the repository
    if [[ -d "../../../etc" ]] || [[ -d "..\\..\\..\\windows" ]]; then
        log_error "Path traversal attack succeeded - security vulnerability!"
        return 1
    fi
    
    return 0
}

# Run all checkout-branch-from-default tests
run_checkout_branch_from_default_tests() {
    log "Running git-worktree-checkout-branch-from-default integration tests..."
    
    run_test "checkout_branch_from_default_basic" "test_checkout_branch_from_default_basic"
    run_test "checkout_branch_from_default_develop" "test_checkout_branch_from_default_develop"
    run_test "checkout_branch_from_default_subdir" "test_checkout_branch_from_default_subdir"
    run_test "checkout_branch_from_default_errors" "test_checkout_branch_from_default_errors"
    run_test "checkout_branch_from_default_naming" "test_checkout_branch_from_default_naming"
    run_test "checkout_branch_from_default_direnv" "test_checkout_branch_from_default_direnv"
    run_test "checkout_branch_from_default_outside_repo" "test_checkout_branch_from_default_outside_repo"
    run_test "checkout_branch_from_default_help" "test_checkout_branch_from_default_help"
    run_test "checkout_branch_from_default_modified" "test_checkout_branch_from_default_modified"
    run_test "checkout_branch_from_default_performance" "test_checkout_branch_from_default_performance"
    run_test "checkout_branch_from_default_large_repo" "test_checkout_branch_from_default_large_repo"
    run_test "checkout_branch_from_default_remote_updates" "test_checkout_branch_from_default_remote_updates"
    run_test "checkout_branch_from_default_security" "test_checkout_branch_from_default_security"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_branch_from_default_tests
    print_summary
fi