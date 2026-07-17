#!/bin/bash

# Integration tests for `daft run` — user-invoked tasks in daft.yml.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Create a contained-layout repo whose daft.yml defines the given tasks block,
# leaving the shell cd'd into the default-branch worktree. Usage:
#   _run_repo <repo-name> <<'YAML'
#   tasks:
#     run:
#       jobs:
#         - name: serve
#           run: echo hi
#   YAML
_run_repo() {
    local repo="$1"
    git-worktree-init --layout contained "$repo" >/dev/null 2>&1 || return 1
    cd "$repo/master" || return 1
    cat > daft.yml || return 1
    # daft run bypasses the trust gate, so no `hooks trust` is needed here.
}

# Bare `daft run` executes the reserved `run` task.
test_run_default_task() {
    _run_repo run-default <<'YAML' || return 1
tasks:
  run:
    jobs:
      - name: serve
        run: echo "default-ran" > .ran
YAML
    daft run >/dev/null 2>&1 || { log_error "daft run failed"; return 1; }
    assert_file_exists ".ran" "reserved run task should have run" || return 1
    assert_file_contains ".ran" "default-ran" "marker content" || return 1
    log_success "Bare daft run executed the reserved task"
    return 0
}

# `daft run <name>` executes only the named task.
test_run_named_task() {
    _run_repo run-named <<'YAML' || return 1
tasks:
  run:
    jobs:
      - name: serve
        run: echo serve > .serve
  seed-db:
    jobs:
      - name: seed
        run: echo seeded > .seeded
YAML
    daft run seed-db >/dev/null 2>&1 || { log_error "daft run seed-db failed"; return 1; }
    assert_file_exists ".seeded" "named task should have run" || return 1
    assert_file_not_exists ".serve" "other task must not have run" || return 1
    log_success "daft run <name> executed only the named task"
    return 0
}

# An unknown task name fails with a non-zero exit.
test_run_unknown_task() {
    _run_repo run-unknown <<'YAML' || return 1
tasks:
  run:
    jobs:
      - name: serve
        run: "true"
YAML
    if daft run nope >/dev/null 2>&1; then
        log_error "unknown task should have exited non-zero"
        return 1
    fi
    log_success "Unknown task name errored"
    return 0
}

# A failing job propagates its exit code.
test_run_propagates_exit_code() {
    _run_repo run-exit <<'YAML' || return 1
tasks:
  run:
    jobs:
      - name: boom
        run: exit 3
YAML
    daft run >/dev/null 2>&1
    local code=$?
    if [[ "$code" -ne 3 ]]; then
        log_error "expected exit code 3, got $code"
        return 1
    fi
    log_success "Failing task propagated its exit code"
    return 0
}

# --- Test runner ---
run_run_tests() {
    run_test "run_default_task" test_run_default_task
    run_test "run_named_task" test_run_named_task
    run_test "run_unknown_task" test_run_unknown_task
    run_test "run_propagates_exit_code" test_run_propagates_exit_code
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_run_tests
    print_summary
    exit $?
fi
