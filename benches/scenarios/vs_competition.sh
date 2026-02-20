#!/usr/bin/env bash
# Benchmark: daft vs shell alias pattern vs optional competitors (git-town)
# This scenario is opt-in and not included in run_all.sh.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

REPO="$TEMP_BASE/fixture-competition.git"
create_bare_repo "$REPO" "medium"

DEST="$TEMP_BASE/competition-run"

# --- Shell alias pattern ---
# This simulates what a user might put in their .bashrc:
# wt-clone() { git clone --bare "$1" "$2/.git" && git -C "$2/.git" worktree add "$2/main" main; }
# wt-branch() { local root; root="$(git rev-parse --git-common-dir)"; git -C "$root" worktree add -b "$1" "$(dirname "$root")/$1"; }

log "=== daft vs shell alias: clone + 3 branches ==="

DAFT_CMD="git-worktree-clone -q file://$REPO $DEST/daft-repo \
&& cd $DEST/daft-repo/main \
&& git-worktree-checkout-branch feature-a \
&& git-worktree-checkout-branch feature-b \
&& git-worktree-checkout-branch feature-c"

SHELL_ALIAS_CMD="git clone --bare file://$REPO $DEST/alias-repo/.git 2>/dev/null \
&& git -C $DEST/alias-repo/.git worktree add $DEST/alias-repo/main main 2>/dev/null \
&& git -C $DEST/alias-repo/.git worktree add -b feature-a $DEST/alias-repo/feature-a 2>/dev/null \
&& git -C $DEST/alias-repo/.git worktree add -b feature-b $DEST/alias-repo/feature-b 2>/dev/null \
&& git -C $DEST/alias-repo/.git worktree add -b feature-c $DEST/alias-repo/feature-c 2>/dev/null"

# Always run: daft vs shell aliases
bench_compare \
    "vs-shell-alias" \
    "rm -rf $DEST" \
    "$DAFT_CMD" \
    "$SHELL_ALIAS_CMD"

log_success "daft vs shell alias done"

# --- Optional: git-town ---
if command -v git-town >/dev/null 2>&1; then
    log "=== daft vs git-town: new feature branch ==="

    # git-town operates on regular (non-bare) clones, so set up accordingly
    ROOT_DAFT="$TEMP_BASE/competition-town-daft"
    ROOT_TOWN="$TEMP_BASE/competition-town-gittown"

    # Prepare fresh repos each run
    PREPARE_TOWN="rm -rf $ROOT_DAFT $ROOT_TOWN"

    # daft: clone + new branch
    DAFT_TOWN_CMD="git-worktree-clone -q file://$REPO $ROOT_DAFT/repo \
&& cd $ROOT_DAFT/repo/main \
&& git-worktree-checkout-branch feature-x"

    # git-town: clone + new branch (git-town uses regular clones)
    GIT_TOWN_CMD="git clone file://$REPO $ROOT_TOWN/repo 2>/dev/null \
&& cd $ROOT_TOWN/repo \
&& git-town config setup --auto 2>/dev/null; \
git-town hack feature-x 2>/dev/null"

    bench_compare \
        "vs-git-town" \
        "$PREPARE_TOWN" \
        "$DAFT_TOWN_CMD" \
        "$GIT_TOWN_CMD"

    log_success "daft vs git-town done"
else
    log_warn "git-town not found, skipping git-town comparison"
    log_warn "Install with: brew install git-town"
fi
