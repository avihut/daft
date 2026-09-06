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
#   <project root>/herdr-layout.sh          next to daft.yml, outside every
#                                           checkout in the contained layout
#   <plugin config dir>/layouts/<repo>.sh    layout-agnostic per-repo home
#   <plugin config dir>/layouts/default.sh   fallback for every repo
#   ~/.config/herdr/layouts/default.sh       the pre-plugin location

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

# layout_file_for ROOT REPO_NAME — the first readable layout file, or nothing.
layout_file_for() {
  local f
  for f in "$1/herdr-layout.sh" "$CONFIG_DIR/layouts/$2.sh" "$CONFIG_DIR/layouts/default.sh" \
    "${XDG_CONFIG_HOME:-$HOME/.config}/herdr/layouts/default.sh"; do
    if [ -r "$f" ]; then
      printf '%s\n' "$f"
      return 0
    fi
  done
  return 1
}

# apply_layout PANE CHECKOUT BRANCH ROOT REPO_NAME
apply_layout() {
  local pane=$1 checkout=$2 branch=$3 root=$4 name=$5 file
  file=$(layout_file_for "$root" "$name") || { log "no layout file for $name; leaving the pane alone"; return 0; }
  after_open() { :; }
  # shellcheck disable=SC1090
  . "$file"
  LAYOUT_CWD=$checkout
  log "applying layout $file to $pane"
  printf 'layout: %s\n' "${file/#$HOME/~}"
  after_open "$pane" "$checkout" "$branch" "$(slugify "$branch")"
}
