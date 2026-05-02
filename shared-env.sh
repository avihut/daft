#!/bin/bash
# Resolve DAFT_CONFIG_DIR to a shared location inside the git common dir.
# This ensures trust, repos.json, and layout config persist across worktrees.
# Only honored in dev builds (release builds ignore DAFT_CONFIG_DIR).
#
# Respect a pre-set DAFT_CONFIG_DIR (defensive — mise loads `_.source` in a
# clean env so the parent value usually isn't visible here, but a direct
# `source ./shared-env.sh` from a sandbox shell would otherwise lose the
# sandbox's per-worktree config dir.) The sandbox-task counterpart lives in
# `mise-tasks/sandbox/_lib.sh::sandbox_use_isolated_daft_env`, which
# explicitly re-points DAFT_CONFIG_DIR at the per-worktree sandbox after
# mise has finished setting up its own env.
git_common_dir=$(git rev-parse --git-common-dir 2>/dev/null)
if [ -n "$git_common_dir" ] && [ -z "${DAFT_CONFIG_DIR:-}" ]; then
  sandbox="$git_common_dir/.daft-sandbox"
  mkdir -p "$sandbox"
  export DAFT_CONFIG_DIR="$sandbox"
fi
