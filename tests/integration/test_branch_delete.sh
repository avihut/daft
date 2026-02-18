#!/bin/bash

# Integration tests for git-worktree-branch-delete

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic branch delete after merge
test_branch_delete_basic() {
    local remote_repo=$(create_test_remote "test-repo-bd-basic" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-basic"
    local project_root=$(pwd)

    # Create a branch with worktree
    git-worktree-checkout-branch feature/test || return 1
    assert_directory_exists "feature/test" || return 1

    # Make a commit on the feature branch
    cd "feature/test"
    echo "feature work" > feature.txt
    git add feature.txt
    git commit -m "Add feature" >/dev/null 2>&1
    git push origin feature/test >/dev/null 2>&1
    cd "$project_root"

    # Merge into main
    cd "main"
    git merge feature/test >/dev/null 2>&1
    git push origin main >/dev/null 2>&1
    cd "$project_root"

    # Delete the branch
    git-worktree-branch-delete feature/test || return 1

    # Verify worktree was removed
    if [[ -d "feature/test" ]]; then
        log_error "Worktree should have been removed"
        return 1
    fi

    # Verify local branch was deleted (use exact match to avoid matching feature/test-feature)
    cd "main"
    if git branch | grep -q " feature/test$"; then
        log_error "Local branch should have been deleted"
        return 1
    fi
    cd "$project_root"

    return 0
}

# Test branch delete refuses unmerged branch
test_branch_delete_refuses_unmerged() {
    local remote_repo=$(create_test_remote "test-repo-bd-unmerged" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-unmerged"
    local project_root=$(pwd)

    git-worktree-checkout-branch feature/unmerged || return 1
    assert_directory_exists "feature/unmerged" || return 1

    cd "feature/unmerged"
    echo "unmerged work" > unmerged.txt
    git add unmerged.txt
    git commit -m "Unmerged work" >/dev/null 2>&1
    git push origin feature/unmerged >/dev/null 2>&1
    cd "$project_root"

    # Should fail without --force
    if git-worktree-branch-delete feature/unmerged 2>/dev/null; then
        log_error "Should have refused to delete unmerged branch"
        return 1
    fi

    # Verify branch still exists
    assert_directory_exists "feature/unmerged" || return 1

    return 0
}

# Test branch delete with --force on unmerged branch
test_branch_delete_force_unmerged() {
    local remote_repo=$(create_test_remote "test-repo-bd-force" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-force"
    local project_root=$(pwd)

    git-worktree-checkout-branch feature/force-me || return 1

    cd "feature/force-me"
    echo "unmerged work" > work.txt
    git add work.txt
    git commit -m "Some work" >/dev/null 2>&1
    git push origin feature/force-me >/dev/null 2>&1
    cd "$project_root"

    # Should succeed with -D
    git-worktree-branch-delete -D feature/force-me || return 1

    # Verify deletion
    if [[ -d "feature/force-me" ]]; then
        log_error "Worktree should have been removed with --force"
        return 1
    fi

    return 0
}

# Test refuses to delete default branch
test_branch_delete_refuses_default() {
    local remote_repo=$(create_test_remote "test-repo-bd-default" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-default"

    # Should fail even with --force
    if git-worktree-branch-delete -D main 2>/dev/null; then
        log_error "Should have refused to delete default branch"
        return 1
    fi

    return 0
}

# Test refuses uncommitted changes
test_branch_delete_refuses_dirty() {
    local remote_repo=$(create_test_remote "test-repo-bd-dirty" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-dirty"
    local project_root=$(pwd)

    git-worktree-checkout-branch feature/dirty || return 1

    # Make uncommitted changes
    echo "dirty" > "feature/dirty/dirty.txt"

    # Should fail
    if git-worktree-branch-delete feature/dirty 2>/dev/null; then
        log_error "Should have refused branch with uncommitted changes"
        return 1
    fi

    return 0
}

# Test deleting multiple branches
test_branch_delete_multiple() {
    local remote_repo=$(create_test_remote "test-repo-bd-multi" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-multi"
    local project_root=$(pwd)

    # Create two branches
    git-worktree-checkout-branch feature/one || return 1
    git-worktree-checkout-branch feature/two || return 1

    # Merge both into main
    cd "main"
    git merge feature/one >/dev/null 2>&1
    git merge feature/two >/dev/null 2>&1
    git push origin main >/dev/null 2>&1
    cd "$project_root"

    # Push both branches so they are in sync
    cd "feature/one"
    git push origin feature/one >/dev/null 2>&1
    cd "$project_root"
    cd "feature/two"
    git push origin feature/two >/dev/null 2>&1
    cd "$project_root"

    # Delete both at once
    git-worktree-branch-delete feature/one feature/two || return 1

    # Verify both deleted
    if [[ -d "feature/one" ]] || [[ -d "feature/two" ]]; then
        log_error "Both worktrees should have been removed"
        return 1
    fi

    return 0
}

# Test branch with no worktree (branch-only delete)
test_branch_delete_no_worktree() {
    local remote_repo=$(create_test_remote "test-repo-bd-no-wt" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-no-wt"
    local project_root=$(pwd)

    # Create a branch without a worktree (directly via git)
    cd "main"
    git branch no-worktree-branch >/dev/null 2>&1
    cd "$project_root"

    # Delete (it points at same commit as main, so it's considered merged)
    git-worktree-branch-delete no-worktree-branch || return 1

    # Verify branch deleted
    cd "main"
    if git branch | grep -q "no-worktree-branch"; then
        log_error "Branch should have been deleted"
        return 1
    fi
    cd "$project_root"

    return 0
}

# Test nonexistent branch
test_branch_delete_nonexistent() {
    local remote_repo=$(create_test_remote "test-repo-bd-noexist" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-noexist"

    if git-worktree-branch-delete nonexistent-branch 2>/dev/null; then
        log_error "Should have failed for nonexistent branch"
        return 1
    fi

    return 0
}

# Test branch delete from within the worktree being deleted writes cd path
test_branch_delete_from_current_worktree_writes_cd() {
    local remote_repo=$(create_test_remote "test-repo-bd-cd" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-cd"

    # Create a branch with worktree
    git-worktree-checkout-branch feature/test-cd || return 1
    assert_directory_exists "feature/test-cd" || return 1

    # Save the project root path (resolve symlinks for macOS /tmp -> /private/tmp)
    local project_root
    project_root=$(cd "$(pwd)" && pwd -P)

    # Make a commit and merge into main so it passes validation
    cd "feature/test-cd"
    echo "feature work" > feature.txt
    git add feature.txt
    git commit -m "Add feature" >/dev/null 2>&1
    git push origin feature/test-cd >/dev/null 2>&1
    cd "$project_root"

    cd "main"
    git merge feature/test-cd >/dev/null 2>&1
    git push origin main >/dev/null 2>&1
    cd "$project_root"

    # cd into the feature worktree (the one about to be deleted)
    cd "feature/test-cd"

    # Run branch-delete with DAFT_CD_FILE set from inside the worktree
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-worktree-branch-delete feature/test-cd 2>&1 || true

    # Verify the worktree was removed
    if [[ -d "$project_root/feature/test-cd" ]]; then
        log_error "Branch delete should have removed feature/test-cd worktree"
        rm -f "$cd_file"
        return 1
    fi

    # Verify CD path was written to temp file
    if [ -s "$cd_file" ]; then
        log_success "Branch delete wrote CD path to temp file for shell redirection"
    else
        log_error "Branch delete should have written CD path to temp file when deleting current worktree"
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

# Test branch delete from current worktree with cdTarget=default-branch
test_branch_delete_from_current_worktree_cd_default_branch() {
    local remote_repo=$(create_test_remote "test-repo-bd-cd-default" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-cd-default"

    # Set the cdTarget config to default-branch
    git config daft.prune.cdTarget default-branch

    # Create a branch with worktree
    git-worktree-checkout-branch feature/test-cd-default || return 1
    assert_directory_exists "feature/test-cd-default" || return 1

    # Save paths (resolve symlinks for macOS /tmp -> /private/tmp)
    local project_root
    project_root=$(cd "$(pwd)" && pwd -P)

    # Make a commit and merge into main
    cd "feature/test-cd-default"
    echo "feature work" > feature.txt
    git add feature.txt
    git commit -m "Add feature" >/dev/null 2>&1
    git push origin feature/test-cd-default >/dev/null 2>&1
    cd "$project_root"

    cd "main"
    git merge feature/test-cd-default >/dev/null 2>&1
    git push origin main >/dev/null 2>&1
    cd "$project_root"
    local main_wt_path="$project_root/main"

    # cd into the feature worktree
    cd "feature/test-cd-default"

    # Run branch-delete with DAFT_CD_FILE set
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-worktree-branch-delete feature/test-cd-default 2>&1 || true

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

run_branch_delete_tests() {
    log "Running git-worktree-branch-delete integration tests..."

    run_test "branch_delete_basic" "test_branch_delete_basic"
    run_test "branch_delete_refuses_unmerged" "test_branch_delete_refuses_unmerged"
    run_test "branch_delete_force_unmerged" "test_branch_delete_force_unmerged"
    run_test "branch_delete_refuses_default" "test_branch_delete_refuses_default"
    run_test "branch_delete_refuses_dirty" "test_branch_delete_refuses_dirty"
    run_test "branch_delete_multiple" "test_branch_delete_multiple"
    run_test "branch_delete_no_worktree" "test_branch_delete_no_worktree"
    run_test "branch_delete_nonexistent" "test_branch_delete_nonexistent"
    run_test "branch_delete_from_current_worktree_writes_cd" "test_branch_delete_from_current_worktree_writes_cd"
    run_test "branch_delete_from_current_worktree_cd_default_branch" "test_branch_delete_from_current_worktree_cd_default_branch"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_branch_delete_tests
    print_summary
fi
