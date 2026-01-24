#!/bin/bash

# Integration tests for git-worktree-flow-adopt Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic adopt functionality
test_flow_adopt_basic() {
    # Create a traditional git repository
    mkdir -p "test-repo"
    cd "test-repo"
    git init >/dev/null 2>&1
    echo "# Test Repo" > README.md
    mkdir -p src
    echo "fn main() {}" > src/main.rs
    git add . >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1

    # Adopt the repository
    git-worktree-flow-adopt || return 1

    cd ..

    # Verify structure was created
    assert_directory_exists "test-repo/.git" || return 1
    assert_directory_exists "test-repo/master" || return 1
    assert_file_exists "test-repo/master/README.md" || return 1
    assert_file_exists "test-repo/master/src/main.rs" || return 1

    # Verify .git is now bare
    cd "test-repo"
    local is_bare=$(git config --get core.bare)
    if [[ "$is_bare" != "true" ]]; then
        log_error "Repository should be bare after adopt"
        return 1
    fi

    # Verify worktree is registered
    local worktree_count=$(git worktree list | wc -l)
    if [[ $worktree_count -lt 1 ]]; then
        log_error "Should have at least one worktree registered"
        return 1
    fi

    # Verify bare repo doesn't have an index file (bare repos shouldn't have one)
    if [[ -f ".git/index" ]]; then
        log_error "Bare repo should not have an index file"
        return 1
    fi

    # Verify git status is clean in the worktree (index is properly initialized)
    cd "master"
    local status_output=$(git status --porcelain 2>&1)
    if [[ -n "$status_output" ]]; then
        log_error "Git status should be clean after adopt, but got:"
        echo "$status_output"
        return 1
    fi

    return 0
}

# Test adopt preserves uncommitted changes
test_flow_adopt_preserves_changes() {
    # Create a traditional git repository
    mkdir -p "changes-repo"
    cd "changes-repo"
    git init >/dev/null 2>&1
    echo "# Test Repo" > README.md
    git add . >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1

    # Make uncommitted changes
    echo "# Modified" >> README.md
    echo "new file" > new-file.txt

    # Adopt the repository
    git-worktree-flow-adopt || return 1

    cd ..

    # Verify structure was created
    assert_directory_exists "changes-repo/master" || return 1

    # Verify changes were preserved
    cd "changes-repo/master"
    if ! git status --porcelain | grep -q "README.md"; then
        # Changes might have been stashed and popped, check file content
        if ! grep -q "Modified" README.md; then
            log_error "Changes to README.md should be preserved"
            return 1
        fi
    fi

    if [[ ! -f "new-file.txt" ]]; then
        log_error "New file should be preserved after adopt"
        return 1
    fi

    return 0
}

# Test adopt fails on already adopted repository
test_flow_adopt_already_adopted() {
    # Create and adopt a repository first
    mkdir -p "already-adopted"
    cd "already-adopted"
    git init >/dev/null 2>&1
    echo "# Test" > README.md
    git add . >/dev/null 2>&1
    git commit -m "Initial" >/dev/null 2>&1
    git-worktree-flow-adopt || return 1

    # Try to adopt again (should fail)
    cd master
    if git-worktree-flow-adopt 2>/dev/null; then
        log_error "Should fail when already in worktree layout"
        return 1
    fi

    log_success "Correctly rejected adopt on already-adopted repo"
    return 0
}

# Test adopt with dry-run
test_flow_adopt_dry_run() {
    # Create a traditional git repository
    mkdir -p "dry-run-repo"
    cd "dry-run-repo"
    git init >/dev/null 2>&1
    echo "# Test Repo" > README.md
    git add . >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1

    # Run adopt with dry-run
    git-worktree-flow-adopt --dry-run || return 1

    # Verify nothing changed
    local is_bare=$(git config --get core.bare)
    if [[ "$is_bare" == "true" ]]; then
        log_error "Repository should NOT be bare after dry-run"
        return 1
    fi

    # Verify no worktree directory was created
    if [[ -d "master" ]]; then
        log_error "Worktree directory should NOT be created after dry-run"
        return 1
    fi

    # README should still be at root
    assert_file_exists "README.md" || return 1

    return 0
}

# Test adopt with custom branch
test_flow_adopt_custom_branch() {
    # Create a traditional git repository with a non-default branch
    mkdir -p "custom-branch-repo"
    cd "custom-branch-repo"
    git init -b main >/dev/null 2>&1
    echo "# Test Repo" > README.md
    git add . >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create and checkout a feature branch
    git checkout -b feature/test >/dev/null 2>&1
    echo "feature" > feature.txt
    git add . >/dev/null 2>&1
    git commit -m "Add feature" >/dev/null 2>&1

    # Adopt the repository (should adopt current branch: feature/test)
    git-worktree-flow-adopt || return 1

    cd ..

    # Verify the worktree is for the feature branch
    assert_directory_exists "custom-branch-repo/feature/test" || return 1
    assert_file_exists "custom-branch-repo/feature/test/feature.txt" || return 1

    return 0
}

# Test adopt help
test_flow_adopt_help() {
    assert_command_help "git-worktree-flow-adopt" || return 1
    return 0
}

# Test adopt fails on non-git directory
test_flow_adopt_not_git_repo() {
    mkdir -p "not-a-repo"
    cd "not-a-repo"

    if git-worktree-flow-adopt 2>/dev/null; then
        log_error "Should fail when not in a git repository"
        return 1
    fi

    log_success "Correctly rejected adopt on non-git directory"
    return 0
}

# Run all flow_adopt tests
run_flow_adopt_tests() {
    log "Running git-worktree-flow-adopt integration tests..."

    run_test "flow_adopt_basic" "test_flow_adopt_basic"
    run_test "flow_adopt_preserves_changes" "test_flow_adopt_preserves_changes"
    run_test "flow_adopt_already_adopted" "test_flow_adopt_already_adopted"
    run_test "flow_adopt_dry_run" "test_flow_adopt_dry_run"
    run_test "flow_adopt_custom_branch" "test_flow_adopt_custom_branch"
    run_test "flow_adopt_help" "test_flow_adopt_help"
    run_test "flow_adopt_not_git_repo" "test_flow_adopt_not_git_repo"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_flow_adopt_tests
    print_summary
fi
