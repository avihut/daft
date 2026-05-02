#!/bin/bash

# Integration tests for `daft repo remove`.
#
# Each test creates an isolated remote repo via `create_test_remote` (from
# test_framework.sh) so nothing leaks across tests. Per CLAUDE.md, tests must
# never use this repo as a test subject and must never modify global git
# config — `setup` in test_framework.sh sets `GIT_AUTHOR_*`/`GIT_COMMITTER_*`
# env vars and isolates `GIT_CONFIG_GLOBAL` to a per-suite temp file.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test 1: Clone a temp repo with two worktrees, run `daft repo remove --force`,
# assert all dirs gone (and the empty parent project_root chain too).
test_repo_remove_basic() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-remove-basic" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-remove-basic" || return 1

    # Add a second worktree so the operation has more than one to remove.
    (cd main && git-worktree-checkout develop) || return 1
    assert_directory_exists "develop" || return 1
    assert_directory_exists "main" || return 1

    local project_root
    project_root=$(pwd -P)

    # Move out of the project root so removal of empty parents can complete.
    cd ..

    daft repo remove --force "$project_root" || return 1

    if [[ -d "$project_root" ]]; then
        log_error "daft repo remove should have removed $project_root"
        return 1
    fi
    if [[ -d "$project_root/main" ]] || [[ -d "$project_root/develop" ]] || [[ -d "$project_root/.git" ]]; then
        log_error "daft repo remove left worktrees or bare git dir behind"
        return 1
    fi

    log_success "daft repo remove removed two worktrees + bare git dir"
    return 0
}

# Test 2: Configure a worktree-pre-remove hook that writes a marker file;
# trust the repo; remove; assert marker exists and repo dirs are gone.
test_repo_remove_runs_pre_remove_hooks() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-remove-hooks" "main")

    # Step 1: Add a pre-remove hook to the remote main branch and create a
    # NEW feature branch from main AFTER the hook is committed, so both
    # worktrees we'll create below have the hook checked in. We can't reuse
    # the develop branch from create_test_remote because it was branched
    # from main BEFORE the hook was added.
    local setup_clone="$TEMP_BASE_DIR/temp_repo_remove_hooks_setup_$$"
    git clone "$remote_repo" "$setup_clone" >/dev/null 2>&1
    (
        cd "$setup_clone"
        mkdir -p .daft/hooks
        # Hook writes a marker per worktree so the test can prove every
        # worktree's pre-remove hook fired during `daft repo remove`.
        cat > .daft/hooks/worktree-pre-remove <<'HOOKEOF'
#!/bin/bash
echo "$DAFT_BRANCH_NAME" >> "$MARKER_FILE"
HOOKEOF
        chmod +x .daft/hooks/worktree-pre-remove

        git add .daft/hooks/worktree-pre-remove
        git commit -m "Add pre-remove hook to main" >/dev/null 2>&1
        git push origin main >/dev/null 2>&1

        # Branch a new feature line off the now-hook-bearing main and push.
        git checkout -b hook-branch >/dev/null 2>&1
        echo "branch content" > branch.txt
        git add branch.txt >/dev/null 2>&1
        git commit -m "Add branch content" >/dev/null 2>&1
        git push origin hook-branch >/dev/null 2>&1
    ) >/dev/null 2>&1
    rm -rf "$setup_clone"

    # Step 2: Clone the repo with a fresh local copy.
    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-remove-hooks" || return 1

    # Step 3: Trust the repo from inside the main worktree (must be in a git
    # repo — the bare-layout project root is not).
    (
        cd "main"
        git-daft hooks trust --force >/dev/null 2>&1
    ) || {
        log_error "Failed to trust repository"
        return 1
    }

    # Step 4: Fetch the hook-branch and add a worktree (inherits the hook
    # because branch was created off the hook-bearing main). We deliberately
    # avoid nested branch names like feature/foo because remove_bare_directory
    # only walks empty parent dirs above project_root, leaving an empty
    # `feature/` subdir behind that would defeat the assertion below.
    (cd main && git fetch origin >/dev/null 2>&1 && git-worktree-checkout hook-branch) || return 1
    assert_directory_exists "hook-branch" || return 1
    assert_file_exists "hook-branch/.daft/hooks/worktree-pre-remove" || return 1

    local project_root
    project_root=$(pwd -P)

    # Step 5: Marker file path is exported into the subprocess env. Live
    # outside the project root so removal doesn't take it with us.
    local marker_file
    marker_file=$(mktemp "${TMPDIR:-/tmp}/daft-pre-remove-marker.XXXXXX")
    rm -f "$marker_file"

    # Step 6: Run the removal from inside the main worktree (cwd needs to be
    # inside a git repo for compute_repo_id() during hook log-record writes).
    (
        cd "main"
        MARKER_FILE="$marker_file" daft repo remove --force "$project_root"
    ) || return 1

    # Step 7: Assert all repo dirs are gone.
    if [[ -d "$project_root" ]]; then
        log_error "daft repo remove should have removed $project_root"
        rm -f "$marker_file"
        return 1
    fi

    # Step 8: Assert the hook ran for both worktrees.
    if [[ ! -f "$marker_file" ]]; then
        log_error "worktree-pre-remove hook did not write marker file"
        return 1
    fi
    if ! grep -q "main" "$marker_file"; then
        log_error "marker file missing main: $(cat "$marker_file")"
        rm -f "$marker_file"
        return 1
    fi
    if ! grep -q "hook-branch" "$marker_file"; then
        log_error "marker file missing hook-branch: $(cat "$marker_file")"
        rm -f "$marker_file"
        return 1
    fi

    log_success "worktree-pre-remove hook fired for every worktree"
    rm -f "$marker_file"

    # Restore working directory: we cd'd into a repo that's now gone, so we
    # have to step back to a real cwd before subsequent tests run.
    cd "$WORK_DIR" || return 0
    return 0
}

# Test 3: Run `daft repo remove --dry-run`; assert nothing changed and stdout
# mentions "Would remove".
test_repo_remove_dry_run() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-remove-dry-run" "main")

    git-worktree-clone --layout contained "$remote_repo" || return 1
    cd "test-repo-remove-dry-run" || return 1

    local project_root
    project_root=$(pwd -P)

    local dry_output
    dry_output=$(daft repo remove --dry-run "$project_root" 2>&1) || return 1

    if ! echo "$dry_output" | grep -q "Would remove"; then
        log_error "Dry-run output should mention 'Would remove'"
        log_error "Output: $dry_output"
        return 1
    fi

    # Repo + bare git dir + main worktree must all still exist.
    assert_directory_exists "$project_root" || return 1
    assert_directory_exists "$project_root/.git" || return 1
    assert_directory_exists "$project_root/main" || return 1

    log_success "daft repo remove --dry-run reports plan without changes"
    return 0
}

# Test 4: Run `daft repo remove "/tmp/non-git-$$"` (a non-git dir); assert
# non-zero exit + stderr mentions "not inside a Git repository".
test_repo_remove_non_git_path_fails() {
    local non_git_dir
    non_git_dir=$(mktemp -d "${TMPDIR:-/tmp}/daft-non-git.XXXXXX")
    # Trap to clean up even on failure paths below.
    trap 'rm -rf "$non_git_dir"' RETURN

    local err_output
    err_output=$(daft repo remove --force "$non_git_dir" 2>&1)
    local rc=$?

    if [[ $rc -eq 0 ]]; then
        log_error "daft repo remove should have failed on non-git dir"
        return 1
    fi

    if ! echo "$err_output" | grep -q "not inside a Git repository"; then
        log_error "Expected stderr to mention 'not inside a Git repository'"
        log_error "Output: $err_output"
        return 1
    fi

    if [[ ! -d "$non_git_dir" ]]; then
        log_error "daft repo remove must NOT touch a non-git dir on rejection"
        return 1
    fi

    log_success "daft repo remove rejects non-git path with explanatory error"
    return 0
}

# Test 5: Regression test for `mise-tasks/sandbox/clean-repos` denylist.
# Spec calls for a scenario that creates two repos under the sandbox, runs the
# helper, and asserts both repos are gone while no spurious files were touched
# in the denylisted subtrees (bin/, config/, data/, state/).
#
# Implementation choice (Plan A from issue plan, but invoking the script
# directly instead of via `mise run`): the helper resolves its target via
# `sandbox_dir` in `mise-tasks/sandbox/_lib.sh`, which honors
# `DAFT_SANDBOX_BASE` and uses the *current pwd* hash. Going through `mise run`
# would walk up to find the daft project's mise.toml from the test's pwd,
# coupling the test to the daft repo layout. Direct invocation with a faked
# pwd and DAFT_SANDBOX_BASE keeps the test isolated and exercises the same
# denylist logic that the mise task runs.
test_repo_remove_clean_repos_helper() {
    local fake_pwd sandbox_base hash sandbox_dir
    # Resolve through `cd ... && pwd` so the path matches what `pwd` returns
    # inside `sandbox_dir()`. macOS `mktemp` can yield paths like
    # `/var/folders/.../T//foo` (note the double-slash), but `pwd` normalizes
    # to a single slash — and `sandbox_dir()` hashes the `pwd` output, so the
    # two must agree byte-for-byte for the hashes to match.
    fake_pwd=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/daft-clean-repos-pwd.XXXXXX")" && pwd)
    sandbox_base=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/daft-clean-repos-base.XXXXXX")" && pwd)
    # Trap on RETURN to clean up even when assertions fail mid-way.
    # shellcheck disable=SC2064
    trap "rm -rf '$fake_pwd' '$sandbox_base'" RETURN

    if command -v sha256sum &>/dev/null; then
        hash=$(echo -n "$fake_pwd" | sha256sum | cut -c1-8)
    else
        hash=$(echo -n "$fake_pwd" | shasum -a 256 | cut -c1-8)
    fi
    sandbox_dir="$sandbox_base/daft-sandbox-$hash"

    # Two real git repos under test/ — these are what clean-repos should
    # discover and remove.
    mkdir -p "$sandbox_dir/test/scenario1/work"
    mkdir -p "$sandbox_dir/test/scenario2/work"
    (
        cd "$sandbox_dir/test/scenario1/work"
        git init -q
        echo "scenario1" > README
        git add README
        git commit -q -m "init"
    ) || return 1
    (
        cd "$sandbox_dir/test/scenario2/work"
        git init -q
        echo "scenario2" > README
        git add README
        git commit -q -m "init"
    ) || return 1

    # Sentinel files in the denylisted subtrees (and a stray .git that lives
    # inside one of them, to prove the prune actually skips those paths).
    mkdir -p "$sandbox_dir/bin" "$sandbox_dir/config" "$sandbox_dir/data" "$sandbox_dir/state"
    echo "bin sentinel" > "$sandbox_dir/bin/keep-me"
    echo "config sentinel" > "$sandbox_dir/config/keep-me"
    echo "data sentinel" > "$sandbox_dir/data/keep-me"
    echo "state sentinel" > "$sandbox_dir/state/keep-me"
    # A .git file lurking inside data/ to verify the prune denylist excludes
    # it. If the find command walks into data/ this would be discovered as a
    # repo and `daft repo remove` would fail (or worse, blow it away).
    mkdir -p "$sandbox_dir/data/.git"
    echo "ref: refs/heads/main" > "$sandbox_dir/data/.git/HEAD"

    # Run the helper directly. fake_pwd is the cwd that sandbox_dir() hashes,
    # and DAFT_SANDBOX_BASE replaces /tmp as the parent directory.
    (
        cd "$fake_pwd"
        DAFT_SANDBOX_BASE="$sandbox_base" \
            bash "$PROJECT_ROOT/mise-tasks/sandbox/clean-repos"
    ) || {
        log_error "clean-repos helper exited non-zero"
        return 1
    }

    # Both repos must be gone.
    if [[ -d "$sandbox_dir/test/scenario1/work" ]]; then
        log_error "clean-repos did not remove scenario1 repo"
        return 1
    fi
    if [[ -d "$sandbox_dir/test/scenario2/work" ]]; then
        log_error "clean-repos did not remove scenario2 repo"
        return 1
    fi

    # Sentinels in the denylisted subtrees must survive untouched.
    for subtree in bin config data state; do
        if [[ ! -f "$sandbox_dir/$subtree/keep-me" ]]; then
            log_error "clean-repos clobbered $subtree/keep-me sentinel"
            return 1
        fi
        local got
        got=$(cat "$sandbox_dir/$subtree/keep-me")
        if [[ "$got" != "$subtree sentinel" ]]; then
            log_error "$subtree/keep-me sentinel was modified: $got"
            return 1
        fi
    done

    # The .git lurking inside data/ must still be there — it must not have
    # been discovered through the denylist, and `daft repo remove` must not
    # have eaten it.
    if [[ ! -d "$sandbox_dir/data/.git" ]]; then
        log_error "clean-repos walked into data/ and removed the lurker .git"
        return 1
    fi

    log_success "clean-repos removes test repos and respects bin/config/data/state denylist"
    return 0
}

# Test 6: Regression — running `daft repo remove <path>` from a CWD that is
# not itself inside a git repo (e.g. a sandbox /tmp dir) must succeed. Earlier
# revisions called `DaftSettings::load()` unconditionally, which discovers a
# git repo from CWD via gitoxide and errors out before the path argument was
# ever consulted. The fix uses `load_global()`; this test pins it.
test_repo_remove_from_non_repo_cwd() {
    local remote_repo non_git_parent
    remote_repo=$(create_test_remote "test-repo-remove-non-repo-cwd" "main")

    # Clone into a known parent dir that is itself NOT a git repo.
    non_git_parent=$(mktemp -d "${TMPDIR:-/tmp}/daft-non-repo-cwd.XXXXXX")
    trap 'rm -rf "$non_git_parent"' RETURN

    (cd "$non_git_parent" && git-worktree-clone --layout contained "$remote_repo") || return 1
    local project_root="$non_git_parent/test-repo-remove-non-repo-cwd"
    assert_directory_exists "$project_root" || return 1

    # CWD is the non-repo parent. Pass the repo path as an argument.
    cd "$non_git_parent" || return 1
    if [[ -d ".git" ]]; then
        log_error "test setup error: $non_git_parent unexpectedly is a git repo"
        return 1
    fi

    daft repo remove --force "$project_root" || return 1

    if [[ -d "$project_root" ]]; then
        log_error "daft repo remove should have removed $project_root"
        return 1
    fi

    log_success "daft repo remove works from a non-repo cwd with explicit path"
    return 0
}

# Test 7: CRITICAL data-loss regression — `daft repo remove` must NEVER
# remove the directory containing the project root. An earlier revision
# walked up the parent chain removing empty directories, which (in the
# real-world case that surfaced this) consumed the user's `test/` dir
# after removing `test/myrepo/`. Anything above project_root is user
# territory.
test_repo_remove_preserves_empty_parent_directory() {
    local remote_repo container
    remote_repo=$(create_test_remote "test-repo-remove-parent-preservation" "main")

    # Container dir whose ONLY child will be the project_root. After removal
    # it would become empty — the bug under test.
    container=$(mktemp -d "${TMPDIR:-/tmp}/daft-empty-parent.XXXXXX")
    trap 'rm -rf "$container"' RETURN

    (cd "$container" && git-worktree-clone --layout contained "$remote_repo") || return 1
    local project_root="$container/test-repo-remove-parent-preservation"
    assert_directory_exists "$project_root" || return 1

    # Sanity: container has only the project_root inside.
    local entry_count
    entry_count=$(find "$container" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ')
    if [[ "$entry_count" != "1" ]]; then
        log_error "test setup: expected container to have exactly 1 child, got $entry_count"
        return 1
    fi

    cd "$container" || return 1
    daft repo remove --force "$project_root" || return 1

    if [[ -d "$project_root" ]]; then
        log_error "project_root not removed: $project_root"
        return 1
    fi
    if [[ ! -d "$container" ]]; then
        log_error "DATA LOSS: container directory was removed: $container"
        return 1
    fi

    log_success "daft repo remove preserves the parent directory even when empty"
    return 0
}

# Test 8: Run with relative path argument from inside the parent directory
# (matches the failure-reporting reproduction: `cd parent && daft repo remove repo`).
test_repo_remove_relative_path_from_parent() {
    local remote_repo container
    remote_repo=$(create_test_remote "test-repo-remove-rel-from-parent" "main")
    container=$(mktemp -d "${TMPDIR:-/tmp}/daft-rel-from-parent.XXXXXX")
    trap 'rm -rf "$container"' RETURN

    (cd "$container" && git-worktree-clone --layout contained "$remote_repo") || return 1
    local project_root="$container/test-repo-remove-rel-from-parent"
    assert_directory_exists "$project_root" || return 1

    cd "$container" || return 1
    daft repo remove --force "test-repo-remove-rel-from-parent" || return 1

    if [[ -d "$project_root" ]]; then
        log_error "project_root not removed via relative path: $project_root"
        return 1
    fi
    if [[ ! -d "$container" ]]; then
        log_error "DATA LOSS via relative path: container removed: $container"
        return 1
    fi

    log_success "daft repo remove works with a relative path from the parent dir"
    return 0
}

# Test 9: Run without a path argument from inside the project_root.
# Default-cwd resolution must find the bare and remove it cleanly.
test_repo_remove_no_arg_from_inside_project_root() {
    local remote_repo container
    remote_repo=$(create_test_remote "test-repo-remove-no-arg" "main")
    container=$(mktemp -d "${TMPDIR:-/tmp}/daft-no-arg.XXXXXX")
    trap 'rm -rf "$container"' RETURN

    (cd "$container" && git-worktree-clone --layout contained "$remote_repo") || return 1
    local project_root="$container/test-repo-remove-no-arg"

    # CWD = project_root. After bare removal the cwd disappears, so we move
    # to a safe parent before checking — but daft must succeed regardless.
    cd "$project_root" || return 1
    daft repo remove --force || return 1

    cd "$container" || return 1
    if [[ -d "$project_root" ]]; then
        log_error "project_root not removed (no-arg form): $project_root"
        return 1
    fi
    if [[ ! -d "$container" ]]; then
        log_error "DATA LOSS via no-arg form: container removed: $container"
        return 1
    fi

    log_success "daft repo remove (no arg) works from inside project_root"
    return 0
}

# Test 10: Run from inside one of the worktrees. The command must resolve
# the bare via the worktree's git common dir, remove every worktree
# (including the one we're standing in), and leave the parent intact.
test_repo_remove_from_inside_worktree() {
    local remote_repo container
    remote_repo=$(create_test_remote "test-repo-remove-from-wt" "main")
    container=$(mktemp -d "${TMPDIR:-/tmp}/daft-from-wt.XXXXXX")
    trap 'rm -rf "$container"' RETURN

    (cd "$container" && git-worktree-clone --layout contained "$remote_repo") || return 1
    local project_root="$container/test-repo-remove-from-wt"
    local main_wt="$project_root/main"
    assert_directory_exists "$main_wt" || return 1

    cd "$main_wt" || return 1
    daft repo remove --force || return 1

    cd "$container" || return 1
    if [[ -d "$project_root" ]]; then
        log_error "project_root not removed (from-worktree): $project_root"
        return 1
    fi
    if [[ ! -d "$container" ]]; then
        log_error "DATA LOSS from-worktree: container removed: $container"
        return 1
    fi

    log_success "daft repo remove works when run from inside a worktree"
    return 0
}

# Test 11: Bare-only repo (no worktrees). The TUI is overkill for a single
# bare-removal task and previously rendered an empty-table-with-headers that
# looked like a glitch. The command now uses the sequential path when the
# worktree list is empty and emits a clear (bare): removed line.
test_repo_remove_bare_only_no_worktrees() {
    local remote_repo container
    remote_repo=$(create_test_remote "test-repo-remove-bare-only" "main")
    container=$(mktemp -d "${TMPDIR:-/tmp}/daft-bare-only.XXXXXX")
    trap 'rm -rf "$container"' RETURN

    # `--no-checkout` produces just the bare repo, no worktrees.
    (cd "$container" && git-worktree-clone --layout contained --no-checkout "$remote_repo") || return 1
    local project_root="$container/test-repo-remove-bare-only"
    assert_directory_exists "$project_root" || return 1
    assert_directory_exists "$project_root/.git" || return 1

    # Sanity: no worktree dirs siblings to .git.
    local sibling_count
    sibling_count=$(find "$project_root" -mindepth 1 -maxdepth 1 -not -name '.git' | wc -l | tr -d ' ')
    if [[ "$sibling_count" != "0" ]]; then
        log_error "test setup: expected no worktree siblings, found $sibling_count"
        return 1
    fi

    cd "$container" || return 1
    local out
    out=$(daft repo remove --force "$project_root" 2>&1) || return 1

    # Must NOT show the TUI status header for an empty worktree list.
    if echo "$out" | grep -qE '^Status[[:space:]]+Branch'; then
        log_error "Empty repo-remove must skip the TUI; got Status header in output:"
        log_error "$out"
        return 1
    fi
    # Must report bare removal explicitly.
    if ! echo "$out" | grep -q "(bare): removed"; then
        log_error "Expected '(bare): removed' line; got:"
        log_error "$out"
        return 1
    fi

    if [[ -d "$project_root" ]]; then
        log_error "project_root not removed: $project_root"
        return 1
    fi
    if [[ ! -d "$container" ]]; then
        log_error "DATA LOSS: container directory removed: $container"
        return 1
    fi

    log_success "daft repo remove handles bare-only repos with sequential output"
    return 0
}

# Run all repo-remove integration tests.
run_repo_remove_tests() {
    log "Running daft repo remove integration tests..."

    run_test "repo_remove_basic" "test_repo_remove_basic"
    run_test "repo_remove_runs_pre_remove_hooks" "test_repo_remove_runs_pre_remove_hooks"
    run_test "repo_remove_dry_run" "test_repo_remove_dry_run"
    run_test "repo_remove_non_git_path_fails" "test_repo_remove_non_git_path_fails"
    run_test "repo_remove_clean_repos_helper" "test_repo_remove_clean_repos_helper"
    run_test "repo_remove_from_non_repo_cwd" "test_repo_remove_from_non_repo_cwd"
    run_test "repo_remove_preserves_empty_parent_directory" "test_repo_remove_preserves_empty_parent_directory"
    run_test "repo_remove_relative_path_from_parent" "test_repo_remove_relative_path_from_parent"
    run_test "repo_remove_no_arg_from_inside_project_root" "test_repo_remove_no_arg_from_inside_project_root"
    run_test "repo_remove_from_inside_worktree" "test_repo_remove_from_inside_worktree"
    run_test "repo_remove_bare_only_no_worktrees" "test_repo_remove_bare_only_no_worktrees"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_repo_remove_tests
    print_summary
fi
