#!/bin/bash
#
# Deferred workspace close: `close-watch.sh PID WORKSPACE PATH`.
#
# Spawned detached by the pre-remove arm of bin/daft-hook.sh. It waits for the
# daft that is running the hook (PID) to exit, so a removal typed inside the
# very workspace it closes stays alive until daft is done, then checks that
# PATH is really gone, so a refused removal keeps its workspace.
#
# It is a file of its own rather than an inline `bash -c` because the close's
# outcome has to be read: herdr 0.9 answers `workspace close` on a workspace
# that still has linked worktree workspaces with `workspace_group_close_required`
# (src/app/api/workspaces.rs), where 0.8.x closed the whole group. Discarding
# that made the 0.9 close a silent no-op.
#
# The stand-down is deliberate, and `--group` is deliberately not the retry.
# The only removal that reaches a parent with live children is
# `daft repo remove`, where every child worktree gets its own pre-remove hook
# and closes its own row; closing the group here would take down workspaces
# whose worktrees still exist.

[ "${HERDR_ENV:-}" = 1 ] || exit 0
[ -n "${HERDR_BIN_PATH:-}" ] || exit 0

. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"
LOG_SCOPE=close-watch

pid=${1:-}
ws=${2:-}
path=${3:-}
[ -n "$ws" ] && [ -n "$path" ] || exit 0

# Bounded at 120 s: the watcher holds no terminal and ignores SIGHUP, so the
# worst case if it outlives its daft is a workspace row that outlives its
# worktree, not a process left behind for good.
i=0
if [ -n "$pid" ]; then
  while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 480 ]; do
    sleep 0.25
    i=$((i + 1))
  done
fi
sleep 0.5
if [ -d "$path" ]; then
  log "$path is still on disk; leaving $ws open"
  exit 0
fi

# Both pipes: herdr prints the error envelope on one of them depending on the
# version, and this one is only ever grepped, never parsed.
out=$("$HERDR" workspace close "$ws" 2>&1 </dev/null)
rc=$?
if [ "$rc" -eq 0 ]; then
  log "closed $ws"
  exit 0
fi
case $out in
  *workspace_group_close_required*)
    log "$ws still has open worktree workspaces; leaving its row (each child closes its own)"
    exit 0
    ;;
esac
log "workspace close $ws failed: $out"
exit 0
