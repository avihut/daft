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

run_merge_gate_lane_tests() {
    log "Running merge gate lane integration tests..."

    run_test "gate_lane_serializes_concurrent_merges" \
        "test_gate_lane_serializes_concurrent_merges"
}

# Main execution when run directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_merge_gate_lane_tests
    print_summary
    exit $?
fi
