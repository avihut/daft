#!/bin/bash
#
# Headless action entrypoint: `action.sh <mode>`. Interactive modes open their
# popup pane (where daft gets a TTY); the rest act right away and report
# through a notification.

. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"
LOG_SCOPE=action
. "$PLUGIN_ROOT/lib/layout.sh"
. "$PLUGIN_ROOT/lib/tokens.sh"

mode=${1:-}
resolve_jq

# open_popup MODE — open the popup entrypoint for MODE where the user is.
open_popup() {
  local cwd placement width height out
  cwd=$(context_cwd)
  write_intent "$1" "$cwd"
  placement=$(config_get popup_placement popup)
  width=$(config_get popup_width 70%)
  height=$(config_get popup_height 60%)
  set -- --plugin "$PLUGIN_ID" --entrypoint "$1" --placement "$placement" --focus
  [ "$placement" = popup ] && set -- "$@" --width "$width" --height "$height"
  [ "$placement" = split ] && set -- "$@" --direction down
  [ -n "$cwd" ] && set -- "$@" --cwd "$cwd"
  if ! out=$("$HERDR" plugin pane open "$@" 2>&1); then
    log "plugin pane open failed: $out"
    notify "daft" "could not open the popup: $out"
    die "could not open the popup: $out"
  fi
}

case $mode in
  start | go | fork | remove | repo)
    open_popup "$mode"
    ;;

  adopt)
    resolve_daft
    cwd=$(context_cwd)
    [ -n "$cwd" ] || die "no working directory in the invocation context"
    info=$(repo_info "$cwd") || { notify "daft" "$cwd is not inside a repository daft knows"; exit 1; }
    root=$(repo_field "$info" .path)
    entry=$(worktree_containing "$info" "$cwd")
    [ -n "$entry" ] || { notify "daft" "$cwd is not inside a worktree of $(repo_field "$info" .name)"; exit 1; }
    branch=${entry%%	*}
    path=${entry#*	}
    [ -n "$branch" ] || branch=$(basename "$path")
    if [ "$(canon "$path")" = "$(canon "$root")" ]; then
      notify "daft" "$path is the repository root, not a worktree"
      exit 1
    fi
    register_worktree "$root" "$path" "$branch" --focus >/dev/null || { notify "daft: adopt failed" "herdr refused to open $path"; exit 1; }
    ( FORCE_REFRESH=1 tokens_refresh_repo "$root" </dev/null >/dev/null 2>&1 & )
    notify "daft" "$(repo_field "$info" .name) ▸ $branch is now grouped"
    ;;

  layout)
    resolve_daft
    pane=$(ctx .focused_pane_id)
    [ -n "$pane" ] || pane=$(read_intent pane_id)
    [ -n "$pane" ] || die "no focused pane in the invocation context"
    cwd=$(context_cwd)
    info=$(repo_info "$cwd") || { notify "daft" "$cwd is not inside a repository daft knows"; exit 1; }
    entry=$(worktree_containing "$info" "$cwd")
    [ -n "$entry" ] || { notify "daft" "$cwd is not inside a worktree"; exit 1; }
    branch=${entry%%	*}
    path=${entry#*	}
    [ -n "$branch" ] || branch=$(basename "$path")
    apply_layout "$pane" "$path" "$branch" "$(repo_field "$info" .path)" "$(repo_field "$info" .name)" >/dev/null
    notify "daft" "layout applied: $(repo_field "$info" .name) ▸ $branch"
    ;;

  tokens)
    resolve_daft
    cwd=$(context_cwd)
    [ -n "$cwd" ] || die "no working directory in the invocation context"
    FORCE_REFRESH=1 tokens_refresh_repo "$cwd"
    ;;

  install-hooks)
    resolve_daft
    . "$PLUGIN_ROOT/lib/install.sh"
    install_user_hooks && notify "daft" "hooks installed in $(user_hooks_dir)"
    ;;

  *)
    die "usage: action.sh start|go|fork|remove|repo|adopt|layout|tokens|install-hooks"
    ;;
esac
