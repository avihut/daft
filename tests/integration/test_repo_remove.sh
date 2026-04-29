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

# Run all repo-remove integration tests.
run_repo_remove_tests() {
    log "Running daft repo remove integration tests..."

    run_test "repo_remove_basic" "test_repo_remove_basic"
    run_test "repo_remove_runs_pre_remove_hooks" "test_repo_remove_runs_pre_remove_hooks"
    run_test "repo_remove_dry_run" "test_repo_remove_dry_run"
    run_test "repo_remove_non_git_path_fails" "test_repo_remove_non_git_path_fails"
}

# Main execution
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_repo_remove_tests
    print_summary
fi
