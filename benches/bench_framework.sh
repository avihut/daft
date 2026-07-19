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
# Usage: bench_compare [--two-way] <name> <prepare_cmd> <daft_cmd> <git_cmd> [extra hyperfine flags...]
#
# By default runs a three-way comparison: daft, daft-subprocess, and git.
# Each command gets its own --prepare that toggles the gitoxide config key
# in the isolated GIT_CONFIG_GLOBAL so the right variant is active: daft
# runs the stable gitoxide default (key unset, #733), daft-subprocess opts
# out (key false).
#
# Pass --two-way as the first argument for a two-way comparison
# (daft vs git only), which skips the backend toggle entirely and uses
# a single --prepare for both commands.
#
# WARNING: Extra flags passed via "$@" must NOT include --prepare, because
# hyperfine pairs --prepare flags positionally with commands. Adding an
# extra --prepare would shift the pairing and produce wrong results.
bench_compare() {
    local two_way=false
    if [[ "${1:-}" == "--two-way" ]]; then
        two_way=true
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

    # Show command output on failure (CI debugging) but not in normal runs
    local show_output=()
    if [[ "${CI:-}" == "true" ]]; then
        show_output=(--show-output)
    fi

    if [[ "$two_way" == true ]]; then
        # Two-way mode: daft vs git, single --prepare, no backend toggle
        hyperfine \
            --warmup 3 \
            --min-runs 10 \
            "${show_output[@]}" \
            "$@" \
            --prepare "$prepare_cmd" \
            --export-json "$json_out" \
            --export-markdown "$md_out" \
            --command-name "daft" "$daft_cmd" \
            --command-name "git" "$git_cmd"
    else
        # Backend toggle: gitoxide default (key unset) vs subprocess opt-out
        # (key false), set in the isolated GIT_CONFIG_GLOBAL
        local unset_backend="git config --file \"$GIT_CONFIG_GLOBAL\" --unset-all daft.experimental.gitoxide 2>/dev/null || [ \$? -eq 5 ]"
        local set_subprocess="git config --file \"$GIT_CONFIG_GLOBAL\" daft.experimental.gitoxide false || exit 1"

        # Build per-command prepare: base cleanup + backend toggle
        local prep_daft=""
        local prep_subprocess=""
        local prep_git=""
        if [[ -n "$prepare_cmd" ]]; then
            prep_daft="$prepare_cmd && $unset_backend"
            prep_subprocess="$prepare_cmd && $set_subprocess"
            prep_git="$prepare_cmd && $unset_backend"
        else
            prep_daft="$unset_backend"
            prep_subprocess="$set_subprocess"
            prep_git="$unset_backend"
        fi

        # Three-way mode: daft, daft-subprocess, git
        hyperfine \
            --warmup 3 \
            --min-runs 10 \
            "${show_output[@]}" \
            "$@" \
            --prepare "$prep_daft" \
            --prepare "$prep_subprocess" \
            --prepare "$prep_git" \
            --export-json "$json_out" \
            --export-markdown "$md_out" \
            --command-name "daft" "$daft_cmd" \
            --command-name "daft-subprocess" "$daft_cmd" \
            --command-name "git" "$git_cmd"
    fi

    log_success "Saved: $json_out"
}

trap cleanup_bench EXIT
