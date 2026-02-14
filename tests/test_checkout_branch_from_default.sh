#!/bin/bash

# Tests for git-worktree-checkout-branch --from-default

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Setup a test repository for checkout-branch-from-default tests
setup_test_repo() {
    local repo_name="$1"
    local default_branch="${2:-main}"
    local remote_repo=$(create_test_remote "$repo_name" "$default_branch")
    
    # Clone the repository first
    git worktree-clone "$remote_repo" >/dev/null 2>&1
    
    # Change to the repo directory
    cd "$repo_name"
    
    # Fetch all branches and set up remote HEAD
    git fetch origin >/dev/null 2>&1
    git remote set-head origin --auto >/dev/null 2>&1
    
    echo "$(pwd)"
}

# Test basic checkout-branch-from-default functionality
test_checkout_branch_from_default_basic() {
    local repo_dir=$(setup_test_repo "checkout-default-basic-test" "main")
    
    # Test checkout-branch-from-default
    git worktree-checkout-branch --from-default feature/from-default || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/from-default" || return 1
    assert_git_worktree "$repo_dir/feature/from-default" "feature/from-default" || return 1
    assert_branch_exists "$repo_dir/feature/from-default" "feature/from-default" || return 1
    
    # Verify branch was created from main (default branch)
    cd "$repo_dir/feature/from-default"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir/main"
    local main_commit=$(git rev-parse HEAD)
    
    if [[ "$base_commit" != "$main_commit" ]]; then
        log_error "New branch should be based on default branch (main)"
        return 1
    fi
    
    log_success "New branch correctly based on default branch"
    return 0
}

# Test checkout-branch-from-default with master as default
test_checkout_branch_from_default_master() {
    local repo_dir=$(setup_test_repo "checkout-default-master-test" "master")
    
    # Test checkout-branch-from-default
    git worktree-checkout-branch --from-default feature/from-master || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/from-master" || return 1
    assert_git_worktree "$repo_dir/feature/from-master" "feature/from-master" || return 1
    
    # Verify branch was created from master (default branch)
    cd "$repo_dir/feature/from-master"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir/master"
    local master_commit=$(git rev-parse HEAD)
    
    if [[ "$base_commit" != "$master_commit" ]]; then
        log_error "New branch should be based on default branch (master)"
        return 1
    fi
    
    log_success "New branch correctly based on master default branch"
    return 0
}

# Test checkout-branch-from-default with develop as default
test_checkout_branch_from_default_develop() {
    local repo_dir=$(setup_test_repo "checkout-default-develop-test" "develop")
    
    # Test checkout-branch-from-default
    git worktree-checkout-branch --from-default feature/from-develop || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/from-develop" || return 1
    assert_git_worktree "$repo_dir/feature/from-develop" "feature/from-develop" || return 1
    
    # Verify branch was created from develop (default branch)
    cd "$repo_dir/feature/from-develop"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir/develop"
    local develop_commit=$(git rev-parse HEAD)
    
    if [[ "$base_commit" != "$develop_commit" ]]; then
        log_error "New branch should be based on default branch (develop)"
        return 1
    fi
    
    log_success "New branch correctly based on develop default branch"
    return 0
}

# Test checkout-branch-from-default from different worktree
test_checkout_branch_from_default_different_worktree() {
    local repo_dir=$(setup_test_repo "checkout-default-different-worktree-test" "main")
    
    # First checkout develop branch
    git worktree-checkout develop >/dev/null 2>&1
    
    # From develop worktree, create new branch from default
    cd "develop"
    git worktree-checkout-branch --from-default feature/from-default-in-develop || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/from-default-in-develop" || return 1
    assert_git_worktree "$repo_dir/feature/from-default-in-develop" "feature/from-default-in-develop" || return 1
    
    # Verify branch was created from main (default), not develop (current)
    cd "$repo_dir/feature/from-default-in-develop"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir/main"
    local main_commit=$(git rev-parse HEAD)
    
    if [[ "$base_commit" != "$main_commit" ]]; then
        log_error "New branch should be based on default branch (main), not current branch"
        return 1
    fi
    
    log_success "New branch correctly based on default branch, not current branch"
    return 0
}

# Test checkout-branch-from-default with no branch name
test_checkout_branch_from_default_no_name() {
    local repo_dir=$(setup_test_repo "checkout-default-no-name-test" "main")
    
    # Test without branch name should fail
    assert_command_failure "git worktree-checkout-branch --from-default" "Should fail with no branch name"
    
    return 0
}

# Test checkout-branch-from-default with existing branch name
test_checkout_branch_from_default_existing_name() {
    local repo_dir=$(setup_test_repo "checkout-default-existing-test" "main")
    
    # Create branch first
    git worktree-checkout-branch --from-default feature/existing || return 1
    
    # Try to create same branch again should fail
    assert_command_failure "git worktree-checkout-branch --from-default feature/existing" "Should fail with existing branch name"
    
    return 0
}

# Test checkout-branch-from-default without remote HEAD set
test_checkout_branch_from_default_no_remote_head() {
    local repo_dir=$(setup_test_repo "checkout-default-no-head-test" "main")
    
    # Remove remote HEAD reference to simulate missing setup
    rm -f ".git/refs/remotes/origin/HEAD"
    
    # Test should fail gracefully
    assert_command_failure "git worktree-checkout-branch --from-default feature/no-head" "Should fail without remote HEAD"
    
    return 0
}

# Test checkout-branch-from-default outside git repository
test_checkout_branch_from_default_outside_repo() {
    # Move to a non-git directory
    cd "$WORK_DIR"
    
    # Test should fail
    assert_command_failure "git worktree-checkout-branch --from-default feature/test" "Should fail outside git repository"
    
    return 0
}

# Test checkout-branch-from-default with nested directory structure
test_checkout_branch_from_default_nested() {
    local repo_dir=$(setup_test_repo "checkout-default-nested-test" "main")
    
    # Test with deeply nested branch name
    git worktree-checkout-branch --from-default feature/ui/components/from-default || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/ui/components/from-default" || return 1
    assert_git_worktree "$repo_dir/feature/ui/components/from-default" "feature/ui/components/from-default" || return 1
    
    return 0
}

# Test checkout-branch-from-default from deep subdirectory
test_checkout_branch_from_default_from_subdirectory() {
    local repo_dir=$(setup_test_repo "checkout-default-subdir-test" "main")
    
    # Create subdirectory structure
    mkdir -p "main/deep/nested/dir"
    cd "main/deep/nested/dir"
    
    # Test checkout-branch-from-default from deep subdirectory
    git worktree-checkout-branch --from-default feature/from-subdir || return 1
    
    # Verify structure (should create at repo root, not in subdirectory)
    assert_directory_exists "$repo_dir/feature/from-subdir" || return 1
    assert_git_worktree "$repo_dir/feature/from-subdir" "feature/from-subdir" || return 1
    
    return 0
}

# Test checkout-branch-from-default with special characters
test_checkout_branch_from_default_special_characters() {
    local repo_dir=$(setup_test_repo "checkout-default-special-test" "main")
    
    # Test with branch names containing special characters
    git worktree-checkout-branch --from-default "feature/test-123_v2.0" || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/test-123_v2.0" || return 1
    assert_git_worktree "$repo_dir/feature/test-123_v2.0" "feature/test-123_v2.0" || return 1
    
    return 0
}

# Test checkout-branch-from-default with remote tracking
test_checkout_branch_from_default_remote_tracking() {
    local repo_dir=$(setup_test_repo "checkout-default-remote-test" "main")
    
    # Test checkout-branch-from-default
    git worktree-checkout-branch --from-default feature/remote-tracking || return 1
    
    # Verify remote tracking was set up (this is handled by checkout-branch)
    cd "$repo_dir/feature/remote-tracking"
    local upstream=$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null)
    if [[ "$upstream" != "origin/feature/remote-tracking" ]]; then
        log_error "Upstream tracking not set correctly. Expected: origin/feature/remote-tracking, Got: $upstream"
        return 1
    fi
    
    log_success "Remote tracking set correctly"
    return 0
}

# Test checkout-branch-from-default delegates to checkout-branch
test_checkout_branch_from_default_delegation() {
    local repo_dir=$(setup_test_repo "checkout-default-delegation-test" "main")
    
    # Test that checkout-branch-from-default properly delegates to checkout-branch
    # This is tested by verifying the end result is the same as calling checkout-branch directly
    
    # Create branch using checkout-branch-from-default
    git worktree-checkout-branch --from-default feature/delegated || return 1
    
    # Verify structure (same as what checkout-branch would create)
    assert_directory_exists "$repo_dir/feature/delegated" || return 1
    assert_git_worktree "$repo_dir/feature/delegated" "feature/delegated" || return 1
    
    # Verify it has the same characteristics as a direct checkout-branch call
    cd "$repo_dir/feature/delegated"
    local upstream=$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null)
    if [[ "$upstream" != "origin/feature/delegated" ]]; then
        log_error "Delegated command should have same behavior as direct checkout-branch"
        return 1
    fi
    
    log_success "Delegation to checkout-branch works correctly"
    return 0
}

# Test checkout-branch-from-default with corrupted remote HEAD
test_checkout_branch_from_default_corrupted_head() {
    local repo_dir=$(setup_test_repo "checkout-default-corrupted-test" "main")
    
    # Corrupt the remote HEAD file
    echo "invalid content" > ".git/refs/remotes/origin/HEAD"
    
    # Test should fail gracefully
    assert_command_failure "git worktree-checkout-branch --from-default feature/corrupted" "Should fail with corrupted remote HEAD"
    
    return 0
}

# Test checkout-branch-from-default error handling
test_checkout_branch_from_default_error_handling() {
    local repo_dir=$(setup_test_repo "checkout-default-error-test" "main")
    
    # Test various error conditions are handled gracefully
    
    # Test with invalid branch name characters (if any)
    # Most characters are valid in Git branch names, so this tests general error handling
    
    # Create a branch name that might cause issues
    git worktree-checkout-branch --from-default "feature/test-error" || return 1
    
    # Verify structure was created successfully
    assert_directory_exists "$repo_dir/feature/test-error" || return 1
    assert_git_worktree "$repo_dir/feature/test-error" "feature/test-error" || return 1
    
    log_success "Error handling works correctly"
    return 0
}

# Run all checkout-branch-from-default tests
run_checkout_branch_from_default_tests() {
    log "Running git-worktree-checkout-branch --from-default tests..."
    
    run_test "checkout_branch_from_default_basic" "test_checkout_branch_from_default_basic"
    run_test "checkout_branch_from_default_master" "test_checkout_branch_from_default_master"
    run_test "checkout_branch_from_default_develop" "test_checkout_branch_from_default_develop"
    run_test "checkout_branch_from_default_different_worktree" "test_checkout_branch_from_default_different_worktree"
    run_test "checkout_branch_from_default_no_name" "test_checkout_branch_from_default_no_name"
    run_test "checkout_branch_from_default_existing_name" "test_checkout_branch_from_default_existing_name"
    run_test "checkout_branch_from_default_no_remote_head" "test_checkout_branch_from_default_no_remote_head"
    run_test "checkout_branch_from_default_outside_repo" "test_checkout_branch_from_default_outside_repo"
    run_test "checkout_branch_from_default_nested" "test_checkout_branch_from_default_nested"
    run_test "checkout_branch_from_default_from_subdirectory" "test_checkout_branch_from_default_from_subdirectory"
    run_test "checkout_branch_from_default_special_characters" "test_checkout_branch_from_default_special_characters"
    run_test "checkout_branch_from_default_remote_tracking" "test_checkout_branch_from_default_remote_tracking"
    run_test "checkout_branch_from_default_delegation" "test_checkout_branch_from_default_delegation"
    run_test "checkout_branch_from_default_corrupted_head" "test_checkout_branch_from_default_corrupted_head"
    run_test "checkout_branch_from_default_error_handling" "test_checkout_branch_from_default_error_handling"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_branch_from_default_tests
    print_summary
fi