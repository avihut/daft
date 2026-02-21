#!/usr/bin/env bash
# Package benchmark results into a single JSON envelope with metadata.
# Usage: package_results.sh <output-file>
set -euo pipefail

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="$BENCH_DIR/results"

OUTPUT="${1:?Usage: package_results.sh <output-file>}"

# Gather metadata
VERSION=$(daft --version 2>/dev/null | awk '{print $2}' || echo "unknown")
COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo "unknown")
COMMIT_FULL=$(git rev-parse HEAD 2>/dev/null || echo "unknown")
COMMIT_MSG=$(git log -1 --format=%s HEAD 2>/dev/null || echo "")
COMMIT_URL="https://github.com/avihut/daft/commit/${COMMIT_FULL}"
DATE=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
RUNNER_OS="${RUNNER_OS:-$(uname -s)}"

# Build the benchmarks object by merging each result file via jq file reads.
# Avoids shell variable size limits (ARG_MAX) by writing intermediate results to a temp file.
BENCH_TMP="$(mktemp)"
echo '{}' > "$BENCH_TMP"
for json_file in "$RESULTS_DIR"/*.json; do
    [[ -f "$json_file" ]] || continue
    name="$(basename "$json_file" .json)"
    jq --arg name "$name" --slurpfile data "$json_file" '. + {($name): $data[0]}' "$BENCH_TMP" > "${BENCH_TMP}.new"
    mv "${BENCH_TMP}.new" "$BENCH_TMP"
done

jq -n \
    --arg version "$VERSION" \
    --arg commit "$COMMIT" \
    --arg commit_msg "$COMMIT_MSG" \
    --arg commit_url "$COMMIT_URL" \
    --arg date "$DATE" \
    --arg runner_os "$RUNNER_OS" \
    --slurpfile benchmarks "$BENCH_TMP" \
    '{
        version: $version,
        commit: $commit,
        commit_msg: $commit_msg,
        commit_url: $commit_url,
        date: $date,
        runner_os: $runner_os,
        benchmarks: $benchmarks[0]
    }' > "$OUTPUT"

rm -f "$BENCH_TMP"

echo "Packaged results to $OUTPUT"
