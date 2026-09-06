# shellcheck shell=bash
#
# Per-repo layouts. A layout file defines
#
#   after_open PANE CHECKOUT BRANCH SLUG
#
# and may call the helpers below; every pane it opens starts in CHECKOUT.
# HERDR_LAYOUT_DRY_RUN=1 makes `run` and `start_agent` print instead of act
# (splits still happen). Files are looked up in this order:
#
#   <plugin config dir>/layouts/<repo>.sh   per repo, by daft's catalog name
#   <plugin config dir>/layouts/default.sh  fallback for every repo
#   ~/.config/herdr/layouts/default.sh      the pre-plugin location
#
# Every candidate lives in a directory the user owns, and that is deliberate.
# `<project root>/herdr-layout.sh` was a fourth candidate until #950's review
# found the hole: in the *sibling* layout — daft's default — the project root
# IS the default branch's checkout, so that file arrives with `git clone`.
# Sourcing it would have run a cloned repository's shell code on the first
# `daft go` into it. daft gates `.daft/hooks` behind a trust database; this
# plugin has none, so it never reads a layout out of a checkout.

LAYOUT_CWD=

split_right() {
  "$HERDR" pane split --pane "$1" --direction right --cwd "$LAYOUT_CWD" --no-focus | "$JQ" -r '.result.pane.pane_id // empty'
}

split_down() {
  "$HERDR" pane split --pane "$1" --direction down --cwd "$LAYOUT_CWD" --no-focus | "$JQ" -r '.result.pane.pane_id // empty'
}

run() {
  if [ -n "${HERDR_LAYOUT_DRY_RUN:-}" ]; then
    printf '[dry-run] run %s: %s\n' "$1" "$2" >&2
    return 0
  fi
  "$HERDR" pane run "$1" "$2" >/dev/null
}

start_agent() {
  if [ -n "${HERDR_LAYOUT_DRY_RUN:-}" ]; then
    printf '[dry-run] agent start %s --kind %s --pane %s\n' "$1" "$2" "$3" >&2
    return 0
  fi
  "$HERDR" agent start "$1" --kind "$2" --pane "$3" >/dev/null
}

# slugify BRANCH — an agent name herdr accepts: [a-z][a-z0-9_-]{0,31}.
slugify() {
  printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9_-]+/-/g; s/^[^a-z]/wt-&/' | cut -c1-32
}

# layout_file_for REPO_NAME — the first readable layout file, or nothing.
# The name indexes into a directory, and daft derives it from a clone URL, so
# anything that could climb out of `layouts/` disqualifies the per-repo
# candidate rather than being sanitized into a different repo's file.
layout_file_for() {
  local name=$1 f
  case $name in
    */* | .* | '') name= ;;
  esac
  for f in ${name:+"$CONFIG_DIR/layouts/$name.sh"} "$CONFIG_DIR/layouts/default.sh" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/layouts/default.sh"; do
    if [ -r "$f" ]; then
      printf '%s\n' "$f"
      return 0
    fi
  done
  return 1
}

# apply_layout PANE CHECKOUT BRANCH REPO_NAME
#
# The file is sourced in a subshell. It is user code: a bare `exit`, an unset
# variable under `set -u`, or a failing helper inside it would otherwise take
# the caller down with it — and for the daft hook that means failing a
# worktree creation daft has already completed.
apply_layout() {
  local pane=$1 checkout=$2 branch=$3 name=$4 file
  file=$(layout_file_for "$name") || { log "no layout file for $name; leaving the pane alone"; return 0; }
  log "applying layout $file to $pane"
  printf 'layout: %s\n' "${file/#$HOME/~}"
  (
    set +u
    LAYOUT_CWD=$checkout
    after_open() { :; }
    # shellcheck disable=SC1090
    . "$file"
    after_open "$pane" "$checkout" "$branch" "$(slugify "$branch")"
  ) || log "layout $file exited non-zero"
  return 0
}
