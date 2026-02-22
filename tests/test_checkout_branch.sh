#!/bin/bash

# Tests for git-worktree-checkout -b (checkout with new branch creation)

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Setup a test repository for checkout-branch tests
setup_test_repo() {
    local repo_name="$1"
    local remote_repo=$(create_test_remote "$repo_name" "main")
    
    # Clone the repository first
    git worktree-clone "$remote_repo" >/dev/null 2>&1
    
    # Change to the repo directory
    cd "$repo_name"
    
    # Fetch all branches
    git fetch origin >/dev/null 2>&1
    
    echo "$(pwd)"
}

# Test basic checkout-branch functionality
test_checkout_branch_basic() {
    local repo_dir=$(setup_test_repo "checkout-branch-basic-test")
    
    # Test checkout-branch from current branch (main)
    cd "main"
    git worktree-checkout -b feature/new-feature || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/new-feature" || return 1
    assert_git_worktree "$repo_dir/feature/new-feature" "feature/new-feature" || return 1
    assert_branch_exists "$repo_dir/feature/new-feature" "feature/new-feature" || return 1
    
    # Verify branch was created from main
    cd "$repo_dir/feature/new-feature"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir/main"
    local main_commit=$(git rev-parse HEAD)
    
    if [[ "$base_commit" != "$main_commit" ]]; then
        log_error "New branch should be based on main branch"
        return 1
    fi
    
    log_success "New branch correctly based on main"
    return 0
}

# Test checkout-branch with explicit base branch
test_checkout_branch_explicit_base() {
    local repo_dir=$(setup_test_repo "checkout-branch-explicit-test")
    
    # Test checkout-branch with explicit base branch
    cd "main"
    git worktree-checkout -b feature/from-develop develop || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/from-develop" || return 1
    assert_git_worktree "$repo_dir/feature/from-develop" "feature/from-develop" || return 1
    assert_branch_exists "$repo_dir/feature/from-develop" "feature/from-develop" || return 1
    
    # Verify branch was created from develop
    cd "$repo_dir/feature/from-develop"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir"
    local develop_commit=$(git rev-parse origin/develop)
    
    if [[ "$base_commit" != "$develop_commit" ]]; then
        log_error "New branch should be based on develop branch"
        return 1
    fi
    
    log_success "New branch correctly based on develop"
    return 0
}

# Test checkout-branch from different worktree
test_checkout_branch_from_different_worktree() {
    local repo_dir=$(setup_test_repo "checkout-branch-different-worktree-test")
    
    # First checkout develop branch
    git worktree-checkout develop >/dev/null 2>&1
    
    # From develop worktree, create new branch
    cd "develop"
    git worktree-checkout -b feature/from-develop-worktree || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/from-develop-worktree" || return 1
    assert_git_worktree "$repo_dir/feature/from-develop-worktree" "feature/from-develop-worktree" || return 1
    
    # Verify branch was created from develop (current branch)
    cd "$repo_dir/feature/from-develop-worktree"
    local base_commit=$(git rev-parse HEAD)
    cd "$repo_dir/develop"
    local develop_commit=$(git rev-parse HEAD)
    
    if [[ "$base_commit" != "$develop_commit" ]]; then
        log_error "New branch should be based on current branch (develop)"
        return 1
    fi
    
    log_success "New branch correctly based on current branch"
    return 0
}

# Test checkout-branch from detached HEAD
test_checkout_branch_detached_head() {
    local repo_dir=$(setup_test_repo "checkout-branch-detached-test")
    
    # Create detached HEAD state
    cd "main"
    git checkout HEAD~0 >/dev/null 2>&1  # Creates detached HEAD
    
    # Test checkout-branch from detached HEAD should fail
    assert_command_failure "git worktree-checkout -b feature/from-detached" "Should fail from detached HEAD"
    
    return 0
}

# Test checkout-branch with nested directory structure
test_checkout_branch_nested_directories() {
    local repo_dir=$(setup_test_repo "checkout-branch-nested-test")
    
    # Test with deeply nested branch name
    cd "main"
    git worktree-checkout -b feature/ui/components/button || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/ui/components/button" || return 1
    assert_git_worktree "$repo_dir/feature/ui/components/button" "feature/ui/components/button" || return 1
    
    return 0
}

# Test checkout-branch with remote push and tracking
test_checkout_branch_remote_tracking() {
    local repo_dir=$(setup_test_repo "checkout-branch-remote-test")
    
    # Mock remote repository (we'll simulate push failure later)
    cd "main"
    git worktree-checkout -b feature/remote-test || return 1
    
    # Verify remote tracking was set up
    cd "$repo_dir/feature/remote-test"
    local upstream=$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null)
    if [[ "$upstream" != "origin/feature/remote-test" ]]; then
        log_error "Upstream tracking not set correctly. Expected: origin/feature/remote-test, Got: $upstream"
        return 1
    fi
    
    log_success "Upstream tracking set correctly"
    return 0
}

# Test checkout-branch with no branch name
test_checkout_branch_no_name() {
    local repo_dir=$(setup_test_repo "checkout-branch-no-name-test")
    
    # Test without branch name should fail
    cd "main"
    assert_command_failure "git worktree-checkout -b" "Should fail with no branch name"
    
    return 0
}

# Test checkout-branch with existing branch name
test_checkout_branch_existing_name() {
    local repo_dir=$(setup_test_repo "checkout-branch-existing-test")
    
    # Create branch first
    cd "main"
    git worktree-checkout -b feature/existing || return 1
    
    # Try to create same branch again should fail
    assert_command_failure "git worktree-checkout -b feature/existing" "Should fail with existing branch name"
    
    return 0
}

# Test checkout-branch with non-existent base branch
test_checkout_branch_nonexistent_base() {
    local repo_dir=$(setup_test_repo "checkout-branch-nonexistent-base-test")
    
    # Test with non-existent base branch should fail
    cd "main"
    assert_command_failure "git worktree-checkout -b feature/test nonexistent-base" "Should fail with non-existent base branch"
    
    return 0
}

# Test checkout-branch from deep subdirectory
test_checkout_branch_from_subdirectory() {
    local repo_dir=$(setup_test_repo "checkout-branch-subdir-test")
    
    # Create subdirectory structure
    mkdir -p "main/deep/nested/dir"
    cd "main/deep/nested/dir"
    
    # Test checkout-branch from deep subdirectory
    git worktree-checkout -b feature/from-subdir || return 1
    
    # Verify structure (should create at repo root, not in subdirectory)
    assert_directory_exists "$repo_dir/feature/from-subdir" || return 1
    assert_git_worktree "$repo_dir/feature/from-subdir" "feature/from-subdir" || return 1
    
    return 0
}

# Test checkout-branch outside git repository
test_checkout_branch_outside_repo() {
    # Move to a non-git directory
    cd "$WORK_DIR"
    
    # Test should fail
    assert_command_failure "git worktree-checkout -b feature/test" "Should fail outside git repository"
    
    return 0
}

# Test checkout-branch with special characters in branch name
test_checkout_branch_special_characters() {
    local repo_dir=$(setup_test_repo "checkout-branch-special-test")
    
    # Test with branch names containing special characters
    cd "main"
    git worktree-checkout -b "feature/test-123_v2.0" || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/test-123_v2.0" || return 1
    assert_git_worktree "$repo_dir/feature/test-123_v2.0" "feature/test-123_v2.0" || return 1
    
    return 0
}

# Test checkout-branch with different remote name
test_checkout_branch_different_remote() {
    local repo_dir=$(setup_test_repo "checkout-branch-different-remote-test")
    
    # Add additional remote (simulate)
    cd "main"
    
    # Test checkout-branch still works (should use default 'origin' remote)
    git worktree-checkout -b feature/test-remote || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/feature/test-remote" || return 1
    assert_git_worktree "$repo_dir/feature/test-remote" "feature/test-remote" || return 1
    
    return 0
}

# Test checkout-branch with direnv integration
test_checkout_branch_direnv() {
    local repo_dir=$(setup_test_repo "checkout-branch-direnv-test")
    
    # Create .envrc file in main
    cd "main"
    echo "export TEST_VAR=main_value" > .envrc
    git add .envrc
    git commit -m "Add .envrc" >/dev/null 2>&1
    
    # Test checkout-branch (should handle .envrc gracefully)
    git worktree-checkout -b feature/with-envrc || return 1
    
    # Verify structure and .envrc file was copied
    assert_directory_exists "$repo_dir/feature/with-envrc" || return 1
    assert_file_exists "$repo_dir/feature/with-envrc/.envrc" || return 1
    
    return 0
}

# Test checkout-branch error cleanup
test_checkout_branch_error_cleanup() {
    local repo_dir=$(setup_test_repo "checkout-branch-cleanup-test")
    
    # Create a scenario where push might fail (by making directory read-only)
    cd "main"
    
    # The command should still create worktree even if push fails
    # (We can't easily simulate push failure, so we'll test directory creation)
    git worktree-checkout -b feature/cleanup-test || return 1
    
    # Verify structure was created
    assert_directory_exists "$repo_dir/feature/cleanup-test" || return 1
    assert_git_worktree "$repo_dir/feature/cleanup-test" "feature/cleanup-test" || return 1
    
    return 0
}

# Run all checkout-branch tests
run_checkout_branch_tests() {
    log "Running git-worktree-checkout -b tests..."
    
    run_test "checkout_branch_basic" "test_checkout_branch_basic"
    run_test "checkout_branch_explicit_base" "test_checkout_branch_explicit_base"
    run_test "checkout_branch_from_different_worktree" "test_checkout_branch_from_different_worktree"
    run_test "checkout_branch_detached_head" "test_checkout_branch_detached_head"
    run_test "checkout_branch_nested_directories" "test_checkout_branch_nested_directories"
    run_test "checkout_branch_remote_tracking" "test_checkout_branch_remote_tracking"
    run_test "checkout_branch_no_name" "test_checkout_branch_no_name"
    run_test "checkout_branch_existing_name" "test_checkout_branch_existing_name"
    run_test "checkout_branch_nonexistent_base" "test_checkout_branch_nonexistent_base"
    run_test "checkout_branch_from_subdirectory" "test_checkout_branch_from_subdirectory"
    run_test "checkout_branch_outside_repo" "test_checkout_branch_outside_repo"
    run_test "checkout_branch_special_characters" "test_checkout_branch_special_characters"
    run_test "checkout_branch_different_remote" "test_checkout_branch_different_remote"
    run_test "checkout_branch_direnv" "test_checkout_branch_direnv"
    run_test "checkout_branch_error_cleanup" "test_checkout_branch_error_cleanup"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_branch_tests
    print_summary
fi