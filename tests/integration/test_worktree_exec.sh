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
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_worktree_exec_tests
    print_summary
    exit $?
fi
