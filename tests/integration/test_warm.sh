#!/bin/bash

# Integration tests for `daft warm` (git-worktree-warm) — #387.
#
# What only this suite can cover: the SYMLINK dispatch. `daft warm` is
# registered on nine packaging/doctor/docs surfaces as `git-worktree-warm`,
# and reaches its command through three separate arms in src/main.rs —
# `"git-worktree-warm"` (argv[0] is the symlink), `"warm"` (daft subcommand)
# and `"worktree-warm"` (git-daft subcommand). The YAML scenarios deliberately
# spell every invocation `daft warm` (a symlink list that drifts turns them
# into exit-127 flakes), so two of those three arms and the whole
# `get_clap_args("git-worktree-warm")` argv derivation are executed by nothing
# else in the test suite. A missing arm, or a `DAFT_VERBS` entry never added,
# fails here and only here.
#
# Each test therefore drives a DIFFERENT argument shape through a different
# arm, because the derivation is what is under test and it is argv-shaped: a
# bare positional, a long flag with a value, and a boolean flag over a
# re-run. All three must reach the same engine and produce the same receipt.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Clone <repo-name> in the contained layout, add a cold `develop` worktree,
# then declare a `copy:` cache in `main` and build it. Leaves the shell in
# `main`, which is the source every test below copies FROM.
#
# `develop` is created before the declaration exists, so nothing can have
# seeded it behind the test's back: the creation-time copy stage had no
# `copy:` key to read. Whatever ends up in `develop/node_modules` was put
# there by the `warm` invocation under test.
_warm_fixture() {
    local repo="$1"
    local remote_repo
    remote_repo=$(create_test_remote "$repo" "main") || return 1

    git-worktree-clone --layout contained "$remote_repo" >/dev/null 2>&1 || return 1
    cd "$repo" || return 1
    git-worktree-checkout develop >/dev/null 2>&1 || return 1

    cd main || return 1
    printf 'node_modules/\n' > .gitignore || return 1
    printf 'copy:\n  - node_modules/\n' > daft.yml || return 1
    mkdir -p node_modules/pkg || return 1
    printf 'built-in-main\n' > node_modules/pkg/index.js || return 1
    # The invariant the engine enforces: the entry must be gitignored. If the
    # fixture ever stops satisfying it, every assertion below would fail for
    # the wrong reason.
    git check-ignore node_modules >/dev/null 2>&1 || {
        log_error "fixture is wrong: node_modules is not gitignored"
        return 1
    }
    return 0
}

# The copied cache must be an independent replica in the target worktree —
# a real directory, not a symlink back into the source (that is `shared:`'s
# job, and `test ! -L` alone passes on a path that does not exist).
_assert_cache_landed_in_develop() {
    local wt="../develop"
    assert_file_exists "$wt/node_modules/pkg/index.js" "cache copied into develop" || return 1
    assert_file_contains "$wt/node_modules/pkg/index.js" "built-in-main" "copied content" || return 1
    if [[ -L "$wt/node_modules" ]]; then
        log_error "the replica is a symlink, not an independent copy"
        return 1
    fi
    return 0
}

# --- Arm 1: argv[0] is the git-worktree-warm symlink, with a positional ---
test_warm_symlink_form_copies_declared_cache() {
    _warm_fixture "test-repo-warm-symlink" || return 1

    local output status
    output=$(git-worktree-warm develop 2>&1)
    status=$?
    if [[ "$status" -ne 0 ]]; then
        log_error "git-worktree-warm exited $status: $output"
        return 1
    fi
    if ! grep -q "Copied 1 of 1 declared path" <<< "$output"; then
        log_error "no copy receipt from the symlink form: $output"
        return 1
    fi
    _assert_cache_landed_in_develop || return 1

    log_success "git-worktree-warm <worktree> copied the declared cache"
    return 0
}

# --- Arm 2: git-daft worktree-warm, with a long flag carrying a value ---
#
# Run from `develop` and pulling FROM `main`, which is the reverse direction
# of arm 1 — so the `--from` value has to survive the argv rewrite, not just
# be accepted.
test_warm_git_daft_verb_dispatches() {
    _warm_fixture "test-repo-warm-gitdaft" || return 1
    cd ../develop || return 1

    local output status
    output=$(git-daft worktree-warm --from main 2>&1)
    status=$?
    if [[ "$status" -ne 0 ]]; then
        log_error "git-daft worktree-warm exited $status: $output"
        return 1
    fi
    if ! grep -q "Copied 1 of 1 declared path" <<< "$output"; then
        log_error "no copy receipt from the git-daft verb: $output"
        return 1
    fi
    # The receipt names both ends; the source is the one `--from` chose.
    if ! grep -q "from 'main'" <<< "$output"; then
        log_error "--from did not survive the argv rewrite: $output"
        return 1
    fi
    cd ../main || return 1
    _assert_cache_landed_in_develop || return 1

    # The symlink also has to answer for itself: `--help` is what a packaging
    # smoke test and a man-page reader both hit first.
    assert_command_help "git-worktree-warm" "git-worktree-warm should have help" || return 1

    log_success "git-daft worktree-warm --from <worktree> dispatched and copied"
    return 0
}

# --- Arm 3: the daft verb, and the boolean flag over a re-run ---
#
# The idempotence contract is what makes `warm` safe in a script: a second run
# reports the entry as already present and leaves it alone, and only `--force`
# replaces it. Both spellings of that pair go through the `"warm"` arm here.
test_warm_daft_verb_parity_and_force() {
    _warm_fixture "test-repo-warm-daft" || return 1

    local output status
    output=$(daft warm develop 2>&1)
    status=$?
    if [[ "$status" -ne 0 ]]; then
        log_error "daft warm exited $status: $output"
        return 1
    fi
    if ! grep -q "Copied 1 of 1 declared path" <<< "$output"; then
        log_error "no copy receipt from the daft verb: $output"
        return 1
    fi
    _assert_cache_landed_in_develop || return 1

    # Diverge the copy so a silent re-copy would be visible.
    printf 'edited-in-develop\n' > ../develop/node_modules/pkg/index.js || return 1

    output=$(daft warm develop 2>&1)
    status=$?
    if [[ "$status" -ne 0 ]]; then
        log_error "second daft warm exited $status: $output"
        return 1
    fi
    if ! grep -q "already present" <<< "$output"; then
        log_error "re-run did not report the entry as already present: $output"
        return 1
    fi
    assert_file_contains "../develop/node_modules/pkg/index.js" "edited-in-develop" \
        "a skipped entry is left byte-for-byte alone" || return 1

    output=$(daft warm develop --force 2>&1)
    status=$?
    if [[ "$status" -ne 0 ]]; then
        log_error "daft warm --force exited $status: $output"
        return 1
    fi
    if ! grep -q "Copied 1 of 1 declared path" <<< "$output"; then
        log_error "--force did not re-copy: $output"
        return 1
    fi
    assert_file_contains "../develop/node_modules/pkg/index.js" "built-in-main" \
        "--force replaced the diverged copy" || return 1

    log_success "daft warm copied, skipped on re-run, and replaced under --force"
    return 0
}

# --- Test runner ---
run_warm_tests() {
    run_test "warm_symlink_form_copies_declared_cache" test_warm_symlink_form_copies_declared_cache
    run_test "warm_git_daft_verb_dispatches" test_warm_git_daft_verb_dispatches
    run_test "warm_daft_verb_parity_and_force" test_warm_daft_verb_parity_and_force
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_warm_tests
    print_summary
    exit $?
fi
