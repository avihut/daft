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

# Build the envelope using jq
BENCHMARKS="{}"
for json_file in "$RESULTS_DIR"/*.json; do
    [[ -f "$json_file" ]] || continue
    name="$(basename "$json_file" .json)"
    BENCHMARKS=$(echo "$BENCHMARKS" | jq --arg name "$name" --slurpfile data "$json_file" '. + {($name): $data[0]}')
done

jq -n \
    --arg version "$VERSION" \
    --arg commit "$COMMIT" \
    --arg commit_msg "$COMMIT_MSG" \
    --arg commit_url "$COMMIT_URL" \
    --arg date "$DATE" \
    --arg runner_os "$RUNNER_OS" \
    --argjson benchmarks "$BENCHMARKS" \
    '{
        version: $version,
        commit: $commit,
        commit_msg: $commit_msg,
        commit_url: $commit_url,
        date: $date,
        runner_os: $runner_os,
        benchmarks: $benchmarks
    }' > "$OUTPUT"

echo "Packaged results to $OUTPUT"
