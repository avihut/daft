#!/bin/bash

# Integration tests for git-worktree-branch -m

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic rename (branch + worktree directory moves)
test_rename_basic() {
    local remote_repo=$(create_test_remote "test-repo-rn-basic" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-basic"
    local project_root=$(pwd)

    # Create a branch with worktree
    git-worktree-checkout -b feature/old-name || return 1
    assert_directory_exists "feature/old-name" || return 1

    # Make a commit on the feature branch
    cd "feature/old-name"
    echo "feature work" > feature.txt
    git add feature.txt
    git commit -m "Add feature" >/dev/null 2>&1
    cd "$project_root"

    # Rename the branch
    git-worktree-branch -m feature/old-name feature/new-name || return 1

    # Verify old worktree is gone
    if [[ -d "feature/old-name" ]]; then
        log_error "Old worktree should have been removed"
        return 1
    fi

    # Verify new worktree exists
    assert_directory_exists "feature/new-name" || return 1

    # Verify the branch was renamed
    cd "main"
    if git branch | grep -q " feature/old-name$"; then
        log_error "Old branch should not exist"
        return 1
    fi
    if ! git branch | grep -q " feature/new-name$"; then
        log_error "New branch should exist"
        return 1
    fi
    cd "$project_root"

    # Verify the new worktree has the expected content
    assert_file_exists "feature/new-name/feature.txt" || return 1

    return 0
}

# Test rename from inside the worktree (cd target)
test_rename_from_inside_worktree() {
    local remote_repo=$(create_test_remote "test-repo-rn-inside" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-inside"
    local project_root
    project_root=$(cd "$(pwd)" && pwd -P)

    # Create a branch with worktree
    git-worktree-checkout -b feature/inside-test || return 1
    assert_directory_exists "feature/inside-test" || return 1

    # cd into the feature worktree
    cd "feature/inside-test"

    # Run rename with DAFT_CD_FILE set
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-worktree-branch -m feature/inside-test feature/renamed 2>&1 || {
        log_error "Rename failed"
        rm -f "$cd_file"
        return 1
    }

    # Verify CD path was written to temp file
    if [ -s "$cd_file" ]; then
        local cd_path
        cd_path=$(cat "$cd_file")
        # The cd path should point to the new worktree location
        if [[ "$cd_path" == *"feature/renamed"* ]]; then
            log_success "CD path points to renamed worktree"
        else
            log_error "CD path '$cd_path' does not contain 'feature/renamed'"
            rm -f "$cd_file"
            return 1
        fi
    else
        log_error "Rename should have written CD path when run from inside worktree"
        rm -f "$cd_file"
        return 1
    fi

    rm -f "$cd_file"
    return 0
}

# Test --no-remote flag
test_rename_no_remote() {
    local remote_repo=$(create_test_remote "test-repo-rn-noremote" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-noremote"
    local project_root=$(pwd)

    git-worktree-checkout -b feature/no-remote || return 1

    cd "feature/no-remote"
    echo "work" > work.txt
    git add work.txt
    git commit -m "Add work" >/dev/null 2>&1
    git push origin feature/no-remote >/dev/null 2>&1
    cd "$project_root"

    # Rename with --no-remote
    git-worktree-branch -m --no-remote feature/no-remote feature/no-remote-renamed || return 1

    # Verify local rename happened
    assert_directory_exists "feature/no-remote-renamed" || return 1
    if [[ -d "feature/no-remote" ]]; then
        log_error "Old worktree should have been removed"
        return 1
    fi

    # Verify old remote branch still exists (since --no-remote was used)
    cd "main"
    if ! git branch -r | grep -q "origin/feature/no-remote$"; then
        log_error "Old remote branch should still exist with --no-remote"
        return 1
    fi
    cd "$project_root"

    return 0
}

# Test --dry-run flag
test_rename_dry_run() {
    local remote_repo=$(create_test_remote "test-repo-rn-dryrun" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-dryrun"
    local project_root=$(pwd)

    git-worktree-checkout -b feature/dry-run-test || return 1
    assert_directory_exists "feature/dry-run-test" || return 1

    # Dry run rename
    git-worktree-branch -m --dry-run feature/dry-run-test feature/dry-run-renamed || return 1

    # Verify nothing actually changed
    assert_directory_exists "feature/dry-run-test" || return 1
    if [[ -d "feature/dry-run-renamed" ]]; then
        log_error "Dry run should not have created new worktree"
        return 1
    fi

    # Verify branch was not renamed
    cd "main"
    if ! git branch | grep -q " feature/dry-run-test$"; then
        log_error "Dry run should not have renamed the branch"
        return 1
    fi
    if git branch | grep -q " feature/dry-run-renamed$"; then
        log_error "Dry run should not have created new branch"
        return 1
    fi
    cd "$project_root"

    return 0
}

# Test error: source branch doesn't exist
test_rename_source_not_found() {
    local remote_repo=$(create_test_remote "test-repo-rn-nosource" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-nosource"

    # Try to rename nonexistent branch
    if git-worktree-branch -m nonexistent-branch new-name 2>/dev/null; then
        log_error "Should have failed for nonexistent branch"
        return 1
    fi

    return 0
}

# Test error: destination branch already exists
test_rename_dest_branch_exists() {
    local remote_repo=$(create_test_remote "test-repo-rn-destexists" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-destexists"
    local project_root=$(pwd)

    # Create two branches
    git-worktree-checkout -b feature/source || return 1
    git-worktree-checkout -b feature/dest || return 1

    # Try to rename source to dest (should fail because dest branch exists)
    if git-worktree-branch -m feature/source feature/dest 2>/dev/null; then
        log_error "Should have failed when destination branch already exists"
        return 1
    fi

    # Verify nothing changed
    assert_directory_exists "feature/source" || return 1
    assert_directory_exists "feature/dest" || return 1

    return 0
}

# Test error: destination path already exists on disk
test_rename_dest_path_exists() {
    local remote_repo=$(create_test_remote "test-repo-rn-pathexists" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-pathexists"
    local project_root=$(pwd)

    git-worktree-checkout -b feature/path-test || return 1

    # Create a directory at the destination path
    mkdir -p "feature/path-blocker"

    # Try to rename (should fail because path exists on disk)
    if git-worktree-branch -m feature/path-test feature/path-blocker 2>/dev/null; then
        log_error "Should have failed when destination path exists on disk"
        return 1
    fi

    # Verify source is unchanged
    assert_directory_exists "feature/path-test" || return 1

    # Clean up
    rm -rf "feature/path-blocker"

    return 0
}

# Test empty parent directory cleanup
test_rename_cleanup_empty_parent() {
    local remote_repo=$(create_test_remote "test-repo-rn-cleanup" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-cleanup"
    local project_root=$(pwd)

    # Create a branch with a nested path
    git-worktree-checkout -b feature/deep/nested || return 1
    assert_directory_exists "feature/deep/nested" || return 1

    # Rename to a different top-level path
    git-worktree-branch -m feature/deep/nested bugfix/moved || return 1

    # Verify the new path exists
    assert_directory_exists "bugfix/moved" || return 1

    # Verify the old nested empty parent directories were cleaned up
    if [[ -d "feature/deep" ]]; then
        log_error "Empty parent directory 'feature/deep' should have been cleaned up"
        return 1
    fi

    # The 'feature' directory might still exist if there are other worktrees in it
    # In this case there are none, so it should also be cleaned up
    if [[ -d "feature" ]]; then
        log_error "Empty parent directory 'feature' should have been cleaned up"
        return 1
    fi

    return 0
}

# Test rename with remote branch
test_rename_with_remote() {
    local remote_repo=$(create_test_remote "test-repo-rn-remote" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-remote"
    local project_root=$(pwd)

    git-worktree-checkout -b feature/remote-test || return 1

    cd "feature/remote-test"
    echo "remote work" > remote.txt
    git add remote.txt
    git commit -m "Add remote work" >/dev/null 2>&1
    git push origin feature/remote-test >/dev/null 2>&1
    cd "$project_root"

    # Rename (should also rename remote branch)
    git-worktree-branch -m feature/remote-test feature/remote-renamed || return 1

    # Verify local rename
    assert_directory_exists "feature/remote-renamed" || return 1
    if [[ -d "feature/remote-test" ]]; then
        log_error "Old worktree should have been removed"
        return 1
    fi

    # Verify remote branch was renamed (new exists, old doesn't)
    cd "main"
    git fetch origin >/dev/null 2>&1
    if git branch -r | grep -q "origin/feature/remote-test$"; then
        log_error "Old remote branch should have been deleted"
        return 1
    fi
    if ! git branch -r | grep -q "origin/feature/remote-renamed$"; then
        log_error "New remote branch should exist"
        return 1
    fi
    cd "$project_root"

    return 0
}

# Test rename by worktree path (relative)
test_rename_by_relative_path() {
    local remote_repo=$(create_test_remote "test-repo-rn-relpath" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-relpath"
    local project_root=$(pwd)

    git-worktree-checkout -b feature/by-path || return 1
    assert_directory_exists "feature/by-path" || return 1

    # Rename using relative path as source
    git-worktree-branch -m feature/by-path feature/path-renamed || return 1

    # Verify rename worked
    assert_directory_exists "feature/path-renamed" || return 1
    if [[ -d "feature/by-path" ]]; then
        log_error "Old worktree should have been removed"
        return 1
    fi

    return 0
}

# Test rename by worktree path (absolute)
test_rename_by_absolute_path() {
    local remote_repo=$(create_test_remote "test-repo-rn-abspath" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-abspath"
    local project_root=$(pwd)

    git-worktree-checkout -b feature/abs-path || return 1
    assert_directory_exists "feature/abs-path" || return 1

    # Rename using absolute path as source
    local abs_path="$project_root/feature/abs-path"
    git-worktree-branch -m "$abs_path" feature/abs-renamed || return 1

    # Verify rename worked
    assert_directory_exists "feature/abs-renamed" || return 1
    if [[ -d "feature/abs-path" ]]; then
        log_error "Old worktree should have been removed"
        return 1
    fi

    return 0
}

# Test rename with simple branch name (no slash)
test_rename_simple_branch() {
    local remote_repo=$(create_test_remote "test-repo-rn-simple" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-rn-simple"
    local project_root=$(pwd)

    git-worktree-checkout -b mybranch || return 1
    assert_directory_exists "mybranch" || return 1

    # Rename to another simple name
    git-worktree-branch -m mybranch renamed-branch || return 1

    # Verify rename
    assert_directory_exists "renamed-branch" || return 1
    if [[ -d "mybranch" ]]; then
        log_error "Old worktree should have been removed"
        return 1
    fi

    return 0
}

run_rename_tests() {
    log "Running git-worktree-branch -m integration tests..."

    run_test "rename_basic" "test_rename_basic"
    run_test "rename_from_inside_worktree" "test_rename_from_inside_worktree"
    run_test "rename_no_remote" "test_rename_no_remote"
    run_test "rename_dry_run" "test_rename_dry_run"
    run_test "rename_source_not_found" "test_rename_source_not_found"
    run_test "rename_dest_branch_exists" "test_rename_dest_branch_exists"
    run_test "rename_dest_path_exists" "test_rename_dest_path_exists"
    run_test "rename_cleanup_empty_parent" "test_rename_cleanup_empty_parent"
    run_test "rename_with_remote" "test_rename_with_remote"
    run_test "rename_by_relative_path" "test_rename_by_relative_path"
    run_test "rename_by_absolute_path" "test_rename_by_absolute_path"
    run_test "rename_simple_branch" "test_rename_simple_branch"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_rename_tests
    print_summary
fi
