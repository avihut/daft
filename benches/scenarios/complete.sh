#!/usr/bin/env bash
# Benchmark: daft __complete daft-go (tab-completion speed)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/../bench_framework.sh"
source "$SCRIPT_DIR/../fixtures/create_repo.sh"

setup_bench_env

# Number of linked worktrees per size
declare -A WT_COUNTS=([small]=1 [medium]=3 [large]=5)

for size in small medium large; do
    log "=== Completion benchmark: $size ==="

    FIXTURE="$TEMP_BASE/fixture-complete-${size}.git"
    create_bare_repo "$FIXTURE" "$size"

    ROOT="$TEMP_BASE/complete-${size}"
    mkdir -p "$ROOT"

    # Set up a contained layout: bare repo + linked worktrees
    git clone --bare "file://$FIXTURE" "$ROOT/.git" 2>/dev/null

    # Create the main worktree
    git -C "$ROOT/.git" worktree add "$ROOT/main" main 2>/dev/null

    # Create additional linked worktrees for branches
    wt_count="${WT_COUNTS[$size]}"
    for i in $(seq 1 "$wt_count"); do
        branch="feature/branch-$i"
        git -C "$ROOT/.git" worktree add "$ROOT/feature-branch-$i" "$branch" 2>/dev/null || true
    done

    log "  Branches: $(git -C "$ROOT/.git" for-each-ref --format='%(refname:short)' refs/heads/ | wc -l | tr -d ' ') local, $(git -C "$ROOT/.git" for-each-ref --format='%(refname:short)' refs/remotes/ | wc -l | tr -d ' ') remote"
    log "  Worktrees: $((wt_count + 1))"

    json_out="$RESULTS_DIR/complete-${size}.json"
    md_out="$RESULTS_DIR/complete-${size}.md"

    # Show command output on failure (CI debugging) but not in normal runs
    show_output=()
    if [[ "${CI:-}" == "true" ]]; then
        show_output=(--show-output)
    fi

    hyperfine \
        --warmup 3 \
        --min-runs 10 \
        "${show_output[@]}" \
        --export-json "$json_out" \
        --export-markdown "$md_out" \
        --command-name "complete-${size}" \
        "cd $ROOT/main && daft __complete daft-go '' --position 1"

    log_success "Completion ($size) done"
done
