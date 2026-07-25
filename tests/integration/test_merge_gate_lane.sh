#!/usr/bin/env bash

# Integration test for the cross-process merge gate lane (#775).
#
# Two concurrent gated merges in one repository must serialize: the second
# waits for the lane (announcing who holds it) and completes after the first
# releases. The holding hook is a plain sleep — never a real test suite —
# so the test proves ordering without shared-scratch hazards.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

test_gate_lane_serializes_concurrent_merges() {
    git-worktree-init --layout contained lane-repo >/dev/null 2>&1 || return 1
    cd "lane-repo/master" || return 1

    cat > daft.yml <<'EOF'
hooks:
  pre-merge:
    jobs:
      - name: slow-ring
        run: sleep 3
EOF
    git add daft.yml
    git commit -q -m "gate config" || return 1
    daft hooks trust --force >/dev/null 2>&1 || return 1

    # Two tracks, each with one commit on top of master.
    git-worktree-checkout -b track-a >/dev/null 2>&1 || return 1
    (cd ../track-a && git commit -q --allow-empty -m "a work") || return 1
    git-worktree-checkout -b track-b >/dev/null 2>&1 || return 1
    (cd ../track-b && git commit -q --allow-empty -m "b work") || return 1

    # Merge #1 in the background: acquires the lane, then sleeps in its ring.
    local m1_log="$TEMP_BASE_DIR/lane-m1.log"
    daft merge track-a --no-edit > "$m1_log" 2>&1 &
    local m1_pid=$!

    # Give it time to reach the hook (lane held from before policy checks).
    sleep 1

    # Merge #2 must announce the wait and land after #1 releases.
    local m2_out
    if ! m2_out=$(daft merge track-b --no-edit 2>&1); then
        log_error "second merge failed: $m2_out"
        kill "$m1_pid" 2>/dev/null
        return 1
    fi

    if ! wait "$m1_pid"; then
        log_error "first merge failed: $(cat "$m1_log")"
        return 1
    fi

    if ! grep -q "waiting for the merge gate lane" <<< "$m2_out"; then
        log_error "second merge did not announce the lane wait: $m2_out"
        return 1
    fi
    log_success "queued merge announced the lane holder and waited"

    # Both landed: master contains both track commits.
    if git merge-base --is-ancestor track-a HEAD && git merge-base --is-ancestor track-b HEAD; then
        log_success "both merges landed after serializing through the lane"
    else
        log_error "expected both tracks to be merged into master"
        return 1
    fi

    return 0
}

# A queued merge must validate the repository it will ACT on, not the one it
# saw before it started waiting.
#
# The lane is acquired before every state-observing check for this reason. If
# the target-safety rails ran first, merge #2 would approve a clean target at
# t=1s, block on the lane for the length of merge #1's ring, then wake and
# merge into whatever the target had become — including a mid-merge or dirty
# tree, which is precisely what those rails exist to refuse.
test_gate_lane_rechecks_target_state_after_waiting() {
    git-worktree-init --layout contained lane-recheck >/dev/null 2>&1 || return 1
    cd "lane-recheck/master" || return 1

    cat > daft.yml <<'EOF'
hooks:
  pre-merge:
    jobs:
      - name: slow-ring
        run: sleep 4
EOF
    git add daft.yml
    git commit -q -m "gate config" || return 1
    daft hooks trust --force >/dev/null 2>&1 || return 1

    git-worktree-checkout -b track-a >/dev/null 2>&1 || return 1
    (cd ../track-a && git commit -q --allow-empty -m "a work") || return 1
    git-worktree-checkout -b track-b >/dev/null 2>&1 || return 1
    (cd ../track-b && git commit -q --allow-empty -m "b work") || return 1

    # Merge #1 holds the lane through its sleeping ring.
    local m1_log="$TEMP_BASE_DIR/lane-recheck-m1.log"
    daft merge track-a --no-edit > "$m1_log" 2>&1 &
    local m1_pid=$!
    sleep 1

    # Merge #2 queues behind the lane while the target is still clean.
    local m2_log="$TEMP_BASE_DIR/lane-recheck-m2.log"
    daft merge track-b --no-edit > "$m2_log" 2>&1 &
    local m2_pid=$!
    sleep 1

    # Dirty the target AFTER merge #2 is already queued. Untracked, so it
    # does not disturb merge #1's own landing.
    echo "written while the lane was held" > dirtied-during-lane.txt

    wait "$m1_pid" || {
        log_error "first merge failed: $(cat "$m1_log")"
        kill "$m2_pid" 2>/dev/null
        return 1
    }

    # Merge #2 wakes, re-checks, and must refuse the now-dirty target.
    if wait "$m2_pid"; then
        log_error "queued merge landed into a target that went dirty while it waited: $(cat "$m2_log")"
        return 1
    fi

    if grep -qi "uncommitted changes" "$m2_log"; then
        log_success "queued merge re-validated the target after acquiring the lane"
    else
        log_error "expected a dirty-target refusal, got: $(cat "$m2_log")"
        return 1
    fi

    rm -f dirtied-during-lane.txt
    return 0
}

run_merge_gate_lane_tests() {
    log "Running merge gate lane integration tests..."

    run_test "gate_lane_serializes_concurrent_merges" \
        "test_gate_lane_serializes_concurrent_merges"

    run_test "gate_lane_rechecks_target_state_after_waiting" \
        "test_gate_lane_rechecks_target_state_after_waiting"
}

# Main execution when run directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_merge_gate_lane_tests
    print_summary
    exit $?
fi
