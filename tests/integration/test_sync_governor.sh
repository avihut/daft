#!/bin/bash

# Integration tests for the sync push resource governor (#678).
#
# Unlike test_sync_cancel.sh these must drive the *parallel* (TUI) path —
# the governor only exists there — so daft runs under a DSR-answering PTY
# (pty_run.py; bare script(1) leaves crossterm's cursor query unanswered).
# Pressure states are forced through the dev-build-only
# DAFT_GOVERNOR_FORCE_STATE_FILE probe (the state-guard preflight already
# guarantees the suite runs a dev binary).

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

PTY_RUN="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pty_run.py"

# Contained-layout clone with N feature branches, each with an upstream and
# one unpushed commit, plus a caller-written pre-push hook. Sets globals:
#   GOV_MARKS  marker dir for hook synchronization
#   GOV_HOOK   path of the pre-push hook to write
setup_governor_repo() {
    local name="$1" branches="$2"
    local remote_repo
    remote_repo=$(create_test_remote "$name" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "$name" || return 1

    # Local (repo-scoped) identity: ownership classifies a branch as the
    # current user's by matching `git config user.email` — the framework's
    # isolated global config is deliberately empty (env-only identity),
    # which would leave every branch unowned and never pushed.
    (cd main && git config user.email "test@example.com" && git config user.name "Test User") ||
        return 1

    GOV_MARKS="$TEMP_BASE_DIR/${name}-marks"
    rm -rf "$GOV_MARKS"
    mkdir -p "$GOV_MARKS/running"

    local i
    for i in $(seq 1 "$branches"); do
        (
            cd main || exit 1
            git worktree add "../feat-$i" -b "feat-$i" >/dev/null 2>&1
        ) || return 1
        (
            cd "feat-$i" || exit 1
            # First push (hook not installed yet) sets the upstream…
            git push -q -u origin "feat-$i" || exit 1
            # …then one unpushed commit so sync --push has real work.
            echo "governor $i" >> README.md
            git add README.md
            git commit -q -m "governor test commit $i"
        ) || return 1
    done

    mkdir -p ".git/hooks"
    GOV_HOOK="$PWD/.git/hooks/pre-push"
    return 0
}

# Poll for a marker file with a bounded wait. Usage: await_marker <path> <what>
await_marker() {
    local path="$1" what="$2" i=0
    while [[ ! -s "$path" ]]; do
        sleep 0.2
        i=$((i + 1))
        if [[ $i -gt 200 ]]; then
            log_error "timed out waiting for $what ($path)"
            return 1
        fi
    done
    return 0
}

# Wait until either recorded hook pid freezes (ps stat T). Which of two
# same-instant units the governor picks as "newest" is not observable from
# outside, so callers assert on whichever froze. Prints the frozen branch
# name. Usage: frozen=$(await_either_frozen feat-1 feat-2)
await_either_frozen() {
    local a="$1" b="$2" i=0 pid stat branch
    while true; do
        for branch in "$a" "$b"; do
            pid=$(cat "$GOV_MARKS/$branch.pid" 2>/dev/null)
            [[ -n "$pid" ]] || continue
            stat=$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')
            case "$stat" in
                T*|t*)
                    echo "$branch"
                    return 0
                    ;;
            esac
        done
        sleep 0.2
        i=$((i + 1))
        if [[ $i -gt 100 ]]; then
            log_error "timed out waiting for a hook to freeze" >&2
            return 1
        fi
    done
}

# Dump the (TUI-escape-laden) sync output for post-mortem on failure.
dump_sync_log() {
    if [[ -f "$GOV_MARKS/out.log" ]]; then
        log_error "sync output tail:"
        tr -d '\r' < "$GOV_MARKS/out.log" | tail -12 >&2
    else
        log_error "no sync output was captured at $GOV_MARKS/out.log"
    fi
}

# Best-effort teardown: thaw + kill anything the hooks recorded.
cleanup_governor() {
    local f p
    for f in "$GOV_MARKS"/*.pid; do
        [[ -f "$f" ]] || continue
        p=$(cat "$f")
        kill -CONT "$p" 2>/dev/null
        kill -KILL "$p" 2>/dev/null
        kill -KILL -"$p" 2>/dev/null
    done
}

# All branches pushed: origin/<branch> caught up in every worktree.
assert_all_pushed() {
    local branches="$1" i behind
    for i in $(seq 1 "$branches"); do
        behind=$(cd "feat-$i" && git rev-list --count "origin/feat-$i..feat-$i" 2>/dev/null)
        if [[ "$behind" != "0" ]]; then
            log_error "feat-$i has $behind unpushed commit(s) after sync --push"
            return 1
        fi
    done
    return 0
}

# --jobs 1 must serialize hook-bearing pushes: the hook counts how many
# instances overlap; the peak must stay at 1 while all 4 branches push.
test_governor_jobs_cap_bounds_concurrency() {
    setup_governor_repo "test-repo-governor-cap" 4 || return 1
    cat > "$GOV_HOOK" <<EOF
#!/bin/sh
touch "$GOV_MARKS/running/\$\$"
ls "$GOV_MARKS/running" | wc -l >> "$GOV_MARKS/peaks"
# The governor's shared jobserver must reach the hook via MAKEFLAGS.
case "\$MAKEFLAGS" in
    *--jobserver-auth=fifo:*) echo ok >> "$GOV_MARKS/jobserver-seen" ;;
esac
sleep 1
rm -f "$GOV_MARKS/running/\$\$"
exit 0
EOF
    chmod +x "$GOV_HOOK"

    local code=0
    (
        cd feat-1 || exit 1
        python3 "$PTY_RUN" "$GOV_MARKS/out.log" git-worktree-sync --push --jobs 1
    ) || code=$?

    if [[ $code -ne 0 ]]; then
        log_error "sync --push --jobs 1 exited $code"
        cleanup_governor
        return 1
    fi

    local peak
    peak=$(sort -rn "$GOV_MARKS/peaks" 2>/dev/null | head -1)
    if [[ -z "$peak" ]]; then
        log_error "hook never ran (no peaks recorded); sync output follows"
        tr -d '\r' < "$GOV_MARKS/out.log" | tail -25 >&2
        return 1
    fi
    if [[ "$peak" -gt 1 ]]; then
        log_error "--jobs 1 violated: $peak hooks ran concurrently"
        return 1
    fi
    assert_all_pushed 4 || return 1
    if ! grep -q "throttled" "$GOV_MARKS/out.log"; then
        log_error "expected a throttle summary line in the output"
        return 1
    fi
    if [[ ! -s "$GOV_MARKS/jobserver-seen" ]]; then
        log_error "hooks never saw the jobserver MAKEFLAGS export"
        return 1
    fi
    return 0
}

# Under forced red pressure the newest of two running units freezes
# (observable as ps stat T), the freeze grace expires, the unit is killed
# and requeued, and the retry completes — the sync still succeeds.
test_governor_freeze_then_kill_requeue_recovers() {
    setup_governor_repo "test-repo-governor-kill" 2 || return 1

    local state="$GOV_MARKS/pressure"
    # Green: 20 GiB available of 32 GiB (reserve auto = 3.2 GiB).
    printf 'mem_total=34359738368\nmem_available=21474836480\nswap_used=0\n' > "$state"

    cat > "$GOV_HOOK" <<EOF
#!/bin/sh
branch=\$(git branch --show-current)
echo run >> "$GOV_MARKS/\$branch-attempts"
attempts=\$(wc -l < "$GOV_MARKS/\$branch-attempts" | tr -d ' ')
echo \$\$ > "$GOV_MARKS/\$branch.pid"
echo ok > "$GOV_MARKS/\$branch-started"
# First attempt: outlive the freeze window (a frozen sleep never ends);
# a requeued retry completes instantly.
if [ "\$attempts" -gt 1 ]; then
    exit 0
fi
sleep 6
exit 0
EOF
    chmod +x "$GOV_HOOK"

    export DAFT_GOVERNOR_FORCE_STATE_FILE="$state"
    local code=0
    (
        cd feat-1 || exit 1
        exec python3 "$PTY_RUN" "$GOV_MARKS/out.log" git-worktree-sync --push
    ) &
    local sync_job=$!

    # Both hooks running (green admits both at initial target 2)…
    if ! await_marker "$GOV_MARKS/feat-1-started" "feat-1 hook" ||
        ! await_marker "$GOV_MARKS/feat-2-started" "feat-2 hook"; then
        dump_sync_log
        cleanup_governor
        unset DAFT_GOVERNOR_FORCE_STATE_FILE
        kill -KILL "$sync_job" 2>/dev/null
        return 1
    fi
    # …then the world turns red: 1 GiB available, swap climbing.
    printf 'mem_total=34359738368\nmem_available=1073741824\nswap_used=2147483648\n' > "$state"

    # One of the two units must freeze (which one is a policy tie-break).
    local frozen_branch
    frozen_branch=$(await_either_frozen feat-1 feat-2) || {
        dump_sync_log
        cleanup_governor
        unset DAFT_GOVERNOR_FORCE_STATE_FILE
        kill -KILL "$sync_job" 2>/dev/null
        return 1
    }

    # The unfrozen sibling completes (6s); the frozen unit sits out the
    # 10s grace under sustained red, is killed and requeued; at zero
    # running units the liveness rule re-admits it (no green flip needed)
    # and the retry exits instantly — the sync still succeeds.
    wait "$sync_job"
    code=$?
    unset DAFT_GOVERNOR_FORCE_STATE_FILE

    if [[ $code -ne 0 ]]; then
        log_error "sync --push exited $code after kill-requeue"
        dump_sync_log
        cleanup_governor
        return 1
    fi
    local killed_attempts survivor_attempts survivor_branch
    if [[ "$frozen_branch" == "feat-1" ]]; then
        survivor_branch="feat-2"
    else
        survivor_branch="feat-1"
    fi
    killed_attempts=$(wc -l < "$GOV_MARKS/$frozen_branch-attempts" | tr -d ' ')
    survivor_attempts=$(wc -l < "$GOV_MARKS/$survivor_branch-attempts" | tr -d ' ')
    if [[ "$killed_attempts" -ne 2 ]]; then
        log_error "expected 2 attempts for the killed unit ($frozen_branch), saw $killed_attempts"
        return 1
    fi
    if [[ "$survivor_attempts" -ne 1 ]]; then
        log_error "the surviving unit ($survivor_branch) should run once, saw $survivor_attempts"
        return 1
    fi
    assert_all_pushed 2 || return 1
    return 0
}

# Ctrl+C while a unit is frozen: the governor stands down and thaws, the
# cancel cascade tears both hook trees down, and sync exits 130 with no
# survivors (thaw-before-terminate, #678/#663).
test_governor_ctrl_c_while_frozen_leaves_no_survivors() {
    setup_governor_repo "test-repo-governor-intfrozen" 2 || return 1

    local state="$GOV_MARKS/pressure"
    printf 'mem_total=34359738368\nmem_available=21474836480\nswap_used=0\n' > "$state"

    cat > "$GOV_HOOK" <<EOF
#!/bin/sh
branch=\$(git branch --show-current)
echo \$\$ > "$GOV_MARKS/\$branch.pid"
echo ok > "$GOV_MARKS/\$branch-started"
exec sleep 120
EOF
    chmod +x "$GOV_HOOK"

    export DAFT_GOVERNOR_FORCE_STATE_FILE="$state"
    (
        cd feat-1 || exit 1
        exec python3 "$PTY_RUN" "$GOV_MARKS/out.log" git-worktree-sync --push
    ) &
    local sync_job=$!

    if ! await_marker "$GOV_MARKS/feat-1-started" "feat-1 hook" ||
        ! await_marker "$GOV_MARKS/feat-2-started" "feat-2 hook"; then
        dump_sync_log
        cleanup_governor
        unset DAFT_GOVERNOR_FORCE_STATE_FILE
        kill -KILL "$sync_job" 2>/dev/null
        return 1
    fi
    printf 'mem_total=34359738368\nmem_available=1073741824\nswap_used=2147483648\n' > "$state"

    local frozen_branch frozen_pid
    frozen_branch=$(await_either_frozen feat-1 feat-2) || {
        dump_sync_log
        cleanup_governor
        unset DAFT_GOVERNOR_FORCE_STATE_FILE
        kill -KILL "$sync_job" 2>/dev/null
        return 1
    }
    frozen_pid=$(cat "$GOV_MARKS/$frozen_branch.pid")

    # SIGINT daft itself: the hook's grandparent (hook ← git push ← daft).
    local git_pid daft_pid
    git_pid=$(ps -o ppid= -p "$frozen_pid" | tr -d ' ')
    daft_pid=$(ps -o ppid= -p "$git_pid" | tr -d ' ')
    if [[ -z "$daft_pid" ]]; then
        log_error "could not resolve the daft pid from the frozen hook"
        cleanup_governor
        unset DAFT_GOVERNOR_FORCE_STATE_FILE
        return 1
    fi
    kill -INT "$daft_pid"

    wait "$sync_job"
    local code=$?
    unset DAFT_GOVERNOR_FORCE_STATE_FILE

    if [[ $code -ne 130 ]]; then
        log_error "expected exit 130 after SIGINT with a frozen unit, got $code"
        cleanup_governor
        return 1
    fi
    local p survivors=0
    for p in $(cat "$GOV_MARKS/feat-1.pid" "$GOV_MARKS/feat-2.pid" 2>/dev/null); do
        if kill -0 "$p" 2>/dev/null; then
            log_error "hook pid $p survived the cancel (frozen unit not thawed?)"
            survivors=1
        fi
    done
    if [[ $survivors -ne 0 ]]; then
        cleanup_governor
        return 1
    fi
    return 0
}

run_sync_governor_tests() {
    run_test "governor_jobs_cap_bounds_concurrency" "test_governor_jobs_cap_bounds_concurrency"
    run_test "governor_freeze_then_kill_requeue_recovers" "test_governor_freeze_then_kill_requeue_recovers"
    run_test "governor_ctrl_c_while_frozen_leaves_no_survivors" "test_governor_ctrl_c_while_frozen_leaves_no_survivors"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_sync_governor_tests
    print_summary
fi
