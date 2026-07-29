#!/bin/bash

# Integration tests for the `daft config` settings screen (#470).
#
# The screen only exists on a TTY, so these drive daft under a DSR-answering
# PTY (pty_run.py) and script the keypresses with --send-after. What the unit
# tests cannot reach is exactly what is here: that raw mode is entered and
# left, that the alternate screen is restored, and that the whole thing
# actually paints against a real terminal rather than a TestBackend.
#
# The YAML suite runs with DAFT_TESTING set and so never opens the screen at
# all — it gets `daft config list` instead, which is the point of that gate.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

CONFIG_TUI_PTY_RUN="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pty_run.py"

# Run daft with the screen actually enabled.
#
# DAFT_TESTING comes off (it is what routes the bare command to `list`), and
# the three background spawns it was standing in for are suppressed
# individually so nothing orphans. TERM is pinned for the same reason the
# verbose-toggle suite pins it: a bare CI runner leaves it unset, and daft's
# rendering should not depend on the runner's ambient value.
_config_tui_daft() {
    env -u DAFT_TESTING \
        TERM=xterm-256color \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# Strip ANSI and split the pty's carriage-returned repaints into lines.
_config_tui_clean() {
    sed -e 's/\x1b\[[0-9;?]*[a-zA-Z]//g' -e 's/\r/\n/g' "$1"
}

# The screen paints its furniture and leaves on `q`
test_config_tui_paints_and_exits() {
    local remote_repo=$(create_test_remote "test-repo-config-tui" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-tui/main" || return 1

    git config --local daft.merge.style squash

    # The screen opens on a behavior, whose detail panel is its members'
    # ladders rather than one effective line — so step onto a setting before
    # quitting and the frames cover both shapes.
    local log="$TEMP_BASE_DIR/config-tui-paint.log"
    _config_tui_daft python3 "$CONFIG_TUI_PTY_RUN" \
        --send-after 'Daft Settings:j' \
        --send-after 'effective:q' \
        "$log" \
        daft config
    local status=$?

    if [[ $status -ne 0 ]]; then
        log_error "The settings screen exited $status (expected 0)"
        _config_tui_clean "$log" | tail -20
        return 1
    fi
    log_success "screen exited cleanly on q"

    local painted
    painted=$(_config_tui_clean "$log")

    # What the screen is for: a title, the scope it writes to, a value with
    # its origin, the resolved effective line — and, on the row it opens on,
    # a behavior with the settings it stands for.
    local expected=(
        "Daft Settings" "writes:" "local" "effective:"
        "Behaviors" "remote-sync" "What it sets"
    )
    for needle in "${expected[@]}"; do
        if [[ "$painted" != *"$needle"* ]]; then
            log_error "The screen never painted '$needle'"
            printf '%s\n' "$painted" | tail -20
            return 1
        fi
    done
    log_success "screen painted the header, the pill, and the ladder"

    # The alternate screen has to be left behind — a leaked one means the
    # user's scrollback is gone and their prompt is drawing into a buffer
    # that vanishes on the next redraw.
    # Fixed-string, not a pattern: the sequence contains [ and ?, which a
    # regex grep reads as syntax.
    if ! LC_ALL=C grep -qF "$(printf '\033[?1049l')" "$log"; then
        log_error "The alternate screen was never left (no ?1049l in the pty log)"
        return 1
    fi
    log_success "alternate screen restored on exit"

    return 0
}

# A change made in the screen actually lands in git config
test_config_tui_write_lands() {
    local remote_repo=$(create_test_remote "test-repo-config-tui-write" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-tui-write/main" || return 1

    # The screen opens on the behaviors block, so one `j` reaches the first
    # ordinary setting — daft.autocd, a boolean — and `space` toggles it off
    # its default. Then wait for the narration before quitting, which is what
    # proves the write completed rather than merely being dispatched.
    local log="$TEMP_BASE_DIR/config-tui-write.log"
    _config_tui_daft python3 "$CONFIG_TUI_PTY_RUN" \
        --send-after 'Daft Settings:j ' \
        --send-after 'Set daft.autocd:q' \
        "$log" \
        daft config
    local status=$?

    if [[ $status -ne 0 ]]; then
        log_error "The settings screen exited $status (expected 0)"
        _config_tui_clean "$log" | tail -20
        return 1
    fi

    local stored
    stored=$(git config --local --get daft.autocd)
    if [[ "$stored" != "false" ]]; then
        log_error "The toggle did not reach git config (got '${stored:-unset}')"
        _config_tui_clean "$log" | tail -20
        return 1
    fi
    log_success "a toggle in the screen writes local git config"

    local painted
    painted=$(_config_tui_clean "$log")
    if [[ "$painted" != *"Set daft.autocd = false (local)"* ]]; then
        log_error "The write was never narrated; got: $(printf '%s' "$painted" | tail -5)"
        return 1
    fi
    log_success "the write is narrated with its value and scope"

    return 0
}

# `space` on the opening row writes every setting the behavior covers
#
# The one thing no unit test reaches: a behavior write is several git-config
# writes behind one keystroke, and only a real run proves all of them land.
test_config_tui_behavior_write_lands() {
    local remote_repo=$(create_test_remote "test-repo-config-tui-behavior" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-tui-behavior/main" || return 1

    # The screen opens on remote-sync, which starts at its default state, so
    # `space` steps it to the other one.
    local log="$TEMP_BASE_DIR/config-tui-behavior.log"
    _config_tui_daft python3 "$CONFIG_TUI_PTY_RUN" \
        --send-after 'Daft Settings: ' \
        --send-after 'Set remote-sync:q' \
        "$log" \
        daft config
    local status=$?

    if [[ $status -ne 0 ]]; then
        log_error "The settings screen exited $status (expected 0)"
        _config_tui_clean "$log" | tail -20
        return 1
    fi

    local key
    for key in daft.checkout.fetch daft.checkout.push daft.branchDelete.remote; do
        local stored
        stored=$(git config --local --get "$key")
        if [[ "$stored" != "true" ]]; then
            log_error "$key did not reach git config (got '${stored:-unset}')"
            _config_tui_clean "$log" | tail -20
            return 1
        fi
    done
    log_success "one keystroke wrote all three of the behavior's settings"

    # And the narration says where it landed, not just what it wrote.
    local painted
    painted=$(_config_tui_clean "$log")
    if [[ "$painted" != *"now reads Full sync"* ]]; then
        log_error "The screen never reported the resulting state"
        printf '%s\n' "$painted" | tail -20
        return 1
    fi
    log_success "and reported the state it left the behavior in"

    return 0
}

# Without a terminal the bare command prints the list rather than hanging
test_config_tui_falls_back_without_a_terminal() {
    local remote_repo=$(create_test_remote "test-repo-config-notty" "main")
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-config-notty/main" || return 1

    # Piped: no TTY on stderr, so there is nothing to take over.
    local output
    output=$(daft config 2>&1 | head -40)

    if [[ "$output" != *"daft.autocd"* ]]; then
        log_error "A piped 'daft config' printed no settings; got: $output"
        return 1
    fi
    log_success "a piped bare command lists instead of opening a screen"

    return 0
}

# Run all config TUI tests
run_config_tui_tests() {
    log "Running config settings screen integration tests..."

    run_test "config_tui_paints_and_exits" "test_config_tui_paints_and_exits"
    run_test "config_tui_write_lands" "test_config_tui_write_lands"
    run_test "config_tui_behavior_write_lands" "test_config_tui_behavior_write_lands"
    run_test "config_tui_falls_back_without_a_terminal" "test_config_tui_falls_back_without_a_terminal"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_config_tui_tests
    print_summary
fi
