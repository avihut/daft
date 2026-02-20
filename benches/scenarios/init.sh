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
    "mkdir -p $DEST/daft && cd $DEST/daft && git-worktree-init -q my-repo" \
    "mkdir -p $DEST/git/my-repo && git init --bare $DEST/git/my-repo/.git 2>/dev/null && git -C $DEST/git/my-repo/.git worktree add --orphan -b main $DEST/git/my-repo/main 2>/dev/null"

log_success "Init benchmark done"
