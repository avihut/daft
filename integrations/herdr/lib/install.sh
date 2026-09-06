# shellcheck shell=bash
#
# Installing the daft side of the bridge: two executable hook scripts in
# daft's user-global hooks directory. daft runs them in every repository,
# trusted or not, after creating and before removing a worktree (post-remove
# would be the natural hook, but daft 1.27 spawns it inside the directory it
# just deleted, so it never runs for `daft remove`); each one execs
# bin/daft-hook.sh from this plugin root. A hook file that does not
# carry the plugin's marker is someone else's and is left alone.

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

install_user_hooks() {
  local dir hook target tmpl
  dir=$(user_hooks_dir) || { log "daft __dirs failed; hooks not installed"; return 1; }
  mkdir -p "$dir" || return 1
  for hook in worktree-post-create worktree-pre-remove; do
    target=$dir/$hook
    tmpl=$PLUGIN_ROOT/hooks/$hook
    if [ -e "$target" ] && ! grep -q "$MANAGED_MARKER" "$target" 2>/dev/null; then
      log "leaving foreign hook alone: $target"
      continue
    fi
    sed "s|__PLUGIN_ROOT__|$PLUGIN_ROOT|g" "$tmpl" >"$target.tmp" \
      && chmod 755 "$target.tmp" \
      && mv -f "$target.tmp" "$target" \
      || { log "could not install $target"; rm -f "$target.tmp"; return 1; }
  done
  log "hooks installed in $dir"
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
