#!/bin/bash
# Resolve DAFT_CONFIG_DIR to a shared location inside the git common dir.
# This ensures trust, repos.json, and layout config persist across worktrees.
# Only honored in dev builds (release builds ignore DAFT_CONFIG_DIR).

git_common_dir=$(git rev-parse --git-common-dir 2>/dev/null)
if [ -n "$git_common_dir" ]; then
  sandbox="$git_common_dir/.daft-sandbox"
  mkdir -p "$sandbox"
  export DAFT_CONFIG_DIR="$sandbox"
fi
