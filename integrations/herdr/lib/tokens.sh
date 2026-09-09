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
#
# Beside it, one token per fact, so herdr 0.9's ordered token `rules` have a
# value they can compare (`gt`/`lt` need a bare number; `"↑3 ~1"` matches
# nothing). Same refresh, same `report-metadata` call:
#
#   $daft_ahead      $daft_behind      commits against the base branch
#   $daft_dirty      staged + unstaged + untracked files, as one count
#   $daft_conflicts  conflicted files
#   $daft_unpushed   $daft_unpulled    commits against the remote
#   $daft_op         the paused git operation, by name
#   $daft_ci         the PR's CI state, by name (only with tokens_pr = true)
#
# A fact that is zero or absent is `--clear-token`ed rather than reported as
# `0`: herdr drops a missing token and its separator, so a quiet worktree
# stays quiet, while a token that is merely left out of the call keeps its
# last value until the TTL expires — a row would go on claiming ↑3 after the
# push. Nine keys are well inside herdr's 16-per-request limit.

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

# token_arg NAME VALUE — append one token to TOKEN_ARGS: `--token NAME=VALUE`
# when VALUE is set, `--clear-token NAME` when it is empty. See the clear-on-
# zero rule at the top of this file.
token_arg() {
  if [ -n "$2" ]; then
    TOKEN_ARGS+=(--token "$1=$2")
  else
    TOKEN_ARGS+=(--clear-token "$1")
  fi
}

# tokens_report_repo ROOT — compute and push the tokens for each worktree of
# ROOT that has an open workspace.
tokens_report_repo() {
  local root=$1 cols pr rows wsmap ttl rel summary abs id path
  local ahead behind dirty conflicts unpushed unpulled op ci
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
    def z: if . > 0 then tostring else "" end;
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
    | [ .path,
        $summary,
        ((.ahead | n) | z),
        ((.behind | n) | z),
        (((.staged | n) + (.unstaged | n) + (.untracked | n)) | z),
        ((.conflicted | n) | z),
        ((.remote_ahead | n) | z),
        ((.remote_behind | n) | z),
        (.operation // ""),
        (if $pr == "true" then (.ci_status // "") else "" end)
      ] | join("\u001f")' 2>/dev/null \
  | while IFS=$'\037' read -r rel summary ahead behind dirty conflicts unpushed unpulled op ci; do
      # Unit separator, not a tab: tab is an IFS *whitespace* character, so
      # `read` folds a run of them into one delimiter and every empty field —
      # which is exactly how a fact says "nothing to report" — would shift the
      # columns after it.
      case $rel in
        /*) abs=$(canon "$rel") ;;
        *) abs=$(canon "$root/$rel") ;;
      esac
      id=$(printf '%s\n' "$wsmap" | while IFS=$'\t' read -r id path; do
        if [ "$(canon "$path")" = "$abs" ]; then printf '%s' "$id"; break; fi
      done)
      [ -n "$id" ] || continue
      # One call per workspace, tokens in a fixed order so the log of a run
      # reads the same way every time.
      TOKEN_ARGS=("$id" --source "$TOKEN_SOURCE" --token "daft=$summary")
      token_arg daft_ahead "$ahead"
      token_arg daft_behind "$behind"
      token_arg daft_dirty "$dirty"
      token_arg daft_conflicts "$conflicts"
      token_arg daft_unpushed "$unpushed"
      token_arg daft_unpulled "$unpulled"
      token_arg daft_op "$op"
      # With tokens_pr off the `pr` column is not even requested, so there is
      # no CI state to report or to clear.
      if [ "$pr" = true ]; then
        token_arg daft_ci "$ci"
      fi
      "$HERDR" workspace report-metadata "${TOKEN_ARGS[@]}" --ttl-ms "$ttl" </dev/null >/dev/null 2>&1 \
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
