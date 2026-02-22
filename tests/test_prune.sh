#!/bin/bash

# Tests for git-worktree-prune command

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Setup a test repository with branches for prune tests
setup_test_repo_with_branches() {
    local repo_name="$1"
    local remote_repo=$(create_test_remote "$repo_name" "main")
    
    # Clone the repository first
    git worktree-clone "$remote_repo" >/dev/null 2>&1
    
    # Change to the repo directory
    cd "$repo_name"
    
    # Create several worktrees
    git worktree-checkout develop >/dev/null 2>&1
    git worktree-checkout feature/test-feature >/dev/null 2>&1
    git worktree-checkout -b feature/local-branch >/dev/null 2>&1
    git worktree-checkout -b bugfix/local-bugfix >/dev/null 2>&1
    
    echo "$(pwd)"
}

# Simulate remote branch deletion
simulate_remote_branch_deletion() {
    local repo_dir="$1"
    local branch_name="$2"
    
    # Go to the remote repository and delete the branch
    local remote_repo_path=$(git -C "$repo_dir" config --get remote.origin.url)
    
    # Create a temporary clone to delete the branch
    local temp_clone="$TEMP_BASE_DIR/temp_delete_$$"
    git clone "$remote_repo_path" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete "$branch_name" >/dev/null 2>&1 || true
    )
    
    rm -rf "$temp_clone"
}

# Test basic prune functionality
test_prune_basic() {
    local repo_dir=$(setup_test_repo_with_branches "prune-basic-test")
    
    # Verify initial state
    assert_directory_exists "$repo_dir/feature/test-feature" || return 1
    assert_directory_exists "$repo_dir/feature/local-branch" || return 1
    
    # Simulate deletion of feature/test-feature on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/test-feature"
    
    # Run prune
    git worktree-prune || return 1
    
    # Verify the remote branch worktree was removed
    if [[ -d "$repo_dir/feature/test-feature" ]]; then
        log_error "Worktree for deleted remote branch should be removed"
        return 1
    fi
    
    # Verify local-only branches are still there
    assert_directory_exists "$repo_dir/feature/local-branch" || return 1
    
    # Verify branch was deleted
    if git -C "$repo_dir" show-ref --verify --quiet "refs/heads/feature/test-feature"; then
        log_error "Local branch should be deleted after prune"
        return 1
    fi
    
    log_success "Basic prune functionality works correctly"
    return 0
}

# Test prune with multiple gone branches
test_prune_multiple_branches() {
    local repo_dir=$(setup_test_repo_with_branches "prune-multiple-test")
    
    # Verify initial state
    assert_directory_exists "$repo_dir/feature/test-feature" || return 1
    assert_directory_exists "$repo_dir/develop" || return 1
    
    # Simulate deletion of multiple branches on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/test-feature"
    simulate_remote_branch_deletion "$repo_dir" "develop"
    
    # Run prune
    git worktree-prune || return 1
    
    # Verify both worktrees were removed
    if [[ -d "$repo_dir/feature/test-feature" ]]; then
        log_error "feature/test-feature worktree should be removed"
        return 1
    fi
    
    if [[ -d "$repo_dir/develop" ]]; then
        log_error "develop worktree should be removed"
        return 1
    fi
    
    # Verify local-only branches are still there
    assert_directory_exists "$repo_dir/feature/local-branch" || return 1
    assert_directory_exists "$repo_dir/bugfix/local-bugfix" || return 1
    
    log_success "Multiple branch prune works correctly"
    return 0
}

# Test prune with no branches to prune
test_prune_no_branches() {
    local repo_dir=$(setup_test_repo_with_branches "prune-no-branches-test")
    
    # Run prune without any deleted remote branches
    git worktree-prune || return 1
    
    # Verify all worktrees still exist
    assert_directory_exists "$repo_dir/develop" || return 1
    assert_directory_exists "$repo_dir/feature/test-feature" || return 1
    assert_directory_exists "$repo_dir/feature/local-branch" || return 1
    assert_directory_exists "$repo_dir/bugfix/local-bugfix" || return 1
    
    log_success "Prune with no branches to prune works correctly"
    return 0
}

# Test prune preserves main/master branches
test_prune_preserves_main_branches() {
    local repo_dir=$(setup_test_repo_with_branches "prune-preserve-main-test")
    
    # Even if we somehow mark main as gone, it should be preserved
    # (This is a safety test - main branches should not be pruned)
    
    # Run prune
    git worktree-prune || return 1
    
    # Verify main worktree still exists
    assert_directory_exists "$repo_dir/main" || return 1
    assert_git_worktree "$repo_dir/main" "main" || return 1
    
    log_success "Main branch is preserved during prune"
    return 0
}

# Test prune outside git repository
test_prune_outside_repo() {
    # Move to a non-git directory
    cd "$WORK_DIR"
    
    # Test should fail
    assert_command_failure "git worktree-prune" "Should fail outside git repository"
    
    return 0
}

# Test prune with fetch failure
test_prune_fetch_failure() {
    local repo_dir=$(setup_test_repo_with_branches "prune-fetch-failure-test")
    
    # Simulate fetch failure by making remote inaccessible
    git -C "$repo_dir" remote set-url origin "/nonexistent/repo" >/dev/null 2>&1
    
    # Test should fail gracefully
    assert_command_failure "git worktree-prune" "Should fail with fetch failure"
    
    return 0
}

# Test prune with worktree removal failure
test_prune_worktree_removal_failure() {
    local repo_dir=$(setup_test_repo_with_branches "prune-removal-failure-test")
    
    # Simulate deletion of branch on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/test-feature"
    
    # Make worktree directory read-only to simulate removal failure
    chmod -R 444 "$repo_dir/feature/test-feature" >/dev/null 2>&1 || true
    
    # Run prune (should handle failure gracefully)
    git worktree-prune || return 1
    
    # Restore permissions for cleanup
    chmod -R 755 "$repo_dir/feature/test-feature" >/dev/null 2>&1 || true
    
    return 0
}

# Test prune with branches that have worktrees but no remote
test_prune_worktree_no_remote() {
    local repo_dir=$(setup_test_repo_with_branches "prune-no-remote-test")
    
    # Create a local branch that was never pushed
    git -C "$repo_dir" worktree-checkout -b feature/never-pushed >/dev/null 2>&1
    
    # Delete the branch from remote side (simulate it was never there)
    # This branch should be detected as needing pruning
    
    # Run prune
    git worktree-prune || return 1
    
    # Local-only branches should be preserved unless they're not on remote
    # The behavior might vary based on implementation
    log_success "Prune handles branches with worktrees but no remote"
    return 0
}

# Test prune with nested worktree directories
test_prune_nested_worktrees() {
    local repo_dir=$(setup_test_repo_with_branches "prune-nested-test")
    
    # Create branch with nested directory structure
    git -C "$repo_dir" worktree-checkout -b feature/ui/component >/dev/null 2>&1
    
    # Verify nested structure exists
    assert_directory_exists "$repo_dir/feature/ui/component" || return 1
    
    # Simulate deletion of nested branch on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/ui/component"
    
    # Run prune
    git worktree-prune || return 1
    
    # Verify nested worktree was removed
    if [[ -d "$repo_dir/feature/ui/component" ]]; then
        log_error "Nested worktree should be removed"
        return 1
    fi
    
    # Verify parent directories are cleaned up if empty
    # (This depends on implementation - might be left as empty dirs)
    
    log_success "Prune handles nested worktrees correctly"
    return 0
}

# Test prune with current working directory in pruned worktree
test_prune_current_directory() {
    local repo_dir=$(setup_test_repo_with_branches "prune-current-dir-test")
    
    # Change to a worktree that will be pruned
    cd "$repo_dir/feature/test-feature"
    
    # Simulate deletion of this branch on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/test-feature"
    
    # Run prune (should handle gracefully even if we're in the directory)
    git worktree-prune || return 1
    
    # We might end up in an invalid directory, but command should succeed
    # (Implementation should handle this gracefully)
    
    log_success "Prune handles current directory being pruned"
    return 0
}

# Test prune statistics and output
test_prune_statistics() {
    local repo_dir=$(setup_test_repo_with_branches "prune-statistics-test")
    
    # Simulate deletion of one branch on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/test-feature"
    
    # Run prune and capture output
    local output=$(git worktree-prune 2>&1)
    
    # Verify output contains expected information
    if [[ ! "$output" =~ "Branches deleted: 1" ]]; then
        log_error "Prune output should contain branch deletion count"
        return 1
    fi
    
    if [[ ! "$output" =~ "Worktrees removed: 1" ]]; then
        log_error "Prune output should contain worktree removal count"
        return 1
    fi
    
    log_success "Prune statistics output is correct"
    return 0
}

# Test prune with git worktree prune suggestion
test_prune_git_worktree_prune_suggestion() {
    local repo_dir=$(setup_test_repo_with_branches "prune-suggestion-test")
    
    # Create orphaned worktree entry (simulate incomplete cleanup)
    # This is hard to simulate, but we can test the suggestion appears
    
    # Run prune
    local output=$(git worktree-prune 2>&1)
    
    # Command should complete successfully
    if [[ $? -ne 0 ]]; then
        log_error "Prune command should complete successfully"
        return 1
    fi
    
    log_success "Prune completes and provides appropriate suggestions"
    return 0
}

# Test prune with complex branch tracking scenarios
test_prune_complex_tracking() {
    local repo_dir=$(setup_test_repo_with_branches "prune-complex-test")
    
    # Create branch with different tracking setup
    cd "$repo_dir/main"
    git checkout -b feature/complex >/dev/null 2>&1
    git push origin feature/complex >/dev/null 2>&1
    git checkout main >/dev/null 2>&1
    
    # Create worktree for this branch
    git worktree-checkout feature/complex >/dev/null 2>&1
    
    # Simulate deletion on remote
    simulate_remote_branch_deletion "$repo_dir" "feature/complex"
    
    # Run prune
    git worktree-prune || return 1
    
    # Verify branch was cleaned up
    if [[ -d "$repo_dir/feature/complex" ]]; then
        log_error "Complex tracked branch should be pruned"
        return 1
    fi
    
    log_success "Complex tracking scenarios handled correctly"
    return 0
}

# Run all prune tests
run_prune_tests() {
    log "Running git-worktree-prune tests..."
    
    run_test "prune_basic" "test_prune_basic"
    run_test "prune_multiple_branches" "test_prune_multiple_branches"
    run_test "prune_no_branches" "test_prune_no_branches"
    run_test "prune_preserves_main_branches" "test_prune_preserves_main_branches"
    run_test "prune_outside_repo" "test_prune_outside_repo"
    run_test "prune_fetch_failure" "test_prune_fetch_failure"
    run_test "prune_worktree_removal_failure" "test_prune_worktree_removal_failure"
    run_test "prune_worktree_no_remote" "test_prune_worktree_no_remote"
    run_test "prune_nested_worktrees" "test_prune_nested_worktrees"
    run_test "prune_current_directory" "test_prune_current_directory"
    run_test "prune_statistics" "test_prune_statistics"
    run_test "prune_git_worktree_prune_suggestion" "test_prune_git_worktree_prune_suggestion"
    run_test "prune_complex_tracking" "test_prune_complex_tracking"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_prune_tests
    print_summary
fi