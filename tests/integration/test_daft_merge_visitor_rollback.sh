#!/usr/bin/env bash

# Integration tests for visitor-config atomic propagation on daft merge.
#
# Verifies that:
#   - When daft merge conflicts, the target worktree's pre-existing untracked
#     daft.yml is restored to its pre-merge state (rollback).
#   - When daft merge succeeds, the propagated daft.yml persists in the target.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test that a conflicting daft merge rolls back daft.yml in the target worktree.
#
# Setup:
#   master  : has visitor daft.yml ("master-original" marker) + shared.txt ("v2-from-master")
#   feat/conflict: has visitor daft.yml ("feat-conflict" marker) + shared.txt ("v2-from-feat")
#
# The merge of feat/conflict into master will conflict on shared.txt.
# propagate_atomic must overlay feat/conflict's daft.yml onto master's before the
# merge runs, then restore master's original daft.yml when the merge fails.
test_daft_merge_visitor_rollback_on_conflict() {
    git-worktree-init --layout contained merge-rollback-repo || return 1
    cd "merge-rollback-repo/master"

    # Seed a tracked file that we will make conflict across both branches.
    echo "v1" > shared.txt
    git add shared.txt
    git commit -m "v1" >/dev/null 2>&1

    # Create feat/conflict worktree and make a conflicting change.
    git-worktree-checkout -b feat/conflict >/dev/null 2>&1

    local repo_root
    repo_root="$(dirname "$(pwd)")"

    # Commit a diverging change in feat/conflict AND place a visitor daft.yml there.
    cd "$repo_root/feat/conflict"
    echo "v2-from-feat" > shared.txt
    git commit -am "feat/conflict: v2" >/dev/null 2>&1

    # Place a visitor daft.yml in feat/conflict (untracked = visitor).
    # This is the SOURCE of propagation; its content will be overlaid onto master.
    cat > daft.yml <<'EOF'
hooks:
  post-clone:
    jobs:
      - run: echo feat-conflict
EOF

    # Back to master — commit a different change to force a merge conflict.
    cd "$repo_root/master"
    echo "v2-from-master" > shared.txt
    git commit -am "master: conflict-v2" >/dev/null 2>&1

    # Place a visitor daft.yml in master with a distinctive marker.
    # This is the TARGET's pre-existing daft.yml; it must be restored on rollback.
    cat > daft.yml <<'EOF'
hooks:
  post-clone:
    jobs:
      - run: echo master-original
EOF
    local original_content
    original_content="$(cat daft.yml)"

    # Attempt to merge feat/conflict into master (no --into = current branch is target).
    # This should fail with a conflict on shared.txt.
    # Pass --no-edit to avoid hanging on editor in non-TTY test environment.
    set +e
    daft merge feat/conflict --no-edit >/dev/null 2>&1
    local merge_exit=$?
    set -e

    if [[ "$merge_exit" -eq 0 ]]; then
        log_error "daft merge unexpectedly succeeded — expected a conflict"
        git merge --abort 2>/dev/null || true
        return 1
    fi

    # daft.yml in master must be restored to its pre-merge state.
    if [[ ! -f "daft.yml" ]]; then
        log_error "daft.yml was removed after failed merge (should be restored)"
        git merge --abort 2>/dev/null || true
        return 1
    fi

    local now_content
    now_content="$(cat daft.yml)"
    if [[ "$original_content" != "$now_content" ]]; then
        log_error "daft.yml was not restored after failed merge"
        log_error "--- expected (master-original):"
        log_error "$original_content"
        log_error "--- actual:"
        log_error "$now_content"
        git merge --abort 2>/dev/null || true
        return 1
    fi

    log_success "daft.yml was correctly restored after conflicting merge"

    # Abort the in-progress merge so the test directory can be cleaned up.
    git merge --abort 2>/dev/null || true
    return 0
}

# Test that a successful daft merge propagates and persists visitor daft.yml.
#
# Setup:
#   master  : NO visitor daft.yml (clean target — passes pre-flight clean check)
#   feat/add: visitor daft.yml ("from-feat-add" marker) + non-conflicting feature.txt
#
# After a successful merge, master should have the propagated daft.yml from feat/add.
# requireCleanTarget is left at its default (true) — the fix ensures the clean check
# fires BEFORE propagation writes daft.yml into the target, so there is no false positive.
test_daft_merge_visitor_propagates_on_success() {
    git-worktree-init --layout contained merge-propagate-repo || return 1
    cd "merge-propagate-repo/master"

    # Initial commit on master — no daft.yml (clean target).
    echo "hello" > README.md
    git add README.md
    git commit -m "init" >/dev/null 2>&1

    # Create feat/add and add a commit that does NOT conflict with master.
    git-worktree-checkout -b feat/add >/dev/null 2>&1

    local repo_root
    repo_root="$(dirname "$(pwd)")"

    # Commit a non-conflicting file in feat/add.
    cd "$repo_root/feat/add"
    echo "feature content" > feature.txt
    git add feature.txt
    git commit -m "feat/add: add feature.txt" >/dev/null 2>&1

    # Place a visitor daft.yml in feat/add (the source being merged in).
    # This file is untracked = visitor, so propagation will carry it to master.
    cat > daft.yml <<'EOF'
hooks:
  post-clone:
    jobs:
      - run: echo from-feat-add
EOF

    # Back to master — merge feat/add (should succeed cleanly).
    # requireCleanTarget is at its default (true); the pre-flight check runs
    # before propagation, so master's clean state is verified correctly.
    cd "$repo_root/master"
    daft merge feat/add --no-edit >/dev/null 2>&1 || {
        log_error "daft merge feat/add failed unexpectedly"
        return 1
    }

    # After a successful merge, master should have the propagated daft.yml.
    assert_file_exists "daft.yml" \
        "daft.yml should be present in master after successful merge" || return 1
    assert_file_contains "daft.yml" "from-feat-add" \
        "master's daft.yml should contain the propagated content" || return 1

    return 0
}

# Test that a successful daft merge consolidates a REFINED source daft.yml into a
# target that already has its own copy, via the three-way merge3 path (seed as
# base), preserving every field of both sides.
#
# Setup:
#   master  : visitor daft.yml (post-clone job + top-level shared:) — written
#             BEFORE branch-out so checkout propagates and seeds it
#   feat/add: the seeded copy REFINED with an extra hook section, an
#             overlay-only top-level field (extends — the historical drop
#             site), and a hook-level scalar (background: true), plus a
#             non-conflicting commit
#
# The refinements are one-sided changes against the seed, so merge3 adopts them
# without conflicts and re-serializes the target. A field drop or null-litter in
# merge3/merge3_hook_defs surfaces directly here.
test_daft_merge_visitor_merges_into_existing_target_daft_yml() {
    git-worktree-init --layout contained merge-consolidate-repo || return 1
    cd "merge-consolidate-repo/master"

    echo "hello" > README.md
    git add README.md
    git commit -m "init" >/dev/null 2>&1

    # Target (master) gets its visitor daft.yml BEFORE branch-out: a post-clone
    # hook plus a top-level shared list. Checkout propagation copies this into
    # feat/add and records it as feat/add's seed — the three-way merge base.
    cat > daft.yml <<'EOF'
shared:
  - .env
hooks:
  post-clone:
    jobs:
      - name: from-master
        run: echo from-master
EOF

    # Keep daft.yml a visitor config: exclude it so it never makes the target
    # "dirty". The merge clean-target pre-flight treats untracked files as dirty
    # (git status --porcelain), so an un-excluded daft.yml would refuse the merge
    # before consolidation. The shared info/exclude covers feat/add's copy too.
    echo '/daft.yml' >> "$(git rev-parse --git-path info/exclude)"

    git-worktree-checkout -b feat/add >/dev/null 2>&1

    local repo_root
    repo_root="$(dirname "$(pwd)")"

    # Source (feat/add): a non-conflicting commit, then REFINE the seeded copy —
    # keep the seeded content and add a different hook section, an overlay-only
    # top-level field (extends), and a hook-level scalar (background: true).
    cd "$repo_root/feat/add"
    echo "feature content" > feature.txt
    git add feature.txt
    git commit -m "feat/add: add feature.txt" >/dev/null 2>&1

    assert_file_contains "daft.yml" "from-master" \
        "checkout must have propagated master's daft.yml into feat/add" || return 1

    cat > daft.yml <<'EOF'
shared:
  - .env
extends:
  - base.yml
hooks:
  post-clone:
    jobs:
      - name: from-master
        run: echo from-master
  worktree-post-create:
    background: true
    jobs:
      - name: from-feat
        run: echo from-feat
EOF

    # Merge feat/add into master. The source copy is refined relative to its
    # seed, so the consolidation runs merge3 (base = seed, ours = master,
    # theirs = feat/add): the refinements are one-sided adds, no conflicts, and
    # the resolved config is written into master before the git merge.
    cd "$repo_root/master"
    daft merge feat/add --no-edit >/dev/null 2>&1 || {
        log_error "daft merge feat/add failed unexpectedly"
        return 1
    }

    assert_file_exists "daft.yml" \
        "master daft.yml should be present after the consolidation merge" || return 1
    assert_file_contains "daft.yml" "post-clone" \
        "target's own hook section must survive the merge" || return 1
    assert_file_contains "daft.yml" "worktree-post-create" \
        "source hook section must be merged in" || return 1
    assert_file_contains "daft.yml" "shared" \
        "target's top-level shared list must survive" || return 1
    assert_file_contains "daft.yml" "extends" \
        "overlay-only top-level field (historical drop site) must survive re-serialize" || return 1
    assert_file_contains "daft.yml" "background" \
        "hook-level scalar must survive merge3_hook_defs" || return 1

    if grep -q 'null' daft.yml; then
        log_error "merged daft.yml contains 'null' litter (sparse-serialize regression)"
        log_error "--- merged daft.yml:"
        log_error "$(cat daft.yml)"
        return 1
    fi

    log_success "daft merge consolidated a refined daft.yml three-way, no field drops, no null litter"
    return 0
}

run_daft_merge_visitor_rollback_tests() {
    log "Running daft merge visitor-config rollback integration tests..."

    run_test "daft_merge_visitor_rollback_on_conflict" \
        "test_daft_merge_visitor_rollback_on_conflict"
    run_test "daft_merge_visitor_propagates_on_success" \
        "test_daft_merge_visitor_propagates_on_success"
    run_test "daft_merge_visitor_merges_into_existing_target_daft_yml" \
        "test_daft_merge_visitor_merges_into_existing_target_daft_yml"
}

# Main execution when run directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_daft_merge_visitor_rollback_tests
    print_summary
    exit $?
fi
