#!/bin/bash
#
# Startup hook: once per herdr server start, and again after a live handoff.
# Records where daft lives, writes a commented default config on first run,
# installs the daft user-global hooks, and re-seeds the sidebar tokens for
# every open worktree workspace. Never fails the server: exit 0 throughout.

. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"
LOG_SCOPE=startup
. "$PLUGIN_ROOT/lib/tokens.sh"
. "$PLUGIN_ROOT/lib/install.sh"

resolve_jq 2>/dev/null || { log "jq not found; the plugin is inert until it is installed"; exit 0; }
write_default_config

if ! resolve_daft 2>/dev/null; then
  log "daft not found; set daft = \"...\" in $CONFIG_FILE"
  notify "daft plugin" "daft not found on the server PATH; set daft = \"/path/to/daft\" in $CONFIG_FILE"
  exit 0
fi
printf '%s' "$DAFT" >"$STATE_DIR/daft-path"
log "startup: daft at $DAFT, plugin at $PLUGIN_ROOT"

install_user_hooks || log "hook install skipped"

# Re-seed tokens: herdr forgets token metadata across restarts.
"$HERDR" workspace list 2>/dev/null \
  | "$JQ" -r '.result.workspaces[]? | select(.worktree != null) | .worktree.repo_root' 2>/dev/null \
  | sort -u \
  | while IFS= read -r root; do
      [ -n "$root" ] || continue
      FORCE_REFRESH=1 tokens_refresh_repo "$root"
    done
exit 0
