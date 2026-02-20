#!/usr/bin/env bash
# Benchmark: git-worktree-prune vs git worktree prune
# Creates a repo with ~10 stale worktrees (directories deleted but not deregistered).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

ROOT="$TEMP_BASE/prune-run"

# Build the initial repo structure once
FIXTURE="$TEMP_BASE/fixture-prune.git"
create_bare_repo "$FIXTURE" "small"

# Setup: create the repo with stale worktrees
setup_prune_repo() {
    rm -rf "$ROOT"
    git clone --bare "file://$FIXTURE" "$ROOT/.git" 2>/dev/null
    git -C "$ROOT/.git" worktree add "$ROOT/main" main 2>/dev/null

    # Create 10 worktrees, then delete their directories to make them stale
    for i in $(seq 1 10); do
        git -C "$ROOT/.git" worktree add -b "stale-$i" "$ROOT/stale-$i" 2>/dev/null
    done
    for i in $(seq 1 10); do
        rm -rf "$ROOT/stale-$i"
    done
}

# Initial setup
setup_prune_repo

# Prepare: recreate the stale state before each run.
# We need to re-register stale worktrees since prune removes them.
PREPARE="rm -rf $ROOT && \
git clone --bare file://$FIXTURE $ROOT/.git 2>/dev/null && \
git -C $ROOT/.git worktree add $ROOT/main main 2>/dev/null && \
for i in \$(seq 1 10); do \
    git -C $ROOT/.git worktree add -b stale-\$i $ROOT/stale-\$i 2>/dev/null; \
done && \
for i in \$(seq 1 10); do \
    rm -rf $ROOT/stale-\$i; \
done"

bench_compare \
    "prune" \
    "$PREPARE" \
    "cd $ROOT/main && git-worktree-prune" \
    "git -C $ROOT/.git worktree prune"

log_success "Prune benchmark done"
