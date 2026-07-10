#!/bin/bash

# Integration tests for `daft worktree-exec` / `daft exec`.
#
# Kept distinct from test_exec.sh, which covers the legacy `-x` option
# on clone/init/checkout. This file targets the dedicated
# `git-worktree-exec` command that fans out user commands across one or
# more worktrees.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Single-target pass-through runs the command with cwd inside the
# selected worktree.
test_exec_single_target_pwd_in_worktree_cwd() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-single" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-single/main" || return 1

    local out
    out=$(git-worktree-exec main -- pwd) || {
        log_error "exec single-target pwd failed"
        return 1
    }

    # Substring match — macOS may resolve /tmp to /private/tmp, so we
    # only check that the path includes the expected worktree tail.
    if [[ "$out" != *"exec-single/main"* ]]; then
        log_error "single-target pwd wrong: $out"
        return 1
    fi

    log_success "single-target command ran with cwd in worktree"
    return 0
}

# --all fans the command out to every worktree.
test_exec_all_runs_everywhere() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-all" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-all/main" || return 1
    git-worktree-checkout -b feat-a || return 1
    cd "../main" || return 1

    git-worktree-exec --all -- sh -c 'echo hi > marker' || return 1

    # Resolve the repo root (parent of main/) for asserting marker files.
    local repo_root
    repo_root="$(cd .. && pwd)"

    assert_file_exists "$repo_root/main/marker" "main marker present" || return 1
    assert_file_exists "$repo_root/feat-a/marker" "feat-a marker present" || return 1

    return 0
}

# --sequential stops as soon as a worktree fails: later worktrees in
# lexicographic order must not run.
test_exec_sequential_stops_on_failure() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-seq" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-seq/main" || return 1
    git-worktree-checkout -b feat-a || return 1
    git-worktree-checkout -b feat-b || return 1
    cd "../main" || return 1

    local repo_root
    repo_root="$(cd .. && pwd)"

    # Resolved order is lexicographic on branch name: feat-a, feat-b, main.
    # feat-a fails; feat-b and main must both be skipped.
    set +e
    git-worktree-exec --all --sequential -x \
        'case "$DAFT_BRANCH_NAME" in feat-a) exit 1;; *) echo did > marker;; esac' \
        >/dev/null 2>&1
    local exit_code=$?
    set -e

    if [[ $exit_code -eq 0 ]]; then
        log_error "expected non-zero exit with --sequential when feat-a fails"
        return 1
    fi

    if [[ -f "$repo_root/feat-b/marker" ]]; then
        log_error "feat-b marker should not exist after --sequential stop"
        return 1
    fi
    if [[ -f "$repo_root/main/marker" ]]; then
        log_error "main marker should not exist after --sequential stop"
        return 1
    fi

    log_success "--sequential stopped on first failure"
    return 0
}

# --keep-going runs every worktree even when one fails, and still
# returns a non-zero aggregate exit code.
test_exec_keep_going_runs_all_despite_failure() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-keep" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-keep/main" || return 1
    git-worktree-checkout -b feat-a || return 1
    git-worktree-checkout -b feat-b || return 1
    cd "../main" || return 1

    local repo_root
    repo_root="$(cd .. && pwd)"

    set +e
    git-worktree-exec --all --keep-going -x \
        'case "$DAFT_BRANCH_NAME" in feat-a) exit 1;; *) echo did > marker;; esac' \
        >/dev/null 2>&1
    local exit_code=$?
    set -e

    if [[ $exit_code -eq 0 ]]; then
        log_error "expected non-zero exit with --keep-going when feat-a fails"
        return 1
    fi

    # feat-b and main both ran even though feat-a failed.
    assert_file_exists "$repo_root/feat-b/marker" "feat-b ran despite feat-a failure" || return 1
    assert_file_exists "$repo_root/main/marker" "main ran despite feat-a failure" || return 1

    log_success "--keep-going continued through failure"
    return 0
}

# Unmatched positional (no such branch / glob match) must fail with a
# non-zero exit code.
test_exec_unmatched_positional_errors() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-err" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-err/main" || return 1

    if git-worktree-exec zzzzz -- echo >/dev/null 2>&1; then
        log_error "expected error for unmatched positional 'zzzzz'"
        return 1
    fi

    log_success "unmatched positional errored as expected"
    return 0
}

# Multi-command pipeline finalization rows include the inline command name
# and distinguish success/failure/skipped states.
test_exec_multi_command_shows_inline_command_names() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-multi" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-multi/main" || return 1
    # Create a second worktree so we trigger the multi-target path
    # (single-target invocations use pass-through mode with no compact rows).
    git-worktree-checkout -b feat-a || return 1
    cd "../main" || return 1

    # Capture stderr (where the compact rows are printed). The harness
    # exports DAFT_TESTING=1 to silence the hook renderer for YAML
    # scenarios; unset it here since this test asserts on the renderer
    # output itself.
    local err
    err=$(env -u DAFT_TESTING git-worktree-exec --all -x 'true' -x 'echo second' 2>&1 >/dev/null)

    # Each finalization row names its command inline (not `[N/M]`).
    if [[ "$err" != *"❯ true"* ]]; then
        log_error "missing inline command name '❯ true' in output: $err"
        return 1
    fi
    if [[ "$err" != *"❯ echo second"* ]]; then
        log_error "missing inline command name '❯ echo second' in output: $err"
        return 1
    fi

    log_success "multi-command pipeline shows inline command names"
    return 0
}

# Fail-fast: when a -x step fails, subsequent steps emit a skipped row
# rather than being silent.
test_exec_fail_fast_emits_skipped_row() {
    local remote_repo
    remote_repo=$(create_test_remote "exec-skipped" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-skipped/main" || return 1
    # Create a second worktree so we trigger the multi-target path
    # (single-target invocations use pass-through mode with no compact rows).
    git-worktree-checkout -b feat-a || return 1
    cd "../main" || return 1

    # The harness sets DAFT_TESTING=1 to silence the hook renderer for YAML
    # scenarios; unset it here since this test asserts on the renderer
    # output itself.
    local err
    set +e
    err=$(env -u DAFT_TESTING git-worktree-exec --all -x 'false' -x 'echo never' 2>&1 >/dev/null)
    local code=$?
    set -e

    if [[ $code -ne 1 ]]; then
        log_error "expected exit 1 for failing pipeline, got $code"
        return 1
    fi
    # The failure row shows ✗ and the inline command.
    if [[ "$err" != *"✗"* ]]; then
        log_error "missing failure sigil ✗: $err"
        return 1
    fi
    # The skipped row shows ○ and ends with 'skipped'.
    if [[ "$err" != *"○"* ]]; then
        log_error "missing skipped sigil ○: $err"
        return 1
    fi
    if [[ "$err" != *"echo never"*"skipped"* ]]; then
        log_error "missing 'echo never ... skipped' row: $err"
        return 1
    fi

    log_success "fail-fast pipeline emits skipped row"
    return 0
}

# Run a command line with a freshly allocated pty as its controlling
# terminal. macOS/BSD and util-linux script(1) disagree on argv shape.
# The command line should tee its own output/exit code to files — BSD
# script's status propagation isn't relied on.
run_under_pty() {
    local cmdline="$1"
    if [[ "$(uname)" == "Darwin" ]]; then
        script -q /dev/null sh -c "$cmdline" < /dev/null > /dev/null 2>&1
    else
        script -q -e -c "$cmdline" /dev/null < /dev/null > /dev/null 2>&1
    fi
}

# Alias capture must survive a controlling terminal (#663 regression):
# an interactive capture shell whose session still holds daft's tty
# job-stops itself with the SIGTTIN foreground dance (bash -i
# force-opens /dev/tty; zsh likewise) unless capture detaches into its
# own session via the `daft __capture-aliases` setsid trampoline.
# Without the trampoline this burns the full 10s capture deadline and
# loses the aliases. CI runners have no tty, so the terminal is
# fabricated with script(1) — this is the only automated coverage of
# the production trampoline dispatch (unit tests substitute perl).
test_exec_alias_capture_under_tty() {
    if ! command -v script >/dev/null 2>&1; then
        log_success "script(1) unavailable — skipped"
        return 0
    fi

    local remote_repo
    remote_repo=$(create_test_remote "exec-alias-tty" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "exec-alias-tty/main" || return 1

    # Fixture: isolated HOME with a .bashrc alias, plus a $SHELL wrapper
    # NAMED bash (ShellKind sniffs the basename) that pins rc lookup to
    # the fixture home even if the environment leaks.
    local fix="$TEMP_BASE_DIR/exec-alias-tty-fixture"
    rm -rf "$fix"
    mkdir -p "$fix/bin" "$fix/home"
    # The alias drops a marker in the worktree it runs in — file
    # assertions don't depend on renderer output layout.
    echo "alias daft_tty_probe='echo TTY_ALIAS_EXPANDED > \$PWD/tty-marker'" \
        > "$fix/home/.bashrc"
    cat > "$fix/bin/bash" <<EOF
#!/bin/sh
export HOME=$fix/home
exec bash "\$@"
EOF
    chmod +x "$fix/bin/bash"

    local out_file="$fix/exec.out" rc_file="$fix/exec.rc"
    # HOME/XDG_CACHE_HOME are scoped to the fixture so the capture cache
    # can't touch the real user cache; stdout goes to a file (keeps the
    # renderer plain) while the pty stays the controlling terminal —
    # which is all the foreground dance needs.
    local cmdline="env HOME=$fix/home XDG_CACHE_HOME=$fix/home/.cache SHELL=$fix/bin/bash \
git-worktree-exec --all -- daft_tty_probe > $out_file 2>&1; echo \$? > $rc_file"

    local t0=$SECONDS
    run_under_pty "$cmdline"
    local elapsed=$((SECONDS - t0))

    local rc
    rc=$(cat "$rc_file" 2>/dev/null)
    if [[ "$rc" != "0" ]]; then
        log_error "exec under pty exited ${rc:-<none>} (alias likely didn't resolve): $(cat "$out_file" 2>/dev/null)"
        return 1
    fi
    local repo_root
    repo_root="$(cd .. && pwd)"
    if ! grep -q "TTY_ALIAS_EXPANDED" "$repo_root/main/tty-marker" 2>/dev/null; then
        log_error "alias did not expand under a controlling tty: $(cat "$out_file" 2>/dev/null)"
        return 1
    fi
    # A stopped capture shell burns the whole 10s deadline before the
    # rc-less fallback runs — a healthy capture finishes in ~1s.
    if [[ $elapsed -gt 8 ]]; then
        log_error "exec under pty took ${elapsed}s — capture deadline burned (tty stop?)"
        return 1
    fi

    log_success "alias capture survived a controlling terminal (${elapsed}s)"
    return 0
}

# --- Test runner ---
run_worktree_exec_tests() {
    run_test "exec single-target pwd uses worktree cwd" \
        test_exec_single_target_pwd_in_worktree_cwd
    run_test "exec --all fans out to every worktree" \
        test_exec_all_runs_everywhere
    run_test "exec --sequential stops on first failure" \
        test_exec_sequential_stops_on_failure
    run_test "exec --keep-going continues through failures" \
        test_exec_keep_going_runs_all_despite_failure
    run_test "exec unmatched positional errors out" \
        test_exec_unmatched_positional_errors
    run_test "exec multi-command pipeline shows inline command names" \
        test_exec_multi_command_shows_inline_command_names
    run_test "exec fail-fast emits skipped row" \
        test_exec_fail_fast_emits_skipped_row
    run_test "exec alias capture survives a controlling tty" \
        test_exec_alias_capture_under_tty
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_worktree_exec_tests
    print_summary
    exit $?
fi
