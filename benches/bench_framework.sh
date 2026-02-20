#!/usr/bin/env bash
# Shared framework for daft benchmark suite.
# Source this from scenario scripts.

set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="$BENCH_DIR/results"
HISTORY_DIR="$BENCH_DIR/history"
TEMP_BASE="/tmp/daft-bench"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log()         { echo -e "${BLUE}[bench]${NC} $*"; }
log_success() { echo -e "${GREEN}[+]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $*"; }
log_error()   { echo -e "${RED}[-]${NC} $*"; }

require_hyperfine() {
    if ! command -v hyperfine >/dev/null 2>&1; then
        log_error "hyperfine not found. Install: brew install hyperfine"
        exit 1
    fi
}

require_daft() {
    if ! command -v git-worktree-clone >/dev/null 2>&1; then
        log_error "daft not in PATH. Run: mise run dev"
        exit 1
    fi
}

setup_bench_env() {
    require_hyperfine
    require_daft
    mkdir -p "$RESULTS_DIR" "$HISTORY_DIR" "$TEMP_BASE"

    export GIT_AUTHOR_NAME="Bench User"
    export GIT_AUTHOR_EMAIL="bench@example.com"
    export GIT_COMMITTER_NAME="Bench User"
    export GIT_COMMITTER_EMAIL="bench@example.com"

    # Isolated git config — never touch global
    export GIT_CONFIG_GLOBAL="$TEMP_BASE/.gitconfig"
    touch "$GIT_CONFIG_GLOBAL"
}

cleanup_bench() {
    rm -rf "$TEMP_BASE"
}

# Run a hyperfine comparison.
# Usage: bench_compare <name> <prepare_cmd> <daft_cmd> <git_cmd> [extra hyperfine flags...]
bench_compare() {
    local name="$1"
    local prepare_cmd="$2"
    local daft_cmd="$3"
    local git_cmd="$4"
    shift 4

    local json_out="$RESULTS_DIR/${name}.json"
    local md_out="$RESULTS_DIR/${name}.md"

    log "Running: $name"

    local prepare_args=()
    if [[ -n "$prepare_cmd" ]]; then
        prepare_args=(--prepare "$prepare_cmd")
    fi

    hyperfine \
        --warmup 3 \
        --min-runs 10 \
        "${prepare_args[@]}" \
        --export-json "$json_out" \
        --export-markdown "$md_out" \
        "$@" \
        --command-name "daft" "$daft_cmd" \
        --command-name "git" "$git_cmd"

    log_success "Saved: $json_out"
}

trap cleanup_bench EXIT
