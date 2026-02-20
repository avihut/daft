#!/usr/bin/env bash
# Benchmark: git-worktree-branch-delete vs manual git worktree remove + branch -D
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

ROOT="$TEMP_BASE/branch-delete-run"

# Create fixture repo
FIXTURE="$TEMP_BASE/fixture-branch-delete.git"
create_bare_repo "$FIXTURE" "small"

# Prepare: recreate the repo with a branch + worktree to delete
PREPARE="rm -rf $ROOT \
&& git clone --bare file://$FIXTURE $ROOT/.git 2>/dev/null \
&& git -C $ROOT/.git worktree add $ROOT/main main 2>/dev/null \
&& git -C $ROOT/.git worktree add -b to-delete $ROOT/to-delete 2>/dev/null"

bench_compare \
    "branch-delete" \
    "$PREPARE" \
    "cd $ROOT/main && git-worktree-branch-delete -D to-delete" \
    "git -C $ROOT/.git worktree remove $ROOT/to-delete 2>/dev/null; git -C $ROOT/.git worktree prune; git -C $ROOT/.git branch -D to-delete"

log_success "Branch delete benchmark done"
