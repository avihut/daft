#!/usr/bin/env bash
# Sourceable helper: ensures a daft release binary exists in a
# content-hashed location under <git-common-dir>/.daft-shared-bin/<hash>/
# and prints its directory on stdout. Sibling worktrees at the same
# source state hit the cache instead of paying the build cost twice.
#
# Used by mise-tasks/test/manual/_default and mise-tasks/test/manual/ramdisk
# from the #514 work. File is intentionally non-executable so mise hides
# it from `mise tasks ls`.
#
# Cache layout:
#   <git-common-dir>/.daft-shared-bin/<state-hash>/target/release/daft
#                                                                 git-worktree-clone -> daft
#                                                                 ... (full multicall farm)
#
# Race-safety: a fresh-state build writes into a private staging dir
# `<root>/<hash>.tmp.<pid>/` and publishes via atomic `mv`. Loser of a
# concurrent populate cleans up its staging and re-uses the winner's
# entry (or hard-errors if neither side left a usable binary). No
# `flock` needed — `rename(2)` of a directory onto a non-existent
# target is atomic same-filesystem.
#
# Escape hatch: set `DAFT_BINARY_DIR=<path>` before invoking the parent
# task to point the runner at a hand-built `target/release/` instead.
# The adapter (`xtask/src/manual_test/daft_executor.rs::resolve_binary_dir`)
# honours that env var verbatim, so this helper isn't consulted at all.

shared_bin_ensure() {
  local workspace_root git_common state_hash cache_dir shared_bin
  workspace_root=$(git rev-parse --show-toplevel)
  git_common=$(git rev-parse --git-common-dir)
  # `--git-common-dir` returns a path relative to the worktree under
  # worktrees; resolve to absolute so the cache root is stable across
  # callers' CWDs.
  if [[ "$git_common" != /* ]]; then
    git_common="$workspace_root/$git_common"
  fi

  state_hash=$(_shared_bin_state_hash "$workspace_root")
  cache_dir="$git_common/.daft-shared-bin/$state_hash"
  shared_bin="$cache_dir/target/release"

  # Both binaries must be present — the runner-output regression scenario
  # shells out to `$BINARY_DIR/xtask` to re-invoke the runner from inside
  # a test step, so a daft-only cache would silently break that scenario.
  if [[ -x "$shared_bin/daft" && -x "$shared_bin/xtask" ]]; then
    echo "$shared_bin"
    return 0
  fi

  _shared_bin_build_and_publish "$workspace_root" "$cache_dir" "$state_hash" >&2 || return 1
  echo "$shared_bin"
}

# SHA-256 of:
#   - HEAD oid (captures the committed state)
#   - blob-hash of every tracked .rs file, every Cargo.toml, and
#     Cargo.lock at their current working-tree state (captures
#     uncommitted edits without needing `git add`).
#
# Covers every workspace path dep (e.g. term-styles/), not just src/
# — editing a workspace crate's source must invalidate the cache.
_shared_bin_state_hash() {
  local root="$1"
  {
    git -C "$root" rev-parse HEAD
    git -C "$root" ls-files -- '*.rs' '**/Cargo.toml' Cargo.toml Cargo.lock \
      | while IFS= read -r f; do
          git -C "$root" hash-object "$f"
        done
  } | shasum -a 256 | awk '{print $1}'
}

_shared_bin_build_and_publish() {
  local root="$1" cache_dir="$2" hash="$3"
  local staging="${cache_dir}.tmp.$$"
  local lib_dir
  lib_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

  mkdir -p "$(dirname "$cache_dir")"
  rm -rf "$staging"
  mkdir -p "$staging/target"

  echo "shared-bin: building daft + xtask for state hash ${hash:0:12} into $staging"
  # Build both binaries the runner consumes. `daft` is the system under
  # test; `xtask` is shelled into by the `runner-output:end-to-end`
  # scenario, which would silently fail with `No such file or directory`
  # if `xtask` was missing from the shared bin dir.
  CARGO_TARGET_DIR="$staging/target" cargo build --release \
    --manifest-path "$root/Cargo.toml" -p daft -p xtask

  # Symlink farm — reuse the canonical list from the setup helper so a
  # new multicall ships to both per-worktree dev and shared-bin paths
  # in one edit.
  # shellcheck source=../../setup/_rust_symlink_lib.sh
  source "$lib_dir/../../setup/_rust_symlink_lib.sh"
  create_daft_symlinks "$staging/target/release"

  # Atomic publish. If another worker won, mv fails and we cleanup.
  if mv "$staging" "$cache_dir" 2>/dev/null; then
    echo "shared-bin: published $cache_dir"
    return 0
  fi
  rm -rf "$staging"

  if [[ -x "$cache_dir/target/release/daft" ]]; then
    echo "shared-bin: rival worker won the publish race; reusing $cache_dir"
    return 0
  fi
  echo "shared-bin: build completed but no usable binary at $cache_dir/target/release/daft" >&2
  return 1
}
