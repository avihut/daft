#!/usr/bin/env bash
# Benchmark: git-worktree-clone vs manual git clone --bare + worktree add
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    log "=== Clone benchmark: $size ==="

    REPO="$TEMP_BASE/fixture-clone-${size}.git"
    create_bare_repo "$REPO" "$size"

    DEST="$TEMP_BASE/clone-run-${size}"

    bench_compare \
        "clone-${size}" \
        "rm -rf $DEST" \
        "mkdir -p $DEST/daft && cd $DEST/daft && git-worktree-clone -q file://$REPO" \
        "git clone --bare file://$REPO $DEST/git-repo/.git 2>/dev/null && git -C $DEST/git-repo/.git worktree add $DEST/git-repo/main main 2>/dev/null"

    log_success "Clone ($size) done"
done
