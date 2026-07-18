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

# An unknown task name fails with a non-zero exit when there is no reserved
# `run` task to receive it as an argument.
test_run_unknown_task() {
    _run_repo run-unknown <<'YAML' || return 1
tasks:
  seed-db:
    jobs:
      - name: seed
        run: "true"
YAML
    if daft run nope >/dev/null 2>&1; then
        log_error "unknown task should have exited non-zero"
        return 1
    fi
    log_success "Unknown task name errored"
    return 0
}

# A first word naming no task is forwarded — with the rest — to the reserved
# `run` task as shell-escaped appended arguments.
test_run_word_fallback_forwards_args() {
    _run_repo run-fallback <<'YAML' || return 1
tasks:
  run:
    jobs:
      - name: toucher
        run: touch run-got
YAML
    daft run hello >/dev/null 2>&1 || { log_error "fallback run failed"; return 1; }
    assert_file_exists "run-got" "reserved task should have run" || return 1
    assert_file_exists "hello" "the word should append to the job command" || return 1
    log_success "Unmatched word fell through to the reserved task as an argument"
    return 0
}

# A first word naming a task takes precedence over argument fallback; a
# leading `--` forces argument forwarding past the name match.
test_run_task_name_precedence_and_escape() {
    _run_repo run-precedence <<'YAML' || return 1
tasks:
  run:
    jobs:
      - name: toucher
        run: touch run-got
  greet:
    jobs:
      - name: greeter
        run: touch greet-got
YAML
    daft run greet extra >/dev/null 2>&1 || { log_error "named run failed"; return 1; }
    assert_file_exists "greet-got" "named task should have run" || return 1
    assert_file_exists "extra" "remaining words forward to the named task" || return 1
    assert_file_not_exists "run-got" "reserved task must not have run" || return 1

    daft run -- greet >/dev/null 2>&1 || { log_error "escaped run failed"; return 1; }
    assert_file_exists "run-got" "-- must force the reserved task" || return 1
    assert_file_exists "greet" "the task name becomes a plain argument" || return 1
    log_success "Task names win over arguments; -- forces the fallback"
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
    run_test "run_word_fallback_forwards_args" test_run_word_fallback_forwards_args
    run_test "run_task_name_precedence_and_escape" test_run_task_name_precedence_and_escape
    run_test "run_propagates_exit_code" test_run_propagates_exit_code
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_run_tests
    print_summary
    exit $?
fi
