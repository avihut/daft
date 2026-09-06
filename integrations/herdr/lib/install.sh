# shellcheck shell=bash
#
# Installing the daft side of the bridge: two executable hook scripts in
# daft's user-global hooks directory. daft runs a user-global hook in every
# repository without consulting its trust database (that gate is for a
# repository's own `.daft/hooks`), after creating and before removing a
# worktree — post-remove would be the natural second hook, but daft 1.27
# spawns it inside the directory it just deleted, so it never runs for
# `daft remove`. Each one execs bin/daft-hook.sh from this plugin root, and
# does nothing at all outside a herdr pane.
#
# The scripts are generated here rather than copied from a template so the
# plugin root reaches them as a single-quoted shell literal. A `sed`
# substitution could not: `&` and `|` are special on the replacement side,
# and a `"` or `$` in the path would have landed inside a double-quoted
# string in the generated file.
#
# A hook file that does not carry the plugin's marker is someone else's and
# is left alone.

# user_hooks_dir — <daft config dir>/hooks, asked from daft itself so the
# platform convention (~/.config on Linux, Application Support on macOS,
# DAFT_CONFIG_DIR in dev builds) is daft's call, not ours.
user_hooks_dir() {
  local dirs config
  dirs=$(daft_quiet __dirs 2>/dev/null) || return 1
  config=$(printf '%s\n' "$dirs" | awk -F'\t' '$1 == "config" { print $2 }')
  [ -n "$config" ] || return 1
  printf '%s/hooks\n' "$config"
}

# shell_quote STRING — STRING as a single-quoted shell literal.
shell_quote() {
  printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# hook_script HOOK — the file contents for one hook.
hook_script() {
  printf '#!/bin/bash\n'
  printf '# %s — reinstalled on every herdr start; edit the plugin, not this file.\n' "$MANAGED_MARKER"
  printf '# daft runs this around its worktree lifecycle. It does nothing outside a\n'
  printf '# herdr pane, and it never fails daft: the bridge exits 0 on every path.\n'
  printf '[ "${HERDR_ENV:-}" = "1" ] || exit 0\n'
  printf 'bridge=%s\n' "$(shell_quote "$PLUGIN_ROOT/bin/daft-hook.sh")"
  # An uninstalled plugin must not turn every worktree creation into a hook
  # failure, so a missing bridge is a no-op rather than a failed exec.
  printf '[ -x "$bridge" ] || exit 0\n'
  printf 'exec /bin/bash "$bridge" %s\n' "$(shell_quote "$1")"
}

# install_user_hooks — 0 when both hooks are the plugin's, 1 when one was
# left alone or could not be written. SKIPPED_HOOKS names the foreign ones.
install_user_hooks() {
  local dir hook target rc=0
  SKIPPED_HOOKS=
  dir=$(user_hooks_dir) || { log "daft __dirs failed; hooks not installed"; return 1; }
  mkdir -p "$dir" || return 1
  for hook in worktree-post-create worktree-pre-remove; do
    target=$dir/$hook
    if [ -e "$target" ] && ! grep -q "$MANAGED_MARKER" "$target" 2>/dev/null; then
      log "leaving foreign hook alone: $target"
      SKIPPED_HOOKS="${SKIPPED_HOOKS:+$SKIPPED_HOOKS, }$hook"
      rc=1
      continue
    fi
    hook_script "$hook" >"$target.tmp" \
      && chmod 755 "$target.tmp" \
      && mv -f "$target.tmp" "$target" \
      || { log "could not install $target"; rm -f "$target.tmp"; return 1; }
  done
  if [ -n "$SKIPPED_HOOKS" ]; then
    log "hooks installed in $dir (kept your own: $SKIPPED_HOOKS)"
  else
    log "hooks installed in $dir"
  fi
  return $rc
}

uninstall_user_hooks() {
  local dir hook target
  dir=$(user_hooks_dir) || return 1
  for hook in worktree-post-create worktree-pre-remove; do
    target=$dir/$hook
    [ -e "$target" ] || continue
    grep -q "$MANAGED_MARKER" "$target" 2>/dev/null && rm -f "$target"
  done
}

write_default_config() {
  [ -e "$CONFIG_FILE" ] && return 0
  mkdir -p "$CONFIG_DIR" 2>/dev/null || return 0
  cat >"$CONFIG_FILE" <<'CFG'
# daft herdr plugin — flat key = value lines; edits apply on the next action.
# There are no [tables]: everything below the first one is ignored.

# Where daft and jq are. Resolved automatically from the usual tool homes when
# unset (a mise shim is replaced by the binary behind it); set them if herdr
# runs as a service with a bare PATH.
# daft = "/opt/homebrew/bin/daft"
# jq = "/opt/homebrew/bin/jq"

# Popup size for start / go / fork / remove / repo. `popup_placement` may also
# be "split" (a pane below the current one).
# popup_placement = "popup"
# popup_width = "70%"
# popup_height = "60%"

# Apply the repository's herdr layout when a worktree arrives through daft's
# hooks (a `daft start` typed in any pane, an agent creating one, daft sync).
# The popup actions always apply it.
# layout_on_hook = false

# The `$daft` sidebar token. `tokens_pr` adds the pull request and CI state,
# which costs a forge call per refresh.
# tokens = true
# tokens_pr = false
# tokens_ttl_ms = 86400000
# refresh_debounce_secs = 15
CFG
}
