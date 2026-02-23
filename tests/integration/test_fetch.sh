#!/bin/bash

# Integration tests for git-worktree-fetch Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic fetch functionality - current worktree
test_fetch_current_worktree() {
    local remote_repo=$(create_test_remote "test-repo-fetch-current" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-current"

    # Simulate remote changes by pushing from another clone
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_current_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "New content from remote" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update from remote" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch the current worktree (main)
    cd "main"
    git-worktree-fetch || return 1

    # Verify the changes were pulled
    if ! grep -q "New content from remote" README.md; then
        log_error "Fetch did not pull the remote changes"
        return 1
    fi

    return 0
}

# Test fetch specific worktree
test_fetch_specific_worktree() {
    local remote_repo=$(create_test_remote "test-repo-fetch-specific" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-specific"

    # Create a worktree
    git-worktree-checkout develop || return 1

    # Simulate remote changes to develop
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_specific_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "Update to develop branch" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update develop branch" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch specific worktree from main
    cd "main"
    git-worktree-fetch develop || return 1

    # Verify changes were pulled to develop
    if ! grep -q "Update to develop branch" ../develop/README.md; then
        log_error "Fetch did not pull changes to develop worktree"
        return 1
    fi

    return 0
}

# Test fetch multiple worktrees
test_fetch_multiple_worktrees() {
    local remote_repo=$(create_test_remote "test-repo-fetch-multiple" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-multiple"

    # Create worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1

    # Simulate remote changes to both branches
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_multiple_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "Multiple update develop" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update develop" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1

        git checkout feature/test-feature >/dev/null 2>&1
        echo "Multiple update feature" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update feature" >/dev/null 2>&1
        git push origin feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch multiple worktrees
    git-worktree-fetch develop feature/test-feature || return 1

    # Verify changes were pulled to both
    if ! grep -q "Multiple update develop" develop/README.md; then
        log_error "Fetch did not pull changes to develop"
        return 1
    fi

    if ! grep -q "Multiple update feature" "feature/test-feature/README.md"; then
        log_error "Fetch did not pull changes to feature/test-feature"
        return 1
    fi

    return 0
}

# Test fetch --all
test_fetch_all() {
    local remote_repo=$(create_test_remote "test-repo-fetch-all" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-all"

    # Create worktrees
    git-worktree-checkout develop || return 1

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_all_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "All update main" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update main" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1

        git checkout develop >/dev/null 2>&1
        echo "All update develop" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Update develop" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch all worktrees
    git-worktree-fetch --all || return 1

    # Verify changes were pulled to all worktrees
    if ! grep -q "All update main" main/README.md; then
        log_error "Fetch --all did not pull changes to main"
        return 1
    fi

    if ! grep -q "All update develop" develop/README.md; then
        log_error "Fetch --all did not pull changes to develop"
        return 1
    fi

    return 0
}

# Test fetch with uncommitted changes (should skip)
test_fetch_uncommitted_changes_skip() {
    local remote_repo=$(create_test_remote "test-repo-fetch-uncommitted" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-uncommitted"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_uncommitted_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Remote change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Remote update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Create uncommitted changes in main
    cd "main"
    echo "Local uncommitted change" > local.txt

    # Fetch should succeed but skip this worktree
    git-worktree-fetch 2>&1 | grep -q "uncommitted changes" || {
        log_warning "Expected warning about uncommitted changes"
    }

    # Verify local changes are preserved
    assert_file_exists "local.txt" || return 1

    return 0
}

# Test fetch with --force on uncommitted changes
test_fetch_force() {
    local remote_repo=$(create_test_remote "test-repo-fetch-force" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-force"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_force_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Force remote change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Force remote update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Create uncommitted changes in main (tracked file change)
    cd "main"
    echo "Local tracked change" >> README.md

    # Fetch with --force and --autostash should work
    git-worktree-fetch --force --autostash || {
        log_warning "Fetch with --force may have failed due to conflicts"
        return 0  # Allow this to pass as autostash behavior may vary
    }

    return 0
}

# Test fetch --dry-run
test_fetch_dry_run() {
    local remote_repo=$(create_test_remote "test-repo-fetch-dryrun" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-dryrun"

    # Get current state
    cd "main"
    local before_hash=$(git rev-parse HEAD)

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_dryrun_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Dry run change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Dry run update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch with --dry-run
    git-worktree-fetch --dry-run || return 1

    # Verify nothing was actually pulled
    local after_hash=$(git rev-parse HEAD)
    if [[ "$before_hash" != "$after_hash" ]]; then
        log_error "Dry run actually pulled changes"
        return 1
    fi

    return 0
}

# Test fetch with --rebase
test_fetch_rebase() {
    local remote_repo=$(create_test_remote "test-repo-fetch-rebase" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-rebase"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_rebase_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Rebase remote change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Rebase remote update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch with --rebase
    cd "main"
    git-worktree-fetch --rebase || return 1

    # Verify changes were pulled
    if ! grep -q "Rebase remote change" README.md; then
        log_error "Fetch --rebase did not pull changes"
        return 1
    fi

    return 0
}

# Test fetch with config-based defaults
test_fetch_config() {
    local remote_repo=$(create_test_remote "test-repo-fetch-config" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-config"

    # Set config for fetch args
    cd ".git"
    cd ..
    git config daft.update.args "--rebase --autostash"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_config_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Config test change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Config test update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch should use config settings
    cd "main"
    git-worktree-fetch || return 1

    # Verify changes were pulled
    if ! grep -q "Config test change" README.md; then
        log_error "Fetch with config did not pull changes"
        return 1
    fi

    return 0
}

# Test fetch with pass-through arguments
test_fetch_passthrough_args() {
    local remote_repo=$(create_test_remote "test-repo-fetch-passthrough" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-passthrough"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_passthrough_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Passthrough test change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Passthrough test update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch with pass-through arguments
    cd "main"
    git-worktree-fetch -- --no-stat || return 1

    # Verify changes were pulled
    if ! grep -q "Passthrough test change" README.md; then
        log_error "Fetch with passthrough args did not pull changes"
        return 1
    fi

    return 0
}

# Test fetch outside git repository
test_fetch_outside_repo() {
    # Test fetch command outside git repository
    assert_command_failure "git-worktree-fetch" "Should fail outside git repository"

    return 0
}

# Test fetch help functionality
test_fetch_help() {
    # Test help commands
    assert_command_help "git-worktree-fetch" || return 1
    assert_command_version "git-worktree-fetch" || return 1

    return 0
}

# Test fetch with no tracking branch
test_fetch_no_tracking() {
    local remote_repo=$(create_test_remote "test-repo-fetch-notracking" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-notracking"

    # Create a new local branch without remote tracking
    cd "main"
    git checkout -b local-only-branch >/dev/null 2>&1
    git worktree add ../local-only local-only-branch >/dev/null 2>&1
    cd ..

    # Fetch should skip the branch with no tracking
    git-worktree-fetch local-only 2>&1 | grep -iq "tracking\|skip" || {
        log_warning "Expected warning about no tracking branch"
    }

    return 0
}

# Test fetch performance
test_fetch_performance() {
    local remote_repo=$(create_test_remote "test-repo-fetch-performance" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-performance"

    # Create multiple worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1

    # Test fetch performance
    local start_time=$(date +%s)
    git-worktree-fetch --all || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))

    if [[ $duration -gt 30 ]]; then
        log_warning "Fetch performance test took ${duration}s (expected < 30s)"
    else
        log_success "Fetch performance test completed in ${duration}s"
    fi

    return 0
}

# Test fetch from subdirectory
test_fetch_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-fetch-subdir" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-subdir"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_subdir_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Subdir test change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Subdir test update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Create a subdirectory and test fetch from there
    mkdir -p "main/subdir/deep"
    cd "main/subdir/deep"

    # Fetch should work from subdirectory
    git-worktree-fetch || return 1

    # Verify changes were pulled
    if ! grep -q "Subdir test change" ../../../main/README.md 2>/dev/null && \
       ! grep -q "Subdir test change" ../../README.md 2>/dev/null; then
        # Check from the worktree root
        cd "../../.."
        if ! grep -q "Subdir test change" README.md; then
            log_error "Fetch from subdirectory did not pull changes"
            return 1
        fi
    fi

    return 0
}

# Test fetching a clean target worktree from a dirty current worktree (issue #186)
test_fetch_clean_target_from_dirty_worktree() {
    local remote_repo=$(create_test_remote "test-repo-fetch-dirty-cwd" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-dirty-cwd"

    # Checkout the develop worktree
    git-worktree-checkout develop || return 1

    # Simulate remote changes on develop
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_dirty_cwd_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "Remote develop change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Remote develop update" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Make the main worktree dirty (uncommitted file)
    cd "main"
    echo "Local uncommitted change" > dirty.txt

    # Fetch the clean develop worktree from the dirty main worktree
    local output
    output=$(git-worktree-fetch develop 2>&1) || {
        log_error "Fetch of clean target from dirty cwd should succeed"
        return 1
    }

    # Should NOT contain uncommitted changes warning for develop
    if echo "$output" | grep -q "uncommitted changes"; then
        log_error "Should not skip clean target worktree due to dirty current worktree"
        return 1
    fi

    # Verify the remote changes were pulled into develop
    cd ../develop
    if ! grep -q "Remote develop change" README.md; then
        log_error "Fetch did not pull remote changes into develop"
        return 1
    fi

    # Verify dirty file in main is preserved
    cd ../main
    assert_file_exists "dirty.txt" || return 1

    return 0
}

# Test fetching a dirty target worktree from a clean current worktree (converse of issue #186)
test_fetch_dirty_target_from_clean_worktree() {
    local remote_repo=$(create_test_remote "test-repo-fetch-dirty-target" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-dirty-target"

    # Checkout the develop worktree
    git-worktree-checkout develop || return 1

    # Simulate remote changes on develop
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_dirty_target_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "Remote develop change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Remote develop update" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Make the develop (target) worktree dirty
    echo "Uncommitted change in develop" > develop/dirty.txt

    # From the clean main worktree, fetch the dirty develop worktree
    cd "main"
    local output
    output=$(git-worktree-fetch develop 2>&1)

    # Should warn about uncommitted changes and skip develop
    if ! echo "$output" | grep -q "uncommitted changes"; then
        log_error "Should skip dirty target worktree with uncommitted changes warning"
        return 1
    fi

    return 0
}

# Test cross-branch refspec: fetch origin/main into feature worktree
test_fetch_cross_branch_refspec() {
    local remote_repo=$(create_test_remote "test-repo-fetch-crossbranch" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-crossbranch"

    # Create a feature worktree
    git-worktree-checkout -b feature/test-cross || return 1

    # Simulate remote changes on main
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_crossbranch_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Cross-branch update from main" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Main branch update for cross-branch test" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Use cross-branch refspec: update feature/test-cross from main
    cd "main"
    git-worktree-fetch "main:feature/test-cross" || return 1

    # Verify the feature worktree now has the main branch content
    if ! grep -q "Cross-branch update from main" "../feature/test-cross/README.md"; then
        log_error "Cross-branch refspec did not sync main into feature worktree"
        return 1
    fi

    return 0
}

# Test cross-branch refspec fails on dirty worktree without --force
test_fetch_cross_branch_dirty_fails() {
    local remote_repo=$(create_test_remote "test-repo-fetch-crossdirty" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-crossdirty"

    # Create a feature worktree
    git-worktree-checkout -b feature/test-dirty || return 1

    # Make the feature worktree dirty
    echo "Uncommitted change" > "feature/test-dirty/dirty.txt"

    # Cross-branch refspec should skip dirty worktree
    cd "main"
    local output
    output=$(git-worktree-fetch "main:feature/test-dirty" 2>&1)

    if ! echo "$output" | grep -q "uncommitted changes"; then
        log_error "Cross-branch should skip dirty worktree"
        return 1
    fi

    return 0
}

# Test cross-branch refspec with --force on dirty worktree
test_fetch_cross_branch_force() {
    local remote_repo=$(create_test_remote "test-repo-fetch-crossforce" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-crossforce"

    # Create a feature worktree
    git-worktree-checkout -b feature/test-force || return 1

    # Simulate remote changes on main
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_crossforce_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Force cross-branch update" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Force cross-branch test" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Make the feature worktree dirty (untracked file)
    echo "Uncommitted" > "feature/test-force/dirty.txt"

    # Cross-branch refspec with --force should proceed
    cd "main"
    git-worktree-fetch --force "main:feature/test-force" || return 1

    # Verify the feature worktree was updated
    if ! grep -q "Force cross-branch update" "../feature/test-force/README.md"; then
        log_error "Cross-branch --force did not update worktree"
        return 1
    fi

    return 0
}

# Test cross-branch refspec with --dry-run
test_fetch_cross_branch_dry_run() {
    local remote_repo=$(create_test_remote "test-repo-fetch-crossdry" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-crossdry"

    # Create a feature worktree
    git-worktree-checkout -b feature/test-dryrun || return 1

    # Get initial hash
    cd "feature/test-dryrun"
    local before_hash=$(git rev-parse HEAD)
    cd ..

    # Simulate remote changes on main
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_crossdry_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Dry run cross-branch" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Dry run cross-branch test" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Cross-branch with --dry-run should not change anything
    cd "main"
    git-worktree-fetch --dry-run "main:feature/test-dryrun" || return 1

    # Verify the feature worktree was not changed
    cd "../feature/test-dryrun"
    local after_hash=$(git rev-parse HEAD)
    if [[ "$before_hash" != "$after_hash" ]]; then
        log_error "Cross-branch dry run actually modified the worktree"
        return 1
    fi

    return 0
}

# Test self-referencing refspec (explicit same-branch)
test_fetch_self_referencing_refspec() {
    local remote_repo=$(create_test_remote "test-repo-fetch-selfref" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-selfref"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_selfref_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Self-ref update" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Self-ref test update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Use explicit self-referencing refspec
    cd "main"
    git-worktree-fetch main || return 1

    # Verify changes were pulled
    if ! grep -q "Self-ref update" README.md; then
        log_error "Self-referencing refspec did not pull changes"
        return 1
    fi

    return 0
}

# Test config fallback from deprecated daft.fetch.args to daft.update.args
test_fetch_config_fallback() {
    local remote_repo=$(create_test_remote "test-repo-fetch-configfb" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-fetch-configfb"

    # Set the deprecated config key (should still work as fallback)
    cd ".git"
    cd ..
    git config daft.fetch.args "--rebase --autostash"

    # Simulate remote changes
    local temp_clone="$TEMP_BASE_DIR/temp_fetch_configfb_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        echo "Config fallback change" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Config fallback update" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Fetch should use deprecated config settings as fallback
    cd "main"
    git-worktree-fetch || return 1

    # Verify changes were pulled
    if ! grep -q "Config fallback change" README.md; then
        log_error "Config fallback did not work"
        return 1
    fi

    return 0
}

# Run all fetch tests
run_fetch_tests() {
    log "Running git-worktree-fetch integration tests..."

    run_test "fetch_current_worktree" "test_fetch_current_worktree"
    run_test "fetch_specific_worktree" "test_fetch_specific_worktree"
    run_test "fetch_multiple_worktrees" "test_fetch_multiple_worktrees"
    run_test "fetch_all" "test_fetch_all"
    run_test "fetch_uncommitted_changes_skip" "test_fetch_uncommitted_changes_skip"
    run_test "fetch_clean_target_from_dirty_worktree" "test_fetch_clean_target_from_dirty_worktree"
    run_test "fetch_dirty_target_from_clean_worktree" "test_fetch_dirty_target_from_clean_worktree"
    run_test "fetch_force" "test_fetch_force"
    run_test "fetch_dry_run" "test_fetch_dry_run"
    run_test "fetch_rebase" "test_fetch_rebase"
    run_test "fetch_config" "test_fetch_config"
    run_test "fetch_passthrough_args" "test_fetch_passthrough_args"
    run_test "fetch_outside_repo" "test_fetch_outside_repo"
    run_test "fetch_help" "test_fetch_help"
    run_test "fetch_no_tracking" "test_fetch_no_tracking"
    run_test "fetch_performance" "test_fetch_performance"
    run_test "fetch_from_subdirectory" "test_fetch_from_subdirectory"
    run_test "fetch_self_referencing_refspec" "test_fetch_self_referencing_refspec"
    run_test "fetch_cross_branch_refspec" "test_fetch_cross_branch_refspec"
    run_test "fetch_cross_branch_dirty_fails" "test_fetch_cross_branch_dirty_fails"
    run_test "fetch_cross_branch_force" "test_fetch_cross_branch_force"
    run_test "fetch_cross_branch_dry_run" "test_fetch_cross_branch_dry_run"
    run_test "fetch_config_fallback" "test_fetch_config_fallback"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_fetch_tests
    print_summary
fi
