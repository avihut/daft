#!/usr/bin/env bash
# Sourceable helper: creates the daft multicall + shortcut symlink farm
# alongside a built `daft` binary. Used by:
#   - mise-tasks/setup/rust (the standard per-worktree dev-build setup)
#   - mise-tasks/test/manual/_shared_bin_lib.sh (the cross-worktree
#     shared-bin path from #514)
#
# Keeping the list in one place means a new multicall (or shortcut) is a
# single-line edit, and both setup paths gain it automatically. The file
# is intentionally non-executable so mise hides it from `mise tasks ls`.

# Single source of truth for every symlink that should live next to the
# daft binary. Mirrors the multicall table in `src/main.rs` (multicall
# dispatch on argv[0]) and the shortcut table in `src/shortcuts.rs`.
daft_multicall_symlinks=(
  # Core command symlinks (git-style invocation).
  git-worktree-clone
  git-worktree-init
  git-worktree-checkout
  git-worktree-checkout-branch
  git-worktree-prune
  git-worktree-carry
  git-worktree-branch
  git-worktree-branch-delete
  git-worktree-fetch
  git-worktree-exec
  git-worktree-flow-adopt
  git-worktree-flow-eject
  git-worktree-sync
  git-worktree-list
  git-worktree-merge
  git-worktree-push
  git-daft
  # daft-* form for the noun-first commands.
  daft-remove
  daft-rename
  daft-start
  daft-go
  # Git-style shortcuts (default development aliases).
  gwtclone
  gwtinit
  gwtco
  gwtcb
  gwtprune
  gwtcarry
  gwtbd
  gwtfetch
  gwtsync
  gwtpush
)

# Create every multicall symlink under <dir>, pointing at the relative
# name `daft` (which must already exist there). Idempotent: re-running
# silently replaces existing symlinks via `ln -sf`.
create_daft_symlinks() {
  local dir="$1"
  if [[ -z "$dir" ]]; then
    echo "create_daft_symlinks: missing target directory" >&2
    return 1
  fi
  if [[ ! -x "$dir/daft" ]]; then
    echo "create_daft_symlinks: no executable 'daft' at $dir — build first" >&2
    return 1
  fi
  local sym
  for sym in "${daft_multicall_symlinks[@]}"; do
    ln -sf daft "$dir/$sym"
  done
}
