#!/bin/bash

# Blessed shell tests for `daft remove`'s deferred reclamation (#200) — the
# detached `daft __reap-trash` process (#447 Tier B).
#
# The deferred path only exists when background work is allowed. The YAML
# runner injects DAFT_TESTING=1, which makes the reap run inline, so a
# scenario asserting "nothing is left in the trash" passes even when the spawn
# is completely broken — and a broken spawn puts the whole O(files) walk back
# on the critical path, the one thing the feature exists to prevent. So this
# stays shell: DAFT_TESTING comes off, a real detached process does the work,
# and the tests wait on it (and, for the sweep, interrupt it).
#
# Moved verbatim from test_branch_delete.sh, whose other tests are YAML
# scenarios now (branch-delete/).

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# The deferred path only exists when background work is allowed, so DAFT_TESTING
# comes off — and every *other* spawn it was suppressing is turned off
# individually, leaving the reaper as the only background process in play.
_bd_daft_with_background() {
    env -u DAFT_TESTING \
        DAFT_NO_UPDATE_CHECK=1 \
        DAFT_NO_TRUST_PRUNE=1 \
        DAFT_NO_LOG_CLEAN=1 \
        DAFT_NO_HINTS=1 \
        "$@"
}

# #200: `daft remove` renames the worktree aside and hands the unlink walk to a
# detached `daft __reap-trash`. The YAML suite cannot cover that: its runner
# injects DAFT_TESTING=1, which makes the reap run inline, so a scenario
# asserting "nothing is left in the trash" passes even when the spawn is
# completely broken — and a broken spawn puts the whole O(files) walk back on
# the critical path, which is the one thing this feature exists to prevent.
#
# So: assert the diagnostic that distinguishes the two paths *first* (the inline
# fallback says "reclaiming inline", never "deferred"), and only then wait for a
# real detached process to finish the job.
test_remove_defers_reclamation_to_a_detached_reaper() {
    local remote_repo=$(create_test_remote "test-repo-bd-defer" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-bd-defer"
    local project_root
    project_root=$(pwd -P)

    git-worktree-checkout -b feature/deferred || return 1

    # Ignore the payload through info/exclude rather than a committed
    # .gitignore: a tracked file would leave the branch unpushed and an
    # untracked one would leave the worktree dirty, and either would
    # (correctly) decline the fast path, testing nothing.
    echo 'payload/' >> "$project_root/.git/info/exclude"
    mkdir -p "$project_root/feature/deferred/payload/deep"
    echo x > "$project_root/feature/deferred/payload/deep/f.txt"

    cd "$project_root/main"
    local log="$project_root/defer.log"
    if ! _bd_daft_with_background daft remove --verbose feature/deferred > "$log" 2>&1; then
        log_error "removal failed"
        cat "$log"
        return 1
    fi

    if ! grep -q "deferred: a detached reaper is reclaiming the space" "$log"; then
        log_error "removal did not defer — the detached spawn is not being taken"
        grep -iE "declined|inline|deferred" "$log" | head -5
        return 1
    fi

    if [[ -e "$project_root/feature/deferred" ]]; then
        log_error "the worktree path was not freed"
        return 1
    fi

    # Convergence: a real process, in its own session, finishes the delete.
    local trash="$project_root/.git/.daft/trash"
    local waited=0
    while [[ -n "$(ls -A "$trash" 2>/dev/null)" ]]; do
        sleep 0.2
        waited=$((waited + 1))
        if (( waited > 150 )); then
            log_error "the detached reaper never drained $trash"
            ls -la "$trash"
            return 1
        fi
    done

    return 0
}

# #200: the guard that keeps deferral from being a way to lose a worktree. A
# removal that dies between the rename and the record drop leaves the tree in
# the trash while git still points at its old path; no sweep may take it, since
# its ignored files may exist nowhere else. Simulated here by hand, because the
# window is a crash — but the sidecar that marks it is written by production
# code, and a reaper that ignored the mark would delete this tree.
test_remove_sweep_spares_an_interrupted_removal() {
    local remote_repo=$(create_test_remote "test-repo-bd-interrupt" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-bd-interrupt"
    local project_root
    project_root=$(pwd -P)

    git-worktree-checkout -b feature/interrupted || return 1
    git-worktree-checkout -b feature/bystander || return 1

    local trash="$project_root/.git/.daft/trash"
    local entry="$trash/feature-interrupted-1-2"
    mkdir -p "$trash"
    # The sidecar names where the tree came from; its presence is what says
    # "git was never told". Written before the move, exactly as dispose does.
    printf '%s' "$project_root/feature/interrupted" > "$entry.origin"
    mv "$project_root/feature/interrupted" "$entry"
    echo irreplaceable > "$entry/secret.env"

    # Any removal in the repo sweeps the trash at command start.
    cd "$project_root/main"
    _bd_daft_with_background daft remove feature/bystander > /dev/null 2>&1 || {
        log_error "the bystander removal failed"
        return 1
    }
    sleep 1

    if [[ ! -f "$entry/secret.env" ]]; then
        log_error "a sweep destroyed a worktree whose removal never completed"
        return 1
    fi
    return 0
}

# Run the reaper tests
run_remove_reaper_tests() {
    log "Running blessed deferred-reclamation integration tests..."

    # #200: deferred reclamation goes to a detached reaper
    run_test "remove_defers_reclamation_to_a_detached_reaper" "test_remove_defers_reclamation_to_a_detached_reaper"
    run_test "remove_sweep_spares_an_interrupted_removal" "test_remove_sweep_spares_an_interrupted_removal"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_remove_reaper_tests
    print_summary
    exit $?
fi
