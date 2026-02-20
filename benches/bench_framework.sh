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
#
# Usage: bench_compare [--no-gitoxide] <name> <prepare_cmd> <daft_cmd> <git_cmd> [extra hyperfine flags...]
#
# By default runs a three-way comparison: daft, daft-gitoxide, and git.
# Each command gets its own --prepare that toggles the gitoxide config key
# in the isolated GIT_CONFIG_GLOBAL so the right variant is active.
#
# Pass --no-gitoxide as the first argument for a two-way comparison
# (daft vs git only), which skips the gitoxide toggle entirely and uses
# a single --prepare for both commands.
#
# WARNING: Extra flags passed via "$@" must NOT include --prepare, because
# hyperfine pairs --prepare flags positionally with commands. Adding an
# extra --prepare would shift the pairing and produce wrong results.
bench_compare() {
    local no_gitoxide=false
    if [[ "${1:-}" == "--no-gitoxide" ]]; then
        no_gitoxide=true
        shift
    fi

    local name="$1"
    local prepare_cmd="$2"
    local daft_cmd="$3"
    local git_cmd="$4"
    shift 4

    local json_out="$RESULTS_DIR/${name}.json"
    local md_out="$RESULTS_DIR/${name}.md"

    log "Running: $name"

    if [[ "$no_gitoxide" == true ]]; then
        # Two-way mode: daft vs git, single --prepare, no gitoxide toggle
        hyperfine \
            --warmup 3 \
            --min-runs 10 \
            "$@" \
            --prepare "$prepare_cmd" \
            --export-json "$json_out" \
            --export-markdown "$md_out" \
            --command-name "daft" "$daft_cmd" \
            --command-name "git" "$git_cmd"
    else
        # Gitoxide toggle: set/unset in the isolated GIT_CONFIG_GLOBAL
        local unset_gix="git config --file \"$GIT_CONFIG_GLOBAL\" --unset-all daft.experimental.gitoxide 2>/dev/null || [ \$? -eq 5 ]"
        local set_gix="git config --file \"$GIT_CONFIG_GLOBAL\" daft.experimental.gitoxide true || exit 1"

        # Build per-command prepare: base cleanup + gitoxide toggle
        local prep_daft=""
        local prep_gix=""
        local prep_git=""
        if [[ -n "$prepare_cmd" ]]; then
            prep_daft="$prepare_cmd && $unset_gix"
            prep_gix="$prepare_cmd && $set_gix"
            prep_git="$prepare_cmd && $unset_gix"
        else
            prep_daft="$unset_gix"
            prep_gix="$set_gix"
            prep_git="$unset_gix"
        fi

        # Three-way mode: daft, daft-gitoxide, git
        hyperfine \
            --warmup 3 \
            --min-runs 10 \
            "$@" \
            --prepare "$prep_daft" \
            --prepare "$prep_gix" \
            --prepare "$prep_git" \
            --export-json "$json_out" \
            --export-markdown "$md_out" \
            --command-name "daft" "$daft_cmd" \
            --command-name "daft-gitoxide" "$daft_cmd" \
            --command-name "git" "$git_cmd"
    fi

    log_success "Saved: $json_out"
}

trap cleanup_bench EXIT
