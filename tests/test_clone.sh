#!/bin/bash

# Tests for git-worktree-clone command

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic clone functionality
test_clone_basic() {
    local remote_repo=$(create_test_remote "test-repo-basic" "main")
    
    # Test basic clone
    git worktree-clone "$remote_repo" || return 1
    
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
    git worktree-clone "$remote_repo" || return 1
    
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
    git worktree-clone --no-checkout "$remote_repo" || return 1
    
    # Verify structure (should only have .git directory)
    assert_directory_exists "test-repo-no-checkout" || return 1
    assert_directory_exists "test-repo-no-checkout/.git" || return 1
    
    # Should NOT have worktree directory
    if [[ -d "test-repo-no-checkout/main" ]]; then
        log_error "Worktree directory should not exist in --no-checkout mode"
        return 1
    fi
    
    return 0
}

# Test clone with --quiet option
test_clone_quiet() {
    local remote_repo=$(create_test_remote "test-repo-quiet" "main")
    
    # Test clone with --quiet (should not produce output)
    local output=$(git worktree-clone --quiet "$remote_repo" 2>&1)
    
    if [[ -n "$output" ]]; then
        log_error "Quiet mode should not produce output, but got: $output"
        return 1
    fi
    
    # Verify structure still works
    assert_directory_exists "test-repo-quiet" || return 1
    assert_directory_exists "test-repo-quiet/main" || return 1
    assert_git_worktree "test-repo-quiet/main" "main" || return 1
    
    return 0
}

# Test clone with --all-branches option
test_clone_all_branches() {
    local remote_repo=$(create_test_remote "test-repo-all-branches" "main")
    
    # Test clone with --all-branches
    git worktree-clone --all-branches "$remote_repo" || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-all-branches" || return 1
    assert_directory_exists "test-repo-all-branches/.git" || return 1
    assert_directory_exists "test-repo-all-branches/main" || return 1
    assert_directory_exists "test-repo-all-branches/develop" || return 1
    # Now we create the full path for the branch directory
    assert_directory_exists "test-repo-all-branches/feature/test-feature" || return 1
    
    # Verify all worktrees
    assert_git_worktree "test-repo-all-branches/main" "main" || return 1
    assert_git_worktree "test-repo-all-branches/develop" "develop" || return 1
    # Git creates a local branch called "feature/test-feature" when checking out "feature/test-feature"
    assert_git_worktree "test-repo-all-branches/feature/test-feature" "feature/test-feature" || return 1
    
    return 0
}

# Test clone with invalid repository
test_clone_invalid_repo() {
    # Test clone with non-existent repository
    assert_command_failure "git worktree-clone /nonexistent/repo" "Should fail with invalid repository"
    
    return 0
}

# Test clone with conflicting options
test_clone_conflicting_options() {
    local remote_repo=$(create_test_remote "test-repo-conflict" "main")
    
    # Test clone with conflicting --no-checkout and --all-branches
    assert_command_failure "git worktree-clone --no-checkout --all-branches '$remote_repo'" "Should fail with conflicting options"
    
    return 0
}

# Test clone with existing directory
test_clone_existing_directory() {
    local remote_repo=$(create_test_remote "test-repo-existing" "main")
    
    # Create conflicting directory
    mkdir -p "test-repo-existing"
    
    # Test clone should fail
    assert_command_failure "git worktree-clone '$remote_repo'" "Should fail with existing directory"
    
    return 0
}

# Test clone with SSH URL format
test_clone_ssh_url() {
    local remote_repo=$(create_test_remote "ssh-repo" "main")
    
    # Test with file:// URL (simulating SSH)
    git worktree-clone "file://$remote_repo" || return 1
    
    # Verify structure
    assert_directory_exists "ssh-repo" || return 1
    assert_directory_exists "ssh-repo/main" || return 1
    assert_git_worktree "ssh-repo/main" "main" || return 1
    
    return 0
}

# Test clone repository name extraction
test_clone_repo_name_extraction() {
    local remote_repo=$(create_test_remote "complex-repo-name" "main")
    
    # Test with basic URL format
    git worktree-clone "$remote_repo" || return 1
    assert_directory_exists "complex-repo-name" || return 1
    
    return 0
}

# Test direnv integration (if available)
test_clone_direnv_integration() {
    local remote_repo=$(create_test_remote "direnv-repo" "main")
    
    # Create a repository with .envrc file
    local temp_clone="$TEMP_BASE_DIR/temp_envrc_$$"
    git clone "$remote_repo" "$temp_clone"
    
    (
        cd "$temp_clone"
        echo "export TEST_VAR=test_value" > .envrc
        git add .envrc
        git commit -m "Add .envrc file"
        git push origin main
    )
    
    rm -rf "$temp_clone"
    
    # Test clone (should handle .envrc gracefully whether direnv is available or not)
    git worktree-clone "$remote_repo" || return 1
    
    # Verify structure
    assert_directory_exists "direnv-repo" || return 1
    assert_directory_exists "direnv-repo/main" || return 1
    assert_file_exists "direnv-repo/main/.envrc" || return 1
    
    return 0
}

# Run all clone tests
run_clone_tests() {
    log "Running git-worktree-clone tests..."
    
    run_test "clone_basic" "test_clone_basic"
    run_test "clone_different_default_branch" "test_clone_different_default_branch"
    run_test "clone_no_checkout" "test_clone_no_checkout"
    run_test "clone_quiet" "test_clone_quiet"
    run_test "clone_all_branches" "test_clone_all_branches"
    run_test "clone_invalid_repo" "test_clone_invalid_repo"
    run_test "clone_conflicting_options" "test_clone_conflicting_options"
    run_test "clone_existing_directory" "test_clone_existing_directory"
    run_test "clone_ssh_url" "test_clone_ssh_url"
    run_test "clone_repo_name_extraction" "test_clone_repo_name_extraction"
    run_test "clone_direnv_integration" "test_clone_direnv_integration"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_clone_tests
    print_summary
fi