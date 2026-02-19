#!/bin/bash

# Integration tests for --exec (-x) option across worktree commands

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test single -x command runs in init worktree directory
test_exec_init_single() {
    git-worktree-init -x 'pwd > exec_output.txt' exec-init-repo || return 1

    # We should be in the worktree now
    cd exec-init-repo/master || return 1
    assert_file_exists "exec_output.txt" "exec command should have created output file" || return 1

    # The pwd output should contain the worktree path
    local exec_pwd
    exec_pwd=$(cat exec_output.txt)
    if [[ "$exec_pwd" != *"exec-init-repo/master"* ]]; then
        log_error "exec command ran in wrong directory: $exec_pwd"
        return 1
    fi
    log_success "Single -x command ran in correct worktree directory"

    return 0
}

# Test multiple -x commands run in order
test_exec_init_multiple() {
    git-worktree-init -x 'echo first > order.txt' -x 'echo second >> order.txt' exec-multi-repo || return 1

    cd exec-multi-repo/master || return 1
    assert_file_exists "order.txt" "exec commands should have created output file" || return 1

    local content
    content=$(cat order.txt)
    if [[ "$content" != "first
second" ]]; then
        log_error "exec commands ran out of order or incorrectly: $content"
        return 1
    fi
    log_success "Multiple -x commands ran in correct order"

    return 0
}

# Test -x with failing command stops and propagates error
test_exec_failing_command() {
    # The command should fail because 'false' returns exit code 1
    if git-worktree-init -x 'echo before > marker.txt' -x 'false' -x 'echo after >> marker.txt' exec-fail-repo 2>/dev/null; then
        log_error "Command should have failed due to 'false'"
        return 1
    fi
    log_success "Failing -x command propagated error"

    # The second command (false) should have failed, so 'after' should NOT be in marker.txt
    cd exec-fail-repo/master || return 1
    assert_file_exists "marker.txt" "First exec command should have run" || return 1

    if grep -q "after" marker.txt; then
        log_error "Commands after the failing one should not have run"
        return 1
    fi
    log_success "Commands after failure were not executed"

    return 0
}

# Test -x with clone
test_exec_clone_single() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-exec-clone" "main")

    git-worktree-clone "$remote_repo" -x 'pwd > exec_output.txt' || return 1

    cd test-repo-exec-clone/main || return 1
    assert_file_exists "exec_output.txt" "exec command should have created output file in clone worktree" || return 1

    local exec_pwd
    exec_pwd=$(cat exec_output.txt)
    if [[ "$exec_pwd" != *"test-repo-exec-clone/main"* ]]; then
        log_error "exec command ran in wrong directory: $exec_pwd"
        return 1
    fi
    log_success "Single -x command ran in clone worktree directory"

    return 0
}

# Test -x with checkout
test_exec_checkout_single() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-exec-co" "main")

    # First clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd test-repo-exec-co || return 1

    # Checkout with exec
    git-worktree-checkout develop -x 'pwd > exec_output.txt' || return 1

    cd develop || return 1
    assert_file_exists "exec_output.txt" "exec command should have created output file in checkout worktree" || return 1

    local exec_pwd
    exec_pwd=$(cat exec_output.txt)
    if [[ "$exec_pwd" != *"test-repo-exec-co/develop"* ]]; then
        log_error "exec command ran in wrong directory: $exec_pwd"
        return 1
    fi
    log_success "Single -x command ran in checkout worktree directory"

    return 0
}

# Test -x with checkout-branch
test_exec_checkout_branch_single() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-exec-cb" "main")

    # First clone the repository
    git-worktree-clone "$remote_repo" || return 1
    cd test-repo-exec-cb/main || return 1

    # Checkout-branch with exec
    git-worktree-checkout-branch my-new-branch -x 'pwd > exec_output.txt' || return 1

    cd ../my-new-branch || return 1
    assert_file_exists "exec_output.txt" "exec command should have created output file in new branch worktree" || return 1

    local exec_pwd
    exec_pwd=$(cat exec_output.txt)
    if [[ "$exec_pwd" != *"test-repo-exec-cb/my-new-branch"* ]]; then
        log_error "exec command ran in wrong directory: $exec_pwd"
        return 1
    fi
    log_success "Single -x command ran in checkout-branch worktree directory"

    return 0
}

# Test -x runs for checkout of existing worktree
test_exec_checkout_existing_worktree() {
    local remote_repo
    remote_repo=$(create_test_remote "test-repo-exec-existing" "main")

    # Clone and create a worktree for develop
    git-worktree-clone "$remote_repo" || return 1
    cd test-repo-exec-existing || return 1
    git-worktree-checkout develop || return 1

    # Now checkout develop again (existing worktree) with -x
    cd main || return 1
    git-worktree-checkout develop -x 'echo ran > exec_marker.txt' || return 1

    cd ../develop || return 1
    assert_file_exists "exec_marker.txt" "exec command should run even for existing worktree checkout" || return 1

    local content
    content=$(cat exec_marker.txt)
    if [[ "$content" != "ran" ]]; then
        log_error "exec command content wrong: $content"
        return 1
    fi
    log_success "-x runs for checkout of existing worktree"

    return 0
}

# Test -x with no commands (empty) works fine
test_exec_no_commands() {
    git-worktree-init exec-no-cmd-repo || return 1

    assert_directory_exists "exec-no-cmd-repo/master" || return 1
    assert_git_worktree "exec-no-cmd-repo/master" "master" || return 1
    log_success "No -x commands works normally"

    return 0
}

# --- Test runner ---
run_exec_tests() {
    run_test "exec_init_single" test_exec_init_single
    run_test "exec_init_multiple" test_exec_init_multiple
    run_test "exec_failing_command" test_exec_failing_command
    run_test "exec_clone_single" test_exec_clone_single
    run_test "exec_checkout_single" test_exec_checkout_single
    run_test "exec_checkout_branch_single" test_exec_checkout_branch_single
    run_test "exec_checkout_existing_worktree" test_exec_checkout_existing_worktree
    run_test "exec_no_commands" test_exec_no_commands
}

# Main execution (when run directly)
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_exec_tests
    print_summary
    exit $?
fi
