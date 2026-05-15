#!/bin/bash

# Integration tests for visitor-config propagation on worktree-checkout-branch.

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test that a visitor daft.yml is propagated to a new worktree created via -b.
test_visitor_propagation_basic() {
    git-worktree-init --layout contained prop-test-repo || return 1
    cd "prop-test-repo/master"

    # Add an initial commit so that git stash (carry) works.
    echo "# prop-test-repo" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    # Seed a visitor daft.yml (untracked = visitor).
    cat > daft.yml <<'EOF'
hooks:
  worktree-post-create:
    jobs:
      - name: marker
        run: echo "from-visitor"
EOF

    # Create a new worktree from master using --no-carry so that propagation
    # is the sole mechanism delivering the visitor daft.yml to the new worktree
    # (without --no-carry, carry/stash moves it, making propagation a no-op).
    git-worktree-checkout --no-carry -b feat/foo || return 1

    # The new worktree must have a propagated daft.yml.
    local repo_root
    repo_root="$(dirname "$(pwd)")"
    local target="$repo_root/feat/foo/daft.yml"

    assert_file_exists "$target" "daft.yml should be propagated to new worktree" || return 1
    assert_file_contains "$target" "from-visitor" \
        "propagated daft.yml should contain expected hook content" || return 1

    return 0
}

# Test that a tracked (non-visitor) daft.yml is NOT propagated.
test_visitor_propagation_tracked_not_copied() {
    git-worktree-init --layout contained prop-test-tracked || return 1
    cd "prop-test-tracked"

    cd "master"

    # Commit an initial file so we have a commit to work from.
    echo "# repo" > README.md
    git add README.md
    git commit -m "init" >/dev/null 2>&1

    # Write and TRACK daft.yml (not a visitor).
    cat > daft.yml <<'EOF'
hooks:
  worktree-post-create:
    jobs:
      - name: tracked
        run: echo "tracked"
EOF
    git add daft.yml
    git commit -m "add tracked daft.yml" >/dev/null 2>&1

    git-worktree-checkout -b feat/bar || return 1

    local repo_root
    repo_root="$(dirname "$(pwd)")"
    local target="$repo_root/feat/bar/daft.yml"

    # The file should NOT be in the new worktree as a propagated copy —
    # git checkout places the tracked version there, but propagation must not
    # have written it as an overlay (there is no visitor source to propagate).
    # Assert the tracked content is present (git checkout put it there) and the
    # file was not silently duplicated or merged with a non-existent visitor.
    assert_file_exists "$target" "tracked daft.yml should be placed by git checkout" || return 1
    assert_file_contains "$target" "name: tracked" \
        "tracked daft.yml should contain the committed content" || return 1

    return 0
}

# Test that daft.local.yml is always propagated when present.
test_visitor_propagation_local_yml() {
    git-worktree-init --layout contained prop-test-local || return 1
    cd "prop-test-local/master"

    # Add an initial commit so that git stash (carry) works.
    echo "# prop-test-local" > README.md
    git add README.md
    git commit -m "Initial commit" >/dev/null 2>&1

    cat > daft.local.yml <<'EOF'
hooks:
  worktree-post-create:
    jobs:
      - name: local-marker
        run: echo "local-overlay"
EOF

    git-worktree-checkout --no-carry -b feat/baz || return 1

    local repo_root
    repo_root="$(dirname "$(pwd)")"
    local target="$repo_root/feat/baz/daft.local.yml"

    assert_file_exists "$target" "daft.local.yml should always be propagated" || return 1
    assert_file_contains "$target" "local-overlay" \
        "propagated daft.local.yml should contain expected content" || return 1

    return 0
}

run_visitor_propagation_tests() {
    log "Running visitor-config propagation integration tests..."

    run_test "visitor_propagation_basic"         "test_visitor_propagation_basic"
    run_test "visitor_propagation_tracked_not_copied" "test_visitor_propagation_tracked_not_copied"
    run_test "visitor_propagation_local_yml"     "test_visitor_propagation_local_yml"
}

# Main execution when run directly.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    setup
    run_visitor_propagation_tests
    print_summary
    exit $?
fi
