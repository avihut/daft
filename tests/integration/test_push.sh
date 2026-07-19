#!/bin/bash

# Integration tests for daft push (git-worktree-push)
#
# The command's single guarantee (#600): the shared pre-push hook runs with
# cwd = the pushed branch's worktree, not the invoking worktree. These tests
# probe that with a hook that records its working directory.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# The core acceptance test: push branch B from worktree A; the hook's cwd
# must be B's worktree.
test_push_runs_hook_in_target_worktree() {
    local remote_repo=$(create_test_remote "test-repo-push-cwd" "main")
    local hook_log="$PWD/push-hook-cwd.log"
    local hooks_dir="$PWD/push-hooks"

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-push-cwd"
    git-worktree-checkout develop || return 1

    # Commit something to push on develop
    (
        cd develop
        echo "pushable change" > change.txt
        git add change.txt
        GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
        GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@test.com" \
            git commit -m "Pushable change" >/dev/null 2>&1
    ) || return 1

    # Shared cwd-recording pre-push hook (core.hooksPath — the shared-hook
    # mechanism; repo config applies to every worktree)
    mkdir -p "$hooks_dir"
    printf '#!/bin/sh\npwd >> "%s"\nexit 0\n' "$hook_log" > "$hooks_dir/pre-push"
    chmod +x "$hooks_dir/pre-push"
    git -C main config core.hooksPath "$hooks_dir" || return 1

    # Invoke from MAIN's worktree, pushing develop
    cd main
    daft push develop || return 1

    if [[ ! -f "$hook_log" ]]; then
        log_error "pre-push hook never ran"
        return 1
    fi
    if ! grep -q "test-repo-push-cwd/develop$" "$hook_log"; then
        log_error "hook cwd was not develop's worktree: $(cat "$hook_log")"
        return 1
    fi
    if grep -q "test-repo-push-cwd/main$" "$hook_log"; then
        log_error "hook ran in the invoking worktree (main): $(cat "$hook_log")"
        return 1
    fi

    # The push actually landed
    git fetch origin >/dev/null 2>&1
    local ahead
    ahead=$(git rev-list --count origin/develop..develop 2>/dev/null)
    if [[ "$ahead" != "0" ]]; then
        log_error "develop was not pushed (still $ahead ahead)"
        return 1
    fi

    return 0
}

# A failing hook blocks the push (non-zero exit); --no-verify bypasses it.
test_push_failing_hook_blocks_and_no_verify_bypasses() {
    local remote_repo=$(create_test_remote "test-repo-push-gate" "main")
    local hooks_dir="$PWD/gate-hooks"

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-push-gate"
    git-worktree-checkout develop || return 1
    (
        cd develop
        echo "gated change" > gated.txt
        git add gated.txt
        GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
        GIT_COMMITTER_NAME="Test" GIT_COMMITTER_EMAIL="test@test.com" \
            git commit -m "Gated change" >/dev/null 2>&1
    ) || return 1

    mkdir -p "$hooks_dir"
    printf '#!/bin/sh\necho "GATE SAYS NO" >&2\nexit 1\n' > "$hooks_dir/pre-push"
    chmod +x "$hooks_dir/pre-push"
    git -C main config core.hooksPath "$hooks_dir" || return 1

    cd main
    if daft push develop >/dev/null 2>&1; then
        log_error "push should have been rejected by the failing pre-push hook"
        return 1
    fi

    git fetch origin >/dev/null 2>&1
    local ahead
    ahead=$(git rev-list --count origin/develop..develop 2>/dev/null)
    if [[ "$ahead" == "0" ]]; then
        log_error "rejected push must not reach the remote"
        return 1
    fi

    # Bypass lets the same push through (git-worktree-push symlink form)
    git-worktree-push --no-verify develop || return 1
    git fetch origin >/dev/null 2>&1
    ahead=$(git rev-list --count origin/develop..develop 2>/dev/null)
    if [[ "$ahead" != "0" ]]; then
        log_error "--no-verify push did not land"
        return 1
    fi

    return 0
}

# A branch with no checked-out worktree pushes from the invoking cwd (not an
# error) and gets an upstream configured (the SetUpstream shape).
test_push_branch_without_worktree_sets_upstream() {
    local remote_repo=$(create_test_remote "test-repo-push-nowt" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-push-nowt/main"

    git branch hotfix || return 1
    daft push hotfix || return 1

    local tracking
    tracking=$(git config branch.hotfix.remote 2>/dev/null)
    if [[ "$tracking" != "origin" ]]; then
        log_error "push of an upstream-less branch must set upstream (got '$tracking')"
        return 1
    fi
    if ! git ls-remote --heads origin hotfix | grep -q hotfix; then
        log_error "hotfix never reached the remote"
        return 1
    fi

    return 0
}

# Bare `daft push` (no BRANCH) resolves the current worktree's branch — the
# documented default, and the only invocation where the resolved worktree IS
# the invoking directory.
test_push_no_argument_uses_current_branch() {
    local remote_repo=$(create_test_remote "test-repo-push-noarg" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-push-noarg/main"

    git-worktree-checkout feature/test-feature || return 1
    cd "../feature/test-feature"

    git config user.email "test@example.com"
    git config user.name "Test User"
    echo "noarg" > noarg.txt
    git add noarg.txt
    git commit -q -m "No-arg change" || return 1

    # The hook must still run here, in the branch's own worktree.
    local hooks_dir="$(pwd)/../../hooks-noarg"
    mkdir -p "$hooks_dir"
    printf '#!/bin/sh\npwd > %s/cwd\nexit 0\n' "$hooks_dir" > "$hooks_dir/pre-push"
    chmod +x "$hooks_dir/pre-push"
    git config core.hooksPath "$hooks_dir"

    local output
    output=$(daft push 2>&1) || {
        log_error "bare daft push failed: $output"
        return 1
    }
    if ! grep -q "Pushed 'feature/test-feature'" <<< "$output"; then
        log_error "bare push did not report the inferred branch: $output"
        return 1
    fi
    if ! grep -q "feature/test-feature" "$hooks_dir/cwd"; then
        log_error "hook cwd was not the current worktree: $(cat "$hooks_dir/cwd")"
        return 1
    fi

    return 0
}

# A ref that is not a local branch (a tag) must be refused, not handed to git
# as if it were a branch — that would publish the tag and report success.
test_push_rejects_non_branch_ref() {
    local remote_repo=$(create_test_remote "test-repo-push-tag" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-push-tag/main"

    git tag v9.9.9 || return 1

    local output
    if output=$(daft push v9.9.9 2>&1); then
        log_error "pushing a tag name must fail, got: $output"
        return 1
    fi
    if ! grep -q "No local branch named 'v9.9.9'" <<< "$output"; then
        log_error "unexpected rejection message: $output"
        return 1
    fi
    if git ls-remote --tags origin | grep -q "v9.9.9"; then
        log_error "the tag was published to the remote"
        return 1
    fi

    return 0
}

# Help output names the guarantee.
test_push_help() {
    if ! daft push --help 2>&1 | grep -q "pushed branch's own worktree"; then
        log_error "daft push --help missing the worktree-correct pitch"
        return 1
    fi
    return 0
}

run_push_tests() {
    log "Running push integration tests..."
    run_test "push_runs_hook_in_target_worktree" "test_push_runs_hook_in_target_worktree"
    run_test "push_failing_hook_blocks_and_no_verify_bypasses" "test_push_failing_hook_blocks_and_no_verify_bypasses"
    run_test "push_branch_without_worktree_sets_upstream" "test_push_branch_without_worktree_sets_upstream"
    run_test "push_no_argument_uses_current_branch" "test_push_no_argument_uses_current_branch"
    run_test "push_rejects_non_branch_ref" "test_push_rejects_non_branch_ref"
    run_test "push_help" "test_push_help"
}
