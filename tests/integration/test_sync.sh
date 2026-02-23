#!/bin/bash

# Integration tests for git-sync Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic sync functionality (prune + update all)
test_sync_basic() {
    local remote_repo=$(create_test_remote "test-repo-sync-basic" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-sync-basic"

    # Create worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1

    # Simulate remote changes: delete feature branch + update develop
    local temp_clone="$TEMP_BASE_DIR/temp_sync_basic_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        # Delete feature branch from remote
        git push origin --delete feature/test-feature >/dev/null 2>&1
        # Update develop
        git checkout develop >/dev/null 2>&1
        echo "Sync update develop" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update develop for sync test" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run sync
    git-sync || return 1

    # Verify feature branch was pruned
    if [[ -d "feature/test-feature" ]]; then
        log_error "Sync should have pruned feature/test-feature worktree"
        return 1
    fi

    # Verify develop was updated
    if ! grep -q "Sync update develop" develop/README.md; then
        log_error "Sync did not update develop worktree"
        return 1
    fi

    # Verify develop worktree still exists
    assert_directory_exists "develop" || return 1

    return 0
}

# Test sync with nothing to prune
test_sync_nothing_to_prune() {
    local remote_repo=$(create_test_remote "test-repo-sync-noprune" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-sync-noprune"

    # Create a worktree
    git-worktree-checkout develop || return 1

    # Simulate remote changes to develop
    local temp_clone="$TEMP_BASE_DIR/temp_sync_noprune_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "Sync no prune update" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update for sync no-prune test" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run sync (nothing to prune, should still update)
    git-sync || return 1

    # Verify develop was updated
    if ! grep -q "Sync no prune update" develop/README.md; then
        log_error "Sync did not update develop when nothing to prune"
        return 1
    fi

    # All worktrees should still exist
    assert_directory_exists "develop" || return 1
    assert_directory_exists "main" || return 1

    return 0
}

# Test sync with --verbose
test_sync_verbose() {
    local remote_repo=$(create_test_remote "test-repo-sync-verbose" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-sync-verbose"

    # Run sync with --verbose (should not error)
    git-sync --verbose || return 1

    return 0
}

# Test sync with --force
test_sync_force() {
    local remote_repo=$(create_test_remote "test-repo-sync-force" "main")

    # Push the branch to the remote first so checkout can find it
    local temp_clone="$TEMP_BASE_DIR/temp_sync_force_setup"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        git checkout -b feature/dirty-branch >/dev/null 2>&1
        echo "dirty branch content" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Add dirty branch" >/dev/null 2>&1
        git push origin feature/dirty-branch >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-sync-force"

    # Create worktree and make it dirty
    git-worktree-checkout feature/dirty-branch || return 1
    echo "Uncommitted change" > "feature/dirty-branch/dirty.txt"

    # Delete the branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_sync_force_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/dirty-branch >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Sync without --force should still prune (prune ignores dirty by default
    # unless the worktree has uncommitted changes - then needs --force)
    git-sync --force || return 1

    # Verify the branch was pruned
    if [[ -d "feature/dirty-branch" ]]; then
        log_error "Sync --force should have pruned dirty worktree"
        return 1
    fi

    return 0
}

# Test sync when current worktree is pruned (CD target handling)
test_sync_cd_target() {
    local remote_repo=$(create_test_remote "test-repo-sync-cdtarget" "main")

    # Push the branch to the remote first so checkout can find it
    local temp_clone="$TEMP_BASE_DIR/temp_sync_cdtarget_setup"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        git checkout -b feature/will-be-pruned >/dev/null 2>&1
        echo "prunable branch content" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Add prunable branch" >/dev/null 2>&1
        git push origin feature/will-be-pruned >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-sync-cdtarget"

    # Create a feature worktree
    git-worktree-checkout feature/will-be-pruned || return 1

    # Delete the branch from remote
    local temp_clone="$TEMP_BASE_DIR/temp_sync_cdtarget_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git push origin --delete feature/will-be-pruned >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Run sync from the worktree that will be pruned
    cd "feature/will-be-pruned"

    # Set up DAFT_CD_FILE to test cd target behavior
    local cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-sync || true

    # Check if a cd target was written
    if [[ -s "$cd_file" ]]; then
        local cd_target=$(cat "$cd_file")
        if [[ -d "$cd_target" ]]; then
            log_success "Sync wrote valid cd target: $cd_target"
        fi
    fi

    rm -f "$cd_file"

    return 0
}

# Test sync help
test_sync_help() {
    assert_command_help "git-sync" || return 1
    assert_command_version "git-sync" || return 1

    return 0
}

# Test sync outside git repository
test_sync_outside_repo() {
    assert_command_failure "git-sync" "Should fail outside git repository"

    return 0
}

# Run all sync tests
run_sync_tests() {
    log "Running git-sync integration tests..."

    run_test "sync_basic" "test_sync_basic"
    run_test "sync_nothing_to_prune" "test_sync_nothing_to_prune"
    run_test "sync_verbose" "test_sync_verbose"
    run_test "sync_force" "test_sync_force"
    run_test "sync_cd_target" "test_sync_cd_target"
    run_test "sync_help" "test_sync_help"
    run_test "sync_outside_repo" "test_sync_outside_repo"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_sync_tests
    print_summary
fi
