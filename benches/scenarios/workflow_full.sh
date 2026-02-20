#!/usr/bin/env bash
# Benchmark: Full end-to-end workflow
# Clone -> create 3 feature branches -> run hooks in each -> prune 2.
# Git side uses maximum parallelism with & + wait at every opportunity.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    log "=== Full workflow benchmark: $size ==="

    REPO="$TEMP_BASE/fixture-workflow-${size}.git"
    create_bare_repo_with_hooks "$REPO" "$size"

    DEST="$TEMP_BASE/workflow-run-${size}"
    DAFT_REPO="$DEST/daft/fixture-workflow-${size}"

    # DAFT side: sequential (as daft works — each command runs from inside a worktree)
    # Hooks fire automatically on clone and each checkout-branch.
    # Prune removes feature-a and feature-b.
    DAFT_CMD="mkdir -p $DEST/daft && cd $DEST/daft && git-worktree-clone -q file://$REPO \
&& cd $DAFT_REPO/main \
&& git-worktree-checkout-branch feature-a \
&& git-worktree-checkout-branch feature-b \
&& git-worktree-checkout-branch feature-c \
&& rm -rf $DAFT_REPO/feature-a $DAFT_REPO/feature-b \
&& git-worktree-prune"

    # GIT side: clone, then parallel worktree creation, parallel hook work, parallel prune.
    # A competent git user would parallelize every independent operation.
    GIT_CMD="git clone --bare file://$REPO $DEST/git-repo/.git 2>/dev/null \
&& git -C $DEST/git-repo/.git worktree add $DEST/git-repo/main main 2>/dev/null \
&& ( \
    echo \"export PROJECT_ROOT=\$(pwd)\" > $DEST/git-repo/main/.envrc & \
    touch $DEST/git-repo/main/.tool-versions & \
    wait; sleep 0.05 \
) \
&& ( \
    git -C $DEST/git-repo/.git worktree add -b feature-a $DEST/git-repo/feature-a 2>/dev/null & \
    git -C $DEST/git-repo/.git worktree add -b feature-b $DEST/git-repo/feature-b 2>/dev/null & \
    git -C $DEST/git-repo/.git worktree add -b feature-c $DEST/git-repo/feature-c 2>/dev/null & \
    wait \
) \
&& ( \
    ( cd $DEST/git-repo/feature-a && echo \"export WORKTREE=\$(pwd)\" > .envrc && touch .mise.local.toml && sleep 0.03 ) & \
    ( cd $DEST/git-repo/feature-b && echo \"export WORKTREE=\$(pwd)\" > .envrc && touch .mise.local.toml && sleep 0.03 ) & \
    ( cd $DEST/git-repo/feature-c && echo \"export WORKTREE=\$(pwd)\" > .envrc && touch .mise.local.toml && sleep 0.03 ) & \
    wait \
) \
&& ( \
    git -C $DEST/git-repo/.git worktree remove $DEST/git-repo/feature-a 2>/dev/null & \
    git -C $DEST/git-repo/.git worktree remove $DEST/git-repo/feature-b 2>/dev/null & \
    wait \
)"

    bench_compare \
        "workflow-full-${size}" \
        "rm -rf $DEST" \
        "$DAFT_CMD" \
        "$GIT_CMD"

    log_success "Full workflow ($size) done"
done
