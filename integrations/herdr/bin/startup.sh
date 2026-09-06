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

# herdr always sets HERDR_PLUGIN_ROOT for a plugin process. Without it this is
# a developer running the script by hand, and installing the hooks would write
# their real daft config directory (CLAUDE.md, "Test Hygiene").
if [ -z "${HERDR_PLUGIN_ROOT:-}" ]; then
  printf 'startup.sh is run by herdr, which sets HERDR_PLUGIN_ROOT.\n' >&2
  printf 'To exercise it by hand, set HERDR_PLUGIN_ROOT, HERDR_PLUGIN_CONFIG_DIR,\n' >&2
  printf 'HERDR_PLUGIN_STATE_DIR and DAFT_CONFIG_DIR to scratch directories first.\n' >&2
  exit 1
fi

# The log is append-only and the server starts often enough to be the right
# place to bound it.
if [ -f "$LOG_FILE" ] && [ -n "$(find "$LOG_FILE" -size +1024k 2>/dev/null)" ]; then
  tail -n 500 "$LOG_FILE" >"$LOG_FILE.tmp" 2>/dev/null && mv -f "$LOG_FILE.tmp" "$LOG_FILE"
fi

# Before anything that can fail: the config file is where a user fixes a
# `jq`/`daft` that could not be found, so it has to exist even then.
write_default_config

if ! resolve_jq; then
  log "$RESOLVE_ERROR"
  log "the plugin is inert until jq is installed"
  notify "daft plugin" "$RESOLVE_ERROR"
  exit 0
fi

if ! resolve_daft; then
  log "$RESOLVE_ERROR"
  notify "daft plugin" "$RESOLVE_ERROR"
  exit 0
fi
printf '%s' "$DAFT" >"$STATE_DIR/daft-path"
log "startup: daft at $DAFT, plugin at $PLUGIN_ROOT"

install_user_hooks || log "hook install incomplete${SKIPPED_HOOKS:+ (kept your own: $SKIPPED_HOOKS)}"

# Re-seed tokens: herdr forgets token metadata across restarts. Everything in
# the loop reads from /dev/null so it cannot eat the rows still to come.
"$HERDR" workspace list 2>/dev/null \
  | "$JQ" -r '.result.workspaces[]? | select(.worktree != null) | .worktree.repo_root' 2>/dev/null \
  | sort -u \
  | while IFS= read -r root; do
      [ -n "$root" ] || continue
      FORCE_REFRESH=1 tokens_refresh_repo "$root" </dev/null
    done
exit 0
