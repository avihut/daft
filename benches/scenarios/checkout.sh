#!/usr/bin/env bash
# Benchmark: git-worktree-checkout / checkout-branch vs manual git worktree add
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    log "=== Checkout benchmark: $size ==="

    # Create a shared fixture repo
    FIXTURE="$TEMP_BASE/fixture-checkout-${size}.git"
    create_bare_repo "$FIXTURE" "$size"

    # --- Benchmark 1: Checkout existing branch ---
    ROOT="$TEMP_BASE/checkout-existing-${size}"

    # Setup function: create a fresh daft-style layout for each run
    SETUP_EXISTING="rm -rf $ROOT \
&& git clone --bare file://$FIXTURE $ROOT/.git 2>/dev/null \
&& git -C $ROOT/.git worktree add $ROOT/main main 2>/dev/null"

    # Prepare: remove the worktree that will be created, so checkout can run clean.
    # daft creates worktrees at $ROOT/feature/branch-1 (preserving branch path),
    # git creates at $ROOT/feature-branch-1 (flat name). Clean up both.
    PREPARE_EXISTING="git -C $ROOT/.git worktree remove $ROOT/feature-branch-1 2>/dev/null; \
git -C $ROOT/.git worktree remove $ROOT/feature/branch-1 2>/dev/null; \
git -C $ROOT/.git worktree prune 2>/dev/null; \
rm -rf $ROOT/feature-branch-1 $ROOT/feature; true"

    # Run setup once before the benchmark
    eval "$SETUP_EXISTING"

    bench_compare \
        "checkout-existing-${size}" \
        "$PREPARE_EXISTING" \
        "cd $ROOT/main && git-worktree-checkout feature/branch-1" \
        "git -C $ROOT/.git worktree add $ROOT/feature-branch-1 feature/branch-1 2>/dev/null"

    log_success "Checkout existing branch ($size) done"

    # --- Benchmark 2: Create new branch (checkout-branch) ---
    ROOT_NEW="$TEMP_BASE/checkout-new-${size}"

    SETUP_NEW="rm -rf $ROOT_NEW \
&& git clone --bare file://$FIXTURE $ROOT_NEW/.git 2>/dev/null \
&& git -C $ROOT_NEW/.git worktree add $ROOT_NEW/main main 2>/dev/null"

    # Prepare: remove the new branch worktree and delete the branch
    PREPARE_NEW="git -C $ROOT_NEW/.git worktree remove $ROOT_NEW/bench-new 2>/dev/null; \
git -C $ROOT_NEW/.git worktree prune 2>/dev/null; \
rm -rf $ROOT_NEW/bench-new; \
git -C $ROOT_NEW/.git branch -D bench-new 2>/dev/null; true"

    eval "$SETUP_NEW"

    bench_compare \
        "checkout-new-branch-${size}" \
        "$PREPARE_NEW" \
        "cd $ROOT_NEW/main && git-worktree-checkout-branch bench-new" \
        "git -C $ROOT_NEW/.git worktree add -b bench-new $ROOT_NEW/bench-new 2>/dev/null"

    log_success "Checkout new branch ($size) done"
done
