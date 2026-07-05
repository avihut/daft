#!/bin/bash

# Integration tests for daft sync graceful cancellation (#663).
#
# All tests drive the sequential (non-TTY) path: stderr is a pipe here,
# and ctrlc catches a kill-delivered SIGINT/SIGTERM without any tty.
# Each scenario installs a pre-push hook shaped like a wedge class from
# the field incident and asserts that one signal tears the whole hook
# subtree down (exit 130), or that a second Ctrl+C force-kills what the
# first one could not.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Set up a contained-layout clone with one unpushed commit (git skips the
# pre-push hook entirely when everything is up to date) and a pre-push
# hook written by the caller into "$SYNC_CANCEL_HOOK". Sets globals:
#   SYNC_CANCEL_MARKS  marker dir the hook writes pids/flags into
#   SYNC_CANCEL_HOOK   path of the pre-push hook to write
setup_sync_cancel_repo() {
    local name="$1"
    local remote_repo
    remote_repo=$(create_test_remote "$name" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "$name" || return 1

    SYNC_CANCEL_MARKS="$TEMP_BASE_DIR/${name}-marks"
    rm -rf "$SYNC_CANCEL_MARKS"
    mkdir -p "$SYNC_CANCEL_MARKS"

    (
        cd main || exit 1
        echo "cancel test" >> README.md
        git add README.md
        git commit -q -m "sync cancel test commit"
    ) || return 1

    mkdir -p ".git/hooks"
    SYNC_CANCEL_HOOK="$PWD/.git/hooks/pre-push"
    return 0
}

# Poll for a marker file with a bounded wait. Usage: await_file <path> <what>
await_file() {
    local path="$1" what="$2" i=0
    while [[ ! -s "$path" ]]; do
        sleep 0.2
        i=$((i + 1))
        if [[ $i -gt 150 ]]; then
            log_error "timed out waiting for $what ($path)"
            return 1
        fi
    done
    return 0
}

# Best-effort teardown for a failed test: never leave a wedged tree behind
# (SIGKILL is the one terminating signal a stopped process acts on).
cleanup_sync_cancel() {
    local daft_pid="$1"
    for f in "$SYNC_CANCEL_MARKS"/*.pid; do
        [[ -f "$f" ]] || continue
        local p
        p=$(cat "$f")
        kill -KILL "$p" 2>/dev/null
        kill -KILL -"$p" 2>/dev/null
    done
    [[ -n "$daft_pid" ]] && kill -KILL "$daft_pid" 2>/dev/null
}

# One SIGINT during the pre-push hook must gracefully cancel: exit 130
# within a bounded window, with the hook process dead.
test_sync_cancel_single_sigint_kills_hook() {
    setup_sync_cancel_repo "test-repo-sync-cancel-int" || return 1
    cat > "$SYNC_CANCEL_HOOK" <<EOF
#!/bin/sh
echo \$\$ > $SYNC_CANCEL_MARKS/hook.pid
echo ok > $SYNC_CANCEL_MARKS/hook-started
exec sleep 60
EOF
    chmod +x "$SYNC_CANCEL_HOOK"

    git-worktree-sync --push > "$SYNC_CANCEL_MARKS/out.log" 2>&1 &
    local daft_pid=$!

    if ! await_file "$SYNC_CANCEL_MARKS/hook-started" "pre-push hook to start"; then
        cleanup_sync_cancel "$daft_pid"
        return 1
    fi

    local t0=$SECONDS
    kill -INT "$daft_pid"
    wait "$daft_pid"
    local code=$?
    local elapsed=$((SECONDS - t0))

    if [[ $code -ne 130 ]]; then
        log_error "expected exit 130 after SIGINT, got $code"
        cleanup_sync_cancel ""
        return 1
    fi
    if [[ $elapsed -gt 15 ]]; then
        log_error "graceful cancel took ${elapsed}s (expected prompt teardown)"
        return 1
    fi
    local hook_pid
    hook_pid=$(cat "$SYNC_CANCEL_MARKS/hook.pid")
    if kill -0 "$hook_pid" 2>/dev/null; then
        log_error "hook process $hook_pid survived the soft cancel"
        cleanup_sync_cancel ""
        return 1
    fi
    return 0
}

# A TERM-immune hook that keeps the pipe write-ends open must NOT wedge
# the soft cancel (daft keeps watching the flag), and a second SIGINT
# must SIGKILL it. This is the run_push pipe-holder regression.
test_sync_cancel_second_sigint_force_kills() {
    setup_sync_cancel_repo "test-repo-sync-cancel-int2" || return 1
    cat > "$SYNC_CANCEL_HOOK" <<EOF
#!/bin/sh
trap '' TERM INT
echo \$\$ > $SYNC_CANCEL_MARKS/hook.pid
echo ok > $SYNC_CANCEL_MARKS/hook-started
while :; do sleep 1; done
EOF
    chmod +x "$SYNC_CANCEL_HOOK"

    git-worktree-sync --push > "$SYNC_CANCEL_MARKS/out.log" 2>&1 &
    local daft_pid=$!

    if ! await_file "$SYNC_CANCEL_MARKS/hook-started" "pre-push hook to start"; then
        cleanup_sync_cancel "$daft_pid"
        return 1
    fi

    kill -INT "$daft_pid"
    # The hook ignores TERM and holds git's stderr open: daft must still
    # be alive and polling (NOT wedged in a pipe-drain join), waiting for
    # the escalation.
    sleep 3
    if ! kill -0 "$daft_pid" 2>/dev/null; then
        # It exited — acceptable only if it force-cleaned already, but a
        # TERM-immune holder should require the second signal. Treat an
        # early exit with 130 + dead hook as a pass (stronger teardown
        # than required); anything else is a failure.
        wait "$daft_pid"
        local early_code=$?
        local early_hook
        early_hook=$(cat "$SYNC_CANCEL_MARKS/hook.pid")
        if [[ $early_code -ne 130 ]] || kill -0 "$early_hook" 2>/dev/null; then
            log_error "daft exited early (code $early_code) leaving the hook behind"
            cleanup_sync_cancel ""
            return 1
        fi
        return 0
    fi

    local t0=$SECONDS
    kill -INT "$daft_pid"
    wait "$daft_pid"
    local code=$?
    local elapsed=$((SECONDS - t0))

    if [[ $code -ne 130 ]]; then
        log_error "expected exit 130 after second SIGINT, got $code"
        cleanup_sync_cancel ""
        return 1
    fi
    if [[ $elapsed -gt 10 ]]; then
        log_error "hard cancel took ${elapsed}s (expected near-immediate exit)"
        return 1
    fi
    local hook_pid
    hook_pid=$(cat "$SYNC_CANCEL_MARKS/hook.pid")
    if kill -0 "$hook_pid" 2>/dev/null; then
        log_error "TERM-immune hook $hook_pid survived the SIGKILL cascade"
        cleanup_sync_cancel ""
        return 1
    fi
    return 0
}

# The #663 incident in miniature: the hook parks a descendant in its OWN
# process group and that group is job-control-stopped (T state). A
# stopped process only acts on a pending TERM after SIGCONT — one SIGINT
# must unstick and kill it (TERM+CONT by group), not wedge.
test_sync_cancel_unsticks_stopped_hook_group() {
    setup_sync_cancel_repo "test-repo-sync-cancel-stop" || return 1
    cat > "$SYNC_CANCEL_HOOK" <<EOF
#!/bin/sh
echo \$\$ > $SYNC_CANCEL_MARKS/hook.pid
perl -e 'setpgrp(0,0); open(F, ">", "$SYNC_CANCEL_MARKS/stopped.pid") or exit 1; print F \$\$; close F; kill("STOP", \$\$); sleep 60' &
child=\$!
echo ok > $SYNC_CANCEL_MARKS/hook-started
wait \$child
EOF
    chmod +x "$SYNC_CANCEL_HOOK"

    git-worktree-sync --push > "$SYNC_CANCEL_MARKS/out.log" 2>&1 &
    local daft_pid=$!

    if ! await_file "$SYNC_CANCEL_MARKS/stopped.pid" "self-stopping hook child"; then
        cleanup_sync_cancel "$daft_pid"
        return 1
    fi
    local stopped_pid
    stopped_pid=$(cat "$SYNC_CANCEL_MARKS/stopped.pid")

    # Confirm the descendant really reached T state before cancelling.
    local i=0
    while :; do
        local stat
        stat=$(ps -o stat= -p "$stopped_pid" 2>/dev/null | tr -d ' ')
        [[ "$stat" == T* ]] && break
        i=$((i + 1))
        if [[ $i -gt 50 ]]; then
            log_error "hook child never reached stopped state (stat: ${stat:-gone})"
            cleanup_sync_cancel "$daft_pid"
            return 1
        fi
        sleep 0.2
    done

    local t0=$SECONDS
    kill -INT "$daft_pid"
    wait "$daft_pid"
    local code=$?
    local elapsed=$((SECONDS - t0))

    if [[ $code -ne 130 ]]; then
        log_error "expected exit 130, got $code"
        cleanup_sync_cancel ""
        return 1
    fi
    if [[ $elapsed -gt 15 ]]; then
        log_error "unsticking the stopped group took ${elapsed}s"
        return 1
    fi
    if kill -0 "$stopped_pid" 2>/dev/null; then
        local stat
        stat=$(ps -o stat= -p "$stopped_pid" 2>/dev/null)
        log_error "stopped descendant $stopped_pid survived cancel (stat: $stat)"
        cleanup_sync_cancel ""
        return 1
    fi
    local hook_pid
    hook_pid=$(cat "$SYNC_CANCEL_MARKS/hook.pid")
    if kill -0 "$hook_pid" 2>/dev/null; then
        log_error "hook process $hook_pid survived cancel"
        cleanup_sync_cancel ""
        return 1
    fi
    return 0
}

# SIGTERM (kill <daft>, closing terminal) takes the same graceful path
# as Ctrl+C — the ctrlc `termination` feature contract.
test_sync_cancel_sigterm_parity() {
    setup_sync_cancel_repo "test-repo-sync-cancel-term" || return 1
    cat > "$SYNC_CANCEL_HOOK" <<EOF
#!/bin/sh
echo \$\$ > $SYNC_CANCEL_MARKS/hook.pid
echo ok > $SYNC_CANCEL_MARKS/hook-started
exec sleep 60
EOF
    chmod +x "$SYNC_CANCEL_HOOK"

    git-worktree-sync --push > "$SYNC_CANCEL_MARKS/out.log" 2>&1 &
    local daft_pid=$!

    if ! await_file "$SYNC_CANCEL_MARKS/hook-started" "pre-push hook to start"; then
        cleanup_sync_cancel "$daft_pid"
        return 1
    fi

    kill -TERM "$daft_pid"
    wait "$daft_pid"
    local code=$?

    if [[ $code -ne 130 ]]; then
        log_error "expected exit 130 after SIGTERM, got $code"
        cleanup_sync_cancel ""
        return 1
    fi
    local hook_pid
    hook_pid=$(cat "$SYNC_CANCEL_MARKS/hook.pid")
    if kill -0 "$hook_pid" 2>/dev/null; then
        log_error "hook process $hook_pid survived SIGTERM cancel"
        cleanup_sync_cancel ""
        return 1
    fi
    return 0
}

run_sync_cancel_tests() {
    run_test "sync_cancel_single_sigint_kills_hook" "test_sync_cancel_single_sigint_kills_hook"
    run_test "sync_cancel_second_sigint_force_kills" "test_sync_cancel_second_sigint_force_kills"
    run_test "sync_cancel_unsticks_stopped_hook_group" "test_sync_cancel_unsticks_stopped_hook_group"
    run_test "sync_cancel_sigterm_parity" "test_sync_cancel_sigterm_parity"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_sync_cancel_tests
    print_summary
fi
