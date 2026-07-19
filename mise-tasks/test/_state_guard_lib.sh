#!/usr/bin/env bash
# Sourceable helper: the real-state test-isolation tripwire (#697).
#
# Guards against the leak class where a test run writes daft's *real*
# config/state/data dirs instead of its sandbox — the failure that put 822
# throwaway /tmp repos into the developer's real catalog.db. Two layers:
#
#   * assert_binary_honors_overrides <daft-bin> — a READ-ONLY preflight. Runs
#     `<daft-bin> __dirs` under a throwaway DAFT_*_DIR sandbox and fails if the
#     binary resolves the real dirs instead. Catches a non-dev-build binary (a
#     release/tagged build with the DAFT_*_DIR overrides compiled out) or a
#     system `daft` shadowing the local one on PATH — *before* the suite can
#     leak. Only meaningful for suites that exec a built binary (integration,
#     manual); unit tests honor the overrides by construction under cfg(test).
#
#   * with_state_guard <cmd...> — snapshots the real dirs, runs the command,
#     then re-verifies them via `xtask real-state-guard`. A change is fatal
#     (exit 1) regardless of the command's own exit code; an unchanged run
#     passes the command's exit code through so genuine test failures still
#     surface.
#
# Escape hatch: DAFT_SKIP_STATE_GUARD=1 disables both layers.
#
# File is intentionally non-executable so mise hides it from `mise tasks ls`.

# ── Git config isolation ─────────────────────────────────────────────────────
# The suite's fixtures shell out to real `git` to build throwaway repos, and
# any ambient global/system git config leaks straight into them. The one that
# bites: a developer whose global `commit.gpgsign = true` makes every fixture
# commit reach the gpg agent, which chokes under the suite's concurrency and
# flakes the pre-push hook — a fixture whose HEAD commit silently failed leaves
# a repo with no commit, and a later `git rev-parse HEAD` comes back empty
# (e.g. repo::remove::build_tui_rows_populates_head_commit_metadata). Per-helper
# `-c commit.gpgsign=false` is whack-a-mole across ~15 fixtures and the flake
# just hops to the next unhardened one; neutralize signing for every git
# subprocess this run spawns instead, so the fix is total, not statistical.
#
# GIT_CONFIG_COUNT injects config at higher precedence than any config file, so
# it overrides the developer's global `true`. Surgical on purpose: overriding
# only *.gpgsign leaves their git identity (user.name/email) intact, so the
# fixtures that rely on an inherited global identity keep working — unlike
# GIT_CONFIG_GLOBAL=/dev/null, which would strip it and break them. Appends to
# any pre-existing GIT_CONFIG_COUNT rather than clobbering it.
_gc="${GIT_CONFIG_COUNT:-0}"
export "GIT_CONFIG_KEY_${_gc}=commit.gpgsign" "GIT_CONFIG_VALUE_${_gc}=false"
_gc=$((_gc + 1))
export "GIT_CONFIG_KEY_${_gc}=tag.gpgsign" "GIT_CONFIG_VALUE_${_gc}=false"
export GIT_CONFIG_COUNT=$((_gc + 1))
unset _gc

# Invoke the xtask guard. Centralized so the snapshot and verify calls stay in
# lockstep (same package, same quieting).
_state_guard_xtask() {
  cargo run -q --package xtask -- real-state-guard "$@"
}

# Read-only preflight: assert the daft binary at $1 honors DAFT_*_DIR.
assert_binary_honors_overrides() {
  [ "${DAFT_SKIP_STATE_GUARD:-}" = "1" ] && return 0
  local bin="$1"
  if [[ ! -x "$bin" ]]; then
    echo "state-guard preflight: daft binary not found or not executable: $bin" >&2
    return 1
  fi

  local probe out key got ok=1
  probe="$(mktemp -d)"
  # `__dirs` is a read-only internal command: it only prints the config/data/
  # state dirs this binary resolves. If the binary honors the overrides, every
  # path lands under $probe.
  if ! out="$(DAFT_CONFIG_DIR="$probe/cfg" DAFT_DATA_DIR="$probe/data" \
              DAFT_STATE_DIR="$probe/state" "$bin" __dirs 2>/dev/null)"; then
    echo "state-guard preflight: '$bin __dirs' failed (binary too old to have __dirs, or broken)" >&2
    rm -rf "$probe"
    return 1
  fi

  local kupper
  for key in config data state; do
    got="$(printf '%s\n' "$out" | awk -F '\t' -v k="$key" '$1 == k { print $2 }')"
    case "$got" in
      "$probe"/*) ;;
      *)
        # Uppercase via tr (not ${key^^}) so the helper stays portable across
        # bash and zsh rather than depending on bash-4 parameter expansion.
        kupper="$(printf '%s' "$key" | tr '[:lower:]' '[:upper:]')"
        echo "state-guard preflight: binary ignores DAFT_${kupper}_DIR (resolved: ${got:-<none>})" >&2
        ok=0
        ;;
    esac
  done
  rm -rf "$probe"

  if [[ "$ok" != "1" ]]; then
    {
      echo ""
      echo "The binary under test does not honor DAFT_*_DIR:"
      echo "  $bin"
      echo "It is not a daft_dev_build (a release/tagged build, or a system daft on"
      echo "PATH). Running the suite would leak test data into your real"
      echo "config/state/data dirs. Rebuild the dev binary. See #697."
    } >&2
    return 1
  fi
}

# Snapshot the real dirs, run "$@", then verify they are unchanged. A change is
# fatal; otherwise the command's own exit code is returned.
with_state_guard() {
  if [ "${DAFT_SKIP_STATE_GUARD:-}" = "1" ]; then
    "$@"
    return $?
  fi

  local fp rc=0
  fp="$(mktemp)"
  if ! _state_guard_xtask snapshot "$fp"; then
    echo "state-guard: failed to snapshot real dirs before the run" >&2
    rm -f "$fp"
    return 1
  fi

  "$@" || rc=$?

  if ! _state_guard_xtask verify "$fp"; then
    rm -f "$fp"
    # The tripwire fired: the run wrote the real dirs. This is fatal even if
    # the suite itself passed — the point is that it must never happen.
    exit 1
  fi
  rm -f "$fp"
  return "$rc"
}
