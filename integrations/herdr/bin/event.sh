#!/bin/bash
#
# Event hook entrypoint. herdr runs one process per event with the payload in
# HERDR_PLUGIN_EVENT_JSON ({"event": "...", "data": {...}}). Everything here
# feeds the sidebar tokens; a failure only costs a stale token, so exit 0.

. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"
LOG_SCOPE=event
. "$PLUGIN_ROOT/lib/tokens.sh"

event=${HERDR_PLUGIN_EVENT:-}
json=${HERDR_PLUGIN_EVENT_JSON:-}
[ -n "$event" ] && [ -n "$json" ] || exit 0
tokens_enabled || exit 0

resolve_jq || exit 0
resolve_daft || exit 0

evt() {
  printf '%s' "$json" | "$JQ" -r "$1 // empty" 2>/dev/null
}

case $event in
  worktree.opened | worktree.created)
    path=$(evt .data.worktree.path)
    [ -n "$path" ] || exit 0
    FORCE_REFRESH=1 tokens_refresh_repo "$path"
    ;;
  workspace.focused)
    ws=$(evt .data.workspace_id)
    [ -n "$ws" ] || exit 0
    tokens_refresh_workspace "$ws"
    ;;
  pane.agent_status_changed)
    case $(evt .data.agent_status) in
      idle | done | blocked) ;;
      *) exit 0 ;;
    esac
    pane=$(evt .data.pane_id)
    [ -n "$pane" ] || exit 0
    tokens_refresh_workspace "${pane%%:*}"
    ;;
esac
exit 0
