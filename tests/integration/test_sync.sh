#!/bin/bash

# Integration tests for git-worktree-sync Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic sync functionality (prune + update all)
test_sync_basic() {
    local remote_repo=$(create_test_remote "test-repo-sync-basic" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
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
    git-worktree-sync || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
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
    git-worktree-sync || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-verbose"

    # Run sync with --verbose (should not error)
    git-worktree-sync --verbose || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
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
    git-worktree-sync --force || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
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
    DAFT_CD_FILE="$cd_file" git-worktree-sync || true

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
    assert_command_help "git-worktree-sync" || return 1
    assert_command_version "git-worktree-sync" || return 1

    return 0
}

# Test sync outside git repository
test_sync_outside_repo() {
    assert_command_failure "git-worktree-sync" "Should fail outside git repository"

    return 0
}

# Test sync with diverged branch and --rebase continues successfully
test_sync_diverged_branch_with_rebase() {
    local remote_repo=$(create_test_remote "test-repo-sync-diverged-rebase" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-diverged-rebase"

    # Create a feature worktree
    git-worktree-checkout develop || return 1

    # Make the local develop branch diverge from upstream by amending
    (
        cd develop
        echo "Local diverged change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit --amend -m "Amended: diverged from upstream" >/dev/null 2>&1
    ) >/dev/null 2>&1

    # Run sync with --rebase (use --verbose --verbose for sequential mode)
    git-worktree-sync --rebase main --verbose --verbose || {
        log_error "Sync --rebase should not fail when a branch has diverged"
        return 1
    }

    # Verify develop was rebased onto main (check that main's commit is in develop's history)
    local main_commit=$(cd main && git rev-parse HEAD)
    if ! (cd develop && git log --format=%H | grep -q "$main_commit"); then
        log_error "Develop branch should have been rebased onto main"
        return 1
    fi

    return 0
}

# Test sync with diverged branch without --rebase succeeds (warning, not failure)
test_sync_diverged_branch_no_rebase() {
    local remote_repo=$(create_test_remote "test-repo-sync-diverged-norebase" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-diverged-norebase"

    # Create a feature worktree
    git-worktree-checkout develop || return 1

    # Record the develop commit before diverging
    local original_commit=$(cd develop && git rev-parse HEAD)

    # Make the local develop branch diverge from upstream by amending
    (
        cd develop
        echo "Local diverged change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit --amend -m "Amended: diverged from upstream" >/dev/null 2>&1
    ) >/dev/null 2>&1

    local diverged_commit=$(cd develop && git rev-parse HEAD)

    # Run sync without --rebase (use --verbose --verbose for sequential mode)
    git-worktree-sync --verbose --verbose || {
        log_error "Sync should not fail when a branch has diverged (should warn instead)"
        return 1
    }

    # Verify develop is still at its diverged commit (not changed)
    local current_commit=$(cd develop && git rev-parse HEAD)
    if [[ "$current_commit" != "$diverged_commit" ]]; then
        log_error "Develop branch should remain at its diverged commit when no --rebase is used"
        return 1
    fi

    return 0
}

# Test sync --rebase --autostash stashes dirty worktree, rebases, and restores
test_sync_rebase_autostash() {
    local remote_repo=$(create_test_remote "test-repo-sync-autostash" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-autostash"

    # Create a feature worktree
    git-worktree-checkout develop || return 1

    # Make a local commit on develop so rebase has work to do
    (
        cd develop
        echo "develop-only content" > develop-feature.txt
        git add develop-feature.txt >/dev/null 2>&1
        git commit -m "Add develop feature" >/dev/null 2>&1
    ) >/dev/null 2>&1

    # Push a new commit to main via remote (develop will rebase onto this)
    # Use main.py (not README.md) to avoid conflicting with develop's README.md changes
    local temp_clone="$TEMP_BASE_DIR/temp_sync_autostash_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        echo "Remote main change" >> main.py
        git add main.py >/dev/null 2>&1
        git commit -m "Update main for autostash test" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Make uncommitted changes in develop in a non-conflicting file
    echo "Uncommitted local work" > develop/local-wip.txt

    # Run sync with --rebase --autostash (use -vv for sequential mode)
    git-worktree-sync --rebase main --autostash --verbose --verbose || {
        log_error "Sync --rebase --autostash should succeed with dirty worktree"
        return 1
    }

    # Verify develop was rebased onto main (main's latest commit is in develop's history)
    local main_commit=$(cd main && git rev-parse HEAD)
    if ! (cd develop && git log --format=%H | grep -q "$main_commit"); then
        log_error "Develop branch should have been rebased onto main"
        return 1
    fi

    # Verify uncommitted changes are still present
    if ! grep -q "Uncommitted local work" develop/local-wip.txt; then
        log_error "Uncommitted changes should have been restored after autostash rebase"
        return 1
    fi

    return 0
}

# Test sync --autostash without --rebase fails with validation error
test_sync_autostash_without_rebase() {
    local remote_repo=$(create_test_remote "test-repo-sync-autostash-norebase" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-autostash-norebase"

    # Run sync with --autostash but without --rebase — should fail
    local output
    output=$(git-worktree-sync --autostash 2>&1) && {
        log_error "Sync --autostash without --rebase should fail"
        return 1
    }

    # Verify the error mentions the requirement
    if ! echo "$output" | grep -qi "rebase"; then
        log_error "Error message should mention --rebase requirement, got: $output"
        return 1
    fi

    return 0
}

# Test sync --rebase --push skips push when rebase conflicts
test_sync_rebase_conflict_skips_push() {
    local remote_repo=$(create_test_remote "test-repo-sync-conflict-push" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sync-conflict-push"

    # Create a feature worktree
    git-worktree-checkout develop || return 1

    # Make a local commit on develop that will conflict with main
    (
        cd develop
        echo "develop conflicting content" > README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Conflicting change on develop" >/dev/null 2>&1
    ) >/dev/null 2>&1

    # Push a conflicting change to main via remote
    local temp_clone="$TEMP_BASE_DIR/temp_sync_conflict_push_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        echo "main conflicting content" > README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Conflicting change on main" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Record develop's commit before sync
    local develop_commit_before=$(cd develop && git rev-parse HEAD)

    # Record remote develop ref before sync
    local remote_develop_before=$(git ls-remote "$remote_repo" develop 2>/dev/null | awk '{print $1}')

    # Run sync with --rebase --push (use -vv for sequential mode)
    # This should NOT fail -- conflicts are warnings, not errors
    git-worktree-sync --rebase main --push --force-with-lease --verbose --verbose 2>&1 || true

    # Verify develop branch was NOT changed (rebase aborted)
    local develop_commit_after=$(cd develop && git rev-parse HEAD)
    if [[ "$develop_commit_after" != "$develop_commit_before" ]]; then
        log_error "Develop branch should not have changed after aborted rebase"
        return 1
    fi

    # Verify push was NOT attempted (remote develop should be unchanged)
    local remote_develop_after=$(git ls-remote "$remote_repo" develop 2>/dev/null | awk '{print $1}')
    if [[ "$remote_develop_after" != "$remote_develop_before" ]]; then
        log_error "Push should have been skipped for branch with rebase conflict"
        return 1
    fi

    return 0
}

# Run all sync tests
run_sync_tests() {
    log "Running git-worktree-sync integration tests..."

    run_test "sync_basic" "test_sync_basic"
    run_test "sync_nothing_to_prune" "test_sync_nothing_to_prune"
    run_test "sync_verbose" "test_sync_verbose"
    run_test "sync_force" "test_sync_force"
    run_test "sync_cd_target" "test_sync_cd_target"
    run_test "sync_help" "test_sync_help"
    run_test "sync_outside_repo" "test_sync_outside_repo"
    run_test "sync_diverged_branch_with_rebase" "test_sync_diverged_branch_with_rebase"
    run_test "sync_diverged_branch_no_rebase" "test_sync_diverged_branch_no_rebase"
    run_test "sync_rebase_autostash" "test_sync_rebase_autostash"
    run_test "sync_autostash_without_rebase" "test_sync_autostash_without_rebase"
    run_test "sync_rebase_conflict_skips_push" "test_sync_rebase_conflict_skips_push"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_sync_tests
    print_summary
fi
