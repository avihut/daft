#!/bin/bash

# Git Worktree Workflow Integration Test Framework for Rust Binaries
# Tests the compiled Rust binaries as they would run in the real world

set -eo pipefail

# --- Configuration ---
TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$(dirname "$TEST_DIR")")"
RUST_BINARY_DIR="$PROJECT_ROOT/target/release"
TEMP_BASE_DIR="/tmp/git-worktree-integration-tests"
REMOTE_REPO_DIR="$TEMP_BASE_DIR/remote-repos"
WORK_DIR="$TEMP_BASE_DIR/work"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test statistics
TESTS_RUN=0
TESTS_PASSED=0
TESTS_FAILED=0
FAILED_TESTS=()

# --- Utility Functions ---
log() {
    echo -e "${BLUE}[$(date '+%H:%M:%S')]${NC} $*"
}

log_success() {
    echo -e "${GREEN}[✓]${NC} $*"
}

log_error() {
    echo -e "${RED}[✗]${NC} $*"
}

log_warning() {
    echo -e "${YELLOW}[!]${NC} $*"
}

# Build Rust binaries if they don't exist or are outdated
ensure_rust_binaries() {
    local cargo_toml="$PROJECT_ROOT/Cargo.toml"
    local daft_binary="$RUST_BINARY_DIR/daft"
    local need_build=false

    # Check if main binary is missing or outdated
    if [[ ! -f "$daft_binary" ]] || [[ "$cargo_toml" -nt "$daft_binary" ]]; then
        need_build=true
    fi

    if [[ "$need_build" == "true" ]]; then
        log "Building Rust binaries..."
        (cd "$PROJECT_ROOT" && cargo build --release) || {
            log_error "Failed to build Rust binaries"
            return 1
        }
        log_success "Rust binaries built successfully"
    else
        log "Rust binaries are up to date"
    fi

    # Create symlinks for the multicall binary (ensures tests use locally built binary)
    local symlink_names=("git-worktree-clone" "git-worktree-init" "git-worktree-checkout" "git-worktree-branch-delete" "git-worktree-prune" "git-worktree-carry" "git-worktree-fetch" "git-worktree-flow-adopt" "git-worktree-flow-eject" "git-worktree-list" "git-daft" "daft-remove" "daft-rename" "gwtclone" "gwtinit" "gwtco" "gwtcb" "gwtprune" "gwtcarry" "gwtfetch" "gwtbd" "gwtls")
    for name in "${symlink_names[@]}"; do
        if [[ ! -L "$RUST_BINARY_DIR/$name" ]]; then
            ln -sf daft "$RUST_BINARY_DIR/$name"
        fi
    done
}

# Clean up function
cleanup() {
    if [[ -d "$TEMP_BASE_DIR" ]]; then
        log "Cleaning up test directory: $TEMP_BASE_DIR"
        rm -rf "$TEMP_BASE_DIR"
    fi
}

# Setup function
setup() {
    log "Setting up Rust integration test environment..."
    
    # Ensure Rust binaries are built
    ensure_rust_binaries || exit 1
    
    # Clean up any previous test runs
    cleanup
    
    # Create test directories
    mkdir -p "$REMOTE_REPO_DIR"
    mkdir -p "$WORK_DIR"
    
    # Add Rust binaries to PATH for testing
    export PATH="$RUST_BINARY_DIR:$PATH"

    # Set git identity via environment variables (avoids modifying global config)
    export GIT_AUTHOR_NAME="Test User"
    export GIT_AUTHOR_EMAIL="test@example.com"
    export GIT_COMMITTER_NAME="Test User"
    export GIT_COMMITTER_EMAIL="test@example.com"

    # Isolate tests from user's global git config to prevent settings
    # like daft.experimental.gitoxide from leaking into tests.
    # When invoked via xtask test-matrix, GIT_CONFIG_GLOBAL is already set.
    if [[ -z "${GIT_CONFIG_GLOBAL:-}" ]]; then
        DAFT_TEST_GLOBAL_CONFIG="$TEMP_BASE_DIR/.gitconfig-test"
        touch "$DAFT_TEST_GLOBAL_CONFIG"
        export GIT_CONFIG_GLOBAL="$DAFT_TEST_GLOBAL_CONFIG"
    fi

    # Verify all binaries are available
    local binary_names=("git-worktree-clone" "git-worktree-checkout" "git-worktree-init" "git-worktree-prune")
    for binary in "${binary_names[@]}"; do
        if ! command -v "$binary" >/dev/null 2>&1; then
            log_error "Binary $binary not found in PATH"
            return 1
        fi
    done
    
    log_success "Rust integration test environment setup complete"
}

# Test assertion functions
assert_directory_exists() {
    local dir="$1"
    local msg="${2:-Directory should exist: $dir}"
    
    if [[ -d "$dir" ]]; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - directory not found)"
        return 1
    fi
}

assert_file_exists() {
    local file="$1"
    local msg="${2:-File should exist: $file}"

    if [[ -f "$file" ]]; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - file not found)"
        return 1
    fi
}

assert_file_not_exists() {
    local file="$1"
    local msg="${2:-File should not exist: $file}"

    if [[ ! -f "$file" ]]; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - file exists when it should not)"
        return 1
    fi
}

assert_file_contains() {
    local file="$1"
    local content="$2"
    local msg="${3:-File should contain: $content}"

    if [[ -f "$file" ]] && grep -q "$content" "$file"; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - content not found in file)"
        return 1
    fi
}

assert_git_repository() {
    local dir="$1"
    local msg="${2:-Should be a valid git repository: $dir}"
    
    if (cd "$dir" && git rev-parse --git-dir >/dev/null 2>&1); then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - not a git repository)"
        return 1
    fi
}

assert_git_worktree() {
    local dir="$1"
    local branch="$2"
    local msg="${3:-Should be a git worktree for branch '$branch': $dir}"
    
    # Debug: show current directory and target directory
    if [[ ! -d "$dir" ]]; then
        log_error "$msg (FAILED - directory '$dir' does not exist from '$(pwd)')"
        return 1
    fi
    
    if (cd "$dir" && git rev-parse --is-inside-work-tree >/dev/null 2>&1 && [[ "$(git branch --show-current)" == "$branch" ]]); then
        log_success "$msg"
        return 0
    else
        local current_branch=""
        if cd "$dir" >/dev/null 2>&1; then
            current_branch="$(git branch --show-current 2>/dev/null || echo 'unknown')"
        fi
        log_error "$msg (FAILED - not a worktree or wrong branch, current branch: '$current_branch')"
        return 1
    fi
}

assert_branch_exists() {
    local repo_dir="$1"
    local branch="$2"
    local msg="${3:-Branch '$branch' should exist in repository}"
    
    if (cd "$repo_dir" && git show-ref --verify --quiet "refs/heads/$branch"); then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - branch not found)"
        return 1
    fi
}

assert_remote_tracking() {
    local repo_dir="$1"
    local branch="$2"
    local remote="$3"
    local msg="${4:-Branch '$branch' should track '$remote/$branch'}"
    
    if (cd "$repo_dir" && git config "branch.$branch.remote" | grep -q "$remote" && git config "branch.$branch.merge" | grep -q "refs/heads/$branch"); then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - tracking not set up correctly)"
        return 1
    fi
}

assert_command_success() {
    local cmd="$1"
    local msg="${2:-Command should succeed: $cmd}"
    
    if eval "$cmd" >/dev/null 2>&1; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - command failed)"
        return 1
    fi
}

assert_command_failure() {
    local cmd="$1"
    local msg="${2:-Command should fail: $cmd}"
    
    if ! eval "$cmd" >/dev/null 2>&1; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - command succeeded when it should have failed)"
        return 1
    fi
}

# Test command help functionality
assert_command_help() {
    local cmd="$1"
    local msg="${2:-Command help should work: $cmd --help}"
    
    if "$cmd" --help >/dev/null 2>&1; then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - help command failed)"
        return 1
    fi
}

# Test command version if available
assert_command_version() {
    local cmd="$1"
    local msg="${2:-Command version should work: $cmd --version}"
    
    if "$cmd" --version >/dev/null 2>&1; then
        log_success "$msg"
        return 0
    else
        log_warning "$msg (version flag not available)"
        return 0  # Not an error, just not implemented
    fi
}

# Test runner function
run_test() {
    local test_name="$1"
    local test_function="$2"
    
    TESTS_RUN=$((TESTS_RUN + 1))
    
    log "Running integration test: $test_name"
    
    # Create isolated test environment
    local test_work_dir="$WORK_DIR/test_$(date +%s%N)"
    mkdir -p "$test_work_dir"
    
    # Run test in subshell to isolate environment
    if (cd "$test_work_dir" && "$test_function" 2>&1); then
        log_success "Integration test passed: $test_name"
        TESTS_PASSED=$((TESTS_PASSED + 1))
    else
        local exit_code=$?
        log_error "Integration test failed: $test_name (exit code: $exit_code)"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        FAILED_TESTS+=("$test_name")
    fi
    
    # Clean up test directory
    rm -rf "$test_work_dir"
}

# Create a test remote repository
create_test_remote() {
    local repo_name="$1"
    local default_branch="${2:-main}"
    local remote_dir="$REMOTE_REPO_DIR/$repo_name"
    
    # Create bare repository
    git init --bare "$remote_dir" >/dev/null 2>&1
    
    # Create a temporary clone to set up initial content
    local temp_clone="$TEMP_BASE_DIR/temp_clone_$$"
    git clone "$remote_dir" "$temp_clone" >/dev/null 2>&1
    
    (
        cd "$temp_clone"
        git checkout -b "$default_branch" >/dev/null 2>&1
        echo "# $repo_name" > README.md
        echo "print('Hello from $repo_name')" > main.py
        git add . >/dev/null 2>&1
        git commit -m "Initial commit" >/dev/null 2>&1
        git push origin "$default_branch" >/dev/null 2>&1
        
        # Create additional branches for testing
        git checkout -b develop >/dev/null 2>&1
        echo "# Development branch" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Add development branch" >/dev/null 2>&1
        git push origin develop >/dev/null 2>&1
        
        git checkout -b feature/test-feature >/dev/null 2>&1
        echo "# Feature branch" >> README.md
        git add README.md >/dev/null 2>&1
        git commit -m "Add feature branch" >/dev/null 2>&1
        git push origin feature/test-feature >/dev/null 2>&1
    ) >/dev/null 2>&1
    
    # Clean up temporary clone
    rm -rf "$temp_clone"
    
    # Set default branch
    git -C "$remote_dir" symbolic-ref HEAD "refs/heads/$default_branch" >/dev/null 2>&1
    
    echo "$remote_dir"
}

# Test results summary
print_summary() {
    echo
    echo "========================================================="
    echo "Rust Integration Test Results Summary"
    echo "========================================================="
    echo "Total tests run: $TESTS_RUN"
    echo "Passed: $TESTS_PASSED"
    echo "Failed: $TESTS_FAILED"
    
    if [[ ${#FAILED_TESTS[@]} -gt 0 ]]; then
        echo
        echo "Failed tests:"
        for test in "${FAILED_TESTS[@]}"; do
            echo "  - $test"
        done
    fi
    
    echo "========================================================="
    
    if [[ $TESTS_FAILED -eq 0 ]]; then
        log_success "All integration tests passed!"
        return 0
    else
        log_error "Some integration tests failed!"
        return 1
    fi
}

# Enable gitoxide for commands that use DaftSettings::load() (post-clone)
enable_gitoxide() {
    git config daft.experimental.gitoxide true
}

# Trap to ensure cleanup on exit
trap cleanup EXIT