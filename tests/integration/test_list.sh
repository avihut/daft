#!/bin/bash

# Integration tests for git-worktree-list / daft list

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic listing shows all worktrees with branch names
test_list_basic() {
    local remote_repo=$(create_test_remote "test-repo-list-basic" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-basic"

    # Create additional worktrees
    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test-feature || return 1

    # Run list from the main worktree
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list failed"
        log_error "Output: $output"
        return 1
    }

    # Verify all worktrees are shown
    if ! echo "$output" | grep -q "main"; then
        log_error "List output should contain 'main'"
        log_error "Output: $output"
        return 1
    fi

    if ! echo "$output" | grep -q "develop"; then
        log_error "List output should contain 'develop'"
        log_error "Output: $output"
        return 1
    fi

    if ! echo "$output" | grep -q "feature/test-feature"; then
        log_error "List output should contain 'feature/test-feature'"
        log_error "Output: $output"
        return 1
    fi

    log_success "Basic listing shows all worktrees"
    return 0
}

# Test current worktree marker
test_list_current_marker() {
    local remote_repo=$(create_test_remote "test-repo-list-current" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-current"

    # Create another worktree
    git-worktree-checkout develop || return 1

    # Run from inside the develop worktree
    cd develop
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    # The current worktree (develop) should have the > marker
    # and other worktrees should not
    if ! echo "$output" | grep "develop" | grep -q ">"; then
        log_error "Current worktree 'develop' should have > marker"
        log_error "Output: $output"
        return 1
    fi

    # Run from inside the main worktree
    cd ../main
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    if ! echo "$output" | grep "main" | grep -q ">"; then
        log_error "Current worktree 'main' should have > marker"
        log_error "Output: $output"
        return 1
    fi

    log_success "Current worktree marker works correctly"
    return 0
}

# Test dirty marker shows for uncommitted changes
test_list_dirty_marker() {
    local remote_repo=$(create_test_remote "test-repo-list-dirty" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-dirty"

    # Create a worktree
    git-worktree-checkout develop || return 1

    # Make uncommitted changes in develop
    echo "dirty change" >> develop/README.md

    # Run list
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    # develop should show -N for unstaged changes
    if ! echo "$output" | grep "develop" | grep -q '\-[0-9]'; then
        log_error "Dirty worktree 'develop' should have -N unstaged indicator"
        log_error "Output: $output"
        return 1
    fi

    # main should NOT have change indicators
    if echo "$output" | grep "main" | grep -qE '[+\-?][0-9]'; then
        log_error "Clean worktree 'main' should not have change indicators"
        log_error "Output: $output"
        return 1
    fi

    log_success "Dirty marker shows correctly"
    return 0
}

# Test JSON output format
test_list_json() {
    local remote_repo=$(create_test_remote "test-repo-list-json" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-json"

    # Create a worktree with dirty state
    git-worktree-checkout develop || return 1
    echo "dirty" >> develop/README.md

    # Run list with --json
    cd main
    local output
    output=$(git-worktree-list --json 2>&1)

    # Verify it's valid JSON (check for array brackets)
    if ! echo "$output" | grep -q '^\['; then
        log_error "JSON output should start with ["
        log_error "Output: $output"
        return 1
    fi

    # Check for expected JSON fields
    local required_fields=("name" "path" "is_current" "is_default_branch" "ahead" "behind" "staged" "unstaged" "untracked" "remote_ahead" "remote_behind" "last_commit_age" "last_commit_subject" "branch_age")
    for field in "${required_fields[@]}"; do
        if ! echo "$output" | grep -q "\"$field\""; then
            log_error "JSON output should contain field '$field'"
            log_error "Output: $output"
            return 1
        fi
    done

    # Verify is_current is true for main (we're in the main worktree)
    # The JSON entries are objects with fields sorted alphabetically.
    # We need to find the main entry and check is_current.
    # Use a block-based approach: find the block containing "name": "main"
    # and verify it also contains "is_current": true.
    local main_block
    main_block=$(echo "$output" | awk '/"name": "main"/{found=1} found && /\}/{print; found=0} found{print}' RS='{' ORS='{')
    if ! echo "$main_block" | grep -q '"is_current": true'; then
        log_error "JSON should show main as current worktree"
        log_error "Output: $output"
        return 1
    fi

    # Verify develop shows as dirty (unstaged > 0 since we modified a tracked file)
    local develop_block
    develop_block=$(echo "$output" | awk '/"name": "develop"/{found=1} found && /\}/{print; found=0} found{print}' RS='{' ORS='{')
    if ! echo "$develop_block" | grep -qE '"unstaged": [1-9]'; then
        log_error "JSON should show develop with non-zero unstaged count"
        log_error "Output: $output"
        return 1
    fi

    log_success "JSON output format is correct"
    return 0
}

# Test detached HEAD state
test_list_detached_head() {
    git-worktree-init detached-test || return 1
    cd "detached-test"

    # Create an initial commit
    cd master
    echo "Initial content" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1
    local commit_sha=$(git rev-parse HEAD)
    cd ..

    # Create a detached HEAD worktree
    git worktree add --detach detached-wt "$commit_sha" >/dev/null 2>&1 || {
        log_error "Failed to create detached worktree"
        return 1
    }

    # Run list
    cd master
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    # Verify "(detached)" appears in the output
    if ! echo "$output" | grep -q "(detached)"; then
        log_error "List should show '(detached)' for detached HEAD worktree"
        log_error "Output: $output"
        return 1
    fi

    log_success "Detached HEAD state shown correctly"
    return 0
}

# Test ahead/behind counts
test_list_ahead_behind() {
    local remote_repo=$(create_test_remote "test-repo-list-ahead" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-ahead"

    # Checkout develop and make additional commits ahead of main
    git-worktree-checkout develop || return 1

    cd develop
    echo "ahead commit 1" > ahead1.txt
    git add ahead1.txt
    git commit -m "Ahead commit 1" >/dev/null 2>&1
    echo "ahead commit 2" > ahead2.txt
    git add ahead2.txt
    git commit -m "Ahead commit 2" >/dev/null 2>&1
    cd ..

    # Run list
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    # develop should show ahead count (+N where N > 0)
    if ! echo "$output" | grep "develop" | grep -q '+[0-9]'; then
        log_error "Develop should show ahead count (+N)"
        log_error "Output: $output"
        return 1
    fi

    log_success "Ahead/behind counts shown correctly"
    return 0
}

# Test listing outside a git repository fails
test_list_outside_repo() {
    assert_command_failure "git-worktree-list" "Should fail outside git repository"

    return 0
}

# Test help functionality
test_list_help() {
    assert_command_help "git-worktree-list" || return 1
    assert_command_version "git-worktree-list" || return 1

    return 0
}

# Test JSON output for a single worktree (init creates one worktree)
test_list_json_single() {
    git-worktree-init json-single-test || return 1
    cd "json-single-test"

    # Create an initial commit so we have commit info
    cd master
    echo "Content" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # JSON should return an array with exactly one entry
    local output
    output=$(git-worktree-list --json 2>&1)

    if ! echo "$output" | grep -q '^\['; then
        log_error "JSON output should be a valid JSON array"
        log_error "Output: $output"
        return 1
    fi

    # Should contain exactly one "name" field (for master)
    local name_count
    name_count=$(echo "$output" | grep -c '"name"')
    if [[ "$name_count" -ne 1 ]]; then
        log_error "Expected exactly 1 entry in JSON, got $name_count"
        log_error "Output: $output"
        return 1
    fi

    if ! echo "$output" | grep -q '"name": "master"'; then
        log_error "JSON should contain master worktree"
        log_error "Output: $output"
        return 1
    fi

    log_success "JSON output for single worktree is correct"
    return 0
}

# Test listing with many worktrees
test_list_many_worktrees() {
    local remote_repo=$(create_test_remote "test-repo-list-many" "main")

    # Create many branches in remote
    local temp_clone="$TEMP_BASE_DIR/temp_list_many_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1

    (
        cd "$temp_clone"
        for i in {1..5}; do
            git checkout -b "feature/branch$i" >/dev/null 2>&1
            echo "Branch $i" > "branch$i.txt"
            git add "branch$i.txt" >/dev/null 2>&1
            git commit -m "Add branch$i" >/dev/null 2>&1
            git push origin "feature/branch$i" >/dev/null 2>&1
        done
    ) >/dev/null 2>&1

    rm -rf "$temp_clone"

    # Clone and checkout all branches
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-many"

    git fetch origin >/dev/null 2>&1
    for i in {1..5}; do
        git-worktree-checkout "feature/branch$i" || return 1
    done

    # Run list
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    # Verify all branches are listed
    for i in {1..5}; do
        if ! echo "$output" | grep -q "feature/branch$i"; then
            log_error "List should contain feature/branch$i"
            log_error "Output: $output"
            return 1
        fi
    done

    # Also verify main is listed
    if ! echo "$output" | grep -q "main"; then
        log_error "List should contain main"
        log_error "Output: $output"
        return 1
    fi

    log_success "Many worktrees listed correctly"
    return 0
}

# Test list shows last commit subject
test_list_commit_subject() {
    git-worktree-init subject-test || return 1
    cd "subject-test"

    # Create a commit with a specific subject
    cd master
    echo "Content" > test.txt
    git add test.txt
    git commit -m "My specific commit subject" >/dev/null 2>&1
    cd ..

    # Run list
    cd master
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1)

    # Verify the commit subject appears
    if ! echo "$output" | grep -q "My specific commit subject"; then
        log_error "List should show last commit subject"
        log_error "Output: $output"
        return 1
    fi

    log_success "Last commit subject shown correctly"
    return 0
}

# Test list from subdirectory of a worktree
test_list_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-list-subdir" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-subdir"

    # Create a worktree
    git-worktree-checkout develop || return 1

    # Run from a subdirectory inside a worktree
    mkdir -p "main/subdir/deep"
    cd "main/subdir/deep"
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list should work from subdirectory"
        log_error "Output: $output"
        return 1
    }

    # Should still list all worktrees
    if ! echo "$output" | grep -q "main"; then
        log_error "List from subdirectory should show main"
        log_error "Output: $output"
        return 1
    fi

    if ! echo "$output" | grep -q "develop"; then
        log_error "List from subdirectory should show develop"
        log_error "Output: $output"
        return 1
    fi

    log_success "List works from subdirectory"
    return 0
}

# Test JSON ahead/behind values
test_list_json_ahead_behind() {
    local remote_repo=$(create_test_remote "test-repo-list-json-ab" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-json-ab"

    # Checkout develop and make commits
    git-worktree-checkout develop || return 1

    cd develop
    echo "ahead" > ahead.txt
    git add ahead.txt
    git commit -m "Ahead commit" >/dev/null 2>&1
    cd ..

    # Run list with --json
    cd main
    local output
    output=$(git-worktree-list --json 2>&1)

    # Verify develop has non-zero ahead count in JSON
    # Find the block containing "name": "develop" and check its "ahead" field
    local develop_block
    develop_block=$(echo "$output" | awk '/"name": "develop"/{found=1} found && /\}/{print; found=0} found{print}' RS='{' ORS='{')
    if ! echo "$develop_block" | grep -qE '"ahead": [1-9]'; then
        log_error "JSON should show non-zero ahead count for develop"
        log_error "Output: $output"
        return 1
    fi

    log_success "JSON ahead/behind values are correct"
    return 0
}

# Test that Age header appears in table output
test_list_branch_age() {
    local remote_repo=$(create_test_remote "test-repo-list-age" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-age"

    # Run list from the main worktree
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list failed"
        log_error "Output: $output"
        return 1
    }

    # Verify the Age header appears (check all lines since stderr may precede header)
    if ! echo "$output" | grep -q "Age"; then
        log_error "List header should contain 'Age'"
        log_error "Output: $output"
        return 1
    fi

    log_success "Branch age column header shown"
    return 0
}

# Test that shorthand ages are used (no "ago" in output)
test_list_shorthand_age() {
    local remote_repo=$(create_test_remote "test-repo-list-shorthand" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-shorthand"

    # Run list from the main worktree
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list failed"
        log_error "Output: $output"
        return 1
    }

    # Output should NOT contain verbose "ago" dates
    if echo "$output" | grep -q " ago"; then
        log_error "List output should use shorthand ages, not verbose dates with 'ago'"
        log_error "Output: $output"
        return 1
    fi

    log_success "Shorthand ages used instead of verbose dates"
    return 0
}

# Test that branch_age field exists in JSON output
test_list_json_branch_age() {
    local remote_repo=$(create_test_remote "test-repo-list-json-age" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-json-age"

    # Run list with --json
    cd main
    local output
    output=$(git-worktree-list --json 2>&1)

    # Check for branch_age field
    if ! echo "$output" | grep -q '"branch_age"'; then
        log_error "JSON output should contain 'branch_age' field"
        log_error "Output: $output"
        return 1
    fi

    log_success "JSON output contains branch_age field"
    return 0
}

# Test that Head column shows uncommitted change count
test_list_head_column() {
    local remote_repo=$(create_test_remote "test-repo-list-head" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-head"

    # Create a worktree and make changes
    git-worktree-checkout develop || return 1
    echo "dirty" >> develop/README.md

    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list failed"
        log_error "Output: $output"
        return 1
    }

    # Verify the Head header appears
    if ! echo "$output" | grep -q "Head"; then
        log_error "List header should contain 'Head'"
        log_error "Output: $output"
        return 1
    fi

    # develop should show -N for unstaged changes
    if ! echo "$output" | grep "develop" | grep -q '\-[0-9]'; then
        log_error "Dirty worktree should show -N in Head column"
        log_error "Output: $output"
        return 1
    fi

    log_success "Head column shows file status indicators"
    return 0
}

# Test that Remote column shows ahead/behind vs upstream
test_list_remote_column() {
    local remote_repo=$(create_test_remote "test-repo-list-remote" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-remote"

    # Create a worktree and make a local commit (ahead of remote)
    git-worktree-checkout develop || return 1
    cd develop
    echo "local change" > local.txt
    git add local.txt
    git commit -m "Local commit" >/dev/null 2>&1
    cd ..

    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list failed"
        log_error "Output: $output"
        return 1
    }

    # Verify the Remote header appears
    if ! echo "$output" | grep -q "Remote"; then
        log_error "List header should contain 'Remote'"
        log_error "Output: $output"
        return 1
    fi

    log_success "Remote column shown correctly"
    return 0
}

# Test that path is shown relative to current directory
test_list_relative_path() {
    local remote_repo=$(create_test_remote "test-repo-list-relpath" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-relpath"

    git-worktree-checkout develop || return 1

    # Run from the main worktree — current should show "."
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || {
        log_error "git-worktree-list failed"
        log_error "Output: $output"
        return 1
    }

    # Current worktree path should be "." (relative to itself)
    if ! echo "$output" | grep "main" | grep -q '\.'; then
        log_error "Current worktree path should show '.'"
        log_error "Output: $output"
        return 1
    fi

    # develop should show as a relative path (../develop)
    if ! echo "$output" | grep "develop" | grep -q '\.\.'; then
        log_error "Other worktree path should be relative (contain '..')"
        log_error "Output: $output"
        return 1
    fi

    log_success "Paths shown relative to current directory"
    return 0
}

# Test JSON includes head_changes and remote_ahead/behind fields
test_list_json_head_remote() {
    local remote_repo=$(create_test_remote "test-repo-list-json-hr" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-json-hr"

    cd main
    local output
    output=$(git-worktree-list --json 2>&1)

    # Check for staged/unstaged/untracked fields
    for field in "staged" "unstaged" "untracked"; do
        if ! echo "$output" | grep -q "\"$field\""; then
            log_error "JSON output should contain '$field' field"
            log_error "Output: $output"
            return 1
        fi
    done

    # Check for remote_ahead and remote_behind fields
    if ! echo "$output" | grep -q '"remote_ahead"'; then
        log_error "JSON output should contain 'remote_ahead' field"
        log_error "Output: $output"
        return 1
    fi

    if ! echo "$output" | grep -q '"remote_behind"'; then
        log_error "JSON output should contain 'remote_behind' field"
        log_error "Output: $output"
        return 1
    fi

    # Check for is_default_branch field
    if ! echo "$output" | grep -q '"is_default_branch"'; then
        log_error "JSON output should contain 'is_default_branch' field"
        log_error "Output: $output"
        return 1
    fi

    log_success "JSON output contains head_changes and remote fields"
    return 0
}

# Run all list tests
run_list_tests() {
    log "Running git-worktree-list integration tests..."

    run_test "list_basic" "test_list_basic"
    run_test "list_current_marker" "test_list_current_marker"
    run_test "list_dirty_marker" "test_list_dirty_marker"
    run_test "list_json" "test_list_json"
    run_test "list_detached_head" "test_list_detached_head"
    run_test "list_ahead_behind" "test_list_ahead_behind"
    run_test "list_outside_repo" "test_list_outside_repo"
    run_test "list_help" "test_list_help"
    run_test "list_json_single" "test_list_json_single"
    run_test "list_many_worktrees" "test_list_many_worktrees"
    run_test "list_commit_subject" "test_list_commit_subject"
    run_test "list_from_subdirectory" "test_list_from_subdirectory"
    run_test "list_json_ahead_behind" "test_list_json_ahead_behind"
    run_test "list_branch_age" "test_list_branch_age"
    run_test "list_shorthand_age" "test_list_shorthand_age"
    run_test "list_json_branch_age" "test_list_json_branch_age"
    run_test "list_head_column" "test_list_head_column"
    run_test "list_remote_column" "test_list_remote_column"
    run_test "list_relative_path" "test_list_relative_path"
    run_test "list_json_head_remote" "test_list_json_head_remote"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_list_tests
    print_summary
fi
