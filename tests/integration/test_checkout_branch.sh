#!/bin/bash

# Integration tests for git-worktree-checkout-branch Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic checkout-branch functionality
test_checkout_branch_basic() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch"
    
    # Test checkout-branch from main
    git-worktree-checkout-branch feature/new-feature || return 1
    
    # Verify structure
    assert_directory_exists "feature/new-feature" || return 1
    assert_git_worktree "feature/new-feature" "feature/new-feature" || return 1
    
    return 0
}

# Test checkout-branch with base branch
test_checkout_branch_with_base() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-base" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-base"
    
    # First checkout develop
    git-worktree-checkout develop || return 1
    
    # Test checkout-branch with base branch
    git-worktree-checkout-branch feature/from-develop develop || return 1
    
    # Verify structure
    assert_directory_exists "feature/from-develop" || return 1
    assert_git_worktree "feature/from-develop" "feature/from-develop" || return 1
    
    return 0
}

# Test checkout-branch from subdirectory
test_checkout_branch_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-subdir" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-subdir"
    
    # Create a subdirectory and test from there
    mkdir -p "main/subdir"
    cd "main/subdir"
    
    # Test checkout-branch from subdirectory
    git-worktree-checkout-branch feature/from-subdir || return 1
    
    # Verify structure (should be created at repository root)
    assert_directory_exists "../../feature/from-subdir" || return 1
    assert_git_worktree "../../feature/from-subdir" "feature/from-subdir" || return 1
    
    return 0
}

# Test checkout-branch error handling
test_checkout_branch_errors() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-errors" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-errors"
    
    # Test checkout-branch with no branch name
    assert_command_failure "git-worktree-checkout-branch" "Should fail without branch name"
    
    # Test checkout-branch with invalid branch name
    assert_command_failure "git-worktree-checkout-branch 'invalid branch name'" "Should fail with invalid branch name"
    
    # Test checkout-branch with existing branch
    git-worktree-checkout-branch feature/test || return 1
    assert_command_failure "git-worktree-checkout-branch feature/test" "Should fail with existing branch"
    
    # Test checkout-branch with nonexistent base branch
    assert_command_failure "git-worktree-checkout-branch feature/test2 nonexistent-base" "Should fail with nonexistent base branch"
    
    return 0
}

# Test checkout-branch with various branch naming conventions
test_checkout_branch_naming() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-naming" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-naming"
    
    # Test various branch naming conventions
    local branch_names=("feature/user-auth" "bugfix-123" "hotfix_urgent" "release-v1.0.0" "chore/update-deps")
    
    for branch in "${branch_names[@]}"; do
        git-worktree-checkout-branch "$branch" || return 1
        assert_directory_exists "$branch" || return 1
        assert_git_worktree "$branch" "$branch" || return 1
    done
    
    return 0
}

# Test checkout-branch with direnv integration
test_checkout_branch_direnv() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-direnv" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-direnv"
    
    # Create a branch with .envrc
    git-worktree-checkout-branch feature/with-envrc || return 1
    
    # Add .envrc file
    echo "export TEST_VAR=feature_value" > "feature/with-envrc/.envrc"
    
    # The binary should handle direnv gracefully
    assert_directory_exists "feature/with-envrc" || return 1
    assert_file_exists "feature/with-envrc/.envrc" || return 1
    
    return 0
}

# Test checkout-branch outside git repository
test_checkout_branch_outside_repo() {
    # Test checkout-branch command outside git repository
    assert_command_failure "git-worktree-checkout-branch some-branch" "Should fail outside git repository"
    
    return 0
}

# Test checkout-branch help functionality
test_checkout_branch_help() {
    # Test help commands
    assert_command_help "git-worktree-checkout-branch" || return 1
    assert_command_version "git-worktree-checkout-branch" || return 1
    
    return 0
}

# Test checkout-branch with complex workflow
test_checkout_branch_workflow() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-workflow" "main")
    local start_dir=$(pwd)
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-workflow"
    local repo_root=$(pwd)
    
    # Create first branch and add content to it
    git-worktree-checkout-branch feature/base-feature || return 1
    
    # The Rust binary changes its own directory but not our shell's directory
    # We need to explicitly cd into the worktree
    cd "$repo_root/feature/base-feature"
    
    echo "Base feature implementation" > base.txt
    git add base.txt >/dev/null 2>&1
    git commit -m "Add base feature" >/dev/null 2>&1
    
    # Create second branch from the first one (go back to repo root first)
    cd "$repo_root"
    git-worktree-checkout-branch feature/extended-feature feature/base-feature || return 1
    
    # Go back to repo root for assertions
    cd "$repo_root"
    
    # Verify both branches exist and have correct content
    assert_directory_exists "feature/base-feature" || return 1
    assert_directory_exists "feature/extended-feature" || return 1
    
    
    assert_file_exists "feature/extended-feature/base.txt" || return 1
    
    return 0
}

# Test checkout-branch performance
test_checkout_branch_performance() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-perf" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-perf"
    
    # Test checkout-branch performance
    local start_time=$(date +%s)
    git-worktree-checkout-branch feature/performance-test || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 10 ]]; then
        log_warning "Checkout-branch performance test took ${duration}s (expected < 10s)"
    else
        log_success "Checkout-branch performance test completed in ${duration}s"
    fi
    
    # Verify structure
    assert_directory_exists "feature/performance-test" || return 1
    assert_git_worktree "feature/performance-test" "feature/performance-test" || return 1
    
    return 0
}

# Test checkout-branch with large repository
test_checkout_branch_large_repo() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-large" "main")
    
    # Add many files to the repository
    local temp_clone="$TEMP_BASE_DIR/temp_large_branch_clone"
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
    cd "test-repo-checkout-branch-large"
    
    # Test checkout-branch with large repository
    git-worktree-checkout-branch feature/large-test || return 1
    
    # Verify structure and some files
    assert_directory_exists "feature/large-test" || return 1
    assert_file_exists "feature/large-test/large_file_1.txt" || return 1
    assert_file_exists "feature/large-test/large_file_50.txt" || return 1
    
    return 0
}

# Test checkout-branch with uncommitted changes
test_checkout_branch_with_uncommitted() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-uncommitted" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-uncommitted"
    
    # Make uncommitted changes in main worktree
    echo "Uncommitted changes" > "main/uncommitted.txt"
    
    # Test checkout-branch should still work (creates new worktree)
    git-worktree-checkout-branch feature/new-branch || return 1
    
    # Verify both worktrees exist
    assert_directory_exists "feature/new-branch" || return 1
    assert_git_worktree "feature/new-branch" "feature/new-branch" || return 1
    assert_file_exists "main/uncommitted.txt" || return 1
    
    return 0
}

# Test checkout-branch security - path traversal prevention
test_checkout_branch_security() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-security" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-security"
    
    # Test that path traversal attempts are handled safely
    assert_command_failure "git-worktree-checkout-branch ../../../etc/passwd" "Should fail with path traversal attempt"
    assert_command_failure "git-worktree-checkout-branch ..\\..\\..\\windows\\system32" "Should fail with Windows path traversal"
    
    # Verify no directories were created outside the repository
    if [[ -d "../../../etc" ]] || [[ -d "..\\..\\..\\windows" ]]; then
        log_error "Path traversal attack succeeded - security vulnerability!"
        return 1
    fi
    
    return 0
}

# Run all checkout-branch tests
run_checkout_branch_tests() {
    log "Running git-worktree-checkout-branch integration tests..."
    
    run_test "checkout_branch_basic" "test_checkout_branch_basic"
    run_test "checkout_branch_with_base" "test_checkout_branch_with_base"
    run_test "checkout_branch_from_subdirectory" "test_checkout_branch_from_subdirectory"
    run_test "checkout_branch_errors" "test_checkout_branch_errors"
    run_test "checkout_branch_naming" "test_checkout_branch_naming"
    run_test "checkout_branch_direnv" "test_checkout_branch_direnv"
    run_test "checkout_branch_outside_repo" "test_checkout_branch_outside_repo"
    run_test "checkout_branch_help" "test_checkout_branch_help"
    run_test "checkout_branch_workflow" "test_checkout_branch_workflow"
    run_test "checkout_branch_performance" "test_checkout_branch_performance"
    run_test "checkout_branch_large_repo" "test_checkout_branch_large_repo"
    run_test "checkout_branch_with_uncommitted" "test_checkout_branch_with_uncommitted"
    run_test "checkout_branch_security" "test_checkout_branch_security"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_branch_tests
    print_summary
fi