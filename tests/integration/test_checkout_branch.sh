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

# =============================================================================
# Carry Feature Tests
# =============================================================================

# Test checkout-branch default carries staged changes
test_checkout_branch_carry_staged() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-staged" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-staged"
    repo_root=$(pwd)

    # Create and stage a file in main worktree
    cd main
    echo "staged content" > staged_file.txt
    git add staged_file.txt

    # Create new branch (default should carry changes)
    git-worktree-checkout-branch feature/carry-staged || return 1

    cd "$repo_root/feature/carry-staged"

    # Verify staged file is in new worktree
    assert_file_exists "staged_file.txt" "Staged file should be carried to new worktree" || return 1
    assert_file_contains "staged_file.txt" "staged content" "File should have correct content" || return 1

    # Verify file is NOT in original worktree (stash moves it)
    assert_file_not_exists "$repo_root/main/staged_file.txt" "Staged file should not remain in original worktree" || return 1

    return 0
}

# Test checkout-branch default carries unstaged changes
test_checkout_branch_carry_unstaged() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-unstaged" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-unstaged"
    repo_root=$(pwd)

    # Modify existing tracked file in main worktree (unstaged)
    cd main
    echo "modified content" >> README.md

    # Create new branch (default should carry changes)
    git-worktree-checkout-branch feature/carry-unstaged || return 1

    cd "$repo_root/feature/carry-unstaged"

    # Verify modification is in new worktree
    assert_file_contains "README.md" "modified content" "Modification should be carried to new worktree" || return 1

    return 0
}

# Test checkout-branch default carries untracked files
test_checkout_branch_carry_untracked() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-untracked" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-untracked"
    repo_root=$(pwd)

    # Create untracked file in main worktree
    cd main
    echo "untracked content" > untracked_file.txt

    # Create new branch (default should carry changes including untracked)
    git-worktree-checkout-branch feature/carry-untracked || return 1

    cd "$repo_root/feature/carry-untracked"

    # Verify untracked file is in new worktree
    assert_file_exists "untracked_file.txt" "Untracked file should be carried to new worktree" || return 1
    assert_file_contains "untracked_file.txt" "untracked content" "Untracked file should have correct content" || return 1

    return 0
}

# Test checkout-branch --carry explicit flag
test_checkout_branch_carry_explicit() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-explicit" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-explicit"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "explicit carry content" > explicit_file.txt

    # Create new branch with explicit --carry flag
    git-worktree-checkout-branch --carry feature/explicit-carry || return 1

    cd "$repo_root/feature/explicit-carry"

    # Verify file is in new worktree
    assert_file_exists "explicit_file.txt" "File should be carried with explicit --carry flag" || return 1

    return 0
}

# Test checkout-branch -c shorthand
test_checkout_branch_carry_shorthand() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-short" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-short"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "shorthand carry content" > shorthand_file.txt

    # Create new branch with -c shorthand
    git-worktree-checkout-branch -c feature/shorthand-carry || return 1

    cd "$repo_root/feature/shorthand-carry"

    # Verify file is in new worktree
    assert_file_exists "shorthand_file.txt" "File should be carried with -c shorthand" || return 1

    return 0
}

# Test checkout-branch --no-carry keeps changes in original
test_checkout_branch_no_carry() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-no-carry" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-no-carry"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "no carry content" > no_carry_file.txt

    # Create new branch with --no-carry flag
    git-worktree-checkout-branch --no-carry feature/no-carry || return 1

    cd "$repo_root"

    # Verify file is NOT in new worktree
    assert_file_not_exists "feature/no-carry/no_carry_file.txt" "File should NOT be in new worktree with --no-carry" || return 1

    # Verify file IS still in original worktree
    assert_file_exists "main/no_carry_file.txt" "File should remain in original worktree with --no-carry" || return 1

    return 0
}

# Test checkout-branch with no uncommitted changes
test_checkout_branch_carry_no_changes() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-clean" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-clean"
    repo_root=$(pwd)

    # No changes - just create new branch
    cd main
    git-worktree-checkout-branch feature/clean-create || return 1

    cd "$repo_root"

    # Verify worktree was created successfully
    assert_directory_exists "feature/clean-create" || return 1
    assert_git_worktree "feature/clean-create" "feature/clean-create" || return 1

    return 0
}

# Test checkout-branch carry with mixed changes (staged + unstaged + untracked)
test_checkout_branch_carry_mixed() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-mixed" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-mixed"
    repo_root=$(pwd)

    # Create mixed changes in main worktree
    cd main
    echo "staged" > staged.txt
    git add staged.txt
    echo "unstaged modification" >> README.md
    echo "untracked" > untracked.txt

    # Create new branch (default carries all)
    git-worktree-checkout-branch feature/mixed-carry || return 1

    cd "$repo_root/feature/mixed-carry"

    # Verify all changes are in new worktree
    assert_file_exists "staged.txt" "Staged file should be carried" || return 1
    assert_file_contains "README.md" "unstaged modification" "Unstaged modification should be carried" || return 1
    assert_file_exists "untracked.txt" "Untracked file should be carried" || return 1

    return 0
}

# Test checkout-branch help shows carry flags
test_checkout_branch_carry_help() {
    # Verify --carry and --no-carry appear in help
    local help_output
    help_output=$(git-worktree-checkout-branch --help 2>&1)

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

# Test checkout-branch carry from base branch worktree (not current worktree)
test_checkout_branch_carry_from_base_branch_worktree() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-from-base" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-from-base"
    repo_root=$(pwd)

    # Create a develop worktree (checking out existing remote branch)
    cd main
    git-worktree-checkout develop || return 1

    # Add uncommitted changes in the develop worktree
    cd "$repo_root/develop"
    echo "develop changes" > develop_file.txt

    # Add uncommitted changes in main worktree too (these should NOT be carried)
    cd "$repo_root/main"
    echo "main changes" > main_file.txt

    # From main worktree, create a new branch based on develop
    # Changes should be carried from develop's worktree, not from main
    git-worktree-checkout-branch feature/from-develop develop || return 1

    cd "$repo_root/feature/from-develop"

    # Verify develop's changes are in new worktree (carried from develop)
    assert_file_exists "develop_file.txt" "File from develop worktree should be carried" || return 1
    assert_file_contains "develop_file.txt" "develop changes" "Content should match develop worktree" || return 1

    # Verify main's changes are NOT in new worktree (should not carry from current worktree)
    assert_file_not_exists "main_file.txt" "File from main worktree should NOT be carried" || return 1

    # Verify main's changes are still in main worktree (not stashed away)
    assert_file_exists "$repo_root/main/main_file.txt" "Main worktree changes should remain untouched" || return 1

    return 0
}

# Test checkout-branch carry silently skips when base branch has no worktree
test_checkout_branch_carry_skip_no_worktree() {
    local remote_repo=$(create_test_remote "test-repo-checkout-branch-carry-skip" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-checkout-branch-carry-skip"
    repo_root=$(pwd)

    # Add uncommitted changes in main worktree
    cd main
    echo "main changes" > main_file.txt

    # Create a new branch from develop (which has no worktree - only exists as remote branch)
    # Should succeed without error, and carry should be silently skipped
    git-worktree-checkout-branch feature/from-remote-only develop || return 1

    cd "$repo_root/feature/from-remote-only"

    # Verify main's changes are NOT in new worktree (carry was skipped)
    assert_file_not_exists "main_file.txt" "Main worktree changes should NOT be carried when base has no worktree" || return 1

    # Verify main's changes are still in main worktree
    assert_file_exists "$repo_root/main/main_file.txt" "Main worktree changes should remain" || return 1

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

    # Carry feature tests
    run_test "checkout_branch_carry_staged" "test_checkout_branch_carry_staged"
    run_test "checkout_branch_carry_unstaged" "test_checkout_branch_carry_unstaged"
    run_test "checkout_branch_carry_untracked" "test_checkout_branch_carry_untracked"
    run_test "checkout_branch_carry_explicit" "test_checkout_branch_carry_explicit"
    run_test "checkout_branch_carry_shorthand" "test_checkout_branch_carry_shorthand"
    run_test "checkout_branch_no_carry" "test_checkout_branch_no_carry"
    run_test "checkout_branch_carry_no_changes" "test_checkout_branch_carry_no_changes"
    run_test "checkout_branch_carry_mixed" "test_checkout_branch_carry_mixed"
    run_test "checkout_branch_carry_help" "test_checkout_branch_carry_help"
    run_test "checkout_branch_carry_from_base_branch_worktree" "test_checkout_branch_carry_from_base_branch_worktree"
    run_test "checkout_branch_carry_skip_no_worktree" "test_checkout_branch_carry_skip_no_worktree"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_branch_tests
    print_summary
fi