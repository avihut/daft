#!/bin/bash

# Integration tests for daft env — deterministic per-worktree env values (#388).
#
# Ports asserted literally: the derivation is a pure function, and every
# fixture pins `salt: myapp`, so slug 'main' → block 20960 and slug
# 'feature-new' → block 23952 on every machine forever (scheme-1 goldens in
# src/core/env_values). A drift here is a wire-format break, not a flake.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Write the standard env fixture daft.yml into the current worktree and
# commit it (the tracked config is what travels to every worktree).
write_env_daft_yml() {
    cat > daft.yml <<'EOF'
env:
  salt: myapp
  ports:
    - WEBAPP_PORT
    - STORYBOOK_PORT
    - API_PORT: 8
  values:
    COMPOSE_PROJECT_NAME: "myapp-{worktree_slug}"
EOF
    git add daft.yml
    git -c user.email=test@test.com -c user.name=Test commit -qm "add env config" || return 1
}

# =============================================================================
# Env Tests
# =============================================================================

test_env_single_var_and_determinism() {
    local remote_repo=$(create_test_remote "test-repo-env-single" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-env-single/main"
    write_env_daft_yml || return 1

    local first second
    first=$(daft env WEBAPP_PORT) || return 1
    second=$(daft env WEBAPP_PORT) || return 1

    if [[ "$first" != "20960" ]]; then
        log_error "expected WEBAPP_PORT=20960 for slug 'main', got '$first'"
        return 1
    fi
    if [[ "$first" != "$second" ]]; then
        log_error "two invocations disagree: '$first' vs '$second'"
        return 1
    fi
    log_success "single var is literal and stable (20960)"

    # Offsets: enum semantics (0, 1, explicit 8).
    [[ "$(daft env STORYBOOK_PORT)" == "20961" ]] || { log_error "offset 1 wrong"; return 1; }
    [[ "$(daft env API_PORT)" == "20968" ]] || { log_error "explicit offset 8 wrong"; return 1; }
    log_success "offsets index into the block (20961, 20968)"
    return 0
}

test_env_export_is_eval_clean() {
    local remote_repo=$(create_test_remote "test-repo-env-export" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-env-export/main"
    write_env_daft_yml || return 1

    # The eval'd hot path: stdout must be pure export lines, stderr empty.
    local stderr_file="$PWD/.export-stderr"
    local exports
    exports=$(daft env --export 2>"$stderr_file") || return 1
    if [[ -s "$stderr_file" ]]; then
        log_error "stderr not empty on the eval path: $(cat "$stderr_file")"
        return 1
    fi
    eval "$exports"
    if [[ "$WEBAPP_PORT" != "20960" || "$COMPOSE_PROJECT_NAME" != "myapp-main" ]]; then
        log_error "eval'd exports wrong: WEBAPP_PORT='$WEBAPP_PORT' COMPOSE_PROJECT_NAME='$COMPOSE_PROJECT_NAME'"
        return 1
    fi
    log_success "--export is eval-clean and stderr-silent"
    unset WEBAPP_PORT STORYBOOK_PORT API_PORT COMPOSE_PROJECT_NAME
    return 0
}

test_env_strictness_and_adhoc() {
    local remote_repo=$(create_test_remote "test-repo-env-strict" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-env-strict/main"
    write_env_daft_yml || return 1

    if daft env TYPO_PORT >/dev/null 2>&1; then
        log_error "unknown name under a declared schema must exit non-zero"
        return 1
    fi
    log_success "unknown name errors under strictness"

    local adhoc
    adhoc=$(daft env TYPO_PORT --ad-hoc) || return 1
    if (( adhoc < 26384 || adhoc > 32767 )); then
        log_error "ad-hoc port $adhoc outside the disjoint ad-hoc region"
        return 1
    fi
    log_success "--ad-hoc resolves into the ad-hoc region ($adhoc)"
    return 0
}

test_env_cross_worktree_and_cross_repo_addresses() {
    local remote_a=$(create_test_remote "test-repo-env-a" "main")
    local remote_b=$(create_test_remote "test-repo-env-b" "main")
    git-worktree-clone --layout contained "$remote_a" || return 1
    git-worktree-clone --layout contained "$remote_b" || return 1

    cd "test-repo-env-a/main"
    write_env_daft_yml || return 1
    cd ../..
    cd "test-repo-env-b/main"
    write_env_daft_yml || return 1

    # @worktree address: sibling values from here, existing or not.
    local sibling
    sibling=$(daft env WEBAPP_PORT@feature-new 2>/dev/null) || return 1
    if [[ "$sibling" != "23952" ]]; then
        log_error "expected 23952 for hypothetical sibling 'feature-new', got '$sibling'"
        return 1
    fi
    log_success "@worktree address answers before creation (23952)"

    # repo: address — matching-name worktree in the other cataloged repo.
    # Same salt + same slug ⇒ same value; the point is resolution, and that
    # the canonical --repo spelling agrees with the sugar.
    local via_address via_flag
    via_address=$(daft env test-repo-env-a:WEBAPP_PORT 2>/dev/null)
    via_flag=$(daft env WEBAPP_PORT --repo test-repo-env-a 2>/dev/null)
    if [[ -z "$via_address" ]]; then
        log_error "cross-repo address returned nothing"
        return 1
    fi
    if [[ "$via_address" != "$via_flag" ]]; then
        log_error "address sugar ($via_address) and --repo ($via_flag) disagree"
        return 1
    fi
    log_success "repo: address and --repo agree ($via_address)"
    return 0
}

test_env_exec_injection() {
    local remote_repo=$(create_test_remote "test-repo-env-exec" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-env-exec/main"
    write_env_daft_yml || return 1

    local seen
    seen=$(daft exec main -x 'echo "$WEBAPP_PORT"' 2>/dev/null | tail -1) || return 1
    if [[ "$seen" != "20960" ]]; then
        log_error "exec'd command did not see the derived port (got '$seen')"
        return 1
    fi
    log_success "daft exec injects derived values (20960)"
    return 0
}

# Run all env tests
run_env_tests() {
    log "Running daft env integration tests..."

    run_test "env_single_var_and_determinism" "test_env_single_var_and_determinism"
    run_test "env_export_is_eval_clean" "test_env_export_is_eval_clean"
    run_test "env_strictness_and_adhoc" "test_env_strictness_and_adhoc"
    run_test "env_cross_worktree_and_cross_repo_addresses" "test_env_cross_worktree_and_cross_repo_addresses"
    run_test "env_exec_injection" "test_env_exec_injection"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_env_tests
    print_summary
fi
