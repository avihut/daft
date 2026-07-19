#!/bin/bash

# Integration tests for the live `v` verbose toggle on the plan-execute
# rail (#729).
#
# The toggle only exists on a TTY, so these drive daft under a DSR-answering
# PTY (pty_run.py) and script the keypresses with --send-after. Everything
# here is invisible to the YAML suite, which runs with DAFT_TESTING set and
# therefore never materializes a region at all.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

TOGGLE_PTY_RUN="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pty_run.py"

# Run daft with the live rail actually enabled.
#
# The framework exports DAFT_TESTING to keep the suite's ~2000 invocations
# quiet, and one of the things it suppresses is the interactive region
# itself (`TimelineMode::auto` → Hidden) — which is the entire subject of
# these tests. So it comes off here, and the three background spawns it was
# standing in for are suppressed individually instead: no orphaned
# update-check / trust-prune / log-clean daemons.
_toggle_daft() {
    env -u DAFT_TESTING \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# Strip ANSI and split the pty's carriage-returned repaints into lines.
_toggle_clean() {
    sed -e 's/\x1b\[[0-9;]*[a-zA-Z]//g' -e 's/\r/\n/g' "$1"
}

# A repo with three worktrees, so `daft exec` fans out onto the rail rather
# than collapsing to single-target passthrough.
_toggle_fixture() {
    local name="$1"
    local remote_repo
    remote_repo=$(create_test_remote "$name" "master") || return 1
    git -C "$remote_repo" branch feat-a master || return 1
    git -C "$remote_repo" branch feat-b master || return 1
    git-worktree-clone --layout contained "$remote_repo" || return 1
    local root="$PWD/$name"
    # Each checkout runs in a subshell from a known-good directory inside the
    # repo: relying on inherited cwd made this order-dependent.
    (cd "$root/master" && git-worktree-checkout feat-a) || return 1
    (cd "$root/master" && git-worktree-checkout feat-b) || return 1
    # `daft exec --all` runs from the container root, where it sees all three.
    cd "$root" || return 1
    return 0
}

# Pressing `v` mid-run flips the live rows to verbose and threads every
# row's captured log into its receipt.
test_exec_v_toggles_verbose_midrun() {
    _toggle_fixture "toggle-midrun" || return 1
    local log="$TEMP_BASE_DIR/toggle-midrun.log"

    # The keypress lands one poll (~100ms) after `first-line` appears, so the
    # rows must still be running then. Three seconds keeps that margin wide
    # on a loaded CI box; a shorter sleep makes this a timing flake.
    _toggle_daft python3 "$TOGGLE_PTY_RUN" --send-after 'first-line:v' "$log" \
        daft exec --all -- sh -c 'echo first-line; sleep 3; echo second-line' \
        >/dev/null 2>&1

    local out
    out=$(_toggle_clean "$log")

    # A *threaded* line wears the thread gutter; the same text as a row
    # annotation would prove nothing, since terse mode shows it there too.
    if ! grep -qE '^│ +first-line' <<<"$out"; then
        log_error "verbose did not thread a job's log into its receipt"
        return 1
    fi
    # Every row was still running when v landed, so nothing had finished to
    # fold out — the flip must leave no note in scrollback. A note per keypress
    # used to pile up here (#729 field report); it now heads a replay only.
    if grep -q "verbose on\|verbose off" <<<"$out"; then
        log_error "a density note appeared for a flip with nothing to replay"
        return 1
    fi
    log_success "v flips the rail to verbose mid-run"
    return 0
}

# Without the keypress the run is byte-for-byte the terse rail it always
# was: no note, no threads, no hint left in scrollback.
test_exec_without_v_stays_terse() {
    _toggle_fixture "toggle-baseline" || return 1
    local log="$TEMP_BASE_DIR/toggle-baseline.log"

    _toggle_daft python3 "$TOGGLE_PTY_RUN" "$log" \
        daft exec --all -- sh -c 'echo first-line; sleep 1; echo second-line' \
        >/dev/null 2>&1

    local out
    out=$(_toggle_clean "$log")

    if grep -q "verbose on\|verbose off" <<<"$out"; then
        log_error "a density note appeared without any keypress"
        return 1
    fi
    if grep -qE '^│ +first-line' <<<"$out"; then
        log_error "receipts threaded their logs without -v or a toggle"
        return 1
    fi
    log_success "an untoggled run stays terse"
    return 0
}

# #663's two-stage Ctrl-C must survive the listener: `ISIG` stays on, so the
# first ^C is still a real SIGINT that cancels the workers and closes the
# rail — not a keystroke the reader swallows, and not a torn-down region.
test_exec_ctrl_c_still_cancels_with_listener_active() {
    _toggle_fixture "toggle-interrupt" || return 1
    local log="$TEMP_BASE_DIR/toggle-interrupt.log"

    # --ctty: the child needs its own session owning the pty, or the
    # interrupt has no foreground process group to reach.
    _toggle_daft python3 "$TOGGLE_PTY_RUN" --ctty \
        --send-after 'started:v' \
        --send-after 'v quiet:\x03' \
        "$log" \
        daft exec --all -- sh -c 'echo started; sleep 20' \
        >/dev/null 2>&1

    local out
    out=$(_toggle_clean "$log")

    # The toggle is confirmed by the footer hint flipping to `v quiet` (now
    # offering the way back), not by a scrollback note — a mid-run flip with
    # nothing finished writes none.
    if ! grep -q "v quiet" <<<"$out"; then
        log_error "the toggle did not register before the interrupt"
        return 1
    fi
    if ! grep -qi "cancelled" <<<"$out"; then
        log_error "^C did not reach daft's cancellation with a listener active"
        return 1
    fi
    # `ECHOCTL` stays off for the region's life: a literal ^C in the region
    # would desync indicatif's line accounting.
    if grep -qE '^\^C' <<<"$out"; then
        log_error "a raw ^C echoed into the live region"
        return 1
    fi
    log_success "two-stage Ctrl-C survives the key listener"
    return 0
}

# The terminal must be handed back the way it was found. A half-configured
# driver (no echo, no line editing) is a far worse failure than anything the
# rail could print.
test_exec_restores_terminal_modes() {
    _toggle_fixture "toggle-termios" || return 1
    local log="$TEMP_BASE_DIR/toggle-termios.log"

    # `stty -a` runs inside the same pty right after daft exits; the wrapper
    # preserves daft's exit code so the harness still reports it.
    _toggle_daft python3 "$TOGGLE_PTY_RUN" --send-after 'first-line:v' "$log" \
        sh -c 'daft exec --all -- sh -c "echo first-line; sleep 3" >/dev/null 2>&1; rc=$?; stty -a; exit $rc' \
        >/dev/null 2>&1

    local modes
    modes=$(_toggle_clean "$log" | tr ' ' '\n' | grep -E '^-?(icanon|echo)$' | sort -u)

    if grep -q '^-icanon$' <<<"$modes"; then
        log_error "terminal left in non-canonical mode after the run"
        return 1
    fi
    if grep -q '^-echo$' <<<"$modes"; then
        log_error "terminal left with echo disabled after the run"
        return 1
    fi
    if ! grep -q '^icanon$' <<<"$modes" || ! grep -q '^echo$' <<<"$modes"; then
        log_error "could not read the terminal modes back: $modes"
        return 1
    fi
    log_success "terminal modes restored after the run"
    return 0
}

# --- Test runner ---
run_exec_verbose_toggle_tests() {
    run_test "exec_v_toggles_verbose_midrun" test_exec_v_toggles_verbose_midrun
    run_test "exec_without_v_stays_terse" test_exec_without_v_stays_terse
    run_test "exec_ctrl_c_still_cancels_with_listener_active" test_exec_ctrl_c_still_cancels_with_listener_active
    run_test "exec_restores_terminal_modes" test_exec_restores_terminal_modes
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_exec_verbose_toggle_tests
    print_summary
    exit $?
fi
