#!/usr/bin/env bash
# Benchmark: git-worktree-init vs manual git init --bare + worktree add --orphan
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"

setup_bench_env

DEST="$TEMP_BASE/init-run"

bench_compare \
    "init" \
    "rm -rf $DEST" \
    "git-worktree-init -q $DEST/daft-repo" \
    "mkdir -p $DEST/git-repo && git init --bare $DEST/git-repo/.git 2>/dev/null && git -C $DEST/git-repo/.git worktree add $DEST/git-repo/main --orphan main 2>/dev/null"

log_success "Init benchmark done"
