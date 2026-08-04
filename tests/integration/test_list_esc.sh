#!/bin/bash

# Integration tests for `Esc` on the live list (#826).
#
# What lives here vs. in unit tests: the *semantics* of the two-stage key
# (abandon, then exit) and of every cell shape it produces are unit-tested in
# `output::tui::{driver,live_table,render}` — those assert precisely and never
# flake. These tests cover what a unit test structurally cannot: that the key
# reaches a real terminal, through raw mode, into a real collector, and that
# the run still ends by itself afterwards.
#
# This cannot be a YAML scenario. The manual-test runner sets `DAFT_TESTING` on
# every step, and the live table only renders on a TTY — so the whole subject
# of these tests is invisible there.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

ESC_PTY_RUN="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/pty_run.py"

# Run daft with the live table actually enabled.
#
# The framework exports DAFT_TESTING to keep the suite quiet; that also turns
# daft's palette off, so it comes off here and the three background spawns it
# was standing in for are suppressed individually instead — no orphaned
# update-check / trust-prune / log-clean daemons.
#
# TERM is load-bearing and CI is where its absence bites: a bare runner has no
# TERM at all, and an uncolored render makes the cell-shape assertions below
# meaningless. Pin a color-capable one. (Locally TERM is already xterm-*, which
# is how this kind of thing reaches CI green-locally/red-remotely.)
_esc_daft() {
    env -u DAFT_TESTING \
        TERM=xterm-256color \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# Strip ANSI and split the pty's carriage-returned repaints into lines.
_esc_clean() {
    sed -e 's/\x1b\[[0-9;?]*[a-zA-Z]//g' -e 's/\r/\n/g' "$1"
}

# A directory tree fat enough that a single-threaded size walk is still running
# when the keypress lands a few tens of milliseconds into the run.
#
# Directory *count* is the lever, not byte count: the walk is readdir-bound and
# its queue is drained one directory at a time under `DAFT_SIZE_WALK_JOBS=1`.
# The tree is gitignored so `git status` still collapses to a single entry —
# the essential cells must stay fast, or the tests would pass for the wrong
# reason (a slow essential cell keeps the table up just as well as a slow walk).
_esc_plant_tree() {
    python3 - "$1" <<'PY'
import os, sys
root = os.path.join(sys.argv[1], ".esc-bulk")
for i in range(500):
    d = os.path.join(root, "d%03d" % i, "s%03d" % i)
    os.makedirs(d, exist_ok=True)
    for j in range(4):
        with open(os.path.join(d, "f%d" % j), "wb") as fh:
            fh.write(b"x" * 256)
PY
}

# A repo with three worktrees, each carrying a fat tree.
_esc_fixture() {
    local name="$1"
    local remote_repo
    remote_repo=$(create_test_remote "$name" "master") || return 1
    # Ignore the bulk tree at the source, so every worktree inherits it.
    (
        cd "$REMOTE_REPO_DIR" || exit 1
        git clone -q "$remote_repo" "esc-seed-$name" || exit 1
        cd "esc-seed-$name" || exit 1
        echo ".esc-bulk/" >.gitignore
        git add .gitignore || exit 1
        GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
            GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@test.com" \
            git commit -q -m "ignore bulk tree" || exit 1
        git push -q origin master || exit 1
    ) || return 1
    git -C "$remote_repo" branch feat-a master || return 1
    git -C "$remote_repo" branch feat-b master || return 1
    git-worktree-clone --layout contained "$remote_repo" || return 1
    local root="$PWD/$name"
    # Each checkout runs in a subshell from a known-good directory inside the
    # repo: relying on inherited cwd made this order-dependent elsewhere.
    (cd "$root/master" && git-worktree-checkout feat-a) || return 1
    (cd "$root/master" && git-worktree-checkout feat-b) || return 1
    _esc_plant_tree "$root/master" || return 1
    _esc_plant_tree "$root/feat-a" || return 1
    _esc_plant_tree "$root/feat-b" || return 1
    cd "$root/master" || return 1
    return 0
}

# The headline behaviour: Esc drops the size walk, the run keeps going until
# the essential cells land, and the cached figures stay on screen.
#
# The `waiting for` assertion is doing double duty — it is the acknowledgement
# under test, and it is also the proof that the key was processed as an
# abandon. The footer renders on no other path, so a run whose walk finished
# before the key arrived fails here rather than passing vacuously.
test_list_esc_abandons_the_walk_and_finishes() {
    _esc_fixture "esc-abandon" || return 1
    local log="$TEMP_BASE_DIR/esc-abandon.log"

    # Warm run, unthrottled: it completes and persists the sizes that the
    # throttled run below then shows from cache. Skip it and there is no
    # cached figure to assert on.
    _esc_daft python3 "$ESC_PTY_RUN" "$TEMP_BASE_DIR/esc-warm.log" \
        daft list --columns +size >/dev/null 2>&1

    # One walker, so the walk outlasts the first frame. A branch name is the
    # cue: it paints in frame one, before any patch has had time to land.
    _esc_daft env DAFT_SIZE_WALK_JOBS=1 python3 "$ESC_PTY_RUN" \
        --send-after 'feat-a:\x1b' "$log" \
        daft list --columns +size >/dev/null 2>&1
    local rc=$?

    local out
    out=$(_esc_clean "$log")

    if [[ $rc -ne 0 ]]; then
        log_error "abandoning should still be a successful run; got exit $rc"
        return 1
    fi
    if ! grep -q "waiting for" <<<"$out"; then
        log_error "Esc produced no acknowledgement — the abandon never registered"
        return 1
    fi
    if ! grep -q "esc again to exit" <<<"$out"; then
        log_error "the acknowledgement did not offer the way out"
        return 1
    fi
    # The cached figure is the whole point: abandoning must not blank a cell
    # that already had a last-known value.
    if ! grep -qE '[0-9]+(\.[0-9]+)?[BKMG]' <<<"$out"; then
        log_error "no size figure survived the abandon"
        return 1
    fi
    log_success "Esc abandons the walk and the run finishes on its own"
    return 0
}

# The second stage. The two-stage state machine is unit-tested; what this adds
# is that a second Esc arriving on a real terminal ends the process rather than
# being swallowed or wedging the run.
#
# The second cue is the footer the first Esc just produced — text the next
# frame genuinely writes, which is what `--send-after` needs (a cue that is
# merely already on screen is never resent; ratatui sends changed cells only).
test_list_esc_twice_exits() {
    _esc_fixture "esc-twice" || return 1
    local log="$TEMP_BASE_DIR/esc-twice.log"

    _esc_daft env DAFT_SIZE_WALK_JOBS=1 python3 "$ESC_PTY_RUN" \
        --send-after 'feat-a:\x1b' \
        --send-after 'waiting for:\x1b' \
        "$log" \
        daft list --columns +size >/dev/null 2>&1
    local rc=$?

    local out
    out=$(_esc_clean "$log")

    if [[ $rc -ne 0 ]]; then
        log_error "a second Esc should exit cleanly; got exit $rc"
        return 1
    fi
    if ! grep -q "waiting for" <<<"$out"; then
        log_error "the first Esc never registered, so the second proves nothing"
        return 1
    fi
    log_success "a second Esc exits the live list"
    return 0
}

# `daft repo list` answers Esc differently — the size walk is its only async
# work, so there are no essential cells to wait for and the key ends the run
# outright. Asserted here because it is a different `LiveScreen` impl with its
# own `on_escape`, wired through the same driver arm.
test_repo_list_esc_exits_immediately() {
    _esc_fixture "esc-repo" || return 1
    local log="$TEMP_BASE_DIR/esc-repo.log"

    # A catalog of its own, holding only this fixture. The suite-wide catalog
    # accumulates every earlier fixture, and those directories are gone by now
    # — each renders a didn't-load marker of its own, which would satisfy the
    # assertion below no matter what Esc did. (That is not hypothetical: the
    # control run at the end of this test caught exactly that.)
    local iso="$TEMP_BASE_DIR/esc-repo-catalog"
    mkdir -p "$iso" || return 1
    # Registration is ambient, so any command run inside the repo enrols it.
    # No size column here: this must not warm the walk it is about to measure.
    _esc_daft env DAFT_DATA_DIR="$iso" daft list >/dev/null 2>&1

    _esc_daft env DAFT_DATA_DIR="$iso" DAFT_SIZE_WALK_JOBS=1 python3 "$ESC_PTY_RUN" \
        --send-after 'esc-repo:\x1b' "$log" \
        daft repo list --columns +size >/dev/null 2>&1
    local rc=$?

    local out
    out=$(_esc_clean "$log")

    if [[ $rc -ne 0 ]]; then
        log_error "Esc on repo list should exit cleanly; got exit $rc"
        return 1
    fi
    if ! grep -q "esc-repo" <<<"$out"; then
        log_error "repo list never rendered the cataloged repo"
        return 1
    fi
    # The proof that Esc actually ended the run: the walk was still going, so
    # the Size cell never got a figure and settles to the didn't-load marker.
    if ! grep -q $'—' <<<"$out"; then
        log_error "repo list completed its walk — Esc did not end the run"
        return 1
    fi
    # There is no wait to acknowledge here, so the live list's footer must not
    # appear — that would mean repo list took the abandon-and-wait path.
    if grep -q "esc again to exit" <<<"$out"; then
        log_error "repo list offered a second-Esc exit it has no use for"
        return 1
    fi

    # The control. Same command, same fixture, no keypress — and the marker
    # must be gone, replaced by a real figure. Without this the assertion above
    # would also be satisfied by a marker that had nothing to do with Esc, and
    # the test would keep passing after `CatalogTable::on_escape` stopped
    # working. It runs second because the Esc run must find a cold cache: a
    # completed walk persists its size, and the next run would seed from it and
    # render the cached figure instead of the marker.
    local baseline="$TEMP_BASE_DIR/esc-repo-baseline.log"
    _esc_daft env DAFT_DATA_DIR="$iso" python3 "$ESC_PTY_RUN" "$baseline" \
        daft repo list --columns +size >/dev/null 2>&1
    local base_out
    base_out=$(_esc_clean "$baseline")
    if grep -q $'—' <<<"$base_out"; then
        log_error "repo list shows the didn't-load marker even without Esc"
        return 1
    fi
    if ! grep -qE '[0-9]+(\.[0-9]+)?[BKMG]' <<<"$base_out"; then
        log_error "the control run produced no size figure to compare against"
        return 1
    fi
    log_success "Esc exits repo list outright"
    return 0
}

# Esc must stay inert on a mutating run. `daft sync`'s table is the same
# `TuiState`, and the opt-in defaults to false precisely so a stray keypress
# cannot abort a rebase or a push — this is the regression test for that
# default being wired the right way round.
test_sync_ignores_esc() {
    _esc_fixture "esc-sync" || return 1
    local log="$TEMP_BASE_DIR/esc-sync.log"

    _esc_daft python3 "$ESC_PTY_RUN" \
        --send-after 'feat-a:\x1b' "$log" \
        daft sync >/dev/null 2>&1
    local rc=$?

    local out
    out=$(_esc_clean "$log")

    if [[ $rc -ne 0 ]]; then
        log_error "Esc must not disturb a sync; got exit $rc"
        return 1
    fi
    if grep -qi "cancelled" <<<"$out"; then
        log_error "Esc cancelled a sync — the opt-in default is inverted"
        return 1
    fi
    if grep -q "esc again to exit" <<<"$out"; then
        log_error "sync offered the live list's abandon footer"
        return 1
    fi
    log_success "Esc is inert on a mutating run"
    return 0
}

# --- Test runner ---
run_list_esc_tests() {
    run_test "list_esc_abandons_the_walk_and_finishes" test_list_esc_abandons_the_walk_and_finishes
    run_test "list_esc_twice_exits" test_list_esc_twice_exits
    run_test "repo_list_esc_exits_immediately" test_repo_list_esc_exits_immediately
    run_test "sync_ignores_esc" test_sync_ignores_esc
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_list_esc_tests
    print_summary
    exit $?
fi
