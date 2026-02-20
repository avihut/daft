#!/usr/bin/env bash
# Benchmark: git-worktree-fetch --all vs manual parallel git fetch
# Creates a repo with 2 remotes to test multi-remote fetch performance.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

ROOT="$TEMP_BASE/fetch-run"

# Create two fixture repos to act as different remotes
REMOTE1="$TEMP_BASE/fixture-fetch-remote1.git"
REMOTE2="$TEMP_BASE/fixture-fetch-remote2.git"
create_bare_repo "$REMOTE1" "small"
create_bare_repo "$REMOTE2" "small"

# Setup: clone from remote1, add remote2
rm -rf "$ROOT"
git clone --bare "file://$REMOTE1" "$ROOT/.git" 2>/dev/null
git -C "$ROOT/.git" worktree add "$ROOT/main" main 2>/dev/null
git -C "$ROOT/.git" remote add upstream "file://$REMOTE2" 2>/dev/null

# The prepare step is a no-op here since fetch is idempotent.
# Each run fetches from all remotes.
bench_compare \
    "fetch" \
    "" \
    "cd $ROOT/main && git-worktree-fetch --all" \
    "git -C $ROOT/.git remote | xargs -P 0 -I{} git -C $ROOT/.git fetch {} 2>/dev/null"

log_success "Fetch benchmark done"
