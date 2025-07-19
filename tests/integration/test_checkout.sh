#!/bin/bash

# Integration tests for git-worktree-checkout Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic checkout functionality
test_checkout_basic() {
    local remote_repo=$(create_test_remote "test-repo-checkout" "main")
    
    # First clone the repository
    git-worktree-clone "$remote_repo" || return 1
    
    # Change to the repo directory
    cd "test-repo-checkout"
    
    # Test checkout existing branch
    git-worktree-checkout develop || return 1
    
    # Verify structure
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1
    
    return 0
}

# Test checkout with remote branch
test_checkout_remote_branch() {
    local remote_repo=$(create_test_remote "test-repo-checkout-remote" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-remote"
    
    # Test checkout remote branch
    git-worktree-checkout feature/test-feature || return 1
    
    # Verify structure
    assert_directory_exists "feature/test-feature" || return 1
    assert_git_worktree "feature/test-feature" "feature/test-feature" || return 1
    
    return 0
}

# Test checkout from subdirectory
test_checkout_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-checkout-subdir" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-subdir"
    
    # Create a subdirectory and test checkout from there
    mkdir -p "main/subdir"
    cd "main/subdir"
    
    # Test checkout from subdirectory
    git-worktree-checkout develop || return 1
    
    # Verify structure (should be created at repository root)
    assert_directory_exists "../../develop" || return 1
    assert_git_worktree "../../develop" "develop" || return 1
    
    return 0
}

# Test checkout error handling
test_checkout_errors() {
    local remote_repo=$(create_test_remote "test-repo-checkout-errors" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-errors"
    
    # Test checkout nonexistent branch
    assert_command_failure "git-worktree-checkout nonexistent-branch" "Should fail with nonexistent branch"
    
    # Test checkout existing worktree
    git-worktree-checkout develop || return 1
    assert_command_failure "git-worktree-checkout develop" "Should fail with existing worktree"
    
    return 0
}

# Test checkout with direnv integration
test_checkout_direnv() {
    local remote_repo=$(create_test_remote "test-repo-checkout-direnv" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-direnv"
    
    # Add .envrc to a branch
    local temp_clone="$TEMP_BASE_DIR/temp_envrc_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "export TEST_VAR=develop_value" > .envrc
        git add .envrc >/dev/null 2>&1
        git commit -m "Add .envrc to develop" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Fetch the changes
    git fetch origin >/dev/null 2>&1
    
    # Test checkout with direnv file
    git-worktree-checkout develop || return 1
    
    # Verify structure and direnv file
    assert_directory_exists "develop" || return 1
    assert_file_exists "develop/.envrc" || return 1
    
    return 0
}

# Test checkout outside git repository
test_checkout_outside_repo() {
    # Test checkout command outside git repository
    assert_command_failure "git-worktree-checkout some-branch" "Should fail outside git repository"
    
    return 0
}

# Test checkout help functionality
test_checkout_help() {
    # Test help commands
    assert_command_help "git-worktree-checkout" || return 1
    assert_command_version "git-worktree-checkout" || return 1
    
    return 0
}

# Test checkout with complex branch structures
test_checkout_complex_branches() {
    local remote_repo=$(create_test_remote "test-repo-checkout-complex" "main")
    
    # Add more complex branch structure
    local temp_clone="$TEMP_BASE_DIR/temp_complex_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        # Create nested feature branches
        git checkout -b feature/user-auth >/dev/null 2>&1
        echo "User auth feature" > auth.txt
        git add auth.txt >/dev/null 2>&1
        git commit -m "Add user auth" >/dev/null 2>&1
        git push origin feature/user-auth >/dev/null 2>&1
        
        git checkout -b release/v1.0 >/dev/null 2>&1
        echo "Release v1.0" > release.txt
        git add release.txt >/dev/null 2>&1
        git commit -m "Add release notes" >/dev/null 2>&1
        git push origin release/v1.0 >/dev/null 2>&1
        
        git checkout -b hotfix/critical-bug >/dev/null 2>&1
        echo "Critical bug fix" > hotfix.txt
        git add hotfix.txt >/dev/null 2>&1
        git commit -m "Fix critical bug" >/dev/null 2>&1
        git push origin hotfix/critical-bug >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-complex"
    
    # Test checkout various branch types
    git-worktree-checkout feature/user-auth || return 1
    assert_directory_exists "feature/user-auth" || return 1
    assert_file_exists "feature/user-auth/auth.txt" || return 1
    
    git-worktree-checkout release/v1.0 || return 1
    assert_directory_exists "release/v1.0" || return 1
    assert_file_exists "release/v1.0/release.txt" || return 1
    
    git-worktree-checkout hotfix/critical-bug || return 1
    assert_directory_exists "hotfix/critical-bug" || return 1
    assert_file_exists "hotfix/critical-bug/hotfix.txt" || return 1
    
    return 0
}

# Test checkout performance
test_checkout_performance() {
    local remote_repo=$(create_test_remote "test-repo-checkout-perf" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-perf"
    
    # Test checkout performance
    local start_time=$(date +%s)
    git-worktree-checkout develop || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 10 ]]; then
        log_warning "Checkout performance test took ${duration}s (expected < 10s)"
    else
        log_success "Checkout performance test completed in ${duration}s"
    fi
    
    # Verify structure
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1
    
    return 0
}

# Test checkout with large repository
test_checkout_large_repo() {
    local remote_repo=$(create_test_remote "test-repo-checkout-large" "main")
    
    # Add many files to the repository
    local temp_clone="$TEMP_BASE_DIR/temp_large_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        # Create many files on develop branch
        git checkout develop >/dev/null 2>&1
        for i in {1..100}; do
            echo "Large repo test file $i" > "large_file_$i.txt"
        done
        git add . >/dev/null 2>&1
        git commit -m "Add many files to develop" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-large"
    
    # Test checkout large branch
    git-worktree-checkout develop || return 1
    
    # Verify structure and some files
    assert_directory_exists "develop" || return 1
    assert_file_exists "develop/large_file_1.txt" || return 1
    assert_file_exists "develop/large_file_100.txt" || return 1
    
    return 0
}

# Test checkout with uncommitted changes in current worktree
test_checkout_with_uncommitted_changes() {
    local remote_repo=$(create_test_remote "test-repo-checkout-uncommitted" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-uncommitted"
    
    # Make uncommitted changes in main worktree
    echo "Uncommitted changes" > "main/uncommitted.txt"
    
    # Test checkout should still work (shouldn't affect other worktrees)
    git-worktree-checkout develop || return 1
    
    # Verify both worktrees exist
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1
    assert_file_exists "main/uncommitted.txt" || return 1
    
    return 0
}

# Run all checkout tests
run_checkout_tests() {
    log "Running git-worktree-checkout integration tests..."
    
    run_test "checkout_basic" "test_checkout_basic"
    run_test "checkout_remote_branch" "test_checkout_remote_branch"
    run_test "checkout_from_subdirectory" "test_checkout_from_subdirectory"
    run_test "checkout_errors" "test_checkout_errors"
    run_test "checkout_direnv" "test_checkout_direnv"
    run_test "checkout_outside_repo" "test_checkout_outside_repo"
    run_test "checkout_help" "test_checkout_help"
    run_test "checkout_complex_branches" "test_checkout_complex_branches"
    run_test "checkout_performance" "test_checkout_performance"
    run_test "checkout_large_repo" "test_checkout_large_repo"
    run_test "checkout_with_uncommitted_changes" "test_checkout_with_uncommitted_changes"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_tests
    print_summary
fi