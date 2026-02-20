#!/usr/bin/env bash
# Benchmark: git-worktree-clone with hooks vs manual git clone + manual hook work
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

for size in small medium large; do
    log "=== Clone with hooks benchmark: $size ==="

    REPO="$TEMP_BASE/fixture-clone-hooks-${size}.git"
    create_bare_repo_with_hooks "$REPO" "$size"

    DEST="$TEMP_BASE/clone-hooks-run-${size}"

    # daft side: hooks run automatically
    DAFT_CMD="git-worktree-clone -q file://$REPO $DEST/daft-repo"

    # git side: clone + manually replicate hook work with parallelism
    GIT_CMD="git clone --bare file://$REPO \$DEST/git-repo/.git 2>/dev/null \
&& git -C \$DEST/git-repo/.git worktree add \$DEST/git-repo/main main 2>/dev/null \
&& ( cd \$DEST/git-repo/main \
  && ( echo \"export PROJECT_ROOT=\$(pwd)\" > .envrc & touch .tool-versions & wait ) \
  && sleep 0.05 \
  && ( echo \"export WORKTREE=\$(pwd)\" > .envrc & touch .mise.local.toml & wait ) \
  && sleep 0.03 )"

    # Export DEST so prepare and git commands can use it
    export DEST

    bench_compare \
        "clone-hooks-${size}" \
        "rm -rf $DEST" \
        "$DAFT_CMD" \
        "$GIT_CMD"

    log_success "Clone with hooks ($size) done"
done
