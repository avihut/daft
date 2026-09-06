#!/bin/bash
#
# Popup entrypoint: `pane.sh <mode>`. Runs in a herdr popup with a real TTY,
# so daft's progress rail, prompts and trust gate all work. Errors stay on
# screen until a key is pressed.
#
# DAFT_HERDR_PLUGIN_ACTIVE=1 tells the daft post-create hook that this popup
# registers the new worktree itself (with focus and layout), so a creation
# is mirrored exactly once.

. "$(dirname "${BASH_SOURCE[0]}")/../lib/common.sh"
LOG_SCOPE=pane
. "$PLUGIN_ROOT/lib/layout.sh"
. "$PLUGIN_ROOT/lib/tokens.sh"

mode=${1:-}

pause() {
  [ -t 0 ] || return 0
  printf '\n'
  read -rs -n 1 -p 'press any key to close'
}

fail() {
  printf 'error: %s\n' "$*" >&2
  log "error: $*"
  pause
  exit 1
}

resolve_jq || fail "$RESOLVE_ERROR"
resolve_daft || fail "$RESOLVE_ERROR"

# ask VAR PROMPT — read one line into VAR; false on EOF.
ask() {
  local __line
  IFS= read -r -p "$2" __line || return 1
  eval "$1=\$__line"
}

# popup_cwd — the directory the user invoked the action from.
popup_cwd() {
  local c
  c=$(read_intent cwd)
  [ -n "$c" ] || c=$(context_cwd)
  [ -n "$c" ] || c=$HOME
  printf '%s' "$c"
}

# open_and_layout PATH LABEL — register PATH with herdr, focus it, apply the
# repo layout to its root pane, refresh tokens in the background.
open_and_layout() {
  local path=$1 label=$2 info root name pane
  info=$(repo_info "$path") || fail "daft does not recognize $path as a worktree"
  root=$(repo_field "$info" .path)
  name=$(repo_field "$info" .name)
  printf '\n» herdr worktree open --cwd %s --path %s\n' "$root" "$path"
  pane=$(register_worktree "$root" "$path" "$label" --focus) || fail "herdr could not open $path"
  if [ -n "$pane" ]; then
    apply_layout "$pane" "$path" "$label" "$name"
  fi
  ( FORCE_REFRESH=1 tokens_refresh_repo "$root" </dev/null >/dev/null 2>&1 & )
  printf 'opened %s under %s%s\n' "$label" "$name" "${pane:+ (pane $pane)}"
}

# run_daft_cd ARGS... — run daft with a cd file; sets DAFT_RESULT_PATH.
run_daft_cd() {
  local cd_file status
  cd_file=$(mktemp "${TMPDIR:-/tmp}/daft-herdr.XXXXXX") || fail "cannot create a temp file"
  DAFT_HERDR_PLUGIN_ACTIVE=1 DAFT_CD_FILE=$cd_file "$DAFT" "$@"
  status=$?
  DAFT_RESULT_PATH=$(cat "$cd_file" 2>/dev/null)
  rm -f "$cd_file"
  return $status
}

DAFT_RESULT_PATH=
cwd=$(popup_cwd)
cd -- "$cwd" 2>/dev/null || cd "$HOME" || exit 1

case $mode in
  start)
    info=$(repo_info "$cwd") || fail "not inside a repository daft can manage: $cwd"
    current=$(git -C "$cwd" branch --show-current 2>/dev/null)
    [ -n "$current" ] || current=detached
    printf 'repo:    %s  (%s)\ncurrent: %s\n\n' "$(repo_field "$info" .name)" "$(repo_field "$info" .path)" "$current"
    ask branch 'new branch name: ' || fail "aborted"
    [ -n "$branch" ] || fail "aborted: empty branch name"
    ask base 'base (enter = current branch): ' || base=
    printf '\n» daft start %s%s\n\n' "$branch" "${base:+ $base}"
    run_daft_cd -C "$cwd" start "$branch" ${base:+"$base"} || fail "daft start failed"
    [ -n "$DAFT_RESULT_PATH" ] && [ -d "$DAFT_RESULT_PATH" ] || fail "daft finished but reported no worktree path (daft.autocd off?)"
    open_and_layout "$DAFT_RESULT_PATH" "$branch"
    ;;

  go)
    info=$(repo_info "$cwd" 2>/dev/null) || info=
    if [ -n "$info" ]; then
      printf 'repo: %s  (%s)\n\n' "$(repo_field "$info" .name)" "$(repo_field "$info" .path)"
    fi
    printf 'a branch, "repo branch", pr:N, a commit, or - for the previous worktree\n'
    ask target 'go to: ' || fail "aborted"
    [ -n "$target" ] || fail "aborted: empty target"
    # Split on whitespace, but never glob: the popup's cwd is a checkout, so
    # a `*` typed at the prompt would otherwise expand to its filenames.
    set -f
    # shellcheck disable=SC2086
    set -- $target
    set +f
    printf '\n» daft go %s\n\n' "$*"
    run_daft_cd -C "$cwd" go "$@" || fail "daft go failed"
    [ -n "$DAFT_RESULT_PATH" ] && [ -d "$DAFT_RESULT_PATH" ] || fail "daft finished but reported no path (daft.autocd off?)"
    info=$(repo_info "$DAFT_RESULT_PATH") || fail "daft landed outside a repository it knows: $DAFT_RESULT_PATH"
    if [ "$(canon "$DAFT_RESULT_PATH")" = "$(canon "$(repo_field "$info" .path)")" ]; then
      # A repository root with no worktree for its default branch: a plain workspace.
      "$HERDR" workspace create --cwd "$DAFT_RESULT_PATH" --label "$(repo_field "$info" .name)" --focus >/dev/null \
        || fail "herdr could not open $DAFT_RESULT_PATH"
      printf 'opened %s\n' "$(repo_field "$info" .name)"
    else
      entry=$(worktree_containing "$info" "$DAFT_RESULT_PATH")
      IFS=$'\t' read -r label _ <<<"$entry"
      [ -n "$label" ] || label=$(basename "$DAFT_RESULT_PATH")
      open_and_layout "$DAFT_RESULT_PATH" "$label"
    fi
    ;;

  fork)
    info=$(repo_info "$cwd") || fail "not inside a repository daft can manage: $cwd"
    printf 'repo: %s  (%s)\n\n' "$(repo_field "$info" .name)" "$(repo_field "$info" .path)"
    ask base 'fork from (enter = HEAD): ' || base=
    ask count 'how many (enter = 1): ' || count=
    case ${count:-1} in
      '' | *[!0-9]*) fail "count must be a number" ;;
    esac
    printf '\n» daft start --fork%s%s\n\n' "${base:+ $base}" "${count:+ -n $count}"
    paths=$(DAFT_HERDR_PLUGIN_ACTIVE=1 "$DAFT" -C "$cwd" start --fork ${base:+"$base"} ${count:+-n "$count"}) || fail "daft start --fork failed"
    [ -n "$paths" ] || fail "daft created no fork"
    focus=--focus
    printf '%s\n' "$paths" | while IFS= read -r path; do
      [ -d "$path" ] || continue
      label=$(basename "$path")
      forkinfo=$(repo_info "$path")
      root=$(repo_field "$forkinfo" .path)
      name=$(repo_field "$forkinfo" .name)
      pane=$(register_worktree "$root" "$path" "$label" "$focus") || { printf 'herdr could not open %s\n' "$path" >&2; continue; }
      [ -n "$pane" ] && apply_layout "$pane" "$path" "$label" "$name"
      printf 'opened %s%s\n' "$label" "${pane:+ (pane $pane)}"
      focus=--no-focus
    done
    ( FORCE_REFRESH=1 tokens_refresh_repo "$cwd" </dev/null >/dev/null 2>&1 & )
    ;;

  remove)
    info=$(repo_info "$cwd") || fail "not inside a repository daft can manage: $cwd"
    root=$(repo_field "$info" .path)
    entry=$(worktree_containing "$info" "$cwd")
    [ -n "$entry" ] || fail "not inside a worktree: $cwd"
    IFS=$'\t' read -r branch path <<<"$entry"
    label=$branch
    [ -n "$label" ] || label=$(basename "$path")
    if [ "$(canon "$path")" = "$(canon "$root")" ]; then
      fail "$path is the repository root, not a worktree"
    fi
    printf 'repo:     %s\nworktree: %s\npath:     %s\n\n' "$(repo_field "$info" .name)" "$label" "$path"
    if [ -n "$branch" ]; then
      ask answer "remove '$label' and delete its branch? [y/N] " || fail "aborted"
    else
      ask answer "remove the sandbox '$label'? [y/N] " || fail "aborted"
    fi
    case $answer in y | Y | yes) ;; *) fail "aborted" ;; esac
    ask answer 'force (-f, skips the merge and sync safety checks)? [y/N] ' || answer=
    force=
    case $answer in y | Y | yes) force=-f ;; esac
    printf '\n» daft remove %s%s\n\n' "$path" "${force:+ $force}"
    cd -- "$root" || fail "cannot cd to $root"
    # The pre-remove hook closes the workspace once daft has exited and the directory is gone.
    "$DAFT" -C "$root" remove "$path" ${force:+"$force"} || fail "daft remove failed"
    printf 'removed %s\n' "$label"
    ;;

  repo)
    list=$(daft_quiet repo list --format json 2>/dev/null) || fail "daft repo list failed"
    count=$(printf '%s' "$list" | "$JQ" 'length')
    [ "${count:-0}" -gt 0 ] || fail "the daft catalog is empty (daft repo add <path>)"
    printf 'cataloged repositories:\n\n'
    printf '%s' "$list" | "$JQ" -r 'to_entries[] | "\(.key + 1 | tostring | if length < 2 then " " + . else . end)  \(.value.name)\t\(.value.path)"' \
      | awk -F'\t' '{ printf "%-40s %s\n", $1, $2 }'
    printf '\n'
    ask pick 'open: ' || fail "aborted"
    case $pick in
      '' | *[!0-9]*) fail "aborted" ;;
    esac
    [ "$pick" -ge 1 ] && [ "$pick" -le "$count" ] || fail "no such entry: $pick"
    name=$(printf '%s' "$list" | "$JQ" -r ".[$((pick - 1))].name")
    root=$(printf '%s' "$list" | "$JQ" -r ".[$((pick - 1))].path")
    info=$(repo_info "$root") || fail "daft repo info failed for $name"
    default=$(repo_field "$info" .default_branch)
    path=$(printf '%s' "$info" | "$JQ" -r --arg b "$default" '.worktrees[]? | select(.branch == $b) | .path' | head -n 1)
    if [ -n "$path" ] && [ -d "$path" ] && [ "$(canon "$path")" != "$(canon "$root")" ]; then
      open_and_layout "$path" "$default"
    else
      "$HERDR" workspace create --cwd "$root" --label "$name" --focus >/dev/null || fail "herdr could not open $root"
      printf 'opened %s\n' "$name"
    fi
    ;;

  *)
    fail "usage: pane.sh start|go|fork|remove|repo"
    ;;
esac
