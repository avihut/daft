#!/usr/bin/env bash
# Sourceable helper: the integration suites, in the order CI's
# `integration-tests` job runs them (.github/workflows/test.yml):
#
#   1. the YAML scenario suite — `xtask manual-test`, the primary surface
#   2. the blessed shell suite — `tests/integration/test_all.sh`, the
#      residue the YAML runner cannot express (PTY, signal, wrapper-cwd; #447)
#
# Both suites run even when the first is red, so one invocation reports every
# failing suite instead of hiding the second behind the first; the function's
# exit status is non-zero if either failed. Extra arguments go to
# `xtask manual-test` (`-v`, `-q`, `-j N`, scenario paths).
#
# Cancellation is the one non-zero exit that must NOT fall through: the YAML
# runner catches Ctrl+C itself and exits 130 from its handler
# (xtask/src/manual_test/mod.rs — its documented contract, so shells can tell
# a cancel from a failure) rather than dying by SIGINT, which means this shell
# survives the keypress. A cancelled first suite therefore returns 130 at once
# instead of launching the shell suite the user just tried to stop. The shell
# suite has no handler — Ctrl+C kills it by signal, bash reports 130, and the
# same branch keeps the summary honest.
#
# Neither suite needs env from here: each provisions its own isolation
# (GIT_CONFIG_GLOBAL, DAFT_*_DIR, DAFT_TESTING) — the shell framework in
# test_framework.sh::setup, the YAML runner per step. Callers wrap this in
# `with_state_guard` (mise-tasks/test/_state_guard_lib.sh) after the
# `assert_binary_honors_overrides` preflight; the function itself adds no
# guard so the snapshot/verify pair brackets both suites once.
#
# File is intentionally non-executable so mise hides it from `mise tasks ls`.

run_integration_suites() {
  local rc=0 yaml_rc=0 shell_rc=0 start

  echo
  echo "=== YAML scenarios (xtask manual-test) ==="
  start=$SECONDS
  cargo run -q --package xtask -- manual-test "$@" || yaml_rc=$?
  local yaml_elapsed=$((SECONDS - start))

  if [ "$yaml_rc" -eq 130 ]; then
    echo
    echo "=== integration suites ==="
    _suite_verdict "yaml" "$yaml_rc" "$yaml_elapsed" || true
    echo "  shell  not run (cancelled)"
    return 130
  fi

  echo
  echo "=== shell suite (tests/integration/test_all.sh) ==="
  start=$SECONDS
  bash tests/integration/test_all.sh || shell_rc=$?
  local shell_elapsed=$((SECONDS - start))

  echo
  echo "=== integration suites ==="
  _suite_verdict "yaml" "$yaml_rc" "$yaml_elapsed" || rc=1
  _suite_verdict "shell" "$shell_rc" "$shell_elapsed" || rc=1
  if [ "$shell_rc" -eq 130 ]; then
    return 130
  fi
  return "$rc"
}

# Print one summary row; return the suite's pass/fail as the exit status.
_suite_verdict() {
  local name="$1" code="$2" secs="$3" verdict
  case "$code" in
    0) verdict="PASS" ;;
    130) verdict="CANCELLED (exit 130)" ;;
    *) verdict="FAIL (exit $code)" ;;
  esac
  printf '  %-6s %s (%ss)\n' "$name" "$verdict" "$secs"
  [ "$code" -eq 0 ]
}
