#!/usr/bin/env bash
# Orchestrator: run all benchmark scenarios (except vs_competition) and
# aggregate results into benches/results/summary.md.
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SCENARIOS_DIR="$BENCH_DIR/scenarios"
RESULTS_DIR="$BENCH_DIR/results"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

log()         { echo -e "${BLUE}[run_all]${NC} $*"; }
log_success() { echo -e "${GREEN}[+]${NC} $*"; }
log_warn()    { echo -e "${YELLOW}[!]${NC} $*"; }
log_error()   { echo -e "${RED}[-]${NC} $*"; }

# All scenarios except vs_competition (opt-in only)
SCENARIOS=(
    clone
    clone_with_hooks
    checkout
    checkout_with_hooks
    init
    prune
    fetch
    branch_delete
    workflow_full
)

# Parse arguments
FILTER=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --only)
            FILTER="$2"
            shift 2
            ;;
        --list)
            echo "Available scenarios:"
            for s in "${SCENARIOS[@]}"; do
                echo "  $s"
            done
            echo "  vs_competition (opt-in, use --only vs_competition)"
            exit 0
            ;;
        --help|-h)
            echo "Usage: $0 [--only <scenario>] [--list] [--help]"
            echo ""
            echo "Options:"
            echo "  --only <name>   Run only the specified scenario"
            echo "  --list          List available scenarios"
            echo "  --help          Show this help"
            exit 0
            ;;
        *)
            log_error "Unknown argument: $1"
            exit 1
            ;;
    esac
done

# If --only is specified, run just that scenario
if [[ -n "$FILTER" ]]; then
    SCENARIOS=("$FILTER")
fi

mkdir -p "$RESULTS_DIR"

PASSED=0
FAILED=0
FAILED_NAMES=()

log "${BOLD}Starting daft benchmark suite${NC}"
log "Scenarios: ${SCENARIOS[*]}"
echo ""

for scenario in "${SCENARIOS[@]}"; do
    script="$SCENARIOS_DIR/${scenario}.sh"

    if [[ ! -x "$script" ]]; then
        log_error "Scenario not found or not executable: $script"
        FAILED=$((FAILED + 1))
        FAILED_NAMES+=("$scenario")
        continue
    fi

    log "=== Running: $scenario ==="
    echo ""

    if "$script"; then
        log_success "$scenario completed"
        PASSED=$((PASSED + 1))
    else
        log_error "$scenario FAILED"
        FAILED=$((FAILED + 1))
        FAILED_NAMES+=("$scenario")
    fi

    echo ""
done

# Aggregate results into summary.md
SUMMARY="$RESULTS_DIR/summary.md"
{
    echo "# Daft Benchmark Results"
    echo ""
    echo "Generated: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
    echo ""
    echo "## Summary"
    echo ""
    echo "- Scenarios run: $((PASSED + FAILED))"
    echo "- Passed: $PASSED"
    echo "- Failed: $FAILED"
    if [[ ${#FAILED_NAMES[@]} -gt 0 ]]; then
        echo "- Failed scenarios: ${FAILED_NAMES[*]}"
    fi
    echo ""

    # Include each individual result markdown
    for md_file in "$RESULTS_DIR"/*.md; do
        [[ "$md_file" == "$SUMMARY" ]] && continue
        [[ -f "$md_file" ]] || continue

        local_name="$(basename "$md_file" .md)"
        echo "## $local_name"
        echo ""
        cat "$md_file"
        echo ""
    done
} > "$SUMMARY"

echo ""
log "${BOLD}=== Benchmark Suite Complete ===${NC}"
log "Passed: $PASSED / $((PASSED + FAILED))"
if [[ $FAILED -gt 0 ]]; then
    log_error "Failed: ${FAILED_NAMES[*]}"
fi
log "Results: $RESULTS_DIR/"
log "Summary: $SUMMARY"
