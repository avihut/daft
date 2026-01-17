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

    return 0
}

# Test checkout cd to existing worktree
test_checkout_existing_worktree() {
    local remote_repo=$(create_test_remote "test-repo-checkout-existing" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-existing"

    # First checkout creates the worktree
    git-worktree-checkout develop || return 1
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1

    # Go back to main
    cd main

    # Second checkout should succeed and cd to existing worktree
    local output
    output=$(git-worktree-checkout develop 2>&1) || {
        log_error "Second checkout should succeed, but failed"
        echo "$output"
        return 1
    }

    # Verify output contains the expected message (switched to existing worktree)
    if ! echo "$output" | grep -qE "(existing worktree|already has a worktree)"; then
        log_error "Output should mention 'existing worktree' or 'already has a worktree'"
        echo "$output"
        return 1
    fi

    # Verify shell integration marker is present when DAFT_SHELL_WRAPPER is set
    local output_with_wrapper
    output_with_wrapper=$(DAFT_SHELL_WRAPPER=1 git-worktree-checkout develop 2>&1) || {
        log_error "Second checkout with shell wrapper should succeed, but failed"
        echo "$output_with_wrapper"
        return 1
    }

    if ! echo "$output_with_wrapper" | grep -q "__DAFT_CD__:"; then
        log_error "Output with DAFT_SHELL_WRAPPER=1 should contain shell integration marker __DAFT_CD__:"
        echo "$output_with_wrapper"
        return 1
    fi

    log_success "Checkout to existing worktree works correctly"
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

# =============================================================================
# Carry Feature Tests
# =============================================================================

# Test checkout default does NOT carry changes
test_checkout_no_carry_default() {
    local remote_repo=$(create_test_remote "test-repo-checkout-no-carry-default" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-no-carry-default"
    repo_root=$(pwd)

    # Create untracked file in main worktree
    cd main
    echo "should stay in original" > local_file.txt

    # Checkout existing branch (default should NOT carry changes)
    git-worktree-checkout develop || return 1

    cd "$repo_root"

    # Verify file is NOT in new worktree
    assert_file_not_exists "develop/local_file.txt" "File should NOT be carried by default" || return 1

    # Verify file IS still in original worktree
    assert_file_exists "main/local_file.txt" "File should remain in original worktree" || return 1

    return 0
}

# Test checkout --carry flag carries changes
test_checkout_carry_flag() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-flag" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-carry-flag"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "carry me" > carry_file.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify file is in new worktree
    assert_file_exists "carry_file.txt" "File should be carried with --carry flag" || return 1
    assert_file_contains "carry_file.txt" "carry me" "File content should be correct" || return 1

    return 0
}

# Test checkout -c shorthand carries changes
test_checkout_carry_shorthand() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-shorthand" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-carry-shorthand"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "shorthand content" > shorthand_file.txt

    # Checkout with -c shorthand
    git-worktree-checkout -c develop || return 1

    cd "$repo_root/develop"

    # Verify file is in new worktree
    assert_file_exists "shorthand_file.txt" "File should be carried with -c shorthand" || return 1

    return 0
}

# Test checkout --no-carry explicit (same as default)
test_checkout_no_carry_explicit() {
    local remote_repo=$(create_test_remote "test-repo-checkout-no-carry-explicit" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-no-carry-explicit"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "explicit no carry" > explicit_file.txt

    # Checkout with explicit --no-carry flag
    git-worktree-checkout --no-carry develop || return 1

    cd "$repo_root"

    # Verify file is NOT in new worktree
    assert_file_not_exists "develop/explicit_file.txt" "File should NOT be carried with --no-carry" || return 1

    # Verify file IS still in original worktree
    assert_file_exists "main/explicit_file.txt" "File should remain in original worktree" || return 1

    return 0
}

# Test checkout --carry with staged changes
test_checkout_carry_staged() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-staged" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-carry-staged"
    repo_root=$(pwd)

    # Create and stage a file in main worktree
    cd main
    echo "staged content" > staged_file.txt
    git add staged_file.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify staged file is in new worktree
    assert_file_exists "staged_file.txt" "Staged file should be carried" || return 1

    return 0
}

# Test checkout --carry with untracked files
test_checkout_carry_untracked() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-untracked" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-carry-untracked"
    repo_root=$(pwd)

    # Create untracked file in main worktree
    cd main
    echo "untracked content" > untracked_file.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify untracked file is in new worktree
    assert_file_exists "untracked_file.txt" "Untracked file should be carried" || return 1

    return 0
}

# Test checkout --carry with mixed changes
test_checkout_carry_mixed() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-mixed" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-carry-mixed"
    repo_root=$(pwd)

    # Create mixed changes in main worktree
    cd main
    echo "staged" > staged.txt
    git add staged.txt
    echo "unstaged modification" >> README.md
    echo "untracked" > untracked.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify all changes are in new worktree
    assert_file_exists "staged.txt" "Staged file should be carried" || return 1
    assert_file_contains "README.md" "unstaged modification" "Unstaged modification should be carried" || return 1
    assert_file_exists "untracked.txt" "Untracked file should be carried" || return 1

    return 0
}

# Test checkout with no uncommitted changes works normally
test_checkout_carry_no_changes() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-clean" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-carry-clean"
    repo_root=$(pwd)

    # No changes - just checkout
    git-worktree-checkout develop || return 1

    cd "$repo_root"

    # Verify worktree was created successfully
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1

    return 0
}

# Test checkout help shows carry flags
test_checkout_carry_help() {
    # Verify --carry and --no-carry appear in help
    local help_output
    help_output=$(git-worktree-checkout --help 2>&1)

    if echo "$help_output" | grep -q "\-\-carry"; then
        log_success "--carry flag appears in help"
    else
        log_error "--carry flag missing from help output"
        return 1
    fi

    if echo "$help_output" | grep -q "\-\-no-carry"; then
        log_success "--no-carry flag appears in help"
    else
        log_error "--no-carry flag missing from help output"
        return 1
    fi

    if echo "$help_output" | grep -q "\-c"; then
        log_success "-c shorthand appears in help"
    else
        log_error "-c shorthand missing from help output"
        return 1
    fi

    return 0
}

# Run all checkout tests
run_checkout_tests() {
    log "Running git-worktree-checkout integration tests..."

    run_test "checkout_basic" "test_checkout_basic"
    run_test "checkout_remote_branch" "test_checkout_remote_branch"
    run_test "checkout_from_subdirectory" "test_checkout_from_subdirectory"
    run_test "checkout_errors" "test_checkout_errors"
    run_test "checkout_existing_worktree" "test_checkout_existing_worktree"
    run_test "checkout_direnv" "test_checkout_direnv"
    run_test "checkout_outside_repo" "test_checkout_outside_repo"
    run_test "checkout_help" "test_checkout_help"
    run_test "checkout_complex_branches" "test_checkout_complex_branches"
    run_test "checkout_performance" "test_checkout_performance"
    run_test "checkout_large_repo" "test_checkout_large_repo"
    run_test "checkout_with_uncommitted_changes" "test_checkout_with_uncommitted_changes"

    # Carry feature tests
    run_test "checkout_no_carry_default" "test_checkout_no_carry_default"
    run_test "checkout_carry_flag" "test_checkout_carry_flag"
    run_test "checkout_carry_shorthand" "test_checkout_carry_shorthand"
    run_test "checkout_no_carry_explicit" "test_checkout_no_carry_explicit"
    run_test "checkout_carry_staged" "test_checkout_carry_staged"
    run_test "checkout_carry_untracked" "test_checkout_carry_untracked"
    run_test "checkout_carry_mixed" "test_checkout_carry_mixed"
    run_test "checkout_carry_no_changes" "test_checkout_carry_no_changes"
    run_test "checkout_carry_help" "test_checkout_carry_help"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_tests
    print_summary
fi