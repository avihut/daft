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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-defaults"

    # Verify default behavior works (remote=origin, push disabled by default)
    git-worktree-checkout -b feature/test-defaults || return 1

    # Verify the worktree was created
    assert_directory_exists "feature/test-defaults" || return 1

    # Verify the branch was NOT pushed (local-first default)
    cd "feature/test-defaults"
    local remote_branch
    remote_branch=$(git ls-remote --heads origin feature/test-defaults 2>/dev/null)
    if [[ -n "$remote_branch" ]]; then
        log_error "Branch was pushed to remote (should NOT push by default in local-first mode)"
        return 1
    fi
    log_success "Branch was not pushed to remote (local-first default)"

    return 0
}

# Test daft.checkout.push=false disables push
test_config_checkout_push_false() {
    local remote_repo=$(create_test_remote "test-repo-config-push-false" "main")

    # Clone the repository
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-push-false"

    # Set local config to disable push
    git config daft.checkout.push false

    # Create a new branch
    git-worktree-checkout -b feature/no-push || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-remote"

    # Add a second remote called "upstream"
    git remote add upstream "$remote_repo"

    # Set local config to use upstream
    git config daft.remote upstream
    git config daft.checkout.push true
    git config daft.checkout.fetch true

    # Create a new branch (should push to upstream, not origin)
    cd main
    git-worktree-checkout -b feature/custom-remote || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-carry-false"
    repo_root=$(pwd)

    # Set local config to disable carry for checkout-branch
    git config daft.checkoutBranch.carry false

    # Create uncommitted changes
    cd main
    echo "uncommitted content" > uncommitted.txt

    # Create a new branch (should NOT carry changes due to config)
    git-worktree-checkout -b feature/no-carry-config || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-override-carry"
    repo_root=$(pwd)

    # Set local config to disable carry
    git config daft.checkoutBranch.carry false

    # Create uncommitted changes
    cd main
    echo "override content" > override.txt

    # Create a new branch with explicit --carry flag (should override config)
    git-worktree-checkout -b --carry feature/override-carry || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-override-no-carry"
    repo_root=$(pwd)

    # Config already has carry=true by default, but let's be explicit
    git config daft.checkoutBranch.carry true

    # Create uncommitted changes
    cd main
    echo "no-carry content" > no_carry.txt

    # Create a new branch with explicit --no-carry flag (should override config)
    git-worktree-checkout -b --no-carry feature/no-carry-override || return 1

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
    git-worktree-clone --layout contained "$remote_repo" || return 1
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
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-bool"

    # Test various boolean representations
    # Test "no"
    git config daft.checkout.push no
    cd main
    git-worktree-checkout -b feature/test-no || return 1
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
    git-worktree-checkout -b feature/test-off || return 1
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
    git-worktree-checkout -b feature/test-zero || return 1
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
    git-worktree-checkout -b feature/test-yes || return 1
    cd ..
    local branch_yes=$(git ls-remote --heads origin feature/test-yes 2>/dev/null)
    if [[ -z "$branch_yes" ]]; then
        log_error "Branch was NOT pushed when config was 'yes'"
        return 1
    fi
    log_success "Config 'yes' parsed as true"

    return 0
}

# =============================================================================
# `daft config` CLI tests (#470)
# =============================================================================

# set → get round-trip, with the value canonicalized on the way in
test_config_cli_set_get_roundtrip() {
    local remote_repo=$(create_test_remote "test-repo-cli-roundtrip" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-roundtrip"

    daft config set daft.merge.style SQUASH || return 1

    # The registry's spelling is what lands in the file, not the user's.
    local stored
    stored=$(git config --local --get daft.merge.style)
    if [[ "$stored" != "squash" ]]; then
        log_error "Stored value was '$stored', expected canonicalized 'squash'"
        return 1
    fi
    log_success "set canonicalizes the value before writing"

    local got
    got=$(daft config get daft.merge.style)
    if [[ "$got" != "squash" ]]; then
        log_error "get returned '$got', expected 'squash'"
        return 1
    fi
    log_success "get round-trips what set wrote"

    return 0
}

# unset removes the local value and reveals whatever it was masking
test_config_cli_unset_reveals_lower_layer() {
    local remote_repo=$(create_test_remote "test-repo-cli-unset" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-unset"

    daft config set daft.remote upstream || return 1
    if [[ "$(daft config get daft.remote)" != "upstream" ]]; then
        log_error "local value did not take effect"
        return 1
    fi

    daft config unset daft.remote || return 1
    if [[ "$(daft config get daft.remote)" != "origin" ]]; then
        log_error "unset did not reveal the default"
        return 1
    fi
    log_success "unset reveals the layer below"

    # Unsetting again is a no-op, not an error.
    daft config unset daft.remote || return 1
    log_success "unsetting an absent key is not an error"

    return 0
}

# A value the setting's own type rejects is refused where it is typed
test_config_cli_rejects_invalid_values() {
    local remote_repo=$(create_test_remote "test-repo-cli-invalid" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-invalid"

    if daft config set daft.merge.style octopus-ish >/dev/null 2>&1; then
        log_error "An invalid enum value was accepted"
        return 1
    fi
    log_success "invalid enum refused"

    if daft config set daft.list.columns "+definitelyNotAColumn" >/dev/null 2>&1; then
        log_error "An invalid column spec was accepted"
        return 1
    fi
    log_success "invalid column spec refused at set time"

    # Nothing should have been written by either refusal.
    if git config --local --get daft.merge.style >/dev/null 2>&1; then
        log_error "A refused set still wrote to config"
        return 1
    fi
    log_success "a refused set writes nothing"

    return 0
}

# --global writes the global file, and the sandbox is what that means here
test_config_cli_global_scope() {
    # The real-state guard does not cover git config, so prove the redirect is
    # in place before writing anything global. Without it this test would edit
    # the developer's own ~/.gitconfig.
    if [[ -z "${GIT_CONFIG_GLOBAL:-}" ]]; then
        log_error "GIT_CONFIG_GLOBAL is unset — refusing to write global config"
        return 1
    fi
    case "$GIT_CONFIG_GLOBAL" in
        "$HOME"/*)
            log_error "GIT_CONFIG_GLOBAL ($GIT_CONFIG_GLOBAL) points inside HOME"
            return 1
            ;;
    esac
    log_success "global config is redirected away from HOME"

    local remote_repo=$(create_test_remote "test-repo-cli-global" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-global"

    daft config set --global daft.go.autoStart true || return 1
    if [[ "$(git config --global --get daft.go.autoStart)" != "true" ]]; then
        log_error "--global did not write the global file"
        return 1
    fi
    log_success "--global writes global config"

    # A local value outranks it, and unsetting the local one reveals it again.
    daft config set daft.go.autoStart false || return 1
    if [[ "$(daft config get daft.go.autoStart)" != "false" ]]; then
        log_error "local did not outrank global"
        return 1
    fi
    daft config unset daft.go.autoStart || return 1
    if [[ "$(daft config get daft.go.autoStart)" != "true" ]]; then
        log_error "unsetting local did not reveal the global value"
        return 1
    fi
    log_success "local outranks global, and unset reveals it"

    daft config unset --global daft.go.autoStart || return 1
    return 0
}

# A key daft only reads globally refuses a local write instead of pretending
test_config_cli_global_only_key_refuses_local() {
    local remote_repo=$(create_test_remote "test-repo-cli-globalonly" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-globalonly"

    if daft config set daft.updateCheck false >/dev/null 2>&1; then
        log_error "A local write to a global-only key was accepted"
        return 1
    fi
    log_success "global-only key refuses a local write"

    return 0
}

# An unknown key exits non-zero and suggests the near spelling
test_config_cli_unknown_key_suggests() {
    local remote_repo=$(create_test_remote "test-repo-cli-unknown" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-unknown"

    local output
    output=$(daft config get daft.merge.stile 2>&1)
    if [[ $? -eq 0 ]]; then
        log_error "An unknown key exited zero"
        return 1
    fi
    if [[ "$output" != *"daft.merge.style"* ]]; then
        log_error "No suggestion offered; got: $output"
        return 1
    fi
    log_success "unknown key suggests the near spelling"

    return 0
}

# list reports the value and the layer that decided it
test_config_cli_list_shows_origin() {
    local remote_repo=$(create_test_remote "test-repo-cli-list" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-list"

    daft config set daft.merge.style rebase || return 1

    local output
    output=$(daft config list --modified 2>/dev/null)
    if [[ "$output" != *"daft.merge.style"* ]]; then
        log_error "--modified omitted a set value; got: $output"
        return 1
    fi
    if [[ "$output" != *"local"* ]]; then
        log_error "--modified did not name the origin; got: $output"
        return 1
    fi
    log_success "list --modified shows the value and its origin"

    # Structured output carries the machine-readable view.
    local json
    json=$(daft config list --modified --format json 2>/dev/null)
    if [[ "$json" != *'"daft.merge.style"'* ]]; then
        log_error "--format json omitted the key; got: $json"
        return 1
    fi
    log_success "list --format json emits the key"

    return 0
}

# Reading one layer: the same rung a write at that layer would land in
#
# The property worth an integration test rather than a unit one is that the two
# directions agree against real git config — `get --local` reporting what
# `set --local` put there, and exit 1 standing for "this layer is silent"
# rather than "no value anywhere".
test_config_cli_reads_one_layer() {
    if [[ -z "$GIT_CONFIG_GLOBAL" ]]; then
        log_error "GIT_CONFIG_GLOBAL is unset — refusing to touch the real global config"
        return 1
    fi

    local remote_repo=$(create_test_remote "test-repo-cli-layers" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-layers" || return 1

    daft config set --global daft.remote shared || return 1
    daft config set --local daft.remote mine || return 1

    local got
    got=$(daft config get daft.remote)
    if [[ "$got" != "mine" ]]; then
        log_error "the resolved read should be the local value; got '$got'"
        return 1
    fi
    got=$(daft config get daft.remote --global)
    if [[ "$got" != "shared" ]]; then
        log_error "--global should read the global file alone; got '$got'"
        return 1
    fi
    got=$(daft config get daft.remote --local)
    if [[ "$got" != "mine" ]]; then
        log_error "--local should read the local file alone; got '$got'"
        return 1
    fi
    log_success "each layer reads its own value"

    # Silent layer: exit 1, the same contract `git config --get` has. The key
    # resolves to a default, so a read that fell back would exit 0 and hide it.
    if daft config get daft.merge.style --local >/dev/null 2>&1; then
        log_error "--local on a layer that sets nothing should exit non-zero"
        return 1
    fi
    if [[ -z "$(daft config get daft.merge.style)" ]]; then
        log_error "the unnarrowed read should still resolve to the default"
        return 1
    fi
    log_success "a silent layer exits 1 while the resolved read still answers"

    # The pair is exclusive, and --origin already shows every layer.
    if daft config get daft.remote --local --global >/dev/null 2>&1; then
        log_error "--local and --global together should be refused"
        return 1
    fi
    if daft config get daft.remote --origin --local >/dev/null 2>&1; then
        log_error "--origin with a layer flag should be refused"
        return 1
    fi
    log_success "the flags refuse the combinations that mean nothing"

    # And a narrowed list is the contents of that layer, with a warning where
    # the layer is not the one in force.
    local output
    output=$(daft config list --global 2>/dev/null)
    if [[ "$output" != *"shared"* ]]; then
        log_error "list --global omitted the global value; got: $output"
        return 1
    fi
    if [[ "$output" != *"outranked by local"* ]]; then
        log_error "list --global did not warn that local outranks it; got: $output"
        return 1
    fi
    if [[ "$output" == *"mine"* ]]; then
        log_error "list --global showed the local value; got: $output"
        return 1
    fi
    log_success "list --global is the global layer, and says what outranks it"

    daft config unset --global daft.remote || return 1
    return 0
}

# A behavior is readable at a layer only when that layer names a whole state
#
# `set <behavior> --local` writes every member, so "what state is set here" has
# an answer. A layer holding only some members has none, and reporting the
# nearest preset there is the single-scope claim this command exists to avoid.
test_config_cli_behavior_at_one_layer() {
    local remote_repo=$(create_test_remote "test-repo-cli-behavior-layer" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-behavior-layer" || return 1

    # One member of three: effective reads custom, and local names no state.
    daft config set --local daft.checkout.push true || return 1
    local got
    got=$(daft config get remote-sync)
    if [[ "$got" != "custom" ]]; then
        log_error "one member out of step should read custom; got '$got'"
        return 1
    fi
    if daft config get remote-sync --local >/dev/null 2>&1; then
        log_error "a layer with one of three members should name no state"
        return 1
    fi
    log_success "a partial layer names no state rather than guessing"

    # A behavior write makes it whole, and then the layer does name one.
    daft config set remote-sync on --local >/dev/null || return 1
    got=$(daft config get remote-sync --local)
    if [[ "$got" != "on" ]]; then
        log_error "--local should name the state the behavior write left; got '$got'"
        return 1
    fi
    if daft config get remote-sync --global >/dev/null 2>&1; then
        log_error "global sets no member, so it should name no state"
        return 1
    fi
    log_success "a whole layer names the state its write left"

    return 0
}

# Every verb emits a machine-readable form, and --format never moves the exit code
test_config_cli_structured_output() {
    local remote_repo=$(create_test_remote "test-repo-cli-json" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-cli-json" || return 1

    # A read carries the ladder whether or not --origin was passed, so a script
    # never needs a verbosity flag to get a complete answer.
    local json
    json=$(daft config get daft.remote --format json 2>/dev/null)
    for needle in '"layers"' '"diagnostics"' '"writable_scopes"' '"effective"'; do
        if [[ "$json" != *"$needle"* ]]; then
            log_error "get --format json omitted $needle; got: $json"
            return 1
        fi
    done
    log_success "get --format json carries the whole ladder without --origin"

    # Exit code unchanged by --format: a silent layer still exits 1, and still
    # emits its document, so either signal answers.
    json=$(daft config get daft.merge.style --local --format json 2>/dev/null)
    local status=$?
    if [[ $status -eq 0 ]]; then
        log_error "--format must not turn a silent layer into a success"
        return 1
    fi
    if [[ "$json" != *'"daft.merge.style"'* ]]; then
        log_error "a silent layer should still emit its document; got: $json"
        return 1
    fi
    log_success "--format leaves the exit code alone"

    # A write reports what landed. A behavior write is several keys behind one
    # command, and the record is the only place that says which.
    json=$(daft config set remote-sync on --format json 2>/dev/null)
    for needle in '"action": "set"' '"state": "on"' 'daft.checkout.fetch' 'daft.branchDelete.remote'; do
        if [[ "$json" != *"$needle"* ]]; then
            log_error "set --format json omitted $needle; got: $json"
            return 1
        fi
    done
    log_success "a behavior write records every key and the resulting state"

    # An unset that removed nothing is a success that changed nothing.
    json=$(daft config unset daft.remote --format json 2>/dev/null) || return 1
    if [[ "$json" != *'"changed": false'* ]]; then
        log_error "an unset of an absent key should report changed=false; got: $json"
        return 1
    fi
    log_success "a write that changed nothing says so"

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

    run_test "config_cli_set_get_roundtrip" "test_config_cli_set_get_roundtrip"
    run_test "config_cli_unset_reveals_lower_layer" "test_config_cli_unset_reveals_lower_layer"
    run_test "config_cli_rejects_invalid_values" "test_config_cli_rejects_invalid_values"
    run_test "config_cli_global_scope" "test_config_cli_global_scope"
    run_test "config_cli_global_only_key_refuses_local" "test_config_cli_global_only_key_refuses_local"
    run_test "config_cli_unknown_key_suggests" "test_config_cli_unknown_key_suggests"
    run_test "config_cli_list_shows_origin" "test_config_cli_list_shows_origin"
    run_test "config_cli_reads_one_layer" "test_config_cli_reads_one_layer"
    run_test "config_cli_behavior_at_one_layer" "test_config_cli_behavior_at_one_layer"
    run_test "config_cli_structured_output" "test_config_cli_structured_output"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_config_tests
    print_summary
fi
