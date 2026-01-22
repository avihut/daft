#!/bin/bash

# Integration tests for git config-based settings

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# =============================================================================
# Config Tests
# =============================================================================

# Test that default settings work when no config is set
test_config_defaults() {
    local remote_repo=$(create_test_remote "test-repo-config-defaults" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-defaults"

    # Verify default behavior works (remote=origin, push enabled, etc.)
    git-worktree-checkout-branch feature/test-defaults || return 1

    # Verify the worktree was created
    assert_directory_exists "feature/test-defaults" || return 1

    # Verify the branch was pushed (default behavior)
    cd "feature/test-defaults"
    local remote_branch
    remote_branch=$(git ls-remote --heads origin feature/test-defaults 2>/dev/null)
    if [[ -z "$remote_branch" ]]; then
        log_error "Branch was not pushed to remote (expected by default)"
        return 1
    fi
    log_success "Branch was pushed to remote (default behavior)"

    return 0
}

# Test daft.checkout.push=false disables push
test_config_checkout_push_false() {
    local remote_repo=$(create_test_remote "test-repo-config-push-false" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-push-false"

    # Set local config to disable push
    git config daft.checkout.push false

    # Create a new branch
    git-worktree-checkout-branch feature/no-push || return 1

    # Verify the worktree was created
    assert_directory_exists "feature/no-push" || return 1

    # Verify the branch was NOT pushed
    cd "feature/no-push"
    local remote_branch
    remote_branch=$(git ls-remote --heads origin feature/no-push 2>/dev/null)
    if [[ -n "$remote_branch" ]]; then
        log_error "Branch was pushed to remote (should be disabled)"
        return 1
    fi
    log_success "Branch was not pushed (push disabled in config)"

    return 0
}

# Test daft.remote changes default remote
test_config_remote_custom() {
    local remote_repo=$(create_test_remote "test-repo-config-remote" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-remote"

    # Add a second remote called "upstream"
    git remote add upstream "$remote_repo"

    # Set local config to use upstream
    git config daft.remote upstream

    # Create a new branch (should push to upstream, not origin)
    cd main
    git-worktree-checkout-branch feature/custom-remote || return 1

    # Verify the worktree was created
    cd ..
    assert_directory_exists "feature/custom-remote" || return 1

    # Verify the branch was pushed to upstream
    cd "feature/custom-remote"
    local upstream_branch
    upstream_branch=$(git ls-remote --heads upstream feature/custom-remote 2>/dev/null)
    if [[ -z "$upstream_branch" ]]; then
        log_error "Branch was not pushed to upstream remote"
        return 1
    fi
    log_success "Branch was pushed to upstream (custom remote in config)"

    return 0
}

# Test daft.checkoutBranch.carry=false disables carry by default
test_config_checkout_branch_carry_false() {
    local remote_repo=$(create_test_remote "test-repo-config-carry-false" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-carry-false"
    repo_root=$(pwd)

    # Set local config to disable carry for checkout-branch
    git config daft.checkoutBranch.carry false

    # Create uncommitted changes
    cd main
    echo "uncommitted content" > uncommitted.txt

    # Create a new branch (should NOT carry changes due to config)
    git-worktree-checkout-branch feature/no-carry-config || return 1

    cd "$repo_root"

    # Verify the file is NOT in new worktree
    assert_file_not_exists "feature/no-carry-config/uncommitted.txt" "File should NOT be carried when carry disabled in config" || return 1

    # Verify the file IS still in original worktree
    assert_file_exists "main/uncommitted.txt" "File should remain in original worktree" || return 1

    return 0
}

# Test daft.checkout.carry=true enables carry by default for checkout
test_config_checkout_carry_true() {
    local remote_repo=$(create_test_remote "test-repo-config-checkout-carry" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-checkout-carry"
    repo_root=$(pwd)

    # Set local config to enable carry for checkout
    git config daft.checkout.carry true

    # Create develop branch first
    cd main
    git checkout -b develop
    git push origin develop
    cd ..
    git-worktree-checkout develop || return 1

    # Now create uncommitted changes in develop worktree
    cd develop
    echo "uncommitted content" > uncommitted.txt

    # Go back to main and check out develop (should carry changes due to config)
    cd "$repo_root/main"
    echo "changes in main" > main_changes.txt

    # Create a new remote branch to checkout
    (
        cd "$repo_root/main"
        git checkout -b feature/test-checkout-carry
        git push origin feature/test-checkout-carry
        git checkout main
    ) >/dev/null 2>&1

    # Checkout existing branch (should carry changes due to config)
    git-worktree-checkout feature/test-checkout-carry || return 1

    cd "$repo_root"

    # Verify the file is in new worktree (carry enabled in config)
    assert_file_exists "feature/test-checkout-carry/main_changes.txt" "File should be carried when carry enabled in config" || return 1

    return 0
}

# Test that explicit --carry flag overrides config
test_config_flag_overrides_carry_false() {
    local remote_repo=$(create_test_remote "test-repo-config-override-carry" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-override-carry"
    repo_root=$(pwd)

    # Set local config to disable carry
    git config daft.checkoutBranch.carry false

    # Create uncommitted changes
    cd main
    echo "override content" > override.txt

    # Create a new branch with explicit --carry flag (should override config)
    git-worktree-checkout-branch --carry feature/override-carry || return 1

    cd "$repo_root"

    # Verify the file IS in new worktree (--carry overrides config)
    assert_file_exists "feature/override-carry/override.txt" "File should be carried when --carry flag is used" || return 1

    return 0
}

# Test that explicit --no-carry flag overrides config
test_config_flag_overrides_carry_true() {
    local remote_repo=$(create_test_remote "test-repo-config-override-no-carry" "main")
    local repo_root

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-override-no-carry"
    repo_root=$(pwd)

    # Config already has carry=true by default, but let's be explicit
    git config daft.checkoutBranch.carry true

    # Create uncommitted changes
    cd main
    echo "no-carry content" > no_carry.txt

    # Create a new branch with explicit --no-carry flag (should override config)
    git-worktree-checkout-branch --no-carry feature/no-carry-override || return 1

    cd "$repo_root"

    # Verify the file is NOT in new worktree (--no-carry overrides config)
    assert_file_not_exists "feature/no-carry-override/no_carry.txt" "File should NOT be carried when --no-carry flag is used" || return 1

    # Verify the file IS still in original worktree
    assert_file_exists "main/no_carry.txt" "File should remain in original worktree" || return 1

    return 0
}

# Test daft.checkout.upstream=false disables upstream tracking
test_config_checkout_upstream_false() {
    local remote_repo=$(create_test_remote "test-repo-config-upstream-false" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-upstream-false"

    # Set local config to disable upstream tracking
    git config daft.checkout.upstream false

    # Checkout an existing remote branch
    git-worktree-checkout develop || return 1

    # Verify the worktree was created
    assert_directory_exists "develop" || return 1

    # Check if upstream was NOT set
    cd develop
    local upstream
    upstream=$(git config branch.develop.remote 2>/dev/null)
    if [[ -n "$upstream" ]]; then
        log_error "Upstream was set (should be disabled)"
        return 1
    fi
    log_success "Upstream was not set (upstream disabled in config)"

    return 0
}

# Test config boolean variants (yes/no/on/off/1/0)
test_config_bool_variants() {
    local remote_repo=$(create_test_remote "test-repo-config-bool" "main")

    # Clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-config-bool"

    # Test various boolean representations
    # Test "no"
    git config daft.checkout.push no
    cd main
    git-worktree-checkout-branch feature/test-no || return 1
    cd ..
    local branch_no=$(git ls-remote --heads origin feature/test-no 2>/dev/null)
    if [[ -n "$branch_no" ]]; then
        log_error "Branch was pushed when config was 'no'"
        return 1
    fi
    log_success "Config 'no' parsed as false"

    # Test "off"
    git config daft.checkout.push off
    cd "feature/test-no"
    git-worktree-checkout-branch feature/test-off || return 1
    cd ..
    local branch_off=$(git ls-remote --heads origin feature/test-off 2>/dev/null)
    if [[ -n "$branch_off" ]]; then
        log_error "Branch was pushed when config was 'off'"
        return 1
    fi
    log_success "Config 'off' parsed as false"

    # Test "0"
    git config daft.checkout.push 0
    cd "feature/test-off"
    git-worktree-checkout-branch feature/test-zero || return 1
    cd ..
    local branch_zero=$(git ls-remote --heads origin feature/test-zero 2>/dev/null)
    if [[ -n "$branch_zero" ]]; then
        log_error "Branch was pushed when config was '0'"
        return 1
    fi
    log_success "Config '0' parsed as false"

    # Test "yes" to re-enable
    git config daft.checkout.push yes
    cd "feature/test-zero"
    git-worktree-checkout-branch feature/test-yes || return 1
    cd ..
    local branch_yes=$(git ls-remote --heads origin feature/test-yes 2>/dev/null)
    if [[ -z "$branch_yes" ]]; then
        log_error "Branch was NOT pushed when config was 'yes'"
        return 1
    fi
    log_success "Config 'yes' parsed as true"

    return 0
}

# Run all config tests
run_config_tests() {
    log "Running git config settings integration tests..."

    run_test "config_defaults" "test_config_defaults"
    run_test "config_checkout_push_false" "test_config_checkout_push_false"
    run_test "config_remote_custom" "test_config_remote_custom"
    run_test "config_checkout_branch_carry_false" "test_config_checkout_branch_carry_false"
    run_test "config_checkout_carry_true" "test_config_checkout_carry_true"
    run_test "config_flag_overrides_carry_false" "test_config_flag_overrides_carry_false"
    run_test "config_flag_overrides_carry_true" "test_config_flag_overrides_carry_true"
    run_test "config_checkout_upstream_false" "test_config_checkout_upstream_false"
    run_test "config_bool_variants" "test_config_bool_variants"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_config_tests
    print_summary
fi
