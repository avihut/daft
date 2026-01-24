#!/bin/bash

# Integration tests for git-worktree-flow-eject Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Helper to create a worktree-layout repository
create_worktree_repo() {
    local repo_name="$1"
    local branch="${2:-master}"

    git-worktree-init "$repo_name" >/dev/null 2>&1 || return 1

    # Add initial content
    cd "$repo_name/$branch"
    echo "# Test Repo" > README.md
    mkdir -p src
    echo "fn main() {}" > src/main.rs
    git add . >/dev/null 2>&1
    git commit -m "Initial commit" >/dev/null 2>&1
    cd ../..

    return 0
}

# Test basic eject functionality
test_flow_eject_basic() {
    create_worktree_repo "eject-basic" || return 1

    cd "eject-basic"

    # Eject the repository
    git-worktree-flow-eject || return 1

    cd ..

    # Verify traditional structure
    assert_directory_exists "eject-basic/.git" || return 1
    assert_file_exists "eject-basic/README.md" || return 1
    assert_file_exists "eject-basic/src/main.rs" || return 1

    # Verify the worktree directory is gone
    if [[ -d "eject-basic/master" ]]; then
        log_error "Worktree directory should be removed after eject"
        return 1
    fi

    # Verify .git is not bare
    cd "eject-basic"
    local is_bare=$(git config --get core.bare)
    if [[ "$is_bare" == "true" ]]; then
        log_error "Repository should NOT be bare after eject"
        return 1
    fi

    return 0
}

# Test eject with --branch option
test_flow_eject_with_branch() {
    create_worktree_repo "eject-branch" || return 1

    cd "eject-branch"

    # Create an additional branch/worktree
    git-worktree-checkout-branch feature/test >/dev/null 2>&1 || true
    # Handle case where checkout-branch might fail (no remote)
    if [[ ! -d "feature/test" ]]; then
        # Manually create worktree
        git worktree add feature/test -b feature/test master >/dev/null 2>&1 || return 1
    fi

    # Add content to feature branch
    cd "feature/test"
    echo "feature content" > feature.txt
    git add . >/dev/null 2>&1
    git commit -m "Add feature" >/dev/null 2>&1
    cd ../..  # Back to project root (eject-branch)

    # Eject, keeping the feature branch
    git-worktree-flow-eject -b feature/test || return 1

    cd ..

    # Verify feature content is at root
    assert_file_exists "eject-branch/feature.txt" || return 1

    # Verify on feature branch
    cd "eject-branch"
    local current_branch=$(git branch --show-current)
    if [[ "$current_branch" != "feature/test" ]]; then
        log_error "Should be on feature/test branch, but on '$current_branch'"
        return 1
    fi

    return 0
}

# Test eject fails with dirty worktrees (without --force)
test_flow_eject_dirty_worktrees() {
    create_worktree_repo "eject-dirty" || return 1

    cd "eject-dirty"

    # Create an additional worktree
    git worktree add feature-branch -b feature-branch master >/dev/null 2>&1 || return 1

    # Make uncommitted changes in feature branch
    cd "feature-branch"
    echo "dirty" > dirty.txt
    cd ..

    # Try to eject (should fail)
    if git-worktree-flow-eject 2>/dev/null; then
        log_error "Should fail when non-target worktrees have uncommitted changes"
        return 1
    fi

    log_success "Correctly rejected eject with dirty worktrees"
    return 0
}

# Test eject with --force removes dirty worktrees
test_flow_eject_force() {
    create_worktree_repo "eject-force" || return 1

    cd "eject-force"

    # Create an additional worktree
    git worktree add feature-dirty -b feature-dirty master >/dev/null 2>&1 || return 1

    # Make uncommitted changes in feature branch
    cd "feature-dirty"
    echo "dirty" > dirty.txt
    cd ..

    # Eject with --force
    git-worktree-flow-eject --force || return 1

    cd ..

    # Verify traditional structure
    assert_file_exists "eject-force/README.md" || return 1

    # Verify feature worktree is gone
    if [[ -d "eject-force/feature-dirty" ]]; then
        log_error "Dirty worktree should be removed with --force"
        return 1
    fi

    return 0
}

# Test eject preserves target worktree changes
test_flow_eject_preserves_changes() {
    create_worktree_repo "eject-changes" || return 1

    cd "eject-changes"

    # Make uncommitted changes in target branch
    cd "master"
    echo "uncommitted" >> README.md
    echo "new file" > new-file.txt
    cd ..

    # Eject the repository
    git-worktree-flow-eject || return 1

    cd ..

    # Verify changes were preserved
    cd "eject-changes"
    if ! grep -q "uncommitted" README.md; then
        log_error "Changes to README.md should be preserved"
        return 1
    fi

    if [[ ! -f "new-file.txt" ]]; then
        log_error "New file should be preserved after eject"
        return 1
    fi

    return 0
}

# Test eject with dry-run
test_flow_eject_dry_run() {
    create_worktree_repo "eject-dry" || return 1

    cd "eject-dry"

    # Run eject with dry-run
    git-worktree-flow-eject --dry-run || return 1

    # Verify nothing changed
    local is_bare=$(git config --get core.bare)
    if [[ "$is_bare" != "true" ]]; then
        log_error "Repository should still be bare after dry-run"
        return 1
    fi

    # Verify worktree still exists
    assert_directory_exists "master" || return 1
    assert_file_exists "master/README.md" || return 1

    return 0
}

# Test eject fails on non-worktree layout
test_flow_eject_traditional_repo() {
    # Create a traditional git repository
    mkdir -p "traditional-repo"
    cd "traditional-repo"
    git init >/dev/null 2>&1
    echo "# Test" > README.md
    git add . >/dev/null 2>&1
    git commit -m "Initial" >/dev/null 2>&1

    # Try to eject (should fail)
    if git-worktree-flow-eject 2>/dev/null; then
        log_error "Should fail when not in worktree layout"
        return 1
    fi

    log_success "Correctly rejected eject on traditional repo"
    return 0
}

# Test eject help
test_flow_eject_help() {
    assert_command_help "git-worktree-flow-eject" || return 1
    return 0
}

# Test eject when worktree contains directory matching branch name
# Branch "test" with a "test/" directory inside - tests staging directory approach
test_flow_eject_branch_matches_directory() {
    create_worktree_repo "eject-conflict" || return 1

    cd "eject-conflict"

    # Create a new branch worktree named "test"
    git worktree add test -b test master >/dev/null 2>&1 || return 1

    # Add a directory named "test" inside the "test" worktree
    cd "test"
    mkdir -p test
    echo "nested content" > test/nested.txt
    echo "root content" > root.txt
    git add . >/dev/null 2>&1
    git commit -m "Add test directory" >/dev/null 2>&1
    cd ..

    # Remove master worktree so only "test" remains
    git worktree remove master --force >/dev/null 2>&1 || return 1

    # Eject - this used to fail because we'd try to move test/test to test
    # while test (the worktree) still exists
    git-worktree-flow-eject || return 1

    cd ..

    # Verify traditional structure
    assert_file_exists "eject-conflict/root.txt" || return 1
    assert_file_exists "eject-conflict/test/nested.txt" || return 1

    # Verify worktree directory is gone
    if [[ -d "eject-conflict/test" ]] && [[ -f "eject-conflict/test/root.txt" ]]; then
        # If test/ exists and has root.txt, the worktree wasn't properly removed
        log_error "Worktree directory structure should be flattened"
        return 1
    fi

    # Verify on test branch
    cd "eject-conflict"
    local current_branch=$(git branch --show-current)
    if [[ "$current_branch" != "test" ]]; then
        log_error "Should be on test branch, but on '$current_branch'"
        return 1
    fi

    return 0
}

# Test eject with only a non-default branch (no master/main worktree)
# This tests the fallback to first available worktree
test_flow_eject_only_non_default_branch() {
    # Create a worktree repo, then remove the master worktree and add a test branch
    create_worktree_repo "eject-nondefault" || return 1

    cd "eject-nondefault"

    # Create a new branch worktree
    git worktree add test-branch -b test-branch master >/dev/null 2>&1 || return 1

    # Add content to test branch
    cd "test-branch"
    echo "test content" > test.txt
    git add . >/dev/null 2>&1
    git commit -m "Add test content" >/dev/null 2>&1
    cd ..

    # Remove the master worktree so only test-branch remains
    git worktree remove master --force >/dev/null 2>&1 || return 1

    # Verify only test-branch worktree exists
    if [[ -d "master" ]]; then
        log_error "Master worktree should be removed"
        return 1
    fi

    # Eject without specifying branch - should automatically use test-branch
    git-worktree-flow-eject || return 1

    cd ..

    # Verify traditional structure with test-branch content
    assert_file_exists "eject-nondefault/test.txt" || return 1
    assert_file_exists "eject-nondefault/README.md" || return 1

    # Verify on test-branch
    cd "eject-nondefault"
    local current_branch=$(git branch --show-current)
    if [[ "$current_branch" != "test-branch" ]]; then
        log_error "Should be on test-branch, but on '$current_branch'"
        return 1
    fi

    # Verify .git is not bare
    local is_bare=$(git config --get core.bare)
    if [[ "$is_bare" == "true" ]]; then
        log_error "Repository should NOT be bare after eject"
        return 1
    fi

    return 0
}

# Run all flow_eject tests
run_flow_eject_tests() {
    log "Running git-worktree-flow-eject integration tests..."

    run_test "flow_eject_basic" "test_flow_eject_basic"
    run_test "flow_eject_with_branch" "test_flow_eject_with_branch"
    run_test "flow_eject_dirty_worktrees" "test_flow_eject_dirty_worktrees"
    run_test "flow_eject_force" "test_flow_eject_force"
    run_test "flow_eject_preserves_changes" "test_flow_eject_preserves_changes"
    run_test "flow_eject_dry_run" "test_flow_eject_dry_run"
    run_test "flow_eject_traditional_repo" "test_flow_eject_traditional_repo"
    run_test "flow_eject_help" "test_flow_eject_help"
    run_test "flow_eject_branch_matches_directory" "test_flow_eject_branch_matches_directory"
    run_test "flow_eject_only_non_default_branch" "test_flow_eject_only_non_default_branch"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_flow_eject_tests
    print_summary
fi
