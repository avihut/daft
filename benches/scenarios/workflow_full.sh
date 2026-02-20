#!/usr/bin/env bash
# Benchmark: Full end-to-end workflow
# Clone + create multiple feature branches.
# Git side uses maximum parallelism with & + wait.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    log "=== Full workflow benchmark: $size ==="

    REPO="$TEMP_BASE/fixture-workflow-${size}.git"
    create_bare_repo "$REPO" "$size"

    DEST="$TEMP_BASE/workflow-run-${size}"

    # DAFT side: sequential (as daft works — each checkout-branch runs from inside a worktree)
    DAFT_CMD="git-worktree-clone -q file://$REPO $DEST/daft-repo \
&& cd $DEST/daft-repo/main \
&& git-worktree-checkout-branch feature-a \
&& git-worktree-checkout-branch feature-b \
&& git-worktree-checkout-branch feature-c"

    # GIT side: clone, then parallel worktree creation + parallel cleanup
    # A competent git user would parallelize the independent worktree adds
    GIT_CMD="git clone --bare file://$REPO $DEST/git-repo/.git 2>/dev/null \
&& git -C $DEST/git-repo/.git worktree add $DEST/git-repo/main main 2>/dev/null \
&& ( \
    git -C $DEST/git-repo/.git worktree add -b feature-a $DEST/git-repo/feature-a 2>/dev/null & \
    git -C $DEST/git-repo/.git worktree add -b feature-b $DEST/git-repo/feature-b 2>/dev/null & \
    git -C $DEST/git-repo/.git worktree add -b feature-c $DEST/git-repo/feature-c 2>/dev/null & \
    wait \
)"

    bench_compare \
        "workflow-full-${size}" \
        "rm -rf $DEST" \
        "$DAFT_CMD" \
        "$GIT_CMD"

    log_success "Full workflow ($size) done"
done
