#!/bin/bash

# Blessed shell tests for the herdr plugin (integrations/herdr, #950).
#
# The subject is a set of bash scripts that sit between two binaries: daft
# (the real release build, through its user-global hooks and its CLI) and
# herdr (a stub on PATH that records every call and answers with canned
# JSON). The YAML runner drives daft alone and cannot stand in a second
# program that daft's hooks call back into, so this lives here, beside the
# other foreign-process contracts.
#
# Every test runs the plugin the way herdr would: HERDR_ENV=1, HERDR_BIN_PATH
# pointing at the stub, HERDR_PLUGIN_* dirs under the test's own work dir, and
# HERDR_PLUGIN_CONTEXT_JSON shaped like herdr's invocation context.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/test_framework.sh"

HERDR_PLUGIN_DIR="$PROJECT_ROOT/integrations/herdr"

# --- Helpers ---

# herdr_stub_install — write the stub herdr into the current test dir and
# export the plugin environment. HERDR_STUB_LOG collects one line per call;
# HERDR_STUB_WORKSPACES, when set, is the JSON `workspace list` answers with;
# HERDR_STUB_GROUP_CLOSE=1 makes `workspace close` answer the way herdr 0.9
# answers for a workspace that still has linked worktree workspaces.
herdr_stub_install() {
    local stub_dir="$PWD/stub-bin"
    mkdir -p "$stub_dir"
    cat > "$stub_dir/herdr" <<'STUB'
#!/bin/bash
printf '%s\n' "$*" >> "${HERDR_STUB_LOG:?}"
[ -n "${HERDR_STUB_FAIL:-}" ] && { echo '{"id":"x","error":{"code":"stub","message":"stub failure"}}' >&2; exit 1; }
case "$1 $2" in
  "workspace list")
    if [ -n "${HERDR_STUB_WORKSPACES:-}" ] && [ -r "$HERDR_STUB_WORKSPACES" ]; then cat "$HERDR_STUB_WORKSPACES"; else echo '{"id":"x","result":{"type":"workspace_list","workspaces":[]}}'; fi ;;
  "workspace get")
    echo '{"id":"x","result":{"type":"workspace_info","workspace":{"workspace_id":"'"$3"'","worktree":{"checkout_path":"'"${HERDR_STUB_GET_PATH:-}"'"}}}}' ;;
  "worktree open")
    echo '{"id":"x","result":{"type":"worktree_open","workspace":{"workspace_id":"w2"},"tab":{"tab_id":"w2:t1"},"root_pane":{"pane_id":"w2:p1"},"already_open":'"${HERDR_STUB_ALREADY_OPEN:-false}"'}}' ;;
  "pane split")
    echo '{"id":"x","result":{"type":"pane_split","pane":{"pane_id":"w2:p2"}}}' ;;
  "workspace close")
    if [ -n "${HERDR_STUB_GROUP_CLOSE:-}" ]; then
      echo '{"id":"x","error":{"code":"workspace_group_close_required","message":"workspace has linked worktree workspaces; use --group (close_group=true in the API) to close the group"}}' >&2
      exit 1
    fi
    echo '{"id":"x","result":{"type":"ok"}}' ;;
  *)
    echo '{"id":"x","result":{"type":"ok"}}' ;;
esac
exit 0
STUB
    chmod +x "$stub_dir/herdr"

    # daft's user-global hooks dir is shared by every test in this suite
    # (test_framework.sh exports one DAFT_CONFIG_DIR). Reset it here so a
    # test that returns early cannot leave a hook behind for the next one.
    rm -f "$DAFT_CONFIG_DIR/hooks/worktree-post-create" "$DAFT_CONFIG_DIR/hooks/worktree-pre-remove"

    export HERDR_STUB_LOG="$PWD/herdr-calls.log"
    : > "$HERDR_STUB_LOG"
    export HERDR_ENV=1
    export HERDR_BIN_PATH="$stub_dir/herdr"
    export HERDR_PLUGIN_ID=daft
    export HERDR_PLUGIN_ROOT="$HERDR_PLUGIN_DIR"
    export HERDR_PLUGIN_CONFIG_DIR="$PWD/plugin-config"
    export HERDR_PLUGIN_STATE_DIR="$PWD/plugin-state"
    mkdir -p "$HERDR_PLUGIN_CONFIG_DIR" "$HERDR_PLUGIN_STATE_DIR"
    unset HERDR_STUB_WORKSPACES HERDR_STUB_FAIL HERDR_PLUGIN_CONTEXT_JSON DAFT_HERDR_PLUGIN_ACTIVE HERDR_STUB_ALREADY_OPEN HERDR_STUB_GROUP_CLOSE
}

# catalog_has PATH — true when daft's catalog holds an entry for PATH. The
# path, not the name: `daft repo add` names an entry after the remote's
# basename and auto-suffixes on a collision, so only the path is stable.
catalog_has() {
    daft repo list --format json 2>/dev/null | jq -e --arg p "$1" 'any(.[]?; .path == $p)' >/dev/null 2>&1
}

# plain_clone NAME — a clone made with plain git, so daft has never operated
# in it and nothing has cataloged it. Prints the physical project root.
plain_clone() {
    local remote_dir
    remote_dir=$(create_test_remote "$1" "main")
    git clone -q "$remote_dir" "$PWD/$1" >/dev/null 2>&1 || return 1
    (cd -P "$PWD/$1" && pwd -P)
}

# wait_for_plugin_log PATTERN — the plugin writes its log after the call it
# describes returns, so a recorded stub call is not proof the line is there.
wait_for_plugin_log() {
    local i=0
    while [ "$i" -lt 40 ]; do
        grep -qF -- "$1" "$HERDR_PLUGIN_STATE_DIR/plugin.log" 2>/dev/null && return 0
        sleep 0.25
        i=$((i + 1))
    done
    return 1
}

# herdr_stub_calls PATTERN — the recorded calls matching PATTERN (fixed string).
herdr_stub_calls() {
    grep -F -- "$1" "$HERDR_STUB_LOG" 2>/dev/null || true
}

# herdr_stub_workspaces WORKSPACE_ID CHECKOUT_PATH — make `workspace list`
# report one grouped worktree workspace.
herdr_stub_workspaces() {
    export HERDR_STUB_WORKSPACES="$PWD/workspaces.json"
    cat > "$HERDR_STUB_WORKSPACES" <<JSON
{"id":"x","result":{"type":"workspace_list","workspaces":[{"workspace_id":"$1","label":"x","worktree":{"repo_key":"k","repo_name":"r","repo_root":"$(dirname "$2")","checkout_path":"$2","is_linked_worktree":true}}]}}
JSON
}

# plugin_context CWD [WORKSPACE_ID] [PANE_ID] — herdr's invocation context.
plugin_context() {
    export HERDR_PLUGIN_CONTEXT_JSON="{\"workspace_id\":\"${2:-w1}\",\"workspace_cwd\":\"$1\",\"focused_pane_id\":\"${3:-w1:p1}\",\"focused_pane_cwd\":\"$1\",\"invocation_source\":\"keybinding\"}"
}

# clone_contained NAME — a contained-layout clone of a fresh remote; prints
# the project root.
clone_contained() {
    local remote_dir
    remote_dir=$(create_test_remote "$1" "main")
    git-worktree-clone --layout contained "$remote_dir" >/dev/null 2>&1 || return 1
    # The clone's own default-branch worktree is mirrored too (a post-create
    # hook fires for it); these tests count what happens after the clone.
    : > "$HERDR_STUB_LOG"
    (cd -P "$PWD/$1" && pwd -P)
}

# clone_sibling NAME FILE SOURCE — a sibling-layout clone of a fresh remote
# whose default-branch checkout carries FILE (a copy of SOURCE); prints the
# project root, which in this layout *is* that checkout.
clone_sibling() {
    local remote_dir
    remote_dir=$(create_test_remote "$1" "main")
    # Commit the payload on the remote so it arrives with the clone.
    local staging="$PWD/staging-$1"
    git clone -q "$remote_dir" "$staging" >/dev/null 2>&1 || return 1
    cp "$3" "$staging/$2"
    (cd "$staging" && git add "$2" && git -c user.name=T -c user.email=t@t commit -qm "add $2" && git push -q origin main) >/dev/null 2>&1 || return 1
    rm -rf "$staging"
    git-worktree-clone --layout sibling "$remote_dir" >/dev/null 2>&1 || return 1
    : > "$HERDR_STUB_LOG"
    (cd -P "$PWD/$1" && pwd -P)
}

# plugin_layout REPO_NAME — write a layout file for REPO_NAME (body on stdin)
# where the plugin actually reads layouts from: a directory the user owns.
plugin_layout() {
    mkdir -p "$HERDR_PLUGIN_CONFIG_DIR/layouts"
    cat > "$HERDR_PLUGIN_CONFIG_DIR/layouts/$1.sh"
}

# refresh_idle — wait for any background token refresh (spawned by daft's
# post-create hook) to finish, then forget its debounce stamp so the next
# refresh is the one under test.
refresh_idle() {
    local i=0
    # Wait for the stamp, not the directory: the refresh creates the directory
    # before it takes the lock, so a directory-shaped wait can win the race and
    # delete the state of a refresh that is still running.
    while [ -z "$(find "$HERDR_PLUGIN_STATE_DIR/refresh" -type f ! -name pid 2>/dev/null)" ] && [ "$i" -lt 40 ]; do
        sleep 0.25
        i=$((i + 1))
    done
    # ... and for the lock it took to be released, then forget the stamp.
    i=0
    while [ -n "$(find "$HERDR_PLUGIN_STATE_DIR/refresh" -type d -name '*.lock' 2>/dev/null)" ] && [ "$i" -lt 40 ]; do
        sleep 0.25
        i=$((i + 1))
    done
    rm -rf "$HERDR_PLUGIN_STATE_DIR/refresh"
}

wait_for_stub_call() {
    local pattern=$1 i=0
    while [ "$i" -lt 40 ]; do
        grep -qF -- "$pattern" "$HERDR_STUB_LOG" 2>/dev/null && return 0
        sleep 0.25
        i=$((i + 1))
    done
    return 1
}

# --- Test Functions ---

test_herdr_startup_installs_daft_hooks() {
    log "Testing: startup installs daft's user-global hooks and records the daft path"
    herdr_stub_install

    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || { log_error "startup.sh failed"; return 1; }

    local hooks_dir="$DAFT_CONFIG_DIR/hooks"
    assert_file_exists "$hooks_dir/worktree-post-create" || return 1
    assert_file_exists "$hooks_dir/worktree-pre-remove" || return 1
    [ -x "$hooks_dir/worktree-post-create" ] || { log_error "post-create hook is not executable"; return 1; }
    assert_file_contains "$hooks_dir/worktree-post-create" "$HERDR_PLUGIN_DIR/bin/daft-hook.sh" || return 1
    assert_file_contains "$hooks_dir/worktree-post-create" "managed by the daft herdr plugin" || return 1
    assert_file_exists "$HERDR_PLUGIN_STATE_DIR/daft-path" || return 1
    assert_file_contains "$HERDR_PLUGIN_STATE_DIR/daft-path" "$RUST_BINARY_DIR/daft" || return 1
    assert_file_exists "$HERDR_PLUGIN_CONFIG_DIR/config.toml" || return 1

    log_success "startup installed both hooks and recorded $RUST_BINARY_DIR/daft"
    return 0
}

test_herdr_startup_leaves_foreign_hooks_alone() {
    log "Testing: a user's own hook file is never overwritten"
    herdr_stub_install
    mkdir -p "$DAFT_CONFIG_DIR/hooks"
    printf '#!/bin/sh\necho mine\n' > "$DAFT_CONFIG_DIR/hooks/worktree-post-create"

    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1

    assert_file_contains "$DAFT_CONFIG_DIR/hooks/worktree-post-create" "echo mine" || return 1
    if grep -q "managed by the daft herdr plugin" "$DAFT_CONFIG_DIR/hooks/worktree-post-create"; then
        log_error "the foreign post-create hook was overwritten"
        return 1
    fi
    # herdr_stub_install cleared both hooks, so this proves the run under test
    # installed pre-remove rather than inheriting it from an earlier one.
    assert_file_contains "$DAFT_CONFIG_DIR/hooks/worktree-pre-remove" "managed by the daft herdr plugin" || return 1

    log_success "foreign hook kept, the plugin's other hook still installed"
    return 0
}

test_herdr_post_create_hook_mirrors_daft_start() {
    log "Testing: a plain daft start in a herdr pane registers the worktree with herdr"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-mirror") || { log_error "clone failed"; return 1; }

    (cd "$root/main" && daft start feat/mirror --no-cd >/dev/null 2>&1) || { log_error "daft start failed"; return 1; }
    assert_directory_exists "$root/feat/mirror" || return 1

    local calls
    calls=$(herdr_stub_calls "worktree open")
    if ! printf '%s\n' "$calls" | grep -qF -- "worktree open --cwd $root --path $root/feat/mirror --label feat/mirror --no-focus"; then
        log_error "expected a worktree open call for the new worktree, got:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi
    if [ "$(printf '%s\n' "$calls" | grep -c .)" -ne 1 ]; then
        log_error "expected exactly one worktree open call, got: $calls"
        return 1
    fi

    log_success "post-create hook called: herdr worktree open --cwd $root --path .../feat/mirror --no-focus"
    return 0
}

test_herdr_post_create_hook_never_breaks_daft() {
    log "Testing: a failing herdr never fails daft's worktree creation"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-robust") || return 1

    if ! (cd "$root/main" && HERDR_STUB_FAIL=1 daft start feat/robust --no-cd >/dev/null 2>&1); then
        log_error "daft start failed because the herdr stub failed"
        return 1
    fi
    assert_directory_exists "$root/feat/robust" || return 1

    log_success "daft start succeeded with herdr refusing every call"
    return 0
}

test_herdr_post_remove_hook_closes_workspace() {
    log "Testing: daft remove closes the worktree's herdr workspace"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-close") || return 1
    (cd "$root/main" && daft start feat/gone --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w7" "$root/feat/gone"

    (cd "$root/main" && daft remove feat/gone >/dev/null 2>&1) || { log_error "daft remove failed"; return 1; }
    [ ! -d "$root/feat/gone" ] || { log_error "worktree still on disk"; return 1; }

    # The close lands once daft has exited and the directory is gone.
    if ! wait_for_stub_call "workspace close w7"; then
        log_error "expected a workspace close w7 call, got:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi

    log_success "pre-remove hook closed the workspace: herdr workspace close w7"
    return 0
}

test_herdr_group_close_refusal_stands_down() {
    log "Testing: herdr 0.9's workspace_group_close_required is a stand-down, not a retry"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-group") || return 1
    (cd "$root/main" && daft start feat/grouped --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w8" "$root/feat/grouped"
    : > "$HERDR_STUB_LOG"

    # HERDR_STUB_GROUP_CLOSE is exported into daft, so the hook and the
    # watcher it spawns both inherit it.
    (cd "$root/main" && HERDR_STUB_GROUP_CLOSE=1 daft remove feat/grouped >/dev/null 2>&1) \
        || { log_error "daft remove failed"; return 1; }
    [ ! -d "$root/feat/grouped" ] || { log_error "worktree still on disk"; return 1; }

    if ! wait_for_stub_call "workspace close w8"; then
        log_error "the watcher never called workspace close:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi
    if ! wait_for_plugin_log "still has open worktree workspaces"; then
        log_error "the refusal was not logged as a stand-down:"
        cat "$HERDR_PLUGIN_STATE_DIR/plugin.log"
        return 1
    fi

    # A retry would land shortly after the first call; give it the chance.
    sleep 1
    local closes
    closes=$(herdr_stub_calls "workspace close" | grep -c . || true)
    if [ "$closes" -ne 1 ]; then
        log_error "expected exactly one workspace close call, got $closes:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi
    if herdr_stub_calls "--group" | grep -q .; then
        log_error "the watcher retried with --group, which would close live children:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi

    log_success "one close, stand-down logged, no --group retry"
    return 0
}

test_herdr_pre_remove_keeps_a_refused_removal_open() {
    log "Testing: a removal that never happens leaves the workspace open"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-refused") || return 1
    (cd "$root/main" && daft start feat/stays --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w9" "$root/feat/stays"
    : > "$HERDR_STUB_LOG"

    # The hook as daft runs it, for a removal that then does not happen
    # (a dirty worktree, a refused prompt, a failed git call). The watcher
    # must see the directory still on disk and leave the workspace alone.
    DAFT_WORKTREE_PATH="$root/feat/stays" DAFT_PROJECT_ROOT="$root" \
        bash "$DAFT_CONFIG_DIR/hooks/worktree-pre-remove" || { log_error "pre-remove hook failed"; return 1; }

    sleep 2
    if herdr_stub_calls "workspace close" | grep -q .; then
        log_error "the workspace was closed although $root/feat/stays still exists:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi
    assert_directory_exists "$root/feat/stays" || return 1

    log_success "no workspace close while the worktree is still on disk"
    return 0
}

test_herdr_move_leaves_the_workspace_alone() {
    log "Testing: daft rename does not close or duplicate the worktree's workspace"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-move") || return 1
    (cd "$root/main" && daft start feat/before --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w6" "$root/feat/before"
    : > "$HERDR_STUB_LOG"

    (cd "$root/main" && daft rename feat/before feat/after --no-remote >/dev/null 2>&1) \
        || { log_error "daft rename failed"; return 1; }
    assert_directory_exists "$root/feat/after" || return 1

    # daft replays the whole remove-then-create hook sequence for a move. Both
    # arms must stand down: closing w6 would kill the panes the user is in,
    # and opening the new path would leave a second row beside the stale one.
    sleep 2
    if herdr_stub_calls "workspace close" | grep -q .; then
        log_error "the move closed the workspace:"; cat "$HERDR_STUB_LOG"; return 1
    fi
    if herdr_stub_calls "worktree open" | grep -q .; then
        log_error "the move opened a second workspace:"; cat "$HERDR_STUB_LOG"; return 1
    fi

    log_success "rename left w6 alone: no close, no second open"
    return 0
}

test_herdr_layout_is_never_read_from_a_checkout() {
    log "Testing: a herdr-layout.sh committed to a repository is never sourced"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    printf 'layout_on_hook = true\n' >> "$HERDR_PLUGIN_CONFIG_DIR/config.toml"

    # In the sibling layout — daft's default — the project root IS the default
    # branch's checkout, so this file arrives with the clone.
    cat > "$PWD/payload-layout.sh" <<'LAYOUT'
touch "$HOME/PWNED-BY-A-CLONED-REPO"
after_open() { touch "$HOME/PWNED-BY-A-CLONED-REPO"; }
LAYOUT
    local root
    root=$(clone_sibling "herdr-untrusted" "herdr-layout.sh" "$PWD/payload-layout.sh") \
        || { log_error "clone failed"; return 1; }

    (cd "$root" && daft start feat/untrusted --no-cd >/dev/null 2>&1) || { log_error "daft start failed"; return 1; }
    assert_file_exists "$root/herdr-layout.sh" || return 1
    if [ -e "$HOME/PWNED-BY-A-CLONED-REPO" ]; then
        rm -f "$HOME/PWNED-BY-A-CLONED-REPO"
        log_error "the cloned repository's herdr-layout.sh was executed"
        return 1
    fi

    log_success "the repository's own herdr-layout.sh was ignored"
    return 0
}

test_herdr_layout_failure_never_breaks_daft() {
    log "Testing: a layout file that exits non-zero does not fail daft start"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    printf 'layout_on_hook = true\n' >> "$HERDR_PLUGIN_CONFIG_DIR/config.toml"
    local root name
    root=$(clone_contained "herdr-badlayout") || return 1
    name=$(daft repo info "$root" --format json | jq -r .name)
    # Sourced user code: a bare exit, and a reference that trips `set -u`.
    plugin_layout "$name" <<'LAYOUT'
after_open() { :; }
echo "$THIS_VARIABLE_IS_NOT_SET" >/dev/null
exit 1
LAYOUT

    if ! (cd "$root/main" && daft start feat/survives --no-cd >/dev/null 2>&1); then
        log_error "daft start failed because the layout file did"
        return 1
    fi
    assert_directory_exists "$root/feat/survives" || return 1

    log_success "daft start succeeded with a layout file that exits 1"
    return 0
}

test_herdr_already_open_workspace_is_not_relaid_out() {
    log "Testing: registering a checkout herdr already has open reports no pane to lay out"
    herdr_stub_install
    local out
    out=$(HERDR_STUB_ALREADY_OPEN=true bash -c \
        '. "$1/lib/common.sh"; resolve_jq || exit 1; register_worktree /tmp/root /tmp/root/wt label --focus' \
        _ "$HERDR_PLUGIN_DIR") || { log_error "register_worktree failed"; return 1; }
    if [ -n "$out" ]; then
        log_error "expected no pane id for an already-open workspace, got: $out"
        return 1
    fi
    out=$(bash -c '. "$1/lib/common.sh"; resolve_jq || exit 1; register_worktree /tmp/root /tmp/root/wt label --focus' \
        _ "$HERDR_PLUGIN_DIR") || return 1
    if [ "$out" != "w2:p1" ]; then
        log_error "expected the root pane id for a newly opened workspace, got: $out"
        return 1
    fi

    log_success "already_open reports no pane; a fresh open reports w2:p1"
    return 0
}

test_herdr_popup_start_creates_registers_and_lays_out() {
    log "Testing: the start popup runs daft start, registers once with focus, and applies the repo layout"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-popup") || return 1
    plugin_layout "$(daft repo info "$root" --format json | jq -r .name)" <<'LAYOUT'
after_open() {
  local pane=$1 checkout=$2 branch=$3 slug=$4
  split_right "$pane" >/dev/null
  run "$pane" "echo layout-for-$slug"
}
LAYOUT
    plugin_context "$root/main"

    printf 'feat/popup\n\n' | bash "$HERDR_PLUGIN_DIR/bin/pane.sh" start >popup.out 2>&1 || {
        log_error "pane.sh start failed:"; cat popup.out; return 1; }
    assert_directory_exists "$root/feat/popup" || return 1

    local opens
    opens=$(herdr_stub_calls "worktree open")
    if [ "$(printf '%s\n' "$opens" | grep -c .)" -ne 1 ]; then
        log_error "expected exactly one worktree open (the popup's, not the hook's), got: $opens"
        return 1
    fi
    printf '%s\n' "$opens" | grep -qF -- "--path $root/feat/popup --label feat/popup --focus" || { log_error "unexpected open call: $opens"; return 1; }
    herdr_stub_calls "pane split --pane w2:p1 --direction right --cwd $root/feat/popup --no-focus" | grep -q . || { log_error "layout did not split the root pane"; cat "$HERDR_STUB_LOG"; return 1; }
    herdr_stub_calls "pane run w2:p1 echo layout-for-feat-popup" | grep -q . || { log_error "layout did not run its command"; cat "$HERDR_STUB_LOG"; return 1; }

    log_success "popup: daft start, one focused registration, layout applied to w2:p1"
    return 0
}

test_herdr_adopt_action_groups_a_checkout() {
    log "Testing: the adopt action registers the checkout the user is standing in"
    herdr_stub_install
    local root
    root=$(clone_contained "herdr-adopt") || return 1
    (cd "$root/main" && daft start feat/adopt --no-cd >/dev/null 2>&1) || return 1
    : > "$HERDR_STUB_LOG"
    # A herdr pane can report its cwd through a symlink (on macOS every /tmp
    # path is one); the plugin must still resolve it to the physical checkout.
    ln -s "$root" "$PWD/link-to-adopt"
    plugin_context "$PWD/link-to-adopt/feat/adopt"

    bash "$HERDR_PLUGIN_DIR/bin/action.sh" adopt || { log_error "action.sh adopt failed"; return 1; }
    herdr_stub_calls "worktree open --cwd $root --path $root/feat/adopt --label feat/adopt --focus" | grep -q . || {
        log_error "expected a focused worktree open, got:"; cat "$HERDR_STUB_LOG"; return 1; }

    log_success "adopt registered $root/feat/adopt with focus"
    return 0
}

test_herdr_tokens_report_worktree_status() {
    log "Testing: the token refresh reports daft's status for each open worktree workspace"
    herdr_stub_install
    local root
    root=$(clone_contained "herdr-tokens") || return 1
    (cd "$root/main" && daft start feat/tokens --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w4" "$root/feat/tokens"
    refresh_idle
    : > "$HERDR_STUB_LOG"

    # A clean, in-sync worktree reports the check mark, and every per-fact
    # token is cleared rather than reported as 0. daft_ci is absent entirely:
    # tokens_pr is off, so the pr column was never asked for.
    bash -c '. "$1/lib/common.sh"; . "$1/lib/tokens.sh"; resolve_jq; resolve_daft; FORCE_REFRESH=1 tokens_refresh_repo "$2"' _ "$HERDR_PLUGIN_DIR" "$root" \
        || { log_error "token refresh failed"; return 1; }
    herdr_stub_calls "workspace report-metadata w4 --source plugin:daft --token daft=✓ --clear-token daft_ahead --clear-token daft_behind --clear-token daft_dirty --clear-token daft_conflicts --clear-token daft_unpushed --clear-token daft_unpulled --clear-token daft_op --ttl-ms 86400000" | grep -q . || {
        log_error "expected a clean token with every fact cleared, got:"; cat "$HERDR_STUB_LOG"; return 1; }
    if herdr_stub_calls "daft_ci" | grep -q .; then
        log_error "daft_ci was reported with tokens_pr off:"; cat "$HERDR_STUB_LOG"; return 1
    fi

    # Two untracked files and one modified file change the token: the composite
    # keeps its text, and daft_dirty carries the number the `gt` rules compare.
    touch "$root/feat/tokens/a.txt" "$root/feat/tokens/b.txt"
    echo "change" >> "$root/feat/tokens/README.md"
    : > "$HERDR_STUB_LOG"
    bash -c '. "$1/lib/common.sh"; . "$1/lib/tokens.sh"; resolve_jq; resolve_daft; FORCE_REFRESH=1 tokens_refresh_repo "$2"' _ "$HERDR_PLUGIN_DIR" "$root" || return 1
    herdr_stub_calls "report-metadata w4 --source plugin:daft --token daft=~1 ?2 --clear-token daft_ahead --clear-token daft_behind --token daft_dirty=3 --clear-token daft_conflicts --clear-token daft_unpushed --clear-token daft_unpulled --clear-token daft_op --ttl-ms" | grep -q . || {
        log_error "expected a dirty token (~1 ?2) with daft_dirty=3, got:"; cat "$HERDR_STUB_LOG"; return 1; }

    log_success "tokens: ✓ with every fact cleared when clean, ~1 ?2 + daft_dirty=3 with changes"
    return 0
}

test_herdr_explicit_action_adopts_an_uncataloged_repo() {
    log "Testing: an explicit action catalogs a repository daft has never operated in"
    herdr_stub_install
    local clone wt
    clone=$(plain_clone "herdr-uncataloged") || { log_error "git clone failed"; return 1; }
    # A linked worktree, also by hand: `adopt` refuses a repository root, and
    # nothing here may go through daft or the repo would be cataloged already.
    git -C "$clone" worktree add -q -b feat/adopted "$PWD/uncataloged-wt" >/dev/null 2>&1 \
        || { log_error "git worktree add failed"; return 1; }
    wt=$(cd -P "$PWD/uncataloged-wt" && pwd -P)
    if catalog_has "$clone"; then
        log_error "the plain clone was in the catalog before the action ran"
        return 1
    fi

    plugin_context "$wt"
    bash "$HERDR_PLUGIN_DIR/bin/action.sh" adopt || {
        log_error "action.sh adopt failed:"; cat "$HERDR_PLUGIN_STATE_DIR/plugin.log"; return 1; }

    herdr_stub_calls "worktree open --cwd $clone --path $wt --label feat/adopted --focus" | grep -q . || {
        log_error "expected a focused worktree open for the adopted checkout, got:"; cat "$HERDR_STUB_LOG"; return 1; }
    if ! catalog_has "$clone"; then
        log_error "adopt did not catalog $clone:"
        daft repo list --format json || true
        return 1
    fi

    log_success "adopt cataloged the plain clone and grouped its worktree"
    return 0
}

test_herdr_focus_event_leaves_the_catalog_alone() {
    log "Testing: a focus event never catalogs the repository it only looked at"
    herdr_stub_install
    local clone
    clone=$(plain_clone "herdr-glanced") || { log_error "git clone failed"; return 1; }
    herdr_stub_workspaces "w10" "$clone"
    export HERDR_STUB_GET_PATH="$clone"
    export HERDR_PLUGIN_EVENT=workspace.focused
    export HERDR_PLUGIN_EVENT_JSON='{"event":"workspace.focused","data":{"workspace_id":"w10"}}'

    bash "$HERDR_PLUGIN_DIR/bin/event.sh" || { log_error "event.sh failed"; return 1; }
    unset HERDR_PLUGIN_EVENT HERDR_PLUGIN_EVENT_JSON HERDR_STUB_GET_PATH

    if catalog_has "$clone"; then
        log_error "a focus event put $clone into the catalog; every repo herdr touches would land in daft update --all-repos"
        daft repo list --format json || true
        return 1
    fi
    if herdr_stub_calls "report-metadata" | grep -q .; then
        log_error "tokens were reported for a repository daft does not know:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi

    log_success "focus on an uncataloged clone: no catalog entry, no token report"
    return 0
}

test_herdr_event_refresh_is_debounced() {
    log "Testing: workspace.focused events refresh tokens at most once per debounce window"
    herdr_stub_install
    local root
    # Tokens off while the worktree is created, so the post-create hook's own
    # background refresh cannot stamp the repo under this test's feet.
    printf 'tokens = false\n' > "$HERDR_PLUGIN_CONFIG_DIR/config.toml"
    root=$(clone_contained "herdr-debounce") || return 1
    (cd "$root/main" && daft start feat/debounce --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w5" "$root/feat/debounce"
    printf 'tokens = true\n' > "$HERDR_PLUGIN_CONFIG_DIR/config.toml"
    : > "$HERDR_STUB_LOG"
    export HERDR_STUB_GET_PATH="$root/feat/debounce"
    export HERDR_PLUGIN_EVENT=workspace.focused
    export HERDR_PLUGIN_EVENT_JSON='{"event":"workspace.focused","data":{"workspace_id":"w5"}}'

    bash "$HERDR_PLUGIN_DIR/bin/event.sh" || return 1
    bash "$HERDR_PLUGIN_DIR/bin/event.sh" || return 1
    local reports
    reports=$(herdr_stub_calls "report-metadata w5" | grep -c .)
    if [ "$reports" -ne 1 ]; then
        log_error "expected one token report across two focus events, got $reports:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi
    unset HERDR_PLUGIN_EVENT HERDR_PLUGIN_EVENT_JSON HERDR_STUB_GET_PATH

    log_success "two focus events, one report"
    return 0
}

# Run all herdr plugin tests
run_herdr_plugin_tests() {
    log "Running herdr plugin integration tests..."

    if ! command -v jq >/dev/null 2>&1; then
        log_error "jq is required by the herdr plugin suite and is not on PATH"
        TESTS_RUN=$((TESTS_RUN + 1)); TESTS_FAILED=$((TESTS_FAILED + 1)); FAILED_TESTS+=("herdr_plugin_requires_jq")
        return 1
    fi

    run_test "herdr_startup_installs_daft_hooks" test_herdr_startup_installs_daft_hooks
    run_test "herdr_startup_leaves_foreign_hooks_alone" test_herdr_startup_leaves_foreign_hooks_alone
    run_test "herdr_post_create_hook_mirrors_daft_start" test_herdr_post_create_hook_mirrors_daft_start
    run_test "herdr_post_create_hook_never_breaks_daft" test_herdr_post_create_hook_never_breaks_daft
    run_test "herdr_post_remove_hook_closes_workspace" test_herdr_post_remove_hook_closes_workspace
    run_test "herdr_group_close_refusal_stands_down" test_herdr_group_close_refusal_stands_down
    run_test "herdr_pre_remove_keeps_a_refused_removal_open" test_herdr_pre_remove_keeps_a_refused_removal_open
    run_test "herdr_move_leaves_the_workspace_alone" test_herdr_move_leaves_the_workspace_alone
    run_test "herdr_layout_is_never_read_from_a_checkout" test_herdr_layout_is_never_read_from_a_checkout
    run_test "herdr_layout_failure_never_breaks_daft" test_herdr_layout_failure_never_breaks_daft
    run_test "herdr_already_open_workspace_is_not_relaid_out" test_herdr_already_open_workspace_is_not_relaid_out
    run_test "herdr_popup_start_creates_registers_and_lays_out" test_herdr_popup_start_creates_registers_and_lays_out
    run_test "herdr_adopt_action_groups_a_checkout" test_herdr_adopt_action_groups_a_checkout
    run_test "herdr_tokens_report_worktree_status" test_herdr_tokens_report_worktree_status
    run_test "herdr_explicit_action_adopts_an_uncataloged_repo" test_herdr_explicit_action_adopts_an_uncataloged_repo
    run_test "herdr_focus_event_leaves_the_catalog_alone" test_herdr_focus_event_leaves_the_catalog_alone
    run_test "herdr_event_refresh_is_debounced" test_herdr_event_refresh_is_debounced
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_herdr_plugin_tests
    print_summary
    exit $?
fi
