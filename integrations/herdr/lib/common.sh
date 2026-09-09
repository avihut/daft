# shellcheck shell=bash
#
# Shared helpers for the daft herdr plugin, sourced by every entrypoint in bin/.
#
# Bash 3.2 compatible on purpose: plugin commands inherit the herdr *server's*
# environment, and on macOS under `brew services` that PATH is
# /usr/bin:/bin:/usr/sbin:/sbin — where `bash` is 3.2 and neither `daft` nor
# `jq` exist (herdrdev/herdr#3346). Every tool is resolved here, never assumed.

set -u

PLUGIN_ROOT=${HERDR_PLUGIN_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)}
PLUGIN_ID=${HERDR_PLUGIN_ID:-daft}
HERDR=${HERDR_BIN_PATH:-herdr}
CONFIG_DIR=${HERDR_PLUGIN_CONFIG_DIR:-${XDG_CONFIG_HOME:-$HOME/.config}/herdr/plugins/config/$PLUGIN_ID}
STATE_DIR=${HERDR_PLUGIN_STATE_DIR:-${XDG_STATE_HOME:-$HOME/.local/state}/herdr/plugins/$PLUGIN_ID}
CONFIG_FILE=$CONFIG_DIR/config.toml
LOG_FILE=$STATE_DIR/plugin.log
INTENT_FILE=$STATE_DIR/intent.json
TOKEN_SOURCE=plugin:$PLUGIN_ID
LOG_SCOPE=${LOG_SCOPE:-plugin}
# Marker every file this plugin installs outside its own directory carries, so
# the installer never overwrites something the user wrote.
MANAGED_MARKER="managed by the daft herdr plugin"
DAFT=
JQ=
RESOLVE_ERROR=

mkdir -p "$STATE_DIR" 2>/dev/null || true

log() {
  printf '%s [%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S')" "$LOG_SCOPE" "$*" >>"$LOG_FILE" 2>/dev/null || true
}

die() {
  printf 'error: %s\n' "$*" >&2
  log "error: $*"
  exit 1
}

# notify TITLE BODY — best-effort toast through herdr's configured delivery.
notify() {
  "$HERDR" notification show "$1" --body "$2" >/dev/null 2>&1 || true
}

# The server's PATH is not the user's shell PATH; add the usual tool homes.
augment_path() {
  local d
  for d in "$HOME/.cargo/bin" "$HOME/.local/bin" /opt/homebrew/bin /usr/local/bin "$HOME/.local/share/mise/shims"; do
    case ":$PATH:" in
      *":$d:"*) ;;
      *) [ -d "$d" ] && PATH="$PATH:$d" ;;
    esac
  done
  export PATH
}

# config_get KEY [DEFAULT] — read a flat `key = value` line from config.toml.
# The file is flat by contract (the generated default says so), so reading
# stops at the first `[table]` header rather than letting a key nested under
# one masquerade as a global.  Double-quoted strings keep everything inside
# the quotes, including a `#`; an unquoted value drops a trailing comment.
config_get() {
  local key=$1 default=${2:-} line
  [ -r "$CONFIG_FILE" ] || { printf '%s' "$default"; return 0; }
  line=$(awk -v key="$key" '
    /^[[:space:]]*\[/ { exit }
    $0 ~ "^[[:space:]]*" key "[[:space:]]*=" { last = $0 }
    END { if (last != "") print last }
  ' "$CONFIG_FILE" 2>/dev/null)
  [ -n "$line" ] || { printf '%s' "$default"; return 0; }
  line=${line#*=}
  line=$(printf '%s' "$line" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')
  case $line in
    '"'*'"') line=${line#\"}; line=${line%\"} ;;
    *) line=$(printf '%s' "$line" | sed -E 's/[[:space:]]*#.*$//; s/[[:space:]]+$//') ;;
  esac
  printf '%s' "$line"
}

# resolve_daft — sets DAFT: config `daft = "..."`, then the path recorded by
# the startup hook, then PATH (augmented). On failure it sets RESOLVE_ERROR
# and returns 1; it never exits. Each entrypoint decides what a missing tool
# means for it — a popup shows the message, an event hook shrugs, and startup
# still has to write the config file the user needs in order to fix it. A
# `die` here would have made every one of those `|| ...` fallbacks dead code
# (`||` cannot catch an `exit` in the current shell).
resolve_daft() {
  local c
  c=$(config_get daft)
  if [ -n "$c" ]; then
    c=${c/#\~/$HOME}
    [ -x "$c" ] || { RESOLVE_ERROR="daft is not executable: $c (from $CONFIG_FILE)"; return 1; }
    DAFT=$c
    return 0
  fi
  if [ -r "$STATE_DIR/daft-path" ]; then
    c=$(cat "$STATE_DIR/daft-path")
    if [ -x "$c" ]; then
      DAFT=$c
      return 0
    fi
  fi
  augment_path
  c=$(command -v daft 2>/dev/null || true)
  if [ -n "$c" ]; then
    DAFT=$(deshim "$c")
    return 0
  fi
  RESOLVE_ERROR="daft not found on the herdr server's PATH; set daft = \"/path/to/daft\" in $CONFIG_FILE"
  return 1
}

# deshim PATH — the real binary behind a mise shim, or PATH unchanged. A shim
# refuses to run in a directory whose mise.toml is not trusted, and a popup's
# cwd is whatever repository the user is standing in.
deshim() {
  local name real
  case $1 in
    */mise/shims/*)
      name=$(basename "$1")
      # A globally active tool answers from anywhere; a project-scoped one
      # only from inside that project, so also look at what mise installed.
      real=$(cd / && mise which "$name" 2>/dev/null) || real=
      if [ -z "$real" ] || [ ! -x "$real" ]; then
        # -V so 1.10 beats 1.9 (BSD sort has it too).
        real=$(find "${MISE_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/mise}/installs" -maxdepth 4 -type f -name "$name" -perm -u+x 2>/dev/null | sort -V | tail -n 1)
      fi
      if [ -n "$real" ] && [ -x "$real" ]; then
        printf '%s' "$real"
        return 0
      fi
      ;;
  esac
  printf '%s' "$1"
}

# resolve_jq — sets JQ, or sets RESOLVE_ERROR and returns 1. See resolve_daft.
resolve_jq() {
  local c
  c=$(config_get jq)
  if [ -n "$c" ]; then
    c=${c/#\~/$HOME}
    [ -x "$c" ] || { RESOLVE_ERROR="jq is not executable: $c (from $CONFIG_FILE)"; return 1; }
    JQ=$c
    return 0
  fi
  augment_path
  c=$(command -v jq 2>/dev/null || true)
  if [ -z "$c" ]; then
    RESOLVE_ERROR="jq not found on the herdr server's PATH (brew install jq / apt install jq), or set jq = \"/path/to/jq\" in $CONFIG_FILE"
    return 1
  fi
  JQ=$(deshim "$c")
}

# daft_quiet ARGS... — daft for machine consumption: no update check, no trust
# prune, no log clean, no hints, no live table, and never a cd redirect.
daft_quiet() {
  [ -n "$DAFT" ] || { log "daft_quiet called before resolve_daft"; return 1; }
  (
    unset DAFT_CD_FILE
    DAFT_NO_UPDATE_CHECK=1 DAFT_NO_TRUST_PRUNE=1 DAFT_NO_LOG_CLEAN=1 DAFT_NO_HINTS=1 DAFT_NO_LIVE=1 \
      "$DAFT" "$@"
  )
}

# canon PATH — the physical path. For a path that no longer exists (a removed
# worktree, possibly with its now-empty parents) the nearest existing ancestor
# is canonicalized and the missing tail re-attached.
canon() {
  local p=$1 tail=
  while [ ! -d "$p" ]; do
    case $p in
      */*) ;;
      *) printf '%s\n' "$1"; return 0 ;;
    esac
    tail="$(basename "$p")${tail:+/$tail}"
    p=$(dirname "$p")
  done
  p=$(cd -P -- "$p" 2>/dev/null && pwd -P) || { printf '%s\n' "$1"; return 0; }
  printf '%s%s\n' "${p%/}" "${tail:+/$tail}"
}

# ctx JQ_EXPR — one field of HERDR_PLUGIN_CONTEXT_JSON, empty when absent.
ctx() {
  [ -n "${HERDR_PLUGIN_CONTEXT_JSON:-}" ] || return 0
  printf '%s' "$HERDR_PLUGIN_CONTEXT_JSON" | "$JQ" -r "$1 // empty" 2>/dev/null
}

# context_cwd — where the invoking user is: the focused pane's cwd, else the
# workspace cwd, else the workspace's checkout. Never $PWD: plugin processes
# start in the plugin root (actions) or the server's cwd (popups, #2050).
context_cwd() {
  local c
  c=$(ctx .focused_pane_cwd)
  [ -n "$c" ] || c=$(ctx .workspace_cwd)
  [ -n "$c" ] || c=$(ctx .worktree.checkout_path)
  printf '%s' "$c"
}

# write_intent MODE CWD — what an action wanted, for the popup it opens. The
# popup gets its own context, but the action's is the one the user pressed the
# key in.
write_intent() {
  "$JQ" -n --arg mode "$1" --arg cwd "$2" --arg ws "$(ctx .workspace_id)" --arg pane "$(ctx .focused_pane_id)" \
    --argjson ts "$(date +%s)" '{mode: $mode, cwd: $cwd, workspace_id: $ws, pane_id: $pane, ts: $ts}' \
    >"$INTENT_FILE" 2>/dev/null || true
}

# read_intent FIELD — a field of a recent (< 60 s) intent, else empty.
read_intent() {
  local ts now
  [ -r "$INTENT_FILE" ] || return 0
  ts=$("$JQ" -r '.ts // 0' "$INTENT_FILE" 2>/dev/null)
  now=$(date +%s)
  [ $((now - ts)) -lt 60 ] || return 0
  "$JQ" -r ".$1 // empty" "$INTENT_FILE" 2>/dev/null
}

# repo_info PATH — daft's catalog view of the repository containing PATH:
# name, path (project root), git_common_dir, default_branch, layout, worktrees.
repo_info() {
  daft_quiet repo info "$1" --format json 2>/dev/null
}

# repo_info_adopting PATH — repo_info, and on a catalog miss register PATH's
# repository with daft and ask once more.
#
# `repo info` is the catalog *view*: it reads and never upserts, so in a plain
# `git clone` daft has not operated in yet it fails with "not found in the
# catalog". daft's own catalog is ambient — one daft command inside that clone
# registers it — so refusing there asked daft the one question it cannot
# answer about a new repository, then gave up before daft got to touch it.
#
# Only the explicit actions use this. The passive paths (the token refresh
# behind workspace.focused / pane.agent_status_changed, bin/event.sh, the daft
# hooks) stay on plain repo_info: they fire for every workspace the user so
# much as looks at, and cataloging on a glance would put every repository
# herdr ever touched into `daft repo list`, and so into the scope of
# `daft update --all-repos` and `daft prune --all-repos`. daft's own rule is
# "operated in", not "looked at".
repo_info_adopting() {
  local info out
  if info=$(repo_info "$1") && [ -n "$info" ]; then
    printf '%s' "$info"
    return 0
  fi
  # stderr only: with -q there is nothing on stdout, and the message worth
  # logging is the refusal ("not a git repository", a catalog error).
  if ! out=$(daft_quiet repo add -q "$1" 2>&1 >/dev/null); then
    log "repo add failed for $1: $out"
    return 1
  fi
  if ! info=$(repo_info "$1") || [ -z "$info" ]; then
    log "repo info still refuses $1 after adding it to the catalog"
    return 1
  fi
  log "added $1 to the daft catalog"
  printf '%s' "$info"
}

repo_field() {
  printf '%s' "$1" | "$JQ" -r "$2 // empty"
}

# worktree_containing INFO PATH — "branch<TAB>path" of the worktree that holds
# PATH (deepest match). The branch is empty for a detached sandbox.
worktree_containing() {
  local info=$1 want branch path cp
  want=$(canon "$2")
  printf '%s' "$info" \
    | "$JQ" -r '.worktrees[]? | [(.branch // ""), .path] | @tsv' \
    | awk -F'\t' '{ print length($2) "\t" $0 }' | sort -t "$(printf '\t')" -k1,1 -rn | cut -f2- \
    | while IFS=$'\t' read -r branch path; do
        cp=$(canon "$path")
        case "$want" in
          "$cp" | "$cp"/*) printf '%s\t%s\n' "$branch" "$cp"; break ;;
        esac
      done
}

# workspace_for_path PATH — the herdr workspace whose worktree provenance is
# PATH, compared physically on both sides (macOS keeps the caller's casing,
# herdrdev/herdr#3677).
workspace_for_path() {
  local want id path
  want=$(canon "$1")
  "$HERDR" workspace list 2>/dev/null \
    | "$JQ" -r '.result.workspaces[]? | select(.worktree != null) | [.workspace_id, .worktree.checkout_path] | @tsv' 2>/dev/null \
    | while IFS=$'\t' read -r id path; do
        if [ "$(canon "$path")" = "$want" ]; then
          printf '%s\n' "$id"
          break
        fi
      done
}

# register_worktree ROOT PATH LABEL [--focus|--no-focus] — open PATH as a
# grouped child of ROOT's repo row. Prints the root pane id of a newly opened
# workspace; prints nothing when herdr only focused an already-open one.
#
# herdr answers an already-open checkout with a full `root_pane` all the same
# (src/app/api/worktrees.rs), so `already_open` is the only thing separating
# "here is a fresh pane to lay out" from "that is the workspace you are
# working in". Reading the pane id unconditionally re-ran the layout over a
# live workspace: extra splits, commands typed into a pane in use, a second
# agent. Stderr is captured apart from stdout for the same reason — folded
# in, one herdr warning makes the reply unparseable and the layout silently
# does not happen.
register_worktree() {
  local root=$1 path=$2 label=$3 focus=${4:---focus} out err rc
  err=$(mktemp "${TMPDIR:-/tmp}/daft-herdr-err.XXXXXX") || err=/dev/null
  out=$("$HERDR" worktree open --cwd "$root" --path "$path" --label "$label" "$focus" 2>"$err")
  rc=$?
  if [ "$rc" -ne 0 ]; then
    out="${out}$(cat "$err" 2>/dev/null)"
    [ "$err" = /dev/null ] || rm -f "$err"
    log "worktree open failed for $path: $out"
    printf '%s\n' "$out" >&2
    return 1
  fi
  [ "$err" = /dev/null ] || rm -f "$err"
  log "registered $path as $(printf '%s' "$out" | "$JQ" -r '.result.workspace.workspace_id // "?"' 2>/dev/null) ($focus)"
  printf '%s' "$out" | "$JQ" -r 'select(.result.already_open != true) | .result.root_pane.pane_id // empty' 2>/dev/null
}
