# shellcheck shell=bash
#
# The `$daft` sidebar token: one compact status string per worktree row, from
# `daft list --format json`. herdr hides its built-in branch/status tokens on
# indented worktree rows by design, so this is the row's only status.
#
#   ↑3 ↓1     commits ahead / behind the base branch
#   +2 ~1 ?4  staged / modified / untracked files, !N conflicted
#   ⇡2 ⇣0     commits unpushed / unpulled against the remote
#   rebasing  a paused git operation
#   #42 open  the pull request, with ✓ ✗ ◌ for CI (opt-in: tokens_pr = true)
#   ✓         nothing to report

tokens_enabled() {
  [ "$(config_get tokens true)" = true ]
}

# lock_is_stale DIR — a crashed refresh rather than a running one: the holder
# it recorded is gone, or it recorded none and the directory is over a minute
# old. Reading the holder's pid rather than only the mtime is what lets a
# legitimately slow refresh (tokens_pr = true adds a forge call per repo) keep
# its lock instead of having it stolen out from under it.
lock_is_stale() {
  local pid
  pid=$(cat "$1/pid" 2>/dev/null)
  case $pid in
    '' | *[!0-9]*) [ -n "$(find "$1" -maxdepth 0 -mmin +1 2>/dev/null)" ] ;;
    *) ! kill -0 "$pid" 2>/dev/null ;;
  esac
}

# tokens_refresh_repo PATH — refresh every open workspace of the repository
# containing PATH. Debounced per repo (refresh_debounce_secs, FORCE_REFRESH=1
# bypasses) and serialized with a lock so concurrent event hooks cost one
# `daft list`.
tokens_refresh_repo() {
  tokens_enabled || return 0
  local info root key stamp lock now last debounce
  info=$(repo_info "$1") || { log "tokens: no daft repository at $1"; return 0; }
  root=$(repo_field "$info" .path)
  [ -n "$root" ] || return 0
  key=$(printf '%s' "$root" | tr -c 'A-Za-z0-9' '_')
  mkdir -p "$STATE_DIR/refresh" 2>/dev/null
  stamp=$STATE_DIR/refresh/$key
  lock=$STATE_DIR/refresh/$key.lock
  debounce=$(config_get refresh_debounce_secs 15)
  # Both sides of the comparison are arithmetic: a hand-edited debounce and a
  # stamp truncated by a crash between the open and the write would otherwise
  # be a syntax error inside `$(( ))`.
  case $debounce in '' | *[!0-9]*) debounce=15 ;; esac
  now=$(date +%s)
  last=$(cat "$stamp" 2>/dev/null)
  case $last in '' | *[!0-9]*) last=0 ;; esac
  if [ "${FORCE_REFRESH:-0}" != 1 ] && [ $((now - last)) -lt "$debounce" ]; then
    return 0
  fi
  # Another refresh is running: a forced refresh waits for it (the caller
  # wants fresh tokens now), a debounced one yields to it.
  local waited=0
  until mkdir "$lock" 2>/dev/null; do
    if lock_is_stale "$lock" && rm -rf "$lock" 2>/dev/null && mkdir "$lock" 2>/dev/null; then
      break
    fi
    [ "${FORCE_REFRESH:-0}" = 1 ] || return 0
    [ "$waited" -lt 40 ] || { log "tokens: lock held too long for $root"; return 0; }
    sleep 0.25
    waited=$((waited + 1))
  done
  printf '%s' $$ >"$lock/pid" 2>/dev/null
  printf '%s' "$(date +%s)" >"$stamp"
  tokens_report_repo "$root"
  rm -rf "$lock" 2>/dev/null || true
}

# tokens_report_repo ROOT — compute and push the token for each worktree of
# ROOT that has an open workspace.
tokens_report_repo() {
  local root=$1 cols pr rows wsmap ttl rel summary abs id path
  pr=$(config_get tokens_pr false)
  ttl=$(config_get tokens_ttl_ms 86400000)
  cols=name,annotation,path,base,changes,remote
  [ "$pr" = true ] && cols=$cols,pr
  rows=$(daft_quiet -C "$root" list --format json --columns "$cols" 2>/dev/null) || { log "tokens: daft list failed in $root"; return 0; }
  wsmap=$("$HERDR" workspace list 2>/dev/null \
    | "$JQ" -r '.result.workspaces[]? | select(.worktree != null) | [.workspace_id, .worktree.checkout_path] | @tsv' 2>/dev/null)
  [ -n "$wsmap" ] || return 0
  printf '%s' "$rows" | "$JQ" -r --arg pr "$pr" '
    def n: if . == null then 0 else . end;
    .[] | select(.kind == "worktree")
    | ([
        ([ (if (.ahead | n) > 0 then "↑\(.ahead)" else empty end),
           (if (.behind | n) > 0 then "↓\(.behind)" else empty end) ] | select(length > 0) | join(" ")),
        ([ (if (.staged | n) > 0 then "+\(.staged)" else empty end),
           (if (.unstaged | n) > 0 then "~\(.unstaged)" else empty end),
           (if (.untracked | n) > 0 then "?\(.untracked)" else empty end),
           (if (.conflicted | n) > 0 then "!\(.conflicted)" else empty end) ] | select(length > 0) | join(" ")),
        ([ (if (.remote_ahead | n) > 0 then "⇡\(.remote_ahead)" else empty end),
           (if (.remote_behind | n) > 0 then "⇣\(.remote_behind)" else empty end) ] | select(length > 0) | join(" ")),
        (.operation // empty),
        (if $pr == "true" and .pr_number != null then
           "#\(.pr_number)"
           + (if .pr_state != null then " \(.pr_state)" else "" end)
           + (if .ci_status == "success" then " ✓" elif .ci_status == "failure" then " ✗" elif .ci_status == "pending" then " ◌" else "" end)
         else empty end)
      ] | if length == 0 then "✓" else join(" · ") end) as $summary
    | [.path, $summary] | @tsv' 2>/dev/null \
  | while IFS=$'\t' read -r rel summary; do
      case $rel in
        /*) abs=$(canon "$rel") ;;
        *) abs=$(canon "$root/$rel") ;;
      esac
      id=$(printf '%s\n' "$wsmap" | while IFS=$'\t' read -r id path; do
        if [ "$(canon "$path")" = "$abs" ]; then printf '%s' "$id"; break; fi
      done)
      [ -n "$id" ] || continue
      "$HERDR" workspace report-metadata "$id" --source "$TOKEN_SOURCE" --token "daft=$summary" --ttl-ms "$ttl" </dev/null >/dev/null 2>&1 \
        || log "tokens: report-metadata failed for $id"
    done
}

# tokens_refresh_workspace WORKSPACE_ID — refresh the repo a workspace belongs
# to, if it is a worktree workspace.
tokens_refresh_workspace() {
  local path
  path=$("$HERDR" workspace get "$1" 2>/dev/null \
    | "$JQ" -r '(.result.workspace.worktree.checkout_path // .result.worktree.checkout_path) // empty' 2>/dev/null)
  [ -n "$path" ] || return 0
  tokens_refresh_repo "$path"
}
