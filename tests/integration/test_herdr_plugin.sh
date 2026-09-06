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
# HERDR_STUB_WORKSPACES, when set, is the JSON `workspace list` answers with.
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
    echo '{"id":"x","result":{"type":"worktree_open","workspace":{"workspace_id":"w2"},"tab":{"tab_id":"w2:t1"},"root_pane":{"pane_id":"w2:p1"},"already_open":false}}' ;;
  "pane split")
    echo '{"id":"x","result":{"type":"pane_split","pane":{"pane_id":"w2:p2"}}}' ;;
  *)
    echo '{"id":"x","result":{"type":"ok"}}' ;;
esac
exit 0
STUB
    chmod +x "$stub_dir/herdr"

    export HERDR_STUB_LOG="$PWD/herdr-calls.log"
    : > "$HERDR_STUB_LOG"
    export HERDR_ENV=1
    export HERDR_BIN_PATH="$stub_dir/herdr"
    export HERDR_PLUGIN_ID=daft
    export HERDR_PLUGIN_ROOT="$HERDR_PLUGIN_DIR"
    export HERDR_PLUGIN_CONFIG_DIR="$PWD/plugin-config"
    export HERDR_PLUGIN_STATE_DIR="$PWD/plugin-state"
    mkdir -p "$HERDR_PLUGIN_CONFIG_DIR" "$HERDR_PLUGIN_STATE_DIR"
    unset HERDR_STUB_WORKSPACES HERDR_STUB_FAIL HERDR_WORKSPACE_ID HERDR_PLUGIN_CONTEXT_JSON DAFT_HERDR_PLUGIN_ACTIVE
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

# refresh_idle — wait for any background token refresh (spawned by daft's
# post-create hook) to finish, then forget its debounce stamp so the next
# refresh is the one under test.
refresh_idle() {
    local i=0
    # Let the detached refresh start (it stamps the repo first) ...
    while [ ! -d "$HERDR_PLUGIN_STATE_DIR/refresh" ] && [ "$i" -lt 40 ]; do
        sleep 0.25
        i=$((i + 1))
    done
    # ... and finish, then forget it.
    i=0
    while [ -n "$(find "$HERDR_PLUGIN_STATE_DIR/refresh" -name '*.lock' 2>/dev/null)" ] && [ "$i" -lt 40 ]; do
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
    assert_file_contains "$DAFT_CONFIG_DIR/hooks/worktree-pre-remove" "managed by the daft herdr plugin" || return 1
    rm -f "$DAFT_CONFIG_DIR/hooks/worktree-post-create"

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

test_herdr_post_remove_defers_close_of_own_workspace() {
    log "Testing: removing the worktree you are standing in closes its workspace only after daft exits"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-self") || return 1
    (cd "$root/main" && daft start feat/self --no-cd >/dev/null 2>&1) || return 1
    herdr_stub_workspaces "w9" "$root/feat/self"

    # HERDR_WORKSPACE_ID marks the invoking pane as living in workspace w9.
    (cd "$root/main" && HERDR_WORKSPACE_ID=w9 daft remove feat/self >/dev/null 2>&1) || { log_error "daft remove failed"; return 1; }
    # The close waits for the daft ancestor to exit; daft has exited now.
    if ! wait_for_stub_call "workspace close w9"; then
        log_error "deferred workspace close w9 never arrived:"
        cat "$HERDR_STUB_LOG"
        return 1
    fi

    log_success "deferred close arrived after daft exited"
    return 0
}

test_herdr_popup_start_creates_registers_and_lays_out() {
    log "Testing: the start popup runs daft start, registers once with focus, and applies the repo layout"
    herdr_stub_install
    bash "$HERDR_PLUGIN_DIR/bin/startup.sh" || return 1
    local root
    root=$(clone_contained "herdr-popup") || return 1
    cat > "$root/herdr-layout.sh" <<'LAYOUT'
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
    # A herdr pane reports its cwd through the symlinked /tmp spelling on macOS;
    # the plugin must still resolve it to the physical checkout.
    plugin_context "${root/#\/private\/tmp\//\/tmp\/}/feat/adopt"

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

    # A clean, in-sync worktree reports the check mark.
    bash -c '. "$1/lib/common.sh"; . "$1/lib/tokens.sh"; resolve_jq; resolve_daft; FORCE_REFRESH=1 tokens_refresh_repo "$2"' _ "$HERDR_PLUGIN_DIR" "$root" \
        || { log_error "token refresh failed"; return 1; }
    herdr_stub_calls "workspace report-metadata w4 --source plugin:daft --token daft=✓ --ttl-ms 86400000" | grep -q . || {
        log_error "expected a clean token, got:"; cat "$HERDR_STUB_LOG"; return 1; }

    # Two untracked files and one modified file change the token.
    touch "$root/feat/tokens/a.txt" "$root/feat/tokens/b.txt"
    echo "change" >> "$root/feat/tokens/README.md"
    : > "$HERDR_STUB_LOG"
    bash -c '. "$1/lib/common.sh"; . "$1/lib/tokens.sh"; resolve_jq; resolve_daft; FORCE_REFRESH=1 tokens_refresh_repo "$2"' _ "$HERDR_PLUGIN_DIR" "$root" || return 1
    herdr_stub_calls "report-metadata w4 --source plugin:daft --token daft=~1 ?2 --ttl-ms" | grep -q . || {
        log_error "expected a dirty token (~1 ?2), got:"; cat "$HERDR_STUB_LOG"; return 1; }

    log_success "tokens: ✓ when clean, ~1 ?2 with changes"
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
    run_test "herdr_post_remove_defers_close_of_own_workspace" test_herdr_post_remove_defers_close_of_own_workspace
    run_test "herdr_popup_start_creates_registers_and_lays_out" test_herdr_popup_start_creates_registers_and_lays_out
    run_test "herdr_adopt_action_groups_a_checkout" test_herdr_adopt_action_groups_a_checkout
    run_test "herdr_tokens_report_worktree_status" test_herdr_tokens_report_worktree_status
    run_test "herdr_event_refresh_is_debounced" test_herdr_event_refresh_is_debounced
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_herdr_plugin_tests
    print_summary
    exit $?
fi
