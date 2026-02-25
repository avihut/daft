#!/usr/bin/env bash
# Shared utilities for sandbox tasks. Source this file, don't execute it.

sandbox_dir() {
    local base="${DAFT_SANDBOX_BASE:-/tmp}"
    local worktree_path
    # pwd returns the project root because mise always runs tasks from there
    worktree_path="$(pwd)"

    local hash
    if command -v sha256sum &>/dev/null; then
        hash=$(echo -n "$worktree_path" | sha256sum | cut -c1-8)
    else
        hash=$(echo -n "$worktree_path" | shasum -a 256 | cut -c1-8)
    fi

    echo "${base}/daft-sandbox-${hash}"
}
