#!/bin/bash

# The blessed shell suite — what stays shell after #447.
#
# tests/manual/scenarios/ is the primary integration surface. This runner
# sources only the tests that need something the YAML runner cannot give them:
#
#   a real terminal     test_rail_pty.sh          the live rail's planning face,
#                                                 receipts and -x rows; the TUI
#                                                 PR column; alias capture under
#                                                 a controlling tty
#                       test_config_tui.sh        the config TUI
#                       test_exec_verbose_toggle.sh  the rail's verbose toggle
#                                                 (needs a controlling tty)
#                       test_list_esc.sh          Esc abandoning slow list cells
#                       test_sync_governor.sh     SIGSTOP/CONT/KILL reaping of a
#                                                 process group
#   signals / timing    test_sync_cancel.sh       kill-delivered SIGINT/TERM
#                                                 with no tty, elapsed-time
#                                                 bounds, SIGTSTP
#                       test_merge_gate_lane.sh   two daft processes racing the
#                                                 merge lane
#                       test_remove_reaper.sh     the detached `__reap-trash`
#                                                 process (DAFT_TESTING would
#                                                 make it inline)
#   one-shell contract  test_shell_init.sh        the wrapper's cd contract:
#                                                 eval, run, `builtin pwd` in
#                                                 the same shell
#
# plus the framework's own self-tests and the #738 state-guard preflight below.
# The sourced list IS the blessed list: a new shell test goes into one of these
# files (or a new one sourced here) only when it needs a PTY, a signal, or the
# wrapper's shell — everything else is a YAML scenario.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Blessed suites
source "$(dirname "${BASH_SOURCE[0]}")/test_rail_pty.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_config_tui.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_exec_verbose_toggle.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_list_esc.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_sync_governor.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_sync_cancel.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_merge_gate_lane.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_remove_reaper.sh"
source "$(dirname "${BASH_SOURCE[0]}")/test_shell_init.sh"

# Test framework self-tests
test_integration_framework_assertions() {
    # Test successful assertions
    assert_command_success "true" || return 1
    assert_command_failure "false" || return 1
    
    # Create test directory for file/directory assertions
    mkdir -p "test_dir"
    touch "test_dir/test_file"
    
    assert_directory_exists "test_dir" || return 1
    assert_file_exists "test_dir/test_file" || return 1
    
    # Clean up
    rm -rf "test_dir"
    
    return 0
}

# Test remote repository creation
test_integration_remote_repo_creation() {
    local remote_repo=$(create_test_remote "test-remote-creation" "main")
    
    # Verify remote repository was created
    assert_directory_exists "$remote_repo" || return 1
    assert_git_repository "$remote_repo" || return 1
    
    # Verify we can clone from it
    git clone "$remote_repo" "test-clone" >/dev/null 2>&1 || return 1
    assert_directory_exists "test-clone" || return 1
    assert_git_repository "test-clone" || return 1
    assert_file_exists "test-clone/README.md" || return 1

    return 0
}

# Regression (#738): the framework's own leak guard must have teeth. The bash
# suites isolate the repo catalog purely via the DAFT_*_DIR overrides, which
# compile out of non-dev builds — a binary that ignores them writes the real
# catalog (the `test-repo-push-*` orphans that motivated this). setup() now
# runs assert_binary_honors_overrides; this proves it both rejects a
# non-honoring binary and accepts the honoring dev binary.
test_integration_state_guard_preflight() {
    # Positive: the real binary honors the overrides setup() exported.
    if ! assert_binary_honors_overrides "$RUST_BINARY_DIR/daft" "$TEMP_BASE_DIR" >/dev/null 2>&1; then
        log_error "preflight rejected the honoring dev binary (false positive)"
        return 1
    fi

    # Negative: a stub daft that ignores DAFT_*_DIR (stands in for a
    # release/tagged build or a system daft) must be rejected.
    local stub_dir="$PWD/stub-bin"
    mkdir -p "$stub_dir"
    cat > "$stub_dir/daft" <<'STUB'
#!/bin/sh
if [ "$1" = "__dirs" ]; then
  printf 'config\t/nonsandbox/config/daft\n'
  printf 'data\t/nonsandbox/share/daft\n'
  printf 'state\t/nonsandbox/state/daft\n'
fi
STUB
    chmod +x "$stub_dir/daft"

    # Both streams: the framework's log_* helpers all write to stdout, so
    # suppressing only stderr would print this suite's loudest alarm —
    # "Refusing to run: integration tests would touch your real ... dirs" —
    # on a run where the guard is working exactly as designed. Anyone reading
    # CI output, or grepping it for that string, would read a green run as a
    # breach.
    if assert_binary_honors_overrides "$stub_dir/daft" "$TEMP_BASE_DIR" >/dev/null 2>&1; then
        log_error "preflight accepted a binary that ignores DAFT_*_DIR (guard has no teeth)"
        return 1
    fi

    log_success "state-guard preflight rejects non-honoring binaries and accepts the dev binary"
    return 0
}


# Run all blessed integration tests
run_all_integration_tests() {
    log "Running the blessed shell integration suite..."

    # Framework self-tests and the state-guard preflight
    run_test "integration_framework_assertions" "test_integration_framework_assertions"
    run_test "integration_remote_repo_creation" "test_integration_remote_repo_creation"
    run_test "integration_state_guard_preflight" "test_integration_state_guard_preflight"

    # A real terminal
    run_rail_pty_tests
    run_config_tui_tests
    run_exec_verbose_toggle_tests
    run_list_esc_tests
    run_sync_governor_tests

    # Signals / timing
    run_sync_cancel_tests
    run_merge_gate_lane_tests
    run_remove_reaper_tests

    # The wrapper's cd contract
    run_shell_init_tests
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_all_integration_tests
    print_summary
    exit $?
fi
