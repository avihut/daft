#!/bin/bash

# Integration tests for git-worktree-init Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic init functionality
test_init_basic() {
    # Test basic init
    git-worktree-init simple-test-repo || return 1
    
    # Verify structure
    assert_directory_exists "simple-test-repo" || return 1
    assert_directory_exists "simple-test-repo/.git" || return 1
    assert_directory_exists "simple-test-repo/master" || return 1
    assert_git_repository "simple-test-repo/master" || return 1
    assert_git_worktree "simple-test-repo/master" "master" || return 1
    
    return 0
}

# Test init with custom initial branch
test_init_custom_branch() {
    # Test init with custom branch
    git-worktree-init --initial-branch main simple-main-repo || return 1
    
    # Verify structure
    assert_directory_exists "simple-main-repo" || return 1
    assert_directory_exists "simple-main-repo/.git" || return 1
    assert_directory_exists "simple-main-repo/main" || return 1
    assert_git_repository "simple-main-repo/main" || return 1
    assert_git_worktree "simple-main-repo/main" "main" || return 1
    
    return 0
}

# Test init with bare mode
test_init_bare() {
    # Test bare init
    git-worktree-init --bare simple-bare-repo || return 1
    
    # Verify structure
    assert_directory_exists "simple-bare-repo" || return 1
    assert_directory_exists "simple-bare-repo/.git" || return 1
    assert_git_repository "simple-bare-repo/.git" || return 1
    
    # Should NOT have worktree
    if [[ -d "simple-bare-repo/master" ]]; then
        log_error "Bare mode should not create worktree"
        return 1
    fi
    
    return 0
}

# Test init with quiet mode
test_init_quiet() {
    # Test quiet init (should produce minimal output)
    local output=$(git-worktree-init --quiet quiet-repo 2>&1)
    
    # Verify structure was created
    assert_directory_exists "quiet-repo" || return 1
    assert_directory_exists "quiet-repo/master" || return 1
    
    # Check output is minimal
    if [[ ${#output} -gt 100 ]]; then
        log_error "Quiet mode produced too much output (${#output} characters)"
        return 1
    fi
    
    return 0
}

# Test init error handling
test_init_errors() {
    # Test no repo name
    assert_command_failure "git-worktree-init" "Should fail without repo name"
    
    # Test existing directory
    mkdir -p "existing-dir"
    assert_command_failure "git-worktree-init existing-dir" "Should fail with existing directory"
    
    # Test invalid branch name
    assert_command_failure "git-worktree-init --initial-branch 'invalid branch name' test-repo" "Should fail with invalid branch name"
    
    return 0
}

# Test init with different branch naming conventions
test_init_branch_conventions() {
    # Test with various valid branch names
    local branch_names=("main" "develop" "release-1.0" "feature/init" "hotfix_urgent")
    local original_dir="$(pwd)"
    
    for i in "${!branch_names[@]}"; do
        local branch="${branch_names[i]}"
        local repo_name="test-repo-$i"
        
        cd "$original_dir"
        git-worktree-init --initial-branch "$branch" "$repo_name" || return 1
        
        cd "$original_dir"
        assert_directory_exists "$repo_name" || return 1
        assert_directory_exists "$repo_name/$branch" || return 1
        assert_git_worktree "$repo_name/$branch" "$branch" || return 1
    done
    
    cd "$original_dir"
    return 0
}

# Test init with direnv integration
test_init_direnv_integration() {
    # Test init
    git-worktree-init direnv-test-repo || return 1
    
    # Add .envrc file
    echo "export TEST_VAR=test_value" > "direnv-test-repo/master/.envrc"
    
    # The binary should handle direnv gracefully regardless of availability
    assert_directory_exists "direnv-test-repo/master" || return 1
    assert_file_exists "direnv-test-repo/master/.envrc" || return 1
    
    return 0
}

# Test init and subsequent operations
test_init_workflow() {
    local original_dir="$(pwd)"
    
    # Test init followed by adding content
    git-worktree-init workflow-test || return 1
    
    # Add content to the repository
    cd "$original_dir/workflow-test/master"
    echo "# Workflow Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    
    cd "$original_dir"
    # Verify the commit was successful
    assert_file_exists "workflow-test/master/README.md" || return 1
    
    # Test that we can create more worktrees
    cd "$original_dir/workflow-test"
    # Need to specify a start point since this is a new repository with no commits yet when we created the worktree
    if ! git worktree add feature-branch -b feature-branch master >/dev/null 2>&1; then
        log_error "Failed to create feature-branch worktree. Git status:"
        git status || true
        git branch -a || true
        return 1
    fi
    cd "$original_dir"
    assert_directory_exists "workflow-test/feature-branch" || return 1
    
    return 0
}

# Test init with absolute and relative paths
test_init_paths() {
    local original_dir="$(pwd)"
    
    # Test with relative path
    git-worktree-init relative-path-test || return 1
    cd "$original_dir"
    assert_directory_exists "relative-path-test" || return 1
    
    # Test with absolute path (should fail - we don't support absolute paths)
    local abs_path="$(pwd)/absolute-path-test"
    if git-worktree-init "$abs_path" >/dev/null 2>&1; then
        log_error "Absolute path should fail validation"
        return 1
    fi
    log_success "Absolute path correctly rejected"
    
    return 0
}

# Test init help functionality
test_init_help() {
    # Test help commands
    assert_command_help "git-worktree-init" || return 1
    assert_command_version "git-worktree-init" || return 1
    
    return 0
}

# Test init performance
test_init_performance() {
    # Test init performance
    local start_time=$(date +%s)
    git-worktree-init performance-test || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 10 ]]; then
        log_warning "Init performance test took ${duration}s (expected < 10s)"
    else
        log_success "Init performance test completed in ${duration}s"
    fi
    
    # Verify structure
    assert_directory_exists "performance-test" || return 1
    assert_directory_exists "performance-test/master" || return 1
    
    return 0
}

# Test init with various repository name formats
test_init_repo_name_formats() {
    # Test with various valid repository names
    local repo_names=("simple-repo" "my_repo" "MyRepo" "repo123" "my-awesome-project")
    
    for i in "${!repo_names[@]}"; do
        local repo_name="${repo_names[i]}"
        
        git-worktree-init "$repo_name" || return 1
        
        assert_directory_exists "$repo_name" || return 1
        assert_directory_exists "$repo_name/master" || return 1
        assert_git_worktree "$repo_name/master" "master" || return 1
    done
    
    return 0
}

# Test init security - path traversal prevention
test_init_security() {
    # Test that path traversal attempts are handled safely
    assert_command_failure "git-worktree-init ../../../etc/passwd" "Should fail with path traversal attempt"
    assert_command_failure "git-worktree-init ..\\..\\..\\windows\\system32" "Should fail with Windows path traversal"
    
    # Verify no directories were created outside the test directory
    if [[ -d "../../../etc" ]] || [[ -d "..\\..\\..\\windows" ]]; then
        log_error "Path traversal attack succeeded - security vulnerability!"
        return 1
    fi
    
    return 0
}

# Run all init tests
run_init_tests() {
    log "Running git-worktree-init integration tests..."
    
    run_test "init_basic" "test_init_basic"
    run_test "init_custom_branch" "test_init_custom_branch"
    run_test "init_bare" "test_init_bare"
    run_test "init_quiet" "test_init_quiet"
    run_test "init_errors" "test_init_errors"
    run_test "init_branch_conventions" "test_init_branch_conventions"
    run_test "init_direnv_integration" "test_init_direnv_integration"
    run_test "init_workflow" "test_init_workflow"
    run_test "init_paths" "test_init_paths"
    run_test "init_help" "test_init_help"
    run_test "init_performance" "test_init_performance"
    run_test "init_repo_name_formats" "test_init_repo_name_formats"
    run_test "init_security" "test_init_security"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_init_tests
    print_summary
fi