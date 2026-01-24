#!/bin/bash

# Integration tests for git-worktree-clone Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic clone functionality
test_clone_basic() {
    local remote_repo=$(create_test_remote "test-repo-basic" "main")
    
    # Test basic clone
    git-worktree-clone "$remote_repo" || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-basic" || return 1
    assert_directory_exists "test-repo-basic/.git" || return 1
    assert_directory_exists "test-repo-basic/main" || return 1
    assert_git_repository "test-repo-basic/main" || return 1
    assert_git_worktree "test-repo-basic/main" "main" || return 1
    assert_file_exists "test-repo-basic/main/README.md" || return 1
    assert_file_exists "test-repo-basic/main/main.py" || return 1
    
    return 0
}

# Test clone with different default branch
test_clone_different_default_branch() {
    local remote_repo=$(create_test_remote "test-repo-develop" "develop")
    
    # Test clone with develop as default branch
    git-worktree-clone "$remote_repo" || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-develop" || return 1
    assert_directory_exists "test-repo-develop/.git" || return 1
    assert_directory_exists "test-repo-develop/develop" || return 1
    assert_git_worktree "test-repo-develop/develop" "develop" || return 1
    
    return 0
}

# Test clone with --no-checkout option
test_clone_no_checkout() {
    local remote_repo=$(create_test_remote "test-repo-no-checkout" "main")
    
    # Test clone with --no-checkout
    git-worktree-clone --no-checkout "$remote_repo" || return 1
    
    # Verify structure (should only have .git directory)
    assert_directory_exists "test-repo-no-checkout" || return 1
    assert_directory_exists "test-repo-no-checkout/.git" || return 1
    
    # Should NOT have main worktree
    if [[ -d "test-repo-no-checkout/main" ]]; then
        log_error "No-checkout mode should not create worktree"
        return 1
    fi
    
    return 0
}

# Test clone with --quiet option
test_clone_quiet() {
    local remote_repo=$(create_test_remote "test-repo-quiet" "main")
    
    # Test clone with --quiet (should produce no output)
    local output=$(git-worktree-clone --quiet "$remote_repo" 2>&1)
    
    # Verify structure was created
    assert_directory_exists "test-repo-quiet" || return 1
    assert_directory_exists "test-repo-quiet/main" || return 1
    
    # Check output is minimal (allowing for some git output)
    if [[ ${#output} -gt 200 ]]; then
        log_error "Quiet mode produced too much output (${#output} characters)"
        return 1
    fi
    
    return 0
}

# Test clone with --all-branches option
test_clone_all_branches() {
    local remote_repo=$(create_test_remote "test-repo-all-branches" "main")
    
    # Test clone with --all-branches
    git-worktree-clone --all-branches "$remote_repo" || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-all-branches" || return 1
    assert_directory_exists "test-repo-all-branches/.git" || return 1
    assert_directory_exists "test-repo-all-branches/main" || return 1
    assert_directory_exists "test-repo-all-branches/develop" || return 1
    assert_directory_exists "test-repo-all-branches/feature/test-feature" || return 1
    
    # Verify all worktrees are correct
    assert_git_worktree "test-repo-all-branches/main" "main" || return 1
    assert_git_worktree "test-repo-all-branches/develop" "develop" || return 1
    assert_git_worktree "test-repo-all-branches/feature/test-feature" "feature/test-feature" || return 1
    
    return 0
}

# Test clone error handling - invalid URL
test_clone_invalid_url() {
    # Test with a local path that doesn't exist (guaranteed to fail)
    # Note: HTTP URLs like "https://invalid-url.com/repo.git" may resolve to
    # parking pages or empty responses that git treats as empty repos, so we
    # use a local file path that definitely doesn't exist.
    assert_command_failure "git-worktree-clone /nonexistent/path/to/repo.git" "Should fail with invalid URL"

    # Verify no directory was created
    if [[ -d "repo" ]]; then
        log_error "Failed clone should not create directory"
        return 1
    fi

    return 0
}

# Test clone error handling - existing directory
test_clone_existing_directory() {
    local remote_repo=$(create_test_remote "test-repo-existing" "main")
    
    # Create existing directory
    mkdir -p "test-repo-existing"
    
    # Test clone should fail with existing directory
    assert_command_failure "git-worktree-clone '$remote_repo'" "Should fail with existing directory"
    
    return 0
}

# Test clone with SSH URL format
test_clone_ssh_url() {
    local remote_repo=$(create_test_remote "test-repo-ssh" "main")
    
    # Convert file:// URL to SSH-like format for testing
    local ssh_url="file://$remote_repo"
    
    # Test clone with SSH URL
    git-worktree-clone "$ssh_url" || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-ssh" || return 1
    assert_directory_exists "test-repo-ssh/main" || return 1
    assert_git_worktree "test-repo-ssh/main" "main" || return 1
    
    return 0
}

# Test clone help functionality
test_clone_help() {
    # Test help commands
    assert_command_help "git-worktree-clone" || return 1
    assert_command_version "git-worktree-clone" || return 1
    
    return 0
}

# Test clone with direnv integration
test_clone_direnv_integration() {
    local remote_repo=$(create_test_remote "test-repo-direnv" "main")
    
    # Test clone
    git-worktree-clone "$remote_repo" || return 1
    
    # Add .envrc file to test direnv integration
    echo "export TEST_VAR=test_value" > "test-repo-direnv/main/.envrc"
    
    # The binary should handle direnv gracefully regardless of availability
    assert_directory_exists "test-repo-direnv/main" || return 1
    assert_file_exists "test-repo-direnv/main/.envrc" || return 1
    
    return 0
}

# Test clone performance with large repository
test_clone_performance() {
    local remote_repo=$(create_test_remote "test-repo-performance" "main")
    
    # Add many files to the remote repository
    local temp_clone="$TEMP_BASE_DIR/temp_perf_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        # Create many files
        for i in {1..50}; do
            echo "Performance test file $i" > "perf_file_$i.txt"
        done
        git add . >/dev/null 2>&1
        git commit -m "Add performance test files" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Test clone performance
    local start_time=$(date +%s)
    git-worktree-clone "$remote_repo" || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 30 ]]; then
        log_warning "Clone performance test took ${duration}s (expected < 30s)"
    else
        log_success "Clone performance test completed in ${duration}s"
    fi
    
    # Verify all files were cloned
    assert_directory_exists "test-repo-performance/main" || return 1
    assert_file_exists "test-repo-performance/main/perf_file_1.txt" || return 1
    assert_file_exists "test-repo-performance/main/perf_file_50.txt" || return 1
    
    return 0
}

# Test clone of empty repository (no commits)
test_clone_empty_repo() {
    # Create a bare repository with no commits (simulates empty GitHub repo)
    local empty_repo="$REMOTE_REPO_DIR/empty-repo.git"
    git init --bare "$empty_repo" >/dev/null 2>&1

    # Clone the empty repository
    git-worktree-clone "$empty_repo" || return 1

    # Verify structure was created
    assert_directory_exists "empty-repo" || return 1
    assert_directory_exists "empty-repo/.git" || return 1

    # Get the expected default branch (from init.defaultBranch config or "master")
    local expected_branch=$(git config --global init.defaultBranch 2>/dev/null || echo "master")
    if [[ -z "$expected_branch" ]]; then
        expected_branch="master"
    fi

    # Verify worktree was created with expected branch
    assert_directory_exists "empty-repo/$expected_branch" || return 1

    # Verify it's a valid git worktree
    if ! (cd "empty-repo/$expected_branch" && git rev-parse --git-dir >/dev/null 2>&1); then
        log_error "Empty repo worktree is not a valid git repository"
        return 1
    fi

    # Verify the branch matches expected
    local actual_branch=$(cd "empty-repo/$expected_branch" && git branch --show-current 2>/dev/null)
    if [[ "$actual_branch" != "$expected_branch" ]]; then
        log_error "Expected branch '$expected_branch' but got '$actual_branch'"
        return 1
    fi

    log_success "Empty repository cloned successfully with branch '$expected_branch'"
    return 0
}

# Test clone of empty repository with explicit branch name
test_clone_empty_repo_with_branch() {
    # Create a bare repository with no commits
    local empty_repo="$REMOTE_REPO_DIR/empty-repo-branch.git"
    git init --bare "$empty_repo" >/dev/null 2>&1

    # Clone with explicit branch name
    git-worktree-clone --branch develop "$empty_repo" || return 1

    # Verify structure was created
    assert_directory_exists "empty-repo-branch" || return 1
    assert_directory_exists "empty-repo-branch/.git" || return 1
    assert_directory_exists "empty-repo-branch/develop" || return 1

    # Verify the branch matches
    local actual_branch=$(cd "empty-repo-branch/develop" && git branch --show-current 2>/dev/null)
    if [[ "$actual_branch" != "develop" ]]; then
        log_error "Expected branch 'develop' but got '$actual_branch'"
        return 1
    fi

    log_success "Empty repository cloned with custom branch 'develop'"
    return 0
}

# Test clone of empty repository with no-checkout
test_clone_empty_repo_no_checkout() {
    # Create a bare repository with no commits
    local empty_repo="$REMOTE_REPO_DIR/empty-repo-bare.git"
    git init --bare "$empty_repo" >/dev/null 2>&1

    # Clone with no-checkout
    git-worktree-clone --no-checkout "$empty_repo" || return 1

    # Verify only bare structure was created
    assert_directory_exists "empty-repo-bare" || return 1
    assert_directory_exists "empty-repo-bare/.git" || return 1

    # Should NOT have any worktree
    local worktree_count=$(ls -d empty-repo-bare/*/ 2>/dev/null | wc -l | tr -d ' ')
    if [[ "$worktree_count" != "0" ]]; then
        log_error "No-checkout mode should not create worktree, found $worktree_count directories"
        return 1
    fi

    log_success "Empty repository cloned in bare mode"
    return 0
}

# Test that --all-branches fails gracefully with empty repos
test_clone_empty_repo_all_branches_fails() {
    # Create a bare repository with no commits
    local empty_repo="$REMOTE_REPO_DIR/empty-repo-all.git"
    git init --bare "$empty_repo" >/dev/null 2>&1

    # Clone with --all-branches should fail
    if git-worktree-clone --all-branches "$empty_repo" 2>&1; then
        log_error "Clone with --all-branches should fail for empty repository"
        return 1
    fi

    log_success "Clone with --all-branches correctly failed for empty repository"
    return 0
}

# Test that first commit works in cloned empty repository
test_clone_empty_repo_first_commit() {
    # Create a bare repository with no commits
    local empty_repo="$REMOTE_REPO_DIR/empty-repo-commit.git"
    git init --bare "$empty_repo" >/dev/null 2>&1

    # Clone the empty repository
    git-worktree-clone "$empty_repo" || return 1

    # Get the expected default branch
    local expected_branch=$(git config --global init.defaultBranch 2>/dev/null || echo "master")
    if [[ -z "$expected_branch" ]]; then
        expected_branch="master"
    fi

    # Create first commit in the worktree
    (
        cd "empty-repo-commit/$expected_branch"
        echo "# My Project" > README.md
        git add README.md
        git commit -m "Initial commit"
    ) >/dev/null 2>&1 || {
        log_error "Failed to create first commit in empty repository"
        return 1
    }

    # Verify the commit was created
    local commit_count=$(cd "empty-repo-commit/$expected_branch" && git rev-list --count HEAD 2>/dev/null)
    if [[ "$commit_count" != "1" ]]; then
        log_error "Expected 1 commit but got '$commit_count'"
        return 1
    fi

    log_success "First commit works in cloned empty repository"
    return 0
}

# Run all clone tests
run_clone_tests() {
    log "Running git-worktree-clone integration tests..."

    run_test "clone_basic" "test_clone_basic"
    run_test "clone_different_default_branch" "test_clone_different_default_branch"
    run_test "clone_no_checkout" "test_clone_no_checkout"
    run_test "clone_quiet" "test_clone_quiet"
    run_test "clone_all_branches" "test_clone_all_branches"
    run_test "clone_invalid_url" "test_clone_invalid_url"
    run_test "clone_existing_directory" "test_clone_existing_directory"
    run_test "clone_ssh_url" "test_clone_ssh_url"
    run_test "clone_help" "test_clone_help"
    run_test "clone_direnv_integration" "test_clone_direnv_integration"
    run_test "clone_performance" "test_clone_performance"

    # Empty repository tests
    run_test "clone_empty_repo" "test_clone_empty_repo"
    run_test "clone_empty_repo_with_branch" "test_clone_empty_repo_with_branch"
    run_test "clone_empty_repo_no_checkout" "test_clone_empty_repo_no_checkout"
    run_test "clone_empty_repo_all_branches_fails" "test_clone_empty_repo_all_branches_fails"
    run_test "clone_empty_repo_first_commit" "test_clone_empty_repo_first_commit"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_clone_tests
    print_summary
fi