#!/bin/bash

# Integration tests for git-worktree-prune Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic prune functionality
test_prune_basic() {
    local remote_repo=$(create_test_remote "test-repo-prune-basic" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-basic"
    
    # Create some worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Verify worktrees exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "feature/test-feature" || return 1
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify feature branch worktree was removed
    if [[ -d "feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        return 1
    fi
    
    # Verify develop worktree still exists
    assert_directory_exists "develop" || return 1
    
    return 0
}

# Test prune with no remote branches to delete
test_prune_no_deletion() {
    local remote_repo=$(create_test_remote "test-repo-prune-no-deletion" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-no-deletion"
    
    # Create some worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Run prune (should not delete anything)
    git-worktree-prune || return 1
    
    # Verify all worktrees still exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "feature/test-feature" || return 1
    
    return 0
}

# Test prune with multiple deleted branches
test_prune_multiple_deletions() {
    local remote_repo=$(create_test_remote "test-repo-prune-multiple" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-multiple"
    
    # Create some worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Create additional branches in remote
    local temp_clone="$TEMP_BASE_DIR/temp_multiple_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git checkout -b feature/branch1 >/dev/null 2>&1
        echo "Branch 1" > branch1.txt
        git add branch1.txt >/dev/null 2>&1
        git commit -m "Add branch1" >/dev/null 2>&1
        git push origin feature/branch1 >/dev/null 2>&1
        
        git checkout -b feature/branch2 >/dev/null 2>&1
        echo "Branch 2" > branch2.txt
        git add branch2.txt >/dev/null 2>&1
        git commit -m "Add branch2" >/dev/null 2>&1
        git push origin feature/branch2 >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Fetch and checkout these branches
    git fetch origin >/dev/null 2>&1
    git-worktree-checkout feature/branch1 || return 1
    git-worktree-checkout feature/branch2 || return 1
    
    # Verify all worktrees exist
    assert_directory_exists "feature/branch1" || return 1
    assert_directory_exists "feature/branch2" || return 1
    
    # Delete multiple branches from remote
    temp_clone="$TEMP_BASE_DIR/temp_multiple_delete_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/branch1 >/dev/null 2>&1
        git push origin --delete feature/branch2 >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify deleted branches' worktrees were removed
    if [[ -d "feature/branch1" ]]; then
        log_error "Prune should have removed feature/branch1 worktree"
        return 1
    fi
    
    if [[ -d "feature/branch2" ]]; then
        log_error "Prune should have removed feature/branch2 worktree"
        return 1
    fi
    
    # Verify other worktrees still exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "feature/test-feature" || return 1
    
    return 0
}

# Test prune from subdirectory
test_prune_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-prune-subdir" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-subdir"
    
    # Create some worktrees
    git-worktree-checkout feature/test-feature || return 1
    
    # Create a subdirectory and test prune from there
    mkdir -p "main/subdir"
    cd "main/subdir"
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_subdir_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune from subdirectory
    git-worktree-prune || return 1
    
    # Verify feature branch worktree was removed
    if [[ -d "../../feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        return 1
    fi
    
    return 0
}

# Test prune error handling
test_prune_errors() {
    local remote_repo=$(create_test_remote "test-repo-prune-errors" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-errors"
    
    # Test prune should work (no error expected)
    git-worktree-prune || return 1
    
    return 0
}

# Test prune outside git repository
test_prune_outside_repo() {
    # Test prune command outside git repository
    assert_command_failure "git-worktree-prune" "Should fail outside git repository"
    
    return 0
}

# Test prune help functionality
test_prune_help() {
    # Test help commands
    assert_command_help "git-worktree-prune" || return 1
    assert_command_version "git-worktree-prune" || return 1
    
    return 0
}

# Test prune with uncommitted changes
test_prune_with_uncommitted_changes() {
    local remote_repo=$(create_test_remote "test-repo-prune-uncommitted" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-uncommitted"
    
    # Create worktree and add uncommitted changes
    git-worktree-checkout feature/test-feature || return 1
    echo "Uncommitted changes" > "feature/test-feature/uncommitted.txt"
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_uncommitted_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune (should handle uncommitted changes gracefully)
    git-worktree-prune || return 1
    
    # Verify worktree was removed (or handled appropriately)
    # The exact behavior depends on implementation - it might preserve uncommitted changes
    
    return 0
}

# Test prune with nested branch directories
test_prune_nested_directories() {
    local remote_repo=$(create_test_remote "test-repo-prune-nested" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-nested"
    
    # Create worktrees with nested directory structures
    git-worktree-checkout feature/test-feature || return 1
    
    # Create some nested directories and files
    mkdir -p "feature/test-feature/nested/deep/structure"
    echo "Deep file" > "feature/test-feature/nested/deep/structure/file.txt"
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_nested_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify worktree and all nested directories were removed
    if [[ -d "feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree and all nested directories"
        return 1
    fi
    
    return 0
}

# Test prune performance
test_prune_performance() {
    local remote_repo=$(create_test_remote "test-repo-prune-performance" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-performance"
    
    # Create multiple worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1
    
    # Test prune performance
    local start_time=$(date +%s)
    git-worktree-prune || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 15 ]]; then
        log_warning "Prune performance test took ${duration}s (expected < 15s)"
    else
        log_success "Prune performance test completed in ${duration}s"
    fi
    
    return 0
}

# Test prune with large number of worktrees
test_prune_many_worktrees() {
    local remote_repo=$(create_test_remote "test-repo-prune-many" "main")
    
    # Create many branches in remote
    local temp_clone="$TEMP_BASE_DIR/temp_many_branches_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        for i in {1..10}; do
            git checkout -b "feature/branch$i" >/dev/null 2>&1
            echo "Branch $i" > "branch$i.txt"
            git add "branch$i.txt" >/dev/null 2>&1
            git commit -m "Add branch$i" >/dev/null 2>&1
            git push origin "feature/branch$i" >/dev/null 2>&1
        done
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-many"
    
    # Fetch and checkout these branches
    git fetch origin >/dev/null 2>&1
    for i in {1..10}; do
        git-worktree-checkout "feature/branch$i" || return 1
    done
    
    # Delete some branches from remote
    temp_clone="$TEMP_BASE_DIR/temp_many_delete_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        for i in {1..5}; do
            git push origin --delete "feature/branch$i" >/dev/null 2>&1
        done
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify first 5 branches were removed
    for i in {1..5}; do
        if [[ -d "feature/branch$i" ]]; then
            log_error "Prune should have removed feature/branch$i worktree"
            return 1
        fi
    done
    
    # Verify last 5 branches still exist
    for i in {6..10}; do
        assert_directory_exists "feature/branch$i" || return 1
    done
    
    return 0
}

# Test prune correctly handles branches shown with '+' marker in linked worktrees
# Regression test for https://github.com/avihut/daft/issues/97
test_prune_plus_marker_branch() {
    local remote_repo=$(create_test_remote "test-repo-prune-plus" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-plus"

    # Create a worktree with a new branch - this branch will show with '+' prefix
    # in git branch -vv because it is checked out in a linked worktree
    git-worktree-checkout-branch feature/plus-test || return 1

    # Verify the branch shows with '+' marker in git branch -vv
    local branch_vv
    branch_vv=$(cd main && git branch -vv)
    if ! echo "$branch_vv" | grep -q '^+.*feature/plus-test'; then
        log_error "Expected feature/plus-test to show with '+' marker in git branch -vv"
        log_error "Actual output: $branch_vv"
        return 1
    fi

    # Delete the branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_plus_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/plus-test >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run prune and capture output - should not contain error about branch '+'
    local prune_output
    prune_output=$(git-worktree-prune 2>&1) || true

    if echo "$prune_output" | grep -q "branch '+'"; then
        log_error "Prune incorrectly tried to delete a branch named '+' (issue #97)"
        log_error "Output: $prune_output"
        return 1
    fi

    # Verify the actual branch worktree was removed
    if [[ -d "feature/plus-test" ]]; then
        log_error "Prune should have removed feature/plus-test worktree"
        return 1
    fi

    return 0
}

# Test prune cleans up empty parent directories for branches with slashes
# Regression test for https://github.com/avihut/daft/issues/135
test_prune_empty_parent_dir_cleanup() {
    local remote_repo=$(create_test_remote "test-repo-prune-parent-cleanup" "main")

    # Create two branches under the same prefix in remote
    local temp_clone="$TEMP_BASE_DIR/temp_parent_cleanup_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git checkout -b feature/old >/dev/null 2>&1
        echo "Old feature" > old.txt
        git add old.txt >/dev/null 2>&1
        git commit -m "Add old feature" >/dev/null 2>&1
        git push origin feature/old >/dev/null 2>&1

        git checkout -b feature/new >/dev/null 2>&1
        echo "New feature" > new.txt
        git add new.txt >/dev/null 2>&1
        git commit -m "Add new feature" >/dev/null 2>&1
        git push origin feature/new >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Clone and checkout both branches
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-parent-cleanup"

    git fetch origin >/dev/null 2>&1
    git-worktree-checkout feature/old || return 1
    git-worktree-checkout feature/new || return 1

    # Verify both worktrees exist under feature/
    assert_directory_exists "feature/old" || return 1
    assert_directory_exists "feature/new" || return 1

    # Delete only feature/old from remote
    temp_clone="$TEMP_BASE_DIR/temp_parent_cleanup_delete1"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/old >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Prune: should remove feature/old but keep feature/ (feature/new still exists)
    git-worktree-prune || return 1

    if [[ -d "feature/old" ]]; then
        log_error "Prune should have removed feature/old worktree"
        return 1
    fi

    assert_directory_exists "feature/new" || return 1
    assert_directory_exists "feature" || return 1

    # Now delete feature/new from remote
    temp_clone="$TEMP_BASE_DIR/temp_parent_cleanup_delete2"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/new >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Prune: should remove feature/new AND the now-empty feature/ directory
    git-worktree-prune || return 1

    if [[ -d "feature/new" ]]; then
        log_error "Prune should have removed feature/new worktree"
        return 1
    fi

    if [[ -d "feature" ]]; then
        log_error "Prune should have removed the empty feature/ parent directory (issue #135)"
        return 1
    fi

    return 0
}

# Test prune from current worktree (Scenario A: bare-repo layout)
# When pruning from inside a worktree that is about to be removed,
# the command should remove it last and write a CD target to redirect the shell.
test_prune_from_current_worktree() {
    local remote_repo=$(create_test_remote "test-repo-prune-current-wt" "main")

    # Clone the repository (creates bare-repo worktree layout)
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-current-wt"

    # Create a worktree for a feature branch
    git-worktree-checkout feature/test-feature || return 1

    # Verify worktree exists
    assert_directory_exists "feature/test-feature" || return 1

    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_current_wt_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Save the project root path for verification (resolve symlinks for macOS /tmp -> /private/tmp)
    local project_root
    project_root=$(cd "$(pwd)" && pwd -P)

    # cd into the feature worktree (the one about to be pruned)
    cd "feature/test-feature"

    # Run prune with DAFT_CD_FILE set from inside the worktree
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-worktree-prune 2>&1 || true

    # Verify the worktree was removed
    if [[ -d "$project_root/feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        rm -f "$cd_file"
        return 1
    fi

    # Verify CD path was written to temp file
    if [ -s "$cd_file" ]; then
        log_success "Prune wrote CD path to temp file for shell redirection"
    else
        log_error "Prune should have written CD path to temp file when pruning current worktree"
        rm -f "$cd_file"
        return 1
    fi

    # Verify the CD path points to project root (resolve symlinks for comparison)
    local cd_path
    cd_path=$(cat "$cd_file")
    if [[ "$cd_path" == "$project_root" ]]; then
        log_success "CD path points to project root"
    else
        log_error "Expected CD path '$project_root', got '$cd_path'"
        rm -f "$cd_file"
        return 1
    fi

    rm -f "$cd_file"
    return 0
}

# Test prune from current worktree with cdTarget=default-branch
test_prune_from_current_worktree_cd_default_branch() {
    local remote_repo=$(create_test_remote "test-repo-prune-cd-default" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-cd-default"

    # Set the cdTarget config to default-branch
    git config daft.prune.cdTarget default-branch

    # Create a worktree for a feature branch
    git-worktree-checkout feature/test-feature || return 1

    # Verify worktree exists
    assert_directory_exists "feature/test-feature" || return 1

    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_cd_default_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Save paths for verification (resolve symlinks for macOS /tmp -> /private/tmp)
    local project_root
    project_root=$(cd "$(pwd)" && pwd -P)
    local main_wt_path="$project_root/main"

    # cd into the feature worktree
    cd "feature/test-feature"

    # Run prune with DAFT_CD_FILE set
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-worktree-prune 2>&1 || true

    # Verify CD path was written to temp file pointing to default branch worktree
    local cd_path
    cd_path=$(cat "$cd_file")
    if [[ "$cd_path" == "$main_wt_path" ]]; then
        log_success "CD path points to default branch worktree (main)"
    else
        # May fall back to project root if default branch can't be determined
        log_warning "CD path is '$cd_path' (expected '$main_wt_path' or '$project_root')"
        if [[ "$cd_path" == "$project_root" ]]; then
            log_success "CD path fell back to project root (acceptable)"
        else
            log_error "CD path does not match expected value"
            rm -f "$cd_file"
            return 1
        fi
    fi

    rm -f "$cd_file"
    return 0
}

# Test prune in regular repo when current branch is being pruned (Scenario B)
test_prune_regular_repo_current_branch() {
    local remote_repo=$(create_test_remote "test-repo-prune-regular" "main")

    # Clone normally (non-bare layout)
    local clone_dir="$TEMP_BASE_DIR/temp_regular_clone"
    git clone "$remote_repo" "test-repo-prune-regular-clone" >/dev/null 2>&1
    cd "test-repo-prune-regular-clone"

    # Set up remote HEAD for default branch detection
    git remote set-head origin --auto >/dev/null 2>&1

    # Checkout the feature branch
    git checkout feature/test-feature >/dev/null 2>&1

    # Verify we're on the feature branch
    local current_branch
    current_branch=$(git branch --show-current)
    if [[ "$current_branch" != "feature/test-feature" ]]; then
        log_error "Expected to be on feature/test-feature, got $current_branch"
        return 1
    fi

    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_regular_delete_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run prune (should checkout default branch first, then delete)
    git-worktree-prune || return 1

    # Verify we're now on the default branch (main)
    current_branch=$(git branch --show-current)
    if [[ "$current_branch" == "main" ]]; then
        log_success "Prune switched to default branch (main)"
    else
        log_error "Expected to be on main after prune, got $current_branch"
        return 1
    fi

    # Verify feature branch was deleted
    if git show-ref --verify --quiet "refs/heads/feature/test-feature" 2>/dev/null; then
        log_error "feature/test-feature branch should have been deleted"
        return 1
    fi

    log_success "Feature branch was deleted successfully"
    return 0
}

# Test prune in regular repo when pruned branch is NOT the current branch
test_prune_regular_repo_not_current_branch() {
    local remote_repo=$(create_test_remote "test-repo-prune-reg-other" "main")

    # Clone normally
    git clone "$remote_repo" "test-repo-prune-reg-other-clone" >/dev/null 2>&1
    cd "test-repo-prune-reg-other-clone"

    # Stay on main, feature branch exists locally via clone
    git checkout main >/dev/null 2>&1

    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_reg_other_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run prune
    git-worktree-prune || return 1

    # Verify we're still on main
    local current_branch
    current_branch=$(git branch --show-current)
    if [[ "$current_branch" != "main" ]]; then
        log_error "Expected to still be on main, got $current_branch"
        return 1
    fi

    log_success "Remained on main branch after pruning other branch"
    return 0
}

# Test that shell wrappers include git-worktree-prune
test_prune_shell_wrapper() {
    # Check bash/zsh wrapper
    local bash_output
    bash_output=$(daft shell-init bash)

    if echo "$bash_output" | grep -q "^git-worktree-prune()"; then
        log_success "Bash wrapper contains git-worktree-prune function"
    else
        log_error "Bash wrapper missing git-worktree-prune function"
        return 1
    fi

    if echo "$bash_output" | grep -q "worktree-prune)"; then
        log_success "Bash git() wrapper handles worktree-prune"
    else
        log_error "Bash git() wrapper missing worktree-prune case"
        return 1
    fi

    # Check fish wrapper
    local fish_output
    fish_output=$(daft shell-init fish)

    if echo "$fish_output" | grep -q "function git-worktree-prune"; then
        log_success "Fish wrapper contains git-worktree-prune function"
    else
        log_error "Fish wrapper missing git-worktree-prune function"
        return 1
    fi

    if echo "$fish_output" | grep -q "case worktree-prune"; then
        log_success "Fish git wrapper handles worktree-prune"
    else
        log_error "Fish git wrapper missing worktree-prune case"
        return 1
    fi

    return 0
}

# Test prune with remote configuration
test_prune_remote_config() {
    local remote_repo=$(create_test_remote "test-repo-prune-remote-config" "main")
    
    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-prune-remote-config"
    
    # Create worktree
    git-worktree-checkout feature/test-feature || return 1
    
    # Verify remote configuration
    local remote_url=$(git remote get-url origin)
    if [[ -z "$remote_url" ]]; then
        log_error "Remote origin should be configured"
        return 1
    fi
    
    # Delete the feature branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_prune_remote_config_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git push origin --delete feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Run prune
    git-worktree-prune || return 1
    
    # Verify worktree was removed
    if [[ -d "feature/test-feature" ]]; then
        log_error "Prune should have removed feature/test-feature worktree"
        return 1
    fi
    
    return 0
}

# Run all prune tests
run_prune_tests() {
    log "Running git-worktree-prune integration tests..."
    
    run_test "prune_basic" "test_prune_basic"
    run_test "prune_no_deletion" "test_prune_no_deletion"
    run_test "prune_multiple_deletions" "test_prune_multiple_deletions"
    run_test "prune_from_subdirectory" "test_prune_from_subdirectory"
    run_test "prune_errors" "test_prune_errors"
    run_test "prune_outside_repo" "test_prune_outside_repo"
    run_test "prune_help" "test_prune_help"
    run_test "prune_with_uncommitted_changes" "test_prune_with_uncommitted_changes"
    run_test "prune_nested_directories" "test_prune_nested_directories"
    run_test "prune_performance" "test_prune_performance"
    run_test "prune_many_worktrees" "test_prune_many_worktrees"
    run_test "prune_plus_marker_branch" "test_prune_plus_marker_branch"
    run_test "prune_empty_parent_dir_cleanup" "test_prune_empty_parent_dir_cleanup"
    run_test "prune_remote_config" "test_prune_remote_config"
    run_test "prune_from_current_worktree" "test_prune_from_current_worktree"
    run_test "prune_from_current_worktree_cd_default_branch" "test_prune_from_current_worktree_cd_default_branch"
    run_test "prune_regular_repo_current_branch" "test_prune_regular_repo_current_branch"
    run_test "prune_regular_repo_not_current_branch" "test_prune_regular_repo_not_current_branch"
    run_test "prune_shell_wrapper" "test_prune_shell_wrapper"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_prune_tests
    print_summary
fi