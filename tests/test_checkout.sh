#!/bin/bash

# Tests for git-worktree-checkout command

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Setup a test repository for checkout tests
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

# Test basic checkout of existing remote branch
test_checkout_existing_remote_branch() {
    local repo_dir=$(setup_test_repo "checkout-remote-test")
    
    # Test checkout of existing remote branch
    git worktree-checkout develop || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/develop" || return 1
    assert_git_worktree "$repo_dir/develop" "develop" || return 1
    assert_branch_exists "$repo_dir/develop" "develop" || return 1
    assert_remote_tracking "$repo_dir/develop" "develop" "origin" || return 1
    
    return 0
}

# Test checkout of existing local branch
test_checkout_existing_local_branch() {
    local repo_dir=$(setup_test_repo "checkout-local-test")
    
    # Create a local branch first
    git checkout -b local-branch main >/dev/null 2>&1
    git checkout main >/dev/null 2>&1
    
    # Test checkout of existing local branch
    git worktree-checkout local-branch || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/local-branch" || return 1
    assert_git_worktree "$repo_dir/local-branch" "local-branch" || return 1
    assert_branch_exists "$repo_dir/local-branch" "local-branch" || return 1
    
    return 0
}

# Test checkout of feature branch with slashes
test_checkout_feature_branch() {
    local repo_dir=$(setup_test_repo "checkout-feature-test")
    
    # Test checkout of feature branch
    git worktree-checkout feature/test-feature || return 1
    
    # Verify structure (should create nested directory)
    assert_directory_exists "$repo_dir/feature/test-feature" || return 1
    assert_git_worktree "$repo_dir/feature/test-feature" "feature/test-feature" || return 1
    assert_branch_exists "$repo_dir/feature/test-feature" "feature/test-feature" || return 1
    
    return 0
}

# Test checkout from deep subdirectory
test_checkout_from_subdirectory() {
    local repo_dir=$(setup_test_repo "checkout-subdir-test")
    
    # Create subdirectory structure
    mkdir -p "main/deep/nested/dir"
    cd "main/deep/nested/dir"
    
    # Test checkout from deep subdirectory
    git worktree-checkout develop || return 1
    
    # Verify structure (should create at repo root, not in subdirectory)
    assert_directory_exists "$repo_dir/develop" || return 1
    assert_git_worktree "$repo_dir/develop" "develop" || return 1
    
    return 0
}

# Test checkout with non-existent branch
test_checkout_nonexistent_branch() {
    local repo_dir=$(setup_test_repo "checkout-nonexistent-test")
    
    # Test checkout of non-existent branch should fail
    assert_command_failure "git worktree-checkout nonexistent-branch" "Should fail with non-existent branch"
    
    return 0
}

# Test checkout with existing worktree directory
test_checkout_existing_worktree() {
    local repo_dir=$(setup_test_repo "checkout-existing-worktree-test")
    
    # Create worktree first
    git worktree-checkout develop >/dev/null 2>&1
    
    # Try to create same worktree again should fail
    assert_command_failure "git worktree-checkout develop" "Should fail with existing worktree"
    
    return 0
}

# Test checkout with no branch name provided
test_checkout_no_branch_name() {
    local repo_dir=$(setup_test_repo "checkout-no-branch-test")
    
    # Test checkout without branch name should fail
    assert_command_failure "git worktree-checkout" "Should fail with no branch name"
    
    return 0
}

# Test checkout from outside git repository
test_checkout_outside_git_repo() {
    # Move to a non-git directory
    cd "$WORK_DIR"
    
    # Test checkout should fail
    assert_command_failure "git worktree-checkout develop" "Should fail outside git repository"
    
    return 0
}

# Test checkout with upstream tracking setup
test_checkout_upstream_tracking() {
    local repo_dir=$(setup_test_repo "checkout-upstream-test")
    
    # Test checkout of remote branch
    git worktree-checkout develop || return 1
    
    # Verify upstream tracking is set
    cd "$repo_dir/develop"
    local upstream=$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null)
    if [[ "$upstream" != "origin/develop" ]]; then
        log_error "Upstream tracking not set correctly. Expected: origin/develop, Got: $upstream"
        return 1
    fi
    
    log_success "Upstream tracking set correctly to origin/develop"
    return 0
}

# Test direnv integration with checkout
test_checkout_direnv_integration() {
    local repo_dir=$(setup_test_repo "checkout-direnv-test")
    
    # Create .envrc file in develop branch
    git checkout develop >/dev/null 2>&1
    echo "export TEST_VAR=develop_value" > .envrc
    git add .envrc
    git commit -m "Add .envrc to develop branch" >/dev/null 2>&1
    git push origin develop >/dev/null 2>&1
    git checkout main >/dev/null 2>&1
    
    # Test checkout (should handle .envrc gracefully)
    git worktree-checkout develop || return 1
    
    # Verify structure and .envrc file
    assert_directory_exists "$repo_dir/develop" || return 1
    assert_file_exists "$repo_dir/develop/.envrc" || return 1
    
    return 0
}

# Test checkout with branch that tracks different remote
test_checkout_different_remote() {
    local repo_dir=$(setup_test_repo "checkout-different-remote-test")
    
    # Create a branch that doesn't exist on origin
    git checkout -b local-only-branch main >/dev/null 2>&1
    git checkout main >/dev/null 2>&1
    
    # Test checkout of local-only branch
    git worktree-checkout local-only-branch || return 1
    
    # Verify structure
    assert_directory_exists "$repo_dir/local-only-branch" || return 1
    assert_git_worktree "$repo_dir/local-only-branch" "local-only-branch" || return 1
    
    # Verify no upstream tracking (since remote branch doesn't exist)
    cd "$repo_dir/local-only-branch"
    local upstream=$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null || echo "no-upstream")
    if [[ "$upstream" != "no-upstream" ]]; then
        log_error "Upstream should not be set for local-only branch"
        return 1
    fi
    
    log_success "No upstream tracking for local-only branch (correct)"
    return 0
}

# Test checkout directory permissions
test_checkout_directory_permissions() {
    local repo_dir=$(setup_test_repo "checkout-permissions-test")
    
    # Test checkout creates directories with correct permissions
    git worktree-checkout develop || return 1
    
    # Verify directory exists and is readable/writable
    assert_directory_exists "$repo_dir/develop" || return 1
    
    # Test we can write to the directory
    echo "test content" > "$repo_dir/develop/test_file.txt"
    assert_file_exists "$repo_dir/develop/test_file.txt" || return 1
    
    return 0
}

# Run all checkout tests
run_checkout_tests() {
    log "Running git-worktree-checkout tests..."
    
    run_test "checkout_existing_remote_branch" "test_checkout_existing_remote_branch"
    run_test "checkout_existing_local_branch" "test_checkout_existing_local_branch"
    run_test "checkout_feature_branch" "test_checkout_feature_branch"
    run_test "checkout_from_subdirectory" "test_checkout_from_subdirectory"
    run_test "checkout_nonexistent_branch" "test_checkout_nonexistent_branch"
    run_test "checkout_existing_worktree" "test_checkout_existing_worktree"
    run_test "checkout_no_branch_name" "test_checkout_no_branch_name"
    run_test "checkout_outside_git_repo" "test_checkout_outside_git_repo"
    run_test "checkout_upstream_tracking" "test_checkout_upstream_tracking"
    run_test "checkout_direnv_integration" "test_checkout_direnv_integration"
    run_test "checkout_different_remote" "test_checkout_different_remote"
    run_test "checkout_directory_permissions" "test_checkout_directory_permissions"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_tests
    print_summary
fi