#!/bin/bash

# Tests for git-worktree-init command

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic init functionality
test_init_basic() {
    # Test basic init
    git worktree-init test-repo || return 1
    
    # Verify structure
    assert_directory_exists "test-repo" || return 1
    assert_directory_exists "test-repo/.git" || return 1
    assert_directory_exists "test-repo/master" || return 1
    assert_git_repository "test-repo/master" || return 1
    assert_git_worktree "test-repo/master" "master" || return 1
    
    return 0
}

# Test init with custom initial branch
test_init_custom_branch() {
    # Test init with custom initial branch
    git worktree-init --initial-branch main test-repo-main || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-main" || return 1
    assert_directory_exists "test-repo-main/.git" || return 1
    assert_directory_exists "test-repo-main/main" || return 1
    assert_git_worktree "test-repo-main/main" "main" || return 1
    
    return 0
}

# Test init with short option for initial branch
test_init_custom_branch_short() {
    # Test init with -b option
    git worktree-init -b develop test-repo-develop || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-develop" || return 1
    assert_directory_exists "test-repo-develop/.git" || return 1
    assert_directory_exists "test-repo-develop/develop" || return 1
    assert_git_worktree "test-repo-develop/develop" "develop" || return 1
    
    return 0
}

# Test init with --bare option
test_init_bare() {
    # Test init with --bare
    git worktree-init --bare test-repo-bare || return 1
    
    # Verify structure (should only have .git directory)
    assert_directory_exists "test-repo-bare" || return 1
    assert_directory_exists "test-repo-bare/.git" || return 1
    
    # Should NOT have worktree directory
    if [[ -d "test-repo-bare/master" ]]; then
        log_error "Worktree directory should not exist in --bare mode"
        return 1
    fi
    
    # Verify it's a valid git repository
    assert_git_repository "test-repo-bare/.git" || return 1
    
    return 0
}

# Test init with --quiet option
test_init_quiet() {
    # Test init with --quiet (should not produce output)
    local output=$(git worktree-init --quiet test-repo-quiet 2>&1)
    
    if [[ -n "$output" ]]; then
        log_error "Quiet mode should not produce output, but got: $output"
        return 1
    fi
    
    # Verify structure still works
    assert_directory_exists "test-repo-quiet" || return 1
    assert_directory_exists "test-repo-quiet/master" || return 1
    assert_git_worktree "test-repo-quiet/master" "master" || return 1
    
    return 0
}

# Test init with existing directory
test_init_existing_directory() {
    # Create conflicting directory
    mkdir -p "test-repo-existing"
    
    # Test init should fail
    assert_command_failure "git worktree-init test-repo-existing" "Should fail with existing directory"
    
    return 0
}

# Test init with no repository name
test_init_no_repo_name() {
    # Test init without repository name should fail
    assert_command_failure "git worktree-init" "Should fail with no repository name"
    
    return 0
}

# Test init with invalid repository name
test_init_invalid_repo_name() {
    # Test init with path separators should fail
    assert_command_failure "git worktree-init path/to/repo" "Should fail with path separators in name"
    assert_command_failure "git worktree-init path\\to\\repo" "Should fail with backslashes in name"
    
    return 0
}

# Test init with empty initial branch name
test_init_empty_branch_name() {
    # Test init with empty branch name should fail
    assert_command_failure "git worktree-init --initial-branch '' test-repo" "Should fail with empty branch name"
    
    return 0
}

# Test init with missing branch name argument
test_init_missing_branch_argument() {
    # Test init with missing branch name after --initial-branch should fail
    assert_command_failure "git worktree-init --initial-branch test-repo" "Should fail with missing branch name"
    
    return 0
}

# Test init with multiple repository names
test_init_multiple_repo_names() {
    # Test init with multiple repository names should fail
    assert_command_failure "git worktree-init repo1 repo2" "Should fail with multiple repository names"
    
    return 0
}

# Test init with unknown option
test_init_unknown_option() {
    # Test init with unknown option should fail
    assert_command_failure "git worktree-init --unknown-option test-repo" "Should fail with unknown option"
    
    return 0
}

# Test init creates valid git repository
test_init_git_repository_validity() {
    # Test init
    git worktree-init test-repo-validity || return 1
    
    # Verify git repository is functional
    cd "test-repo-validity/master"
    
    # Test basic git operations
    echo "# Test Repository" > README.md
    git add README.md || return 1
    git commit -m "Initial commit" || return 1
    
    # Verify commit exists
    assert_command_success "git log --oneline" "Should have commit history"
    
    return 0
}

# Test init with complex branch names
test_init_complex_branch_names() {
    # Test init with branch names containing hyphens and numbers
    git worktree-init -b feature-v2.0 test-repo-complex || return 1
    
    # Verify structure
    assert_directory_exists "test-repo-complex" || return 1
    assert_directory_exists "test-repo-complex/feature-v2.0" || return 1
    assert_git_worktree "test-repo-complex/feature-v2.0" "feature-v2.0" || return 1
    
    return 0
}

# Test init directory permissions
test_init_directory_permissions() {
    # Test init creates directories with correct permissions
    git worktree-init test-repo-permissions || return 1
    
    # Verify directories exist and are readable/writable
    assert_directory_exists "test-repo-permissions" || return 1
    assert_directory_exists "test-repo-permissions/master" || return 1
    
    # Test we can write to the directory
    echo "test content" > "test-repo-permissions/master/test_file.txt"
    assert_file_exists "test-repo-permissions/master/test_file.txt" || return 1
    
    return 0
}

# Test init with direnv integration
test_init_direnv_integration() {
    # Test init
    git worktree-init test-repo-direnv || return 1
    
    # Create .envrc file
    cd "test-repo-direnv/master"
    echo "export TEST_VAR=test_value" > .envrc
    
    # Test that .envrc can be created (direnv integration should handle gracefully)
    assert_file_exists ".envrc" || return 1
    
    return 0
}

# Test init bare mode functionality
test_init_bare_functionality() {
    # Test init with --bare
    git worktree-init --bare test-repo-bare-func || return 1
    
    # Verify we can add worktrees to bare repository
    cd "test-repo-bare-func"
    git worktree add master || return 1
    
    # Verify worktree was created
    assert_directory_exists "master" || return 1
    assert_git_worktree "master" "master" || return 1
    
    return 0
}

# Test init with combination of options
test_init_option_combinations() {
    # Test init with --quiet and --initial-branch
    local output=$(git worktree-init --quiet --initial-branch main test-repo-combo 2>&1)
    
    if [[ -n "$output" ]]; then
        log_error "Quiet mode should not produce output, but got: $output"
        return 1
    fi
    
    # Verify structure
    assert_directory_exists "test-repo-combo" || return 1
    assert_directory_exists "test-repo-combo/main" || return 1
    assert_git_worktree "test-repo-combo/main" "main" || return 1
    
    return 0
}

# Run all init tests
run_init_tests() {
    log "Running git-worktree-init tests..."
    
    run_test "init_basic" "test_init_basic"
    run_test "init_custom_branch" "test_init_custom_branch"
    run_test "init_custom_branch_short" "test_init_custom_branch_short"
    run_test "init_bare" "test_init_bare"
    run_test "init_quiet" "test_init_quiet"
    run_test "init_existing_directory" "test_init_existing_directory"
    run_test "init_no_repo_name" "test_init_no_repo_name"
    run_test "init_invalid_repo_name" "test_init_invalid_repo_name"
    run_test "init_empty_branch_name" "test_init_empty_branch_name"
    run_test "init_missing_branch_argument" "test_init_missing_branch_argument"
    run_test "init_multiple_repo_names" "test_init_multiple_repo_names"
    run_test "init_unknown_option" "test_init_unknown_option"
    run_test "init_git_repository_validity" "test_init_git_repository_validity"
    run_test "init_complex_branch_names" "test_init_complex_branch_names"
    run_test "init_directory_permissions" "test_init_directory_permissions"
    run_test "init_direnv_integration" "test_init_direnv_integration"
    run_test "init_bare_functionality" "test_init_bare_functionality"
    run_test "init_option_combinations" "test_init_option_combinations"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_init_tests
    print_summary
fi