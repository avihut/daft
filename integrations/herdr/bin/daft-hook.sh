#!/bin/bash
#
# Bridges daft's lifecycle hooks into herdr: `daft-hook.sh <hook-name>`.
# Invoked by the scripts lib/install.sh places in daft's user-global hooks
# directory, so it runs inside daft's hook phase with DAFT_* set and the
# pane's HERDR_* environment inherited. It must never fail daft's operation
# (a failed post-create hook aborts the creation), so every path exits 0.
#
# Removal is mirrored from the pre-remove hook rather than post-remove: daft
# 1.27 spawns post-remove hooks inside the directory it just deleted, so they
# never run for `daft remove`.
#
# Both arms stand down for a move. `daft rename` and `daft layout` transform
# replay the full remove-then-create hook sequence with DAFT_IS_MOVE=true
# (src/hooks/move_hooks.rs), and the same worktree is on both sides of it.
# Acting on that would close the workspace the user is working in and open a
# second one for the new path — herdr 0.8.2 has no way to re-point a live
# workspace (`worktree open` keys reuse on the checkout path, `workspace
# close` kills every pane), so the safe outcome is to leave the workspace
# alone with a stale path and let the user re-group it with the adopt
# action.

[ "${HERDR_ENV:-}" = 1 ] || exit 0
[ -n "${HERDR_BIN_PATH:-}" ] || exit 0

. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"
LOG_SCOPE=daft-hook
. "$PLUGIN_ROOT/lib/layout.sh"
. "$PLUGIN_ROOT/lib/tokens.sh"

die() {
  log "error: $*"
  exit 0
}

# daft_ancestor_pid — the daft process this hook runs under, if any.
daft_ancestor_pid() {
  local pid=$PPID comm depth=0
  while [ -n "$pid" ] && [ "$pid" -gt 1 ] && [ "$depth" -lt 15 ]; do
    comm=$(ps -o comm= -p "$pid" 2>/dev/null | sed 's/^ *//')
    comm=$(basename "$comm")
    case $comm in
      daft | daft-* | git-worktree-* | git-daft)
        printf '%s' "$pid"
        return 0
        ;;
    esac
    pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
    depth=$((depth + 1))
  done
  return 1
}

# close_when_removed WORKSPACE PATH — close WORKSPACE once the daft running
# this hook has exited and PATH is really gone. Waiting keeps a removal typed
# inside that very workspace alive until daft is done (closing the pane would
# kill daft mid-flight), and the path check keeps a refused removal's
# workspace open.
close_when_removed() {
  local ws=$1 path=$2 pid
  pid=$(daft_ancestor_pid) || pid=
  log "closing $ws once daft (pid ${pid:-unknown}) exits and $path is gone"
  # `nohup ... &` rather than daft's own setsid contract: macOS ships no
  # setsid(1), and a `command -v setsid` branch would leave the platform the
  # plugin exists for as the untested one. The watcher ignores SIGHUP, holds
  # no terminal, and is bounded at 120 s, so the worst case if herdr reaps the
  # pane's process group is a workspace row that outlives its worktree.
  nohup /bin/bash -c '
    pid=$1 ws=$2 path=$3 herdr=$4 i=0
    if [ -n "$pid" ]; then
      while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 480 ]; do sleep 0.25; i=$((i + 1)); done
    fi
    sleep 0.5
    [ -d "$path" ] && exit 0
    "$herdr" workspace close "$ws" >/dev/null 2>&1
  ' _ "$pid" "$ws" "$path" "$HERDR" </dev/null >/dev/null 2>&1 &
}

hook=${1:-}
resolve_jq || exit 0
resolve_daft || exit 0

case $hook in
  worktree-post-create)
    # The plugin's own popup registers what it creates (with focus and
    # layout). The flag is exported into daft and inherited by what daft
    # spawns; that is harmless because nothing daft spawns in the background
    # runs worktree lifecycle hooks (src/coordinator has no HookExecutor).
    [ "${DAFT_HERDR_PLUGIN_ACTIVE:-}" = 1 ] && exit 0
    if [ "${DAFT_IS_MOVE:-}" = true ]; then
      log "post-create: skipping the move of ${DAFT_OLD_WORKTREE_PATH:-?} to ${DAFT_WORKTREE_PATH:-?}"
      exit 0
    fi
    path=${DAFT_WORKTREE_PATH:-}
    root=${DAFT_PROJECT_ROOT:-}
    [ -n "$path" ] && [ -n "$root" ] || exit 0
    label=${DAFT_BRANCH_NAME:-}
    [ -n "$label" ] || label=$(basename "$path")
    pane=$(register_worktree "$root" "$path" "$label" --no-focus) || exit 0
    if [ -n "$pane" ] && [ "$(config_get layout_on_hook false)" = true ]; then
      info=$(repo_info "$root") || info=
      apply_layout "$pane" "$path" "$label" "$(repo_field "$info" .name)" >/dev/null 2>&1 || true
    fi
    ( FORCE_REFRESH=1 tokens_refresh_repo "$root" </dev/null >/dev/null 2>&1 & )
    ;;

  worktree-pre-remove)
    if [ "${DAFT_IS_MOVE:-}" = true ]; then
      log "pre-remove: skipping the move of ${DAFT_WORKTREE_PATH:-?}"
      exit 0
    fi
    path=${DAFT_WORKTREE_PATH:-}
    [ -n "$path" ] || exit 0
    ws=$(workspace_for_path "$path")
    [ -n "$ws" ] || exit 0
    close_when_removed "$ws" "$path"
    ;;
esac
exit 0
