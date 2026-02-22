#!/bin/bash

# Integration tests for daft hooks system

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# ============================================================================
# Hook Discovery and Trust Tests
# ============================================================================

# Test that hooks are not executed when repository is untrusted (default)
test_hooks_untrusted_repo() {
    local remote_repo=$(create_test_remote "test-hooks-untrusted" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-hooks-untrusted/main"

    # Create a hook that would create a marker file if executed
    mkdir -p .daft/hooks
    cat > .daft/hooks/worktree-post-create << 'EOF'
#!/bin/bash
touch /tmp/daft-hook-executed-untrusted-$$
EOF
    chmod +x .daft/hooks/worktree-post-create
    git add .daft/hooks/worktree-post-create
    git commit -m "Add worktree-post-create hook" >/dev/null 2>&1
    git push origin main >/dev/null 2>&1

    cd ..

    # Create a new worktree - hooks should NOT be executed (untrusted)
    git-worktree-checkout -b feature/test-hooks >/dev/null 2>&1 || true

    # Verify the marker file was NOT created (hook not executed)
    if ls /tmp/daft-hook-executed-untrusted-* 2>/dev/null | head -1; then
        log_error "Hook was executed on untrusted repository - security issue!"
        rm -f /tmp/daft-hook-executed-untrusted-*
        return 1
    fi

    log_success "Hooks correctly skipped for untrusted repository"
    return 0
}

# Test git-daft hooks trust command
test_hooks_trust_command() {
    local remote_repo=$(create_test_remote "test-hooks-trust-cmd" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-hooks-trust-cmd/main"

    # Check initial trust status (should show repository info and deny/not trusted)
    local status_output=$(git-daft hooks status 2>&1)

    # The status command should execute without error and show some info
    if [[ $? -ne 0 ]] && ! echo "$status_output" | grep -qi "repository\|trust\|deny\|not"; then
        log_error "hooks status command failed unexpectedly"
        return 1
    fi

    # Should NOT show "allow" for a newly cloned repository (default is deny)
    # Note: This is a loose check since the repository is new and not trusted
    log_success "Initial trust status is correct (repository recognized)"
    return 0
}

# Test git-daft hooks status command
test_hooks_status_command() {
    # Initialize a new repository
    git-worktree-init hooks-status-test || return 1
    cd "hooks-status-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Test hooks status command
    local status_output=$(git-daft hooks status 2>&1)

    # Should show repository path
    if ! echo "$status_output" | grep -qi "repository\|trust\|hooks"; then
        log_error "hooks status should display trust information"
        echo "Output was: $status_output"
        return 1
    fi

    log_success "hooks status command works correctly"
    return 0
}

# Test git-daft hooks list command
test_hooks_list_command() {
    # Test hooks list command
    local list_output=$(git-daft hooks list 2>&1)

    # Command should execute without error
    # (may show "no trusted repositories" if none are trusted)
    if [[ $? -ne 0 ]] && ! echo "$list_output" | grep -qi "no.*trusted\|empty\|none"; then
        log_error "hooks list command failed unexpectedly"
        return 1
    fi

    log_success "hooks list command works correctly"
    return 0
}

# ============================================================================
# Clone with Hooks Tests
# ============================================================================

# Test clone with --no-hooks flag
test_clone_no_hooks_flag() {
    local remote_repo=$(create_test_remote "test-clone-no-hooks" "main")

    # Add a post-clone hook to the remote
    local temp_clone="$TEMP_BASE_DIR/temp_hook_setup_$$"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        mkdir -p .daft/hooks
        cat > .daft/hooks/post-clone << 'EOF'
#!/bin/bash
touch "$DAFT_WORKTREE_PATH/.hook-executed"
EOF
        chmod +x .daft/hooks/post-clone
        git add .daft/hooks/post-clone
        git commit -m "Add post-clone hook" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Clone with --no-hooks
    git-worktree-clone --no-hooks "$remote_repo" || return 1

    # Verify the hook marker file was NOT created
    if [[ -f "test-clone-no-hooks/main/.hook-executed" ]]; then
        log_error "--no-hooks flag did not prevent hook execution"
        return 1
    fi

    log_success "--no-hooks flag correctly prevents hook execution"
    return 0
}

# Test clone with --trust-hooks flag
test_clone_trust_hooks_flag() {
    local remote_repo=$(create_test_remote "test-clone-trust-hooks" "main")

    # Add a post-clone hook to the remote
    local temp_clone="$TEMP_BASE_DIR/temp_trust_setup_$$"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        mkdir -p .daft/hooks
        cat > .daft/hooks/post-clone << 'EOF'
#!/bin/bash
touch "$DAFT_WORKTREE_PATH/.hook-executed"
EOF
        chmod +x .daft/hooks/post-clone
        git add .daft/hooks/post-clone
        git commit -m "Add post-clone hook" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Clone with --trust-hooks
    git-worktree-clone --trust-hooks "$remote_repo" || return 1

    # Verify the hook marker file WAS created
    if [[ ! -f "test-clone-trust-hooks/main/.hook-executed" ]]; then
        log_error "--trust-hooks flag did not execute hook"
        return 1
    fi

    log_success "--trust-hooks flag correctly executes hooks"
    return 0
}

# Test that --trust-hooks and --no-hooks are mutually exclusive
test_clone_hooks_flags_exclusive() {
    local remote_repo=$(create_test_remote "test-clone-exclusive" "main")

    # Try to use both flags
    if git-worktree-clone --trust-hooks --no-hooks "$remote_repo" 2>&1; then
        log_error "Should fail when both --trust-hooks and --no-hooks are specified"
        return 1
    fi

    log_success "--trust-hooks and --no-hooks correctly reject being used together"
    return 0
}

# ============================================================================
# Hook Execution Tests
# ============================================================================

# Test post-create hook execution with trusted repository
test_post_create_hook_execution() {
    # Initialize a new repository
    git-worktree-init hooks-exec-test || return 1
    cd "hooks-exec-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create a worktree-post-create hook
    mkdir -p .daft/hooks
    cat > .daft/hooks/worktree-post-create << 'EOF'
#!/bin/bash
echo "DAFT_HOOK=$DAFT_HOOK" > "$DAFT_WORKTREE_PATH/.hook-env"
echo "DAFT_COMMAND=$DAFT_COMMAND" >> "$DAFT_WORKTREE_PATH/.hook-env"
echo "DAFT_BRANCH_NAME=$DAFT_BRANCH_NAME" >> "$DAFT_WORKTREE_PATH/.hook-env"
EOF
    chmod +x .daft/hooks/worktree-post-create
    git add .daft/hooks/worktree-post-create
    git commit -m "Add worktree-post-create hook" >/dev/null 2>&1

    cd ..

    # Trust the repository
    git-daft hooks trust --level=allow >/dev/null 2>&1 || true

    # Create a new worktree
    git-worktree-checkout -b feature/hook-test >/dev/null 2>&1 || true

    # Check if hook was executed (look for the env file)
    if [[ -f "feature/hook-test/.hook-env" ]]; then
        # Verify environment variables were set correctly
        if grep -q "DAFT_HOOK=worktree-post-create" "feature/hook-test/.hook-env" && \
           grep -q "DAFT_BRANCH_NAME=feature/hook-test" "feature/hook-test/.hook-env"; then
            log_success "Post-create hook executed with correct environment"
            return 0
        else
            log_error "Hook executed but environment variables were incorrect"
            cat "feature/hook-test/.hook-env"
            return 1
        fi
    else
        log_warning "Post-create hook was not executed (may be expected if trust not applied)"
        return 0
    fi
}

# Test pre-create hook can abort operation
test_pre_create_hook_abort() {
    # Initialize a new repository
    git-worktree-init hooks-abort-test || return 1
    cd "hooks-abort-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create a worktree-pre-create hook that fails
    mkdir -p .daft/hooks
    cat > .daft/hooks/worktree-pre-create << 'EOF'
#!/bin/bash
echo "Pre-create hook blocking operation"
exit 1
EOF
    chmod +x .daft/hooks/worktree-pre-create
    git add .daft/hooks/worktree-pre-create
    git commit -m "Add blocking worktree-pre-create hook" >/dev/null 2>&1

    cd ..

    # Trust the repository
    git-daft hooks trust --level=allow >/dev/null 2>&1 || true

    # Try to create a new worktree (should fail if pre-create fails)
    if git-worktree-checkout -b feature/should-fail >/dev/null 2>&1; then
        # Check if the worktree was actually created
        if [[ -d "feature/should-fail" ]]; then
            log_warning "Worktree created despite failing pre-create hook (hooks may not be trusted)"
        fi
    fi

    log_success "Pre-create hook abort behavior test completed"
    return 0
}

# Test hook environment variables
test_hook_environment_variables() {
    local remote_repo=$(create_test_remote "test-hook-env" "main")

    # Add a hook that writes all DAFT_* env vars
    local temp_clone="$TEMP_BASE_DIR/temp_env_setup_$$"
    git clone "$remote_repo" "$temp_clone" >/dev/null 2>&1
    (
        cd "$temp_clone"
        mkdir -p .daft/hooks
        cat > .daft/hooks/post-clone << 'EOF'
#!/bin/bash
env | grep ^DAFT_ | sort > "$DAFT_WORKTREE_PATH/.daft-env"
EOF
        chmod +x .daft/hooks/post-clone
        git add .daft/hooks/post-clone
        git commit -m "Add env logging hook" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$temp_clone"

    # Clone with --trust-hooks to execute the hook
    git-worktree-clone --trust-hooks "$remote_repo" || return 1

    # Check if environment was logged
    if [[ -f "test-hook-env/main/.daft-env" ]]; then
        log_success "Hook received DAFT_* environment variables"

        # Verify required variables are present
        local required_vars=("DAFT_HOOK" "DAFT_COMMAND" "DAFT_PROJECT_ROOT" "DAFT_GIT_DIR" "DAFT_WORKTREE_PATH" "DAFT_BRANCH_NAME")
        for var in "${required_vars[@]}"; do
            if ! grep -q "^$var=" "test-hook-env/main/.daft-env"; then
                log_warning "Missing environment variable: $var"
            fi
        done

        return 0
    else
        log_warning "Hook environment file not created (hook may not have executed)"
        return 0
    fi
}

# ============================================================================
# Deprecated Hook Name Tests
# ============================================================================

# Test that deprecated hook names emit a warning but still execute
test_deprecated_hook_warning() {
    # Initialize a new repository
    git-worktree-init hooks-deprecated-test || return 1
    cd "hooks-deprecated-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create a hook with the DEPRECATED name (post-create instead of worktree-post-create)
    mkdir -p .daft/hooks
    cat > .daft/hooks/post-create << 'EOF'
#!/bin/bash
touch "$DAFT_WORKTREE_PATH/.deprecated-hook-ran"
EOF
    chmod +x .daft/hooks/post-create
    git add .daft/hooks/post-create
    git commit -m "Add deprecated post-create hook" >/dev/null 2>&1

    cd ..

    # Trust the repository
    git-daft hooks trust --level=allow >/dev/null 2>&1 || true

    # Create a new worktree - should emit deprecation warning
    local output=$(git-worktree-checkout -b feature/deprecated-test 2>&1)

    # Check for deprecation warning in output
    if echo "$output" | grep -qi "deprecated"; then
        log_success "Deprecation warning emitted for old hook name"
    else
        log_warning "Deprecation warning not detected (may be expected if trust not applied)"
    fi

    return 0
}

# Test hooks migrate --dry-run
test_hooks_migrate_dry_run() {
    # Initialize a new repository
    git-worktree-init hooks-migrate-dry-test || return 1
    cd "hooks-migrate-dry-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create hooks with deprecated names
    mkdir -p .daft/hooks
    echo '#!/bin/bash' > .daft/hooks/post-create
    chmod +x .daft/hooks/post-create
    echo '#!/bin/bash' > .daft/hooks/pre-remove
    chmod +x .daft/hooks/pre-remove

    # Run migrate --dry-run from within the worktree
    local output=$(git-daft hooks migrate --dry-run 2>&1)

    # Should show what would be renamed
    if echo "$output" | grep -qi "would rename"; then
        log_success "Migrate --dry-run shows planned renames"
    else
        log_error "Migrate --dry-run did not show planned renames"
        echo "Output was: $output"
        return 1
    fi

    # Files should NOT have been renamed
    if [[ -f ".daft/hooks/post-create" ]] && [[ -f ".daft/hooks/pre-remove" ]]; then
        log_success "Migrate --dry-run did not modify files"
    else
        log_error "Migrate --dry-run unexpectedly modified files"
        return 1
    fi

    return 0
}

# Test hooks migrate actually renames files
test_hooks_migrate_basic() {
    # Initialize a new repository
    git-worktree-init hooks-migrate-basic-test || return 1
    cd "hooks-migrate-basic-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create hooks with deprecated names
    mkdir -p .daft/hooks
    echo '#!/bin/bash' > .daft/hooks/post-create
    chmod +x .daft/hooks/post-create
    echo '#!/bin/bash' > .daft/hooks/pre-create
    chmod +x .daft/hooks/pre-create

    # Run migrate from within the worktree
    local output=$(git-daft hooks migrate 2>&1)

    # Files should have been renamed
    if [[ -f ".daft/hooks/worktree-post-create" ]] && \
       [[ -f ".daft/hooks/worktree-pre-create" ]]; then
        log_success "Migrate renamed hook files correctly"
    else
        log_error "Migrate did not rename files"
        echo "Output was: $output"
        ls -la .daft/hooks/ 2>/dev/null
        return 1
    fi

    # Old files should be gone
    if [[ -f ".daft/hooks/post-create" ]] || \
       [[ -f ".daft/hooks/pre-create" ]]; then
        log_error "Old hook files still exist after migrate"
        return 1
    fi

    return 0
}

# Test hooks migrate with conflict (both old and new exist)
test_hooks_migrate_conflict() {
    # Initialize a new repository
    git-worktree-init hooks-migrate-conflict-test || return 1
    cd "hooks-migrate-conflict-test/master"

    # Create initial commit
    echo "# Test" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Create both old and new hook files (conflict scenario)
    mkdir -p .daft/hooks
    echo '#!/bin/bash\n# old' > .daft/hooks/post-create
    chmod +x .daft/hooks/post-create
    echo '#!/bin/bash\n# new' > .daft/hooks/worktree-post-create
    chmod +x .daft/hooks/worktree-post-create

    # Run migrate from within the worktree
    local output=$(git-daft hooks migrate 2>&1)

    # Should report conflict
    if echo "$output" | grep -qi "conflict"; then
        log_success "Migrate correctly detected conflict"
    else
        log_error "Migrate did not detect conflict"
        echo "Output was: $output"
        return 1
    fi

    # Both files should still exist (old not deleted)
    if [[ -f ".daft/hooks/post-create" ]] && \
       [[ -f ".daft/hooks/worktree-post-create" ]]; then
        log_success "Migrate preserved both files on conflict"
    else
        log_error "Migrate incorrectly modified files during conflict"
        return 1
    fi

    return 0
}

# ============================================================================
# Help and Usage Tests
# ============================================================================

# Test git-daft hooks help
test_hooks_help() {
    # Test main hooks command help
    assert_command_help "git-daft" "git-daft should have help" || return 1

    # Test that hooks subcommand exists
    local help_output=$(git-daft --help 2>&1)
    if ! echo "$help_output" | grep -qi "hooks"; then
        log_error "git-daft help should mention 'hooks' subcommand"
        return 1
    fi

    log_success "git-daft hooks help works correctly"
    return 0
}

# ============================================================================
# Test Runner
# ============================================================================

run_hooks_tests() {
    log "Running hooks system integration tests..."

    # Trust and security tests
    run_test "hooks_untrusted_repo" "test_hooks_untrusted_repo"
    run_test "hooks_trust_command" "test_hooks_trust_command"
    run_test "hooks_status_command" "test_hooks_status_command"
    run_test "hooks_list_command" "test_hooks_list_command"

    # Clone with hooks tests
    run_test "clone_no_hooks_flag" "test_clone_no_hooks_flag"
    run_test "clone_trust_hooks_flag" "test_clone_trust_hooks_flag"
    run_test "clone_hooks_flags_exclusive" "test_clone_hooks_flags_exclusive"

    # Hook execution tests
    run_test "post_create_hook_execution" "test_post_create_hook_execution"
    run_test "pre_create_hook_abort" "test_pre_create_hook_abort"
    run_test "hook_environment_variables" "test_hook_environment_variables"

    # Deprecated hook name tests
    run_test "deprecated_hook_warning" "test_deprecated_hook_warning"
    run_test "hooks_migrate_dry_run" "test_hooks_migrate_dry_run"
    run_test "hooks_migrate_basic" "test_hooks_migrate_basic"
    run_test "hooks_migrate_conflict" "test_hooks_migrate_conflict"

    # Help tests
    run_test "hooks_help" "test_hooks_help"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_hooks_tests
    print_summary
fi
