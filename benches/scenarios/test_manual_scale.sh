#!/usr/bin/env bash
# Manual-test runner scaling sweep — times the YAML manual-test suite at
# varying --jobs values so #509 progress can be measured SHA-over-SHA.
#
# Outputs:
#   benches/results/test-manual-scale.md     human-readable summary
#   benches/results/test-manual-scale.json   hyperfine-native JSON
#
# Configurable via env vars:
#   BENCH_JOBS   comma-separated jobs values (default: 1,2,4,8)
#   BENCH_RUNS   trials per jobs value (default: 3)
#   BENCH_SKIP_TIMING   if set, skip Phase 2 per-scenario timing collection

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
RESULTS_DIR="$REPO_ROOT/benches/results"
mkdir -p "$RESULTS_DIR"

MD_OUT="$RESULTS_DIR/test-manual-scale.md"
JSON_OUT="$RESULTS_DIR/test-manual-scale.json"

JOBS_VALUES="${BENCH_JOBS:-1,2,4,8}"
RUNS="${BENCH_RUNS:-3}"

if ! command -v hyperfine >/dev/null 2>&1; then
    echo "hyperfine not found. brew install hyperfine"
    exit 1
fi

echo "Building daft + xtask (release)..."
cargo build --release --quiet
cargo build --package xtask --release --quiet

if [[ "$(uname)" == "Darwin" ]]; then
    CPU_PHYS=$(sysctl -n hw.physicalcpu)
    CPU_LOG=$(sysctl -n hw.logicalcpu)
    CPU_MODEL=$(sysctl -n machdep.cpu.brand_string)
else
    CPU_PHYS=$(nproc 2>/dev/null || echo "?")
    CPU_LOG=$CPU_PHYS
    CPU_MODEL=$(grep -m1 "model name" /proc/cpuinfo 2>/dev/null | sed 's/.*: //' || echo "?")
fi
DEFAULT_CAP=$CPU_LOG
[ "$DEFAULT_CAP" -lt 1 ] && DEFAULT_CAP=1
DAFT_SHA=$(git -C "$REPO_ROOT" rev-parse --short HEAD)
DAFT_BRANCH=$(git -C "$REPO_ROOT" symbolic-ref --short HEAD 2>/dev/null || echo "DETACHED")
SCENARIO_COUNT=$(find "$REPO_ROOT/tests/manual/scenarios" -type f \( -name '*.yml' -o -name '*.yaml' \) | wc -l | tr -d ' ')

echo "=== Manual-test scaling sweep ==="
echo "  Daft:       $DAFT_BRANCH @ $DAFT_SHA"
echo "  CPU:        $CPU_MODEL ($CPU_PHYS physical / $CPU_LOG logical)"
echo "  Scenarios:  $SCENARIO_COUNT"
echo "  Default cap: $DEFAULT_CAP (auto-default = available_parallelism())"
echo "  Jobs values: $JOBS_VALUES"
echo "  Trials:      $RUNS per value"
echo

# Pre-cleanup runs before each timed invocation so a previous run's
# leftover sandboxes don't slow down the next one's mkdir/rm syscalls.
PREPARE='find -L /tmp -maxdepth 1 -name "daft-manual-test-*" -type d -exec rm -rf {} + 2>/dev/null || true'

# Phase 1 — hyperfine wall-clock sweep over --jobs values.
# Don't let a transient hyperfine failure (we've seen sporadic
# "No such file or directory" errors mid-sweep, root cause unclear)
# short-circuit `set -e` and lose the data already collected. If
# hyperfine bails partway, the JSON it has already written stays usable.
echo "=== Phase 1: wall-clock sweep ==="
set +e
hyperfine \
    --warmup 0 \
    --min-runs "$RUNS" \
    --max-runs "$RUNS" \
    --ignore-failure \
    --prepare "$PREPARE" \
    --parameter-list jobs "$JOBS_VALUES" \
    --command-name "jobs={jobs}" \
    --export-json "$JSON_OUT" \
    --export-markdown "${MD_OUT}.phase1" \
    "$REPO_ROOT/target/release/xtask manual-test --ci --jobs {jobs}"
HF_STATUS=$?
set -e
if [[ $HF_STATUS -ne 0 ]]; then
    echo "Phase 1: hyperfine exited $HF_STATUS — keeping partial results in $JSON_OUT"
fi
if [[ ! -s "${MD_OUT}.phase1" ]]; then
    echo "Phase 1: no markdown output; nothing to report."
    echo "(empty)" > "${MD_OUT}.phase1"
fi

# Phase 2 — per-scenario timing at jobs=1 and at the default cap.
SERIAL_TIMINGS=$(mktemp -t daft-bench-serial)
PARALLEL_TIMINGS=$(mktemp -t daft-bench-parallel)
trap 'rm -f "$SERIAL_TIMINGS" "$PARALLEL_TIMINGS"' EXIT

if [[ -z "${BENCH_SKIP_TIMING:-}" ]]; then
    echo
    echo "=== Phase 2: per-scenario timing — jobs=1 ==="
    eval "$PREPARE"
    DAFT_MANUAL_TEST_EMIT_TIMING=1 \
        "$REPO_ROOT/target/release/xtask" manual-test --ci --jobs 1 2>&1 \
        | grep '^\[bench\] scenario=' > "$SERIAL_TIMINGS" || true

    echo "=== Phase 2: per-scenario timing — jobs=$DEFAULT_CAP ==="
    eval "$PREPARE"
    DAFT_MANUAL_TEST_EMIT_TIMING=1 \
        "$REPO_ROOT/target/release/xtask" manual-test --ci --jobs "$DEFAULT_CAP" 2>&1 \
        | grep '^\[bench\] scenario=' > "$PARALLEL_TIMINGS" || true
fi

# Compute distribution stats from a timings file. Emits the markdown body
# directly to stdout so callers can splice it into a report.
percentiles() {
    local f="$1"
    if [[ ! -s "$f" ]]; then
        echo "_(no samples — Phase 2 skipped or no timings emitted)_"
        return
    fi
    awk -F'elapsed_ms=' '{print $2}' "$f" | sort -n | awk '
    {
        a[NR] = $1;
    }
    END {
        n = NR;
        if (n == 0) { print "_(no samples)_"; exit }
        p50 = a[int((n + 1) * 0.50)];
        p95 = a[int((n + 1) * 0.95)];
        max = a[n];
        sum = 0;
        for (i = 1; i <= n; i++) sum += a[i];
        printf "| scenarios | p50 | p95 | max | cumulative |\n";
        printf "|---|---|---|---|---|\n";
        printf "| %d | %d ms | %d ms | %d ms | %.1f s |\n", n, p50, p95, max, sum / 1000;
    }'
}

# Compose the final markdown report.
{
    echo "# Manual-test runner scaling sweep"
    echo
    echo "- **Generated:** $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
    echo "- **Daft:** \`$DAFT_BRANCH @ $DAFT_SHA\`"
    echo "- **CPU:** $CPU_MODEL ($CPU_PHYS physical / $CPU_LOG logical)"
    echo "- **Scenarios in corpus:** $SCENARIO_COUNT"
    echo "- **Default cap (auto-default):** $DEFAULT_CAP"
    echo "- **Jobs swept:** \`$JOBS_VALUES\`"
    echo "- **Trials per jobs value:** $RUNS"
    echo
    echo "## Wall-clock by --jobs"
    echo
    cat "${MD_OUT}.phase1"
    if [[ -z "${BENCH_SKIP_TIMING:-}" ]]; then
        echo
        echo "## Per-scenario distribution"
        echo
        echo "### \`--jobs 1\` (serial)"
        echo
        percentiles "$SERIAL_TIMINGS"
        echo
        echo "### \`--jobs $DEFAULT_CAP\` (default cap)"
        echo
        percentiles "$PARALLEL_TIMINGS"
    fi
} > "$MD_OUT"

rm -f "${MD_OUT}.phase1"

echo
echo "=== Done ==="
echo "  Markdown: $MD_OUT"
echo "  JSON:     $JSON_OUT"
