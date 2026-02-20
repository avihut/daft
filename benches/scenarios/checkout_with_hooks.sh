#!/usr/bin/env bash
# Benchmark: git-worktree-checkout / checkout-branch with hooks
# vs manual git worktree add + manual hook work with parallelism
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    log "=== Checkout with hooks benchmark: $size ==="

    # Create a shared fixture repo with hooks
    FIXTURE="$TEMP_BASE/fixture-checkout-hooks-${size}.git"
    create_bare_repo_with_hooks "$FIXTURE" "$size"

    # --- Benchmark 1: Checkout existing branch with hooks ---
    ROOT="$TEMP_BASE/checkout-hooks-existing-${size}"

    SETUP_EXISTING="rm -rf $ROOT \
&& git clone --bare file://$FIXTURE $ROOT/.git 2>/dev/null \
&& git -C $ROOT/.git worktree add $ROOT/main main 2>/dev/null"

    # daft creates worktrees at $ROOT/feature/branch-1 (preserving branch path),
    # git creates at $ROOT/feature-branch-1 (flat name). Clean up both.
    PREPARE_EXISTING="git -C $ROOT/.git worktree remove $ROOT/feature-branch-1 2>/dev/null; \
git -C $ROOT/.git worktree remove $ROOT/feature/branch-1 2>/dev/null; \
git -C $ROOT/.git worktree prune 2>/dev/null; \
rm -rf $ROOT/feature-branch-1 $ROOT/feature; true"

    eval "$SETUP_EXISTING"

    # Git side: create worktree + manually replicate worktree-post-create hook with parallelism
    GIT_EXISTING="git -C $ROOT/.git worktree add $ROOT/feature-branch-1 feature/branch-1 2>/dev/null \
&& ( cd $ROOT/feature-branch-1 \
  && ( echo \"export WORKTREE=\$(pwd)\" > .envrc & touch .mise.local.toml & wait ) \
  && sleep 0.03 )"

    bench_compare \
        "checkout-hooks-existing-${size}" \
        "$PREPARE_EXISTING" \
        "cd $ROOT/main && git-worktree-checkout feature/branch-1" \
        "$GIT_EXISTING"

    log_success "Checkout existing with hooks ($size) done"

    # --- Benchmark 2: Create new branch with hooks ---
    ROOT_NEW="$TEMP_BASE/checkout-hooks-new-${size}"

    SETUP_NEW="rm -rf $ROOT_NEW \
&& git clone --bare file://$FIXTURE $ROOT_NEW/.git 2>/dev/null \
&& git -C $ROOT_NEW/.git worktree add $ROOT_NEW/main main 2>/dev/null"

    PREPARE_NEW="git -C $ROOT_NEW/.git worktree remove $ROOT_NEW/bench-new 2>/dev/null; \
git -C $ROOT_NEW/.git worktree prune 2>/dev/null; \
rm -rf $ROOT_NEW/bench-new; \
git -C $ROOT_NEW/.git branch -D bench-new 2>/dev/null; true"

    eval "$SETUP_NEW"

    # Git side: create worktree with new branch + manual hook work
    GIT_NEW="git -C $ROOT_NEW/.git worktree add -b bench-new $ROOT_NEW/bench-new 2>/dev/null \
&& ( cd $ROOT_NEW/bench-new \
  && ( echo \"export WORKTREE=\$(pwd)\" > .envrc & touch .mise.local.toml & wait ) \
  && sleep 0.03 )"

    bench_compare \
        "checkout-hooks-new-branch-${size}" \
        "$PREPARE_NEW" \
        "cd $ROOT_NEW/main && git-worktree-checkout-branch bench-new" \
        "$GIT_NEW"

    log_success "Checkout new branch with hooks ($size) done"
done
