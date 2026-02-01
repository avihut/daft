#!/bin/bash

# Integration tests for git-worktree-prune Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic prune functionality
test_prune_basic() {
    local remote_repo=$(create_test_remote "test-repo-prune-basic" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-basic"
    
    # Create some worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Verify worktrees exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "feature/test-feature" || return 1
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify feature branch worktree was removed
    if [[ -d "feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        return 1
    fi
    
    # Verify develop worktree still exists
    assert_directory_exists "develop" || return 1
    
    return 0
}

# Test prune with no remote branches to delete
test_prune_no_deletion() {
    local remote_repo=$(create_test_remote "test-repo-prune-no-deletion" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-no-deletion"
    
    # Create some worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Run prune (should not delete anything)
    git-worktree-prune || return 1
    
    # Verify all worktrees still exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "feature/test-feature" || return 1
    
    return 0
}

# Test prune with multiple deleted branches
test_prune_multiple_deletions() {
    local remote_repo=$(create_test_remote "test-repo-prune-multiple" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-multiple"
    
    # Create some worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Create additional branches in remote
    local temp_clone="$TEMP_BASE_DIR/temp_multiple_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git checkout -b feature/branch1 >/dev/null 2>&1
        echo "Branch 1" > branch1.txt
        git add branch1.txt >/dev/null 2>&1
        git commit -m "Add branch1" >/dev/null 2>&1
        git push origin feature/branch1 >/dev/null 2>&1
        
        git checkout -b feature/branch2 >/dev/null 2>&1
        echo "Branch 2" > branch2.txt
        git add branch2.txt >/dev/null 2>&1
        git commit -m "Add branch2" >/dev/null 2>&1
        git push origin feature/branch2 >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Fetch and checkout these branches
    git fetch origin >/dev/null 2>&1
    git-worktree-checkout feature/branch1 || return 1
    git-worktree-checkout feature/branch2 || return 1
    
    # Verify all worktrees exist
    assert_directory_exists "feature/branch1" || return 1
    assert_directory_exists "feature/branch2" || return 1
    
    # Delete multiple branches from remote
    temp_clone="$TEMP_BASE_DIR/temp_multiple_delete_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/branch1 >/dev/null 2>&1
        git push origin --delete feature/branch2 >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify deleted branches' worktrees were removed
    if [[ -d "feature/branch1" ]]; then
        log_error "Prune should have removed feature/branch1 worktree"
        return 1
    fi
    
    if [[ -d "feature/branch2" ]]; then
        log_error "Prune should have removed feature/branch2 worktree"
        return 1
    fi
    
    # Verify other worktrees still exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "feature/test-feature" || return 1
    
    return 0
}

# Test prune from subdirectory
test_prune_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-prune-subdir" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-subdir"
    
    # Create some worktrees
    git-worktree-checkout feature/test-feature || return 1
    
    # Create a subdirectory and test prune from there
    mkdir -p "main/subdir"
    cd "main/subdir"
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_subdir_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune from subdirectory
    git-worktree-prune || return 1
    
    # Verify feature branch worktree was removed
    if [[ -d "../../feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        return 1
    fi
    
    return 0
}

# Test prune error handling
test_prune_errors() {
    local remote_repo=$(create_test_remote "test-repo-prune-errors" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-errors"
    
    # Test prune should work (no error expected)
    git-worktree-prune || return 1
    
    return 0
}

# Test prune outside git repository
test_prune_outside_repo() {
    # Test prune command outside git repository
    assert_command_failure "git-worktree-prune" "Should fail outside git repository"
    
    return 0
}

# Test prune help functionality
test_prune_help() {
    # Test help commands
    assert_command_help "git-worktree-prune" || return 1
    assert_command_version "git-worktree-prune" || return 1
    
    return 0
}

# Test prune with uncommitted changes
test_prune_with_uncommitted_changes() {
    local remote_repo=$(create_test_remote "test-repo-prune-uncommitted" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-uncommitted"
    
    # Create worktree and add uncommitted changes
    git-worktree-checkout feature/test-feature || return 1
    echo "Uncommitted changes" > "feature/test-feature/uncommitted.txt"
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_uncommitted_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune (should handle uncommitted changes gracefully)
    git-worktree-prune || return 1
    
    # Verify worktree was removed (or handled appropriately)
    # The exact behavior depends on implementation - it might preserve uncommitted changes
    
    return 0
}

# Test prune with nested branch directories
test_prune_nested_directories() {
    local remote_repo=$(create_test_remote "test-repo-prune-nested" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-nested"
    
    # Create worktrees with nested directory structures
    git-worktree-checkout feature/test-feature || return 1
    
    # Create some nested directories and files
    mkdir -p "feature/test-feature/nested/deep/structure"
    echo "Deep file" > "feature/test-feature/nested/deep/structure/file.txt"
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_nested_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify worktree and all nested directories were removed
    if [[ -d "feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree and all nested directories"
        return 1
    fi
    
    return 0
}

# Test prune performance
test_prune_performance() {
    local remote_repo=$(create_test_remote "test-repo-prune-performance" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-performance"
    
    # Create multiple worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Test prune performance
    local start_time=$(date +%s)
    git-worktree-prune || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 15 ]]; then
        log_warning "Prune performance test took ${duration}s (expected < 15s)"
    else
        log_success "Prune performance test completed in ${duration}s"
    fi
    
    return 0
}

# Test prune with large number of worktrees
test_prune_many_worktrees() {
    local remote_repo=$(create_test_remote "test-repo-prune-many" "main")
    
    # Create many branches in remote
    local temp_clone="$TEMP_BASE_DIR/temp_many_branches_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        for i in {1..10}; do
            git checkout -b "feature/branch$i" >/dev/null 2>&1
            echo "Branch $i" > "branch$i.txt"
            git add "branch$i.txt" >/dev/null 2>&1
            git commit -m "Add branch$i" >/dev/null 2>&1
            git push origin "feature/branch$i" >/dev/null 2>&1
        done
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-many"
    
    # Fetch and checkout these branches
    git fetch origin >/dev/null 2>&1
    for i in {1..10}; do
        git-worktree-checkout "feature/branch$i" || return 1
    done
    
    # Delete some branches from remote
    temp_clone="$TEMP_BASE_DIR/temp_many_delete_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        for i in {1..5}; do
            git push origin --delete "feature/branch$i" >/dev/null 2>&1
        done
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify first 5 branches were removed
    for i in {1..5}; do
        if [[ -d "feature/branch$i" ]]; then
            log_error "Prune should have removed feature/branch$i worktree"
            return 1
        fi
    done
    
    # Verify last 5 branches still exist
    for i in {6..10}; do
        assert_directory_exists "feature/branch$i" || return 1
    done
    
    return 0
}

# Test prune correctly handles branches shown with '+' marker in linked worktrees
# Regression test for https://github.com/avihut/daft/issues/97
test_prune_plus_marker_branch() {
    local remote_repo=$(create_test_remote "test-repo-prune-plus" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-plus"

    # Create a worktree with a new branch - this branch will show with '+' prefix
    # in git branch -vv because it is checked out in a linked worktree
    git-worktree-checkout-branch feature/plus-test || return 1

    # Verify the branch shows with '+' marker in git branch -vv
    local branch_vv
    branch_vv=$(cd main && git branch -vv)
    if ! echo "$branch_vv" | grep -q '^+.*feature/plus-test'; then
        log_error "Expected feature/plus-test to show with '+' marker in git branch -vv"
        log_error "Actual output: $branch_vv"
        return 1
    fi

    # Delete the branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_plus_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/plus-test >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run prune and capture output - should not contain error about branch '+'
    local prune_output
    prune_output=$(git-worktree-prune 2>&1) || true

    if echo "$prune_output" | grep -q "branch '+'"; then
        log_error "Prune incorrectly tried to delete a branch named '+' (issue #97)"
        log_error "Output: $prune_output"
        return 1
    fi

    # Verify the actual branch worktree was removed
    if [[ -d "feature/plus-test" ]]; then
        log_error "Prune should have removed feature/plus-test worktree"
        return 1
    fi

    return 0
}

# Test prune with remote configuration
test_prune_remote_config() {
    local remote_repo=$(create_test_remote "test-repo-prune-remote-config" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-remote-config"
    
    # Create worktree
    git-worktree-checkout feature/test-feature || return 1
    
    # Verify remote configuration
    local remote_url=$(git remote get-url origin)
    if [[ -z "$remote_url" ]]; then
        log_error "Remote origin should be configured"
        return 1
    fi
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_remote_config_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify worktree was removed
    if [[ -d "feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        return 1
    fi
    
    return 0
}

# Run all prune tests
run_prune_tests() {
    log "Running git-worktree-prune integration tests..."
    
    run_test "prune_basic" "test_prune_basic"
    run_test "prune_no_deletion" "test_prune_no_deletion"
    run_test "prune_multiple_deletions" "test_prune_multiple_deletions"
    run_test "prune_from_subdirectory" "test_prune_from_subdirectory"
    run_test "prune_errors" "test_prune_errors"
    run_test "prune_outside_repo" "test_prune_outside_repo"
    run_test "prune_help" "test_prune_help"
    run_test "prune_with_uncommitted_changes" "test_prune_with_uncommitted_changes"
    run_test "prune_nested_directories" "test_prune_nested_directories"
    run_test "prune_performance" "test_prune_performance"
    run_test "prune_many_worktrees" "test_prune_many_worktrees"
    run_test "prune_plus_marker_branch" "test_prune_plus_marker_branch"
    run_test "prune_remote_config" "test_prune_remote_config"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_prune_tests
    print_summary
fi