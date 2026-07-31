#!/bin/bash

# Integration tests for git-worktree-checkout Rust binary

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

CHECKOUT_PTY_RUN="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pty_run.py"

# Run daft with the live rail enabled (pattern from
# test_exec_verbose_toggle.sh): the framework's DAFT_TESTING hides the
# interactive region — the subject of the rail tests — so it comes off and
# the background spawns it suppressed are turned off individually. TERM is
# pinned because indicatif hides its whole draw target under an unset or
# `dumb` TERM, as on a bare CI runner.
_rail_daft() {
    env -u DAFT_TESTING \
        TERM=xterm-256color \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# Strip ANSI and split the pty's carriage-returned repaints into lines.
_rail_clean() {
    sed -e 's/\x1b\[[0-9;]*[a-zA-Z]//g' -e 's/\r/\n/g' "$1"
}

# Test basic checkout functionality
test_checkout_basic() {
    local remote_repo=$(create_test_remote "test-repo-checkout" "main")
    
    # First clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    
    # Change to the repo directory
    cd "test-repo-checkout"
    
    # Test checkout existing branch
    git-worktree-checkout develop || return 1
    
    # Verify structure
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1
    
    return 0
}

# Test checkout with remote branch
test_checkout_remote_branch() {
    local remote_repo=$(create_test_remote "test-repo-checkout-remote" "main")
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-remote"
    
    # Test checkout remote branch
    git-worktree-checkout feature/test-feature || return 1
    
    # Verify structure
    assert_directory_exists "feature/test-feature" || return 1
    assert_git_worktree "feature/test-feature" "feature/test-feature" || return 1
    
    return 0
}

# Test checkout from subdirectory
test_checkout_from_subdirectory() {
    local remote_repo=$(create_test_remote "test-repo-checkout-subdir" "main")
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-subdir"
    
    # Create a subdirectory and test checkout from there
    mkdir -p "main/subdir"
    cd "main/subdir"
    
    # Test checkout from subdirectory
    git-worktree-checkout develop || return 1
    
    # Verify structure (should be created at repository root)
    assert_directory_exists "../../develop" || return 1
    assert_git_worktree "../../develop" "develop" || return 1
    
    return 0
}

# Test checkout error handling
test_checkout_errors() {
    local remote_repo=$(create_test_remote "test-repo-checkout-errors" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-errors"

    # Test checkout nonexistent branch
    assert_command_failure "git-worktree-checkout nonexistent-branch" "Should fail with nonexistent branch"

    return 0
}

# Test checkout cd to existing worktree
test_checkout_existing_worktree() {
    local remote_repo=$(create_test_remote "test-repo-checkout-existing" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-existing"

    # First checkout creates the worktree
    git-worktree-checkout develop || return 1
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1

    # Go back to main
    cd main

    # Second checkout should succeed and cd to existing worktree
    local output
    output=$(git-worktree-checkout develop 2>&1) || {
        log_error "Second checkout should succeed, but failed"
        echo "$output"
        return 1
    }

    # Verify output contains the expected message (switched to existing worktree)
    if ! echo "$output" | grep -qE "(existing worktree|already has a worktree)"; then
        log_error "Output should mention 'existing worktree' or 'already has a worktree'"
        echo "$output"
        return 1
    fi

    # Verify shell integration writes CD path to temp file when DAFT_CD_FILE is set
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    DAFT_CD_FILE="$cd_file" git-worktree-checkout develop 2>&1 || {
        log_error "Second checkout with DAFT_CD_FILE should succeed, but failed"
        rm -f "$cd_file"
        return 1
    }

    if ! [ -s "$cd_file" ]; then
        log_error "Checkout with DAFT_CD_FILE set should write CD path to temp file"
        rm -f "$cd_file"
        return 1
    fi
    rm -f "$cd_file"

    log_success "Checkout to existing worktree works correctly"
    return 0
}

# Test checkout with direnv integration
test_checkout_direnv() {
    local remote_repo=$(create_test_remote "test-repo-checkout-direnv" "main")
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-direnv"

    # Enable fetch so checkout picks up new remote content
    git config daft.checkout.fetch true

    # Add .envrc to a branch
    local temp_clone="$TEMP_BASE_DIR/temp_envrc_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git checkout develop >/dev/null 2>&1
        echo "export TEST_VAR=develop_value" > .envrc
        git add .envrc >/dev/null 2>&1
        git commit -m "Add .envrc to develop" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Fetch the changes
    git fetch origin >/dev/null 2>&1
    
    # Test checkout with direnv file
    git-worktree-checkout develop || return 1
    
    # Verify structure and direnv file
    assert_directory_exists "develop" || return 1
    assert_file_exists "develop/.envrc" || return 1
    
    return 0
}

# Test checkout outside git repository
test_checkout_outside_repo() {
    # Test checkout command outside git repository
    assert_command_failure "git-worktree-checkout some-branch" "Should fail outside git repository"
    
    return 0
}

# Test checkout help functionality
test_checkout_help() {
    # Test help commands
    assert_command_help "git-worktree-checkout" || return 1
    assert_command_version "git-worktree-checkout" || return 1
    
    return 0
}

# Test checkout with complex branch structures
test_checkout_complex_branches() {
    local remote_repo=$(create_test_remote "test-repo-checkout-complex" "main")
    
    # Add more complex branch structure
    local temp_clone="$TEMP_BASE_DIR/temp_complex_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        # Create nested feature branches
        git checkout -b feature/user-auth >/dev/null 2>&1
        echo "User auth feature" > auth.txt
        git add auth.txt >/dev/null 2>&1
        git commit -m "Add user auth" >/dev/null 2>&1
        git push origin feature/user-auth >/dev/null 2>&1
        
        git checkout -b release/v1.0 >/dev/null 2>&1
        echo "Release v1.0" > release.txt
        git add release.txt >/dev/null 2>&1
        git commit -m "Add release notes" >/dev/null 2>&1
        git push origin release/v1.0 >/dev/null 2>&1
        
        git checkout -b hotfix/critical-bug >/dev/null 2>&1
        echo "Critical bug fix" > hotfix.txt
        git add hotfix.txt >/dev/null 2>&1
        git commit -m "Fix critical bug" >/dev/null 2>&1
        git push origin hotfix/critical-bug >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-complex"
    
    # Test checkout various branch types
    git-worktree-checkout feature/user-auth || return 1
    assert_directory_exists "feature/user-auth" || return 1
    assert_file_exists "feature/user-auth/auth.txt" || return 1
    
    git-worktree-checkout release/v1.0 || return 1
    assert_directory_exists "release/v1.0" || return 1
    assert_file_exists "release/v1.0/release.txt" || return 1
    
    git-worktree-checkout hotfix/critical-bug || return 1
    assert_directory_exists "hotfix/critical-bug" || return 1
    assert_file_exists "hotfix/critical-bug/hotfix.txt" || return 1
    
    return 0
}

# Test checkout performance
test_checkout_performance() {
    local remote_repo=$(create_test_remote "test-repo-checkout-perf" "main")
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-perf"
    
    # Test checkout performance
    local start_time=$(date +%s)
    git-worktree-checkout develop || return 1
    local end_time=$(date +%s)
    local duration=$((end_time - start_time))
    
    if [[ $duration -gt 10 ]]; then
        log_warning "Checkout performance test took ${duration}s (expected < 10s)"
    else
        log_success "Checkout performance test completed in ${duration}s"
    fi
    
    # Verify structure
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1
    
    return 0
}

# Test checkout with large repository
test_checkout_large_repo() {
    local remote_repo=$(create_test_remote "test-repo-checkout-large" "main")
    
    # Add many files to the repository
    local temp_clone="$TEMP_BASE_DIR/temp_large_clone"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        # Create many files on develop branch
        git checkout develop >/dev/null 2>&1
        for i in {1..100}; do
            echo "Large repo test file $i" > "large_file_$i.txt"
        done
        git add . >/dev/null 2>&1
        git commit -m "Add many files to develop" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    rm -rf "$temp_clone"
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-large"
    
    # Test checkout large branch
    git-worktree-checkout develop || return 1
    
    # Verify structure and some files
    assert_directory_exists "develop" || return 1
    assert_file_exists "develop/large_file_1.txt" || return 1
    assert_file_exists "develop/large_file_100.txt" || return 1
    
    return 0
}

# Test checkout with uncommitted changes in current worktree
test_checkout_with_uncommitted_changes() {
    local remote_repo=$(create_test_remote "test-repo-checkout-uncommitted" "main")
    
    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-uncommitted"
    
    # Make uncommitted changes in main worktree
    echo "Uncommitted changes" > "main/uncommitted.txt"
    
    # Test checkout should still work (shouldn't affect other worktrees)
    git-worktree-checkout develop || return 1
    
    # Verify both worktrees exist
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1
    assert_file_exists "main/uncommitted.txt" || return 1
    
    return 0
}

# =============================================================================
# Carry Feature Tests
# =============================================================================

# Test checkout default does NOT carry changes
test_checkout_no_carry_default() {
    local remote_repo=$(create_test_remote "test-repo-checkout-no-carry-default" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-no-carry-default"
    repo_root=$(pwd)

    # Create untracked file in main worktree
    cd main
    echo "should stay in original" > local_file.txt

    # Checkout existing branch (default should NOT carry changes)
    git-worktree-checkout develop || return 1

    cd "$repo_root"

    # Verify file is NOT in new worktree
    assert_file_not_exists "develop/local_file.txt" "File should NOT be carried by default" || return 1

    # Verify file IS still in original worktree
    assert_file_exists "main/local_file.txt" "File should remain in original worktree" || return 1

    return 0
}

# Test checkout --carry flag carries changes
test_checkout_carry_flag() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-flag" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-carry-flag"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "carry me" > carry_file.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify file is in new worktree
    assert_file_exists "carry_file.txt" "File should be carried with --carry flag" || return 1
    assert_file_contains "carry_file.txt" "carry me" "File content should be correct" || return 1

    return 0
}

# Test checkout -c shorthand carries changes
test_checkout_carry_shorthand() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-shorthand" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-carry-shorthand"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "shorthand content" > shorthand_file.txt

    # Checkout with -c shorthand
    git-worktree-checkout -c develop || return 1

    cd "$repo_root/develop"

    # Verify file is in new worktree
    assert_file_exists "shorthand_file.txt" "File should be carried with -c shorthand" || return 1

    return 0
}

# Test checkout --no-carry explicit (same as default)
test_checkout_no_carry_explicit() {
    local remote_repo=$(create_test_remote "test-repo-checkout-no-carry-explicit" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-no-carry-explicit"
    repo_root=$(pwd)

    # Create file in main worktree
    cd main
    echo "explicit no carry" > explicit_file.txt

    # Checkout with explicit --no-carry flag
    git-worktree-checkout --no-carry develop || return 1

    cd "$repo_root"

    # Verify file is NOT in new worktree
    assert_file_not_exists "develop/explicit_file.txt" "File should NOT be carried with --no-carry" || return 1

    # Verify file IS still in original worktree
    assert_file_exists "main/explicit_file.txt" "File should remain in original worktree" || return 1

    return 0
}

# Test checkout --carry with staged changes
test_checkout_carry_staged() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-staged" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-carry-staged"
    repo_root=$(pwd)

    # Create and stage a file in main worktree
    cd main
    echo "staged content" > staged_file.txt
    git add staged_file.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify staged file is in new worktree
    assert_file_exists "staged_file.txt" "Staged file should be carried" || return 1

    return 0
}

# Test checkout --carry with untracked files
test_checkout_carry_untracked() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-untracked" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-carry-untracked"
    repo_root=$(pwd)

    # Create untracked file in main worktree
    cd main
    echo "untracked content" > untracked_file.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify untracked file is in new worktree
    assert_file_exists "untracked_file.txt" "Untracked file should be carried" || return 1

    return 0
}

# Test checkout --carry with mixed changes
test_checkout_carry_mixed() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-mixed" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-carry-mixed"
    repo_root=$(pwd)

    # Create mixed changes in main worktree
    cd main
    echo "staged" > staged.txt
    git add staged.txt
    echo "unstaged modification" >> README.md
    echo "untracked" > untracked.txt

    # Checkout with --carry flag
    git-worktree-checkout --carry develop || return 1

    cd "$repo_root/develop"

    # Verify all changes are in new worktree
    assert_file_exists "staged.txt" "Staged file should be carried" || return 1
    assert_file_contains "README.md" "unstaged modification" "Unstaged modification should be carried" || return 1
    assert_file_exists "untracked.txt" "Untracked file should be carried" || return 1

    return 0
}

# Test checkout with no uncommitted changes works normally
test_checkout_carry_no_changes() {
    local remote_repo=$(create_test_remote "test-repo-checkout-carry-clean" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-carry-clean"
    repo_root=$(pwd)

    # No changes - just checkout
    git-worktree-checkout develop || return 1

    cd "$repo_root"

    # Verify worktree was created successfully
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1

    return 0
}

# Test checkout help shows carry flags
test_checkout_carry_help() {
    # Verify --carry and --no-carry appear in help
    local help_output
    help_output=$(git-worktree-checkout --help 2>&1)

    if echo "$help_output" | grep -q "\-\-carry"; then
        log_success "--carry flag appears in help"
    else
        log_error "--carry flag missing from help output"
        return 1
    fi

    if echo "$help_output" | grep -q "\-\-no-carry"; then
        log_success "--no-carry flag appears in help"
    else
        log_error "--no-carry flag missing from help output"
        return 1
    fi

    if echo "$help_output" | grep -q "\-c"; then
        log_success "-c shorthand appears in help"
    else
        log_error "-c shorthand missing from help output"
        return 1
    fi

    return 0
}

# =============================================================================
# Go Auto-Start Feature Tests
# =============================================================================

# Test improved error message for nonexistent branch
test_checkout_error_message() {
    local remote_repo=$(create_test_remote "test-repo-checkout-error-msg" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-error-msg"

    # Try to checkout a nonexistent branch and capture stderr
    local output
    output=$(git-worktree-checkout nonexistent-branch 2>&1) && {
        log_error "Checkout of nonexistent branch should fail"
        return 1
    }

    # Verify Section 1: Diagnosis — error message mentions "not found"
    if ! echo "$output" | grep -q "not found"; then
        log_error "Error output should contain 'not found'"
        echo "$output"
        return 1
    fi
    log_success "Error output contains 'not found' diagnosis"

    # Verify Section 2: Start suggestion
    if ! echo "$output" | grep -q "daft go --start"; then
        log_error "Error output should contain 'daft go --start' suggestion"
        echo "$output"
        return 1
    fi
    log_success "Error output contains 'daft go --start' suggestion"

    return 0
}

# Test --start flag creates worktree for nonexistent branch
test_checkout_start_flag() {
    local remote_repo=$(create_test_remote "test-repo-checkout-start-flag" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-start-flag"

    # Use --start to create a worktree for a branch that does not exist
    git-worktree-checkout --start new-feature-branch || {
        log_error "--start should create a new worktree for nonexistent branch"
        return 1
    }

    # Verify worktree was created
    assert_directory_exists "new-feature-branch" || return 1
    assert_git_worktree "new-feature-branch" "new-feature-branch" || return 1

    log_success "--start flag creates worktree for nonexistent branch"
    return 0
}

# Test -s shorthand works the same as --start
test_checkout_start_shorthand() {
    local remote_repo=$(create_test_remote "test-repo-checkout-start-short" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-start-short"

    # Use -s shorthand to create a worktree for a branch that does not exist
    git-worktree-checkout -s another-new-branch || {
        log_error "-s shorthand should create a new worktree for nonexistent branch"
        return 1
    }

    # Verify worktree was created
    assert_directory_exists "another-new-branch" || return 1
    assert_git_worktree "another-new-branch" "another-new-branch" || return 1

    log_success "-s shorthand works the same as --start"
    return 0
}

# Test --start with existing remote branch just checks it out normally
test_checkout_start_existing_branch() {
    local remote_repo=$(create_test_remote "test-repo-checkout-start-existing" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-start-existing"

    # Use --start with a branch that already exists on the remote (develop)
    git-worktree-checkout --start develop || {
        log_error "--start should work normally when branch already exists"
        return 1
    }

    # Verify worktree was created for the existing branch
    assert_directory_exists "develop" || return 1
    assert_git_worktree "develop" "develop" || return 1

    log_success "--start with existing branch checks it out normally"
    return 0
}

# Test daft.go.autoStart config auto-creates worktrees
test_checkout_auto_start_config() {
    local remote_repo=$(create_test_remote "test-repo-checkout-autostart" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-autostart"

    # Set the auto-start config in the local git config
    git config daft.go.autoStart true

    # Try to checkout a nonexistent branch — should auto-create
    git-worktree-checkout auto-created-branch || {
        log_error "With daft.go.autoStart=true, checkout should auto-create worktree"
        return 1
    }

    # Verify worktree was created
    assert_directory_exists "auto-created-branch" || return 1
    assert_git_worktree "auto-created-branch" "auto-created-branch" || return 1

    log_success "daft.go.autoStart config auto-creates worktrees"
    return 0
}

# Test fuzzy suggestions in error message
test_checkout_fuzzy_suggestions() {
    local remote_repo=$(create_test_remote "test-repo-checkout-fuzzy" "main")

    # Clone the repository — the remote already has "develop" branch
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-fuzzy"

    # Try a typo of "develop" and capture stderr
    local output
    output=$(git-worktree-checkout "develp" 2>&1) && {
        log_error "Checkout of mistyped branch should fail"
        return 1
    }

    # Verify the error contains fuzzy suggestions
    if ! echo "$output" | grep -q "Did you mean"; then
        log_error "Error output should contain 'Did you mean' fuzzy suggestion"
        echo "$output"
        return 1
    fi
    log_success "Error output contains 'Did you mean' fuzzy suggestion"

    # Verify the suggestion includes "develop"
    if ! echo "$output" | grep -q "develop"; then
        log_error "Fuzzy suggestion should include 'develop'"
        echo "$output"
        return 1
    fi
    log_success "Fuzzy suggestion includes 'develop'"

    return 0
}

# =============================================================================
# Go Dash (Previous Worktree) Tests
# =============================================================================

# Test `daft go -` when no previous worktree exists
test_checkout_dash_no_previous() {
    local remote_repo=$(create_test_remote "test-repo-checkout-dash-noprev" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-dash-noprev"

    # Try `go -` with no previous — should fail
    local output
    output=$(git-worktree-checkout -- - 2>&1) && {
        log_error "go - should fail when no previous worktree exists"
        return 1
    }

    if ! echo "$output" | grep -q "No previous worktree"; then
        log_error "Error output should contain 'No previous worktree'"
        echo "$output"
        return 1
    fi

    log_success "go - correctly errors when no previous worktree exists"
    return 0
}

# Test `daft go -` toggles between two worktrees
test_checkout_dash_toggle() {
    local remote_repo=$(create_test_remote "test-repo-checkout-dash-toggle" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-dash-toggle"
    local repo_root=$(pwd)

    # Go from main to develop (this saves main as previous)
    cd main
    git-worktree-checkout develop || return 1

    # Now go - should take us back to main
    cd "$repo_root/develop"
    local cd_file
    cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-cd-test.XXXXXX")
    local output
    output=$(DAFT_CD_FILE="$cd_file" git-worktree-checkout -- - 2>&1) || {
        log_error "go - should succeed after going to develop"
        echo "$output"
        rm -f "$cd_file"
        return 1
    }

    # Verify DAFT_CD_FILE points to main worktree
    local cd_target
    cd_target=$(cat "$cd_file")
    if ! echo "$cd_target" | grep -q "main"; then
        log_error "go - should navigate to main worktree, got: $cd_target"
        rm -f "$cd_file"
        return 1
    fi
    log_success "go - navigated back to main"

    # Now go - again should take us back to develop
    cd "$repo_root/main"
    output=$(DAFT_CD_FILE="$cd_file" git-worktree-checkout -- - 2>&1) || {
        log_error "go - should succeed after going back to main"
        echo "$output"
        rm -f "$cd_file"
        return 1
    }

    cd_target=$(cat "$cd_file")
    if ! echo "$cd_target" | grep -q "develop"; then
        log_error "go - should navigate to develop worktree, got: $cd_target"
        rm -f "$cd_file"
        return 1
    fi
    rm -f "$cd_file"

    log_success "go - toggles correctly between worktrees"
    return 0
}

# Test `daft go -` when previous worktree was deleted
test_checkout_dash_deleted_previous() {
    local remote_repo=$(create_test_remote "test-repo-checkout-dash-deleted" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-dash-deleted"
    local repo_root=$(pwd)

    # Go from main to develop to establish a previous
    cd main
    git-worktree-checkout develop || return 1

    # Remove the main worktree
    cd "$repo_root"
    git -C develop worktree remove --force "$repo_root/main" 2>/dev/null || rm -rf "$repo_root/main"

    # Now go - should fail because main worktree is gone
    cd develop
    local output
    output=$(git-worktree-checkout -- - 2>&1) && {
        log_error "go - should fail when previous worktree was deleted"
        return 1
    }

    if ! echo "$output" | grep -q "no longer exists"; then
        log_error "Error output should contain 'no longer exists'"
        echo "$output"
        return 1
    fi

    log_success "go - correctly errors when previous worktree was deleted"
    return 0
}

# Test `daft go - -b` (dash with create-branch) is rejected
test_checkout_dash_with_create_branch() {
    local remote_repo=$(create_test_remote "test-repo-checkout-dash-create" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-checkout-dash-create"

    # Try `go -b -` — should fail
    local output
    output=$(git-worktree-checkout -b -- - 2>&1) && {
        log_error "go -b - should fail"
        return 1
    }

    if ! echo "$output" | grep -q "Cannot use '-' with -b"; then
        log_error "Error output should contain \"Cannot use '-' with -b\""
        echo "$output"
        return 1
    fi

    log_success "go -b - correctly rejected"
    return 0
}

# #782: with daft.checkout.fetch=true, a cross-repo catalog hop must not
# commit a worktree-creation plan — resolution runs under the collapsed
# planning face and dissolves without a receipt. The rail only exists on a
# TTY, so this drives daft under the DSR-answering PTY (pty_run.py); the
# YAML suite runs with DAFT_TESTING set and never materializes a region.
test_go_fetch_hop_no_rail_receipt() {
    local remote_a=$(create_test_remote "test-repo-hop-a" "main")
    local remote_b=$(create_test_remote "test-repo-hop-b" "main")
    git-worktree-clone --layout contained "$remote_a" || return 1
    git-worktree-clone --layout contained "$remote_b" || return 1
    cd "test-repo-hop-a/main"
    git config daft.checkout.fetch true

    local log="$PWD/go-hop-rail.log"
    _rail_daft "$CHECKOUT_PTY_RUN" "$log" daft go test-repo-hop-b || return 1

    local clean
    clean=$(_rail_clean "$log")
    if echo "$clean" | grep -q "Failed after"; then
        log_error "hop closed a Failed rail receipt"
        return 1
    fi
    if echo "$clean" | grep -q "┌"; then
        log_error "hop committed a worktree-creation plan"
        return 1
    fi
    # The collapsed line is the region's *only* live line: the rail's shell is
    # built on a hidden draw target and attaches when a plan lands. Drop that
    # hidden() and the detached spacer/footer paint themselves beside the face
    # (`│`, `└  1ms`) — invisible to the InMemoryTerm unit tests, so this is
    # the only guard that catches it.
    if echo "$clean" | grep -q "[│└]"; then
        log_error "detached rail shell painted beside the collapsed line"
        return 1
    fi
    if echo "$clean" | grep -q "Failed to fetch"; then
        log_error "probe fetch warning leaked onto the hop"
        return 1
    fi
    if ! echo "$clean" | grep -q "Opening repository 'test-repo-hop-b'"; then
        log_error "hop line missing from the PTY output"
        return 1
    fi
    return 0
}

# #811: a real terminal Ctrl-C during `-x` must still leave the shell in the
# new worktree. Two things have to hold and only a pty can test either:
#
#   1. daft writes the cd target *before* the exec sequence, so the signal
#      cannot arrive between the command and the write; and
#   2. daft *exits* on SIGINT (130) instead of dying from it (pty_run reports
#      a signal death as 254, Python's -2). This is the load-bearing half:
#      bash and zsh abandon the enclosing function when a foreground child is
#      signal-killed, so a signal-killed daft never reaches `__daft_wrapper`'s
#      `cd` and the target it wrote is read by nobody.
#
# `--ctty` makes daft a session leader owning the pty, so writing \x03 raises
# SIGINT in the foreground process group exactly as a keyboard Ctrl-C does —
# the group-wide delivery a `kill -INT <pid>` cannot reproduce. The cue is
# emitted by the -x command itself: daft's own "Executing" step never reaches
# the pty under the rail, and pty_run splits a cue on its first colon, so a
# self-authored colon-free marker is the only reliable trigger.
#
# The cue must be ASSEMBLED AT RUNTIME, never a literal in the command text.
# Since #812 the rail plans each `-x` command as a row labelled with the
# command exactly as typed, so a literal marker is painted on screen while the
# row is still pending — the Ctrl-C then lands during planning, before the
# worktree exists, and the test interrupts the wrong thing while still
# reporting 130. `printf 'XCUE%s\n' READY` keeps `XCUEREADY` out of the label
# and puts it only in the output the command produces when it actually runs.
_X_CUE_CMD="printf 'XCUE%s\n' READY; sleep 10"
_assert_interrupted_go_kept_cd() {
    local branch="$1" log="$2" cd_file="$3" status="$4"

    if [ "$status" != "130" ]; then
        log_error "expected exit 130 (interrupted, exited cleanly), got $status"
        # 254 is pty_run relaying Python's -2: daft died *from* SIGINT, which
        # is precisely the state that strands the shell.
        [ "$status" = "254" ] && log_error "daft was signal-killed — the wrapper would abandon its cd"
        return 1
    fi
    if ! grep -q "XCUEREADY" "$log"; then
        log_error "-x never started; the interrupt proved nothing"
        return 1
    fi
    assert_directory_exists "../$branch" || return 1
    if [ ! -s "$cd_file" ]; then
        log_error "DAFT_CD_FILE empty after interrupt — the shell would stay put"
        return 1
    fi
    local want got
    want=$(cd "../$branch" && pwd -P)
    got=$(cd "$(cat "$cd_file")" && pwd -P)
    if [ "$got" != "$want" ]; then
        log_error "cd target '$got' != '$want'"
        return 1
    fi
    return 0
}

test_go_exec_interrupt_keeps_cd() {
    local remote_repo=$(create_test_remote "test-repo-x-interrupt" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-x-interrupt/main"

    local log="$PWD/go-x-interrupt.log"
    local cd_file="$PWD/go-x-interrupt.cd"
    : > "$cd_file"
    local status=0
    DAFT_CD_FILE="$cd_file" _rail_daft "$CHECKOUT_PTY_RUN" --ctty \
        --send-after 'XCUEREADY:\x03' "$log" \
        daft go develop -x "$_X_CUE_CMD" || status=$?

    _assert_interrupted_go_kept_cd develop "$log" "$cd_file" "$status"
}

# The same guarantee without a rail. Before #811's interrupt arming this was
# the case that failed even with the cd target written early: with no live
# timeline region nothing installed a SIGINT handler, daft died from the
# signal, and the wrapper abandoned its `cd`. Keep both — the rail path can
# pass on the timeline's handler alone and hide a regression here.
test_go_exec_interrupt_quiet_keeps_cd() {
    local remote_repo=$(create_test_remote "test-repo-x-interrupt-q" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-x-interrupt-q/main"

    local log="$PWD/go-x-interrupt-q.log"
    local cd_file="$PWD/go-x-interrupt-q.cd"
    : > "$cd_file"
    local status=0
    DAFT_CD_FILE="$cd_file" _rail_daft "$CHECKOUT_PTY_RUN" --ctty \
        --send-after 'XCUEREADY:\x03' "$log" \
        daft go develop -q -x "$_X_CUE_CMD" || status=$?

    _assert_interrupted_go_kept_cd develop "$log" "$cd_file" "$status"
}

# The counterpart guard: when resolution does warrant a worktree, the
# collapsed face must expand into the full rail — persisted header, the
# probe fetch as a pre-completed receipt row, and a Ready footer.
test_go_fetch_on_rail_expands() {
    local remote_repo=$(create_test_remote "test-repo-rail-expand" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-rail-expand/main"
    git config daft.checkout.fetch true

    local log="$PWD/go-rail-expand.log"
    _rail_daft "$CHECKOUT_PTY_RUN" "$log" daft go develop || return 1

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "┌  Opening develop"; then
        log_error "expanded rail header missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "✓  Fetched remote"; then
        log_error "pre-completed fetch receipt row missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready in"; then
        log_error "Ready footer missing"
        return 1
    fi
    assert_directory_exists "../develop" || return 1
    return 0
}

# #813: `daft go <full-sha>` opens a sandbox whose directory is a 12-hex
# prefix of the commit, but the sandbox rail's header was seeded with the
# spelling — forty hex characters in the slot that carries identity. A
# sandbox visit never commits a plan for the *branch* reading, so nothing
# downstream corrects it: this seed is the whole sandbox run.
#
# Scoped to the sandbox rail on purpose. `daft go <sha>` first tries the
# branch reading, and that attempt's collapsed face legitimately shows the
# spelling — daft is trying to open a branch of that name and has not probed
# yet. The "Branch '<sha>' not found; opening a detached sandbox" line
# between the two is what marks the handover.
test_go_sandbox_header_names_dirname() {
    local remote_repo=$(create_test_remote "test-repo-sandbox-hdr" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-sandbox-hdr/main"

    local full dirname
    full=$(git rev-parse HEAD)
    # `sandbox::derived_dirname` — DERIVED_DIRNAME_HEX = 12.
    dirname="${full:0:12}"

    local log="$PWD/go-sandbox-hdr.log"
    _rail_daft "$CHECKOUT_PTY_RUN" "$log" daft go "$full" --no-cd || return 1

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "┌  Opening $dirname"; then
        log_error "sandbox rail header did not name the sandbox '$dirname'"
        echo "$clean" | head -20
        return 1
    fi
    if echo "$clean" | grep -q "┌  Opening $full"; then
        log_error "sandbox rail header still carries the full spelling"
        return 1
    fi
    assert_directory_exists "../$dirname" || return 1
    return 0
}

# #812: the `-x` sequence is planned onto the creation rail and runs inside
# the region's lifetime. None of that is reachable from the YAML suite —
# `commit_plan` early-returns off Interactive and the runner captures stderr,
# so every scenario there exercises the off-rail `Executing:` record instead.
# These are the only tests that execute `run_on_rail` itself: row resolution
# against the committed plan, the suspend/redraw seam around a child that owns
# the terminal, and the footer's verdict.
#
# SHELL is pinned to one daft does not alias-capture for (`ShellKind::from_path`
# knows only bash/zsh, and returns None otherwise, short-circuiting the whole
# snapshot path). The snapshot lives under `dirs::cache_dir()`, which no
# DAFT_*_DIR override redirects — capturing here would write the developer's
# real cache directory.
_exec_rail_daft() {
    _rail_daft SHELL=/bin/sh "$@"
}

# Two commands: an `exec` anchor, one row each, labelled as typed, and the
# commands' own output above a Ready footer.
test_start_exec_rows_on_rail() {
    local remote_repo=$(create_test_remote "test-repo-exec-rail" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-exec-rail/main" || return 1

    local log="$TEMP_BASE_DIR/exec-rail.log"
    _exec_rail_daft "$CHECKOUT_PTY_RUN" "$log" \
        daft start feat-exec -x 'echo first-command' -x 'echo second-command' || return 1

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "├─ exec"; then
        log_error "exec section anchor missing from the rail"
        return 1
    fi
    if ! echo "$clean" | grep -q "echo first-command"; then
        log_error "first -x row missing (label is the command as typed)"
        return 1
    fi
    if ! echo "$clean" | grep -q "echo second-command"; then
        log_error "second -x row missing — identical rows must not collapse"
        return 1
    fi
    # The commands really ran, not just got planned.
    if ! echo "$clean" | grep -q "^first-command"; then
        log_error "first command's own output missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "^second-command"; then
        log_error "second command's own output missing"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready in"; then
        log_error "Ready footer missing after a clean sequence"
        return 1
    fi
    assert_directory_exists "../feat-exec" || return 1
    return 0
}

# A failing `-x` fails its own row, marks the rest not-run, and still lands a
# worktree — the footer says so rather than claiming the creation failed.
test_start_exec_failure_on_rail() {
    local remote_repo=$(create_test_remote "test-repo-exec-fail" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-exec-fail/main" || return 1

    local log="$TEMP_BASE_DIR/exec-fail.log"
    # No `|| return 1`: a failing `-x` propagates its status, which is the
    # behavior under test. pty_run.py exits with the child's status.
    _exec_rail_daft "$CHECKOUT_PTY_RUN" "$log" \
        daft start feat-broken -x 'echo ran-first' -x 'false' -x 'echo never-runs'

    local clean
    clean=$(_rail_clean "$log")
    if ! echo "$clean" | grep -q "exit 1"; then
        log_error "failing row missing its exit status"
        return 1
    fi
    if ! echo "$clean" | grep -q "not run"; then
        log_error "commands after the failure must resolve as not run, not vanish"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready with failures in"; then
        log_error "footer must report failures without claiming creation failed"
        return 1
    fi
    if echo "$clean" | grep -q "^never-runs"; then
        log_error "the sequence continued past a failure"
        return 1
    fi
    # The worktree is the point: a failed command must not undo it.
    assert_directory_exists "../feat-broken" || return 1
    return 0
}

# A pasted multi-line command is ordinary input for `-x`. Its label is
# flattened to one line so the region's line accounting and its shared
# annotation column both survive it (#751's failure mode, different door).
test_start_exec_multiline_label_stays_one_row() {
    local remote_repo=$(create_test_remote "test-repo-exec-multiline" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-exec-multiline/main" || return 1

    local log="$TEMP_BASE_DIR/exec-multiline.log"
    _exec_rail_daft "$CHECKOUT_PTY_RUN" "$log" \
        daft start feat-multiline -x $'echo alpha\necho beta' || return 1

    local clean
    clean=$(_rail_clean "$log")
    # Both halves of the label on one line, in order.
    if ! echo "$clean" | grep -q "echo alpha echo beta"; then
        log_error "multi-line -x label was not flattened onto one row"
        return 1
    fi
    # One command is one row, with no section to anchor.
    if echo "$clean" | grep -q "├─ exec"; then
        log_error "a lone -x command must not get a section anchor"
        return 1
    fi
    if ! echo "$clean" | grep -q "Ready in"; then
        log_error "rail did not close cleanly after a multi-line label"
        return 1
    fi
    assert_directory_exists "../feat-multiline" || return 1
    return 0
}

# Run all checkout tests
run_checkout_tests() {
    log "Running git-worktree-checkout integration tests..."

    run_test "checkout_basic" "test_checkout_basic"
    run_test "checkout_remote_branch" "test_checkout_remote_branch"
    run_test "checkout_from_subdirectory" "test_checkout_from_subdirectory"
    run_test "checkout_errors" "test_checkout_errors"
    run_test "checkout_existing_worktree" "test_checkout_existing_worktree"
    run_test "checkout_direnv" "test_checkout_direnv"
    run_test "checkout_outside_repo" "test_checkout_outside_repo"
    run_test "checkout_help" "test_checkout_help"
    run_test "checkout_complex_branches" "test_checkout_complex_branches"
    run_test "checkout_performance" "test_checkout_performance"
    run_test "checkout_large_repo" "test_checkout_large_repo"
    run_test "checkout_with_uncommitted_changes" "test_checkout_with_uncommitted_changes"

    # Carry feature tests
    run_test "checkout_no_carry_default" "test_checkout_no_carry_default"
    run_test "checkout_carry_flag" "test_checkout_carry_flag"
    run_test "checkout_carry_shorthand" "test_checkout_carry_shorthand"
    run_test "checkout_no_carry_explicit" "test_checkout_no_carry_explicit"
    run_test "checkout_carry_staged" "test_checkout_carry_staged"
    run_test "checkout_carry_untracked" "test_checkout_carry_untracked"
    run_test "checkout_carry_mixed" "test_checkout_carry_mixed"
    run_test "checkout_carry_no_changes" "test_checkout_carry_no_changes"
    run_test "checkout_carry_help" "test_checkout_carry_help"

    # Go auto-start feature tests
    run_test "checkout_error_message" "test_checkout_error_message"
    run_test "checkout_start_flag" "test_checkout_start_flag"
    run_test "checkout_start_shorthand" "test_checkout_start_shorthand"
    run_test "checkout_start_existing_branch" "test_checkout_start_existing_branch"
    run_test "checkout_auto_start_config" "test_checkout_auto_start_config"
    run_test "checkout_fuzzy_suggestions" "test_checkout_fuzzy_suggestions"

    # Go dash (previous worktree) tests
    run_test "checkout_dash_no_previous" "test_checkout_dash_no_previous"
    run_test "checkout_dash_toggle" "test_checkout_dash_toggle"
    run_test "checkout_dash_deleted_previous" "test_checkout_dash_deleted_previous"
    run_test "checkout_dash_with_create_branch" "test_checkout_dash_with_create_branch"

    # Rail behavior under a PTY (#782)
    run_test "go_fetch_hop_no_rail_receipt" "test_go_fetch_hop_no_rail_receipt"
    run_test "go_fetch_on_rail_expands" "test_go_fetch_on_rail_expands"

    # Rail header names the sandbox, not the spelling (#813)
    run_test "go_sandbox_header_names_dirname" "test_go_sandbox_header_names_dirname"

    # `-x` rows on the creation rail under a PTY (#812)
    run_test "start_exec_rows_on_rail" "test_start_exec_rows_on_rail"
    run_test "start_exec_failure_on_rail" "test_start_exec_failure_on_rail"
    run_test "start_exec_multiline_label_stays_one_row" "test_start_exec_multiline_label_stays_one_row"

    # Interrupted -x keeps the cd redirect (#811)
    run_test "go_exec_interrupt_keeps_cd" "test_go_exec_interrupt_keeps_cd"
    run_test "go_exec_interrupt_quiet_keeps_cd" "test_go_exec_interrupt_quiet_keeps_cd"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_checkout_tests
    print_summary
fi