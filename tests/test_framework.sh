#!/bin/bash

# Git Worktree Workflow Test Framework
# Comprehensive testing suite for shell commands to ensure migration accuracy

set -eo pipefail
# Note: Removed -u flag to be more tolerant of unset variables in CI environments

# --- Configuration ---
TEST_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$TEST_DIR")"
SCRIPTS_DIR="$PROJECT_ROOT/scripts"
TEMP_BASE_DIR="/tmp/git-worktree-tests"
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

# Clean up function
cleanup() {
    if [[ -d "$TEMP_BASE_DIR" ]]; then
        log "Cleaning up test directory: $TEMP_BASE_DIR"
        rm -rf "$TEMP_BASE_DIR"
    fi
}

# Setup function
setup() {
    log "Setting up test environment..."
    
    # Clean up any previous test runs
    cleanup
    
    # Create test directories
    mkdir -p "$REMOTE_REPO_DIR"
    mkdir -p "$WORK_DIR"
    
    # Add scripts to PATH for testing
    export PATH="$SCRIPTS_DIR:$PATH"
    
    # Ensure git is configured
    git config --global user.name "Test User" 2>/dev/null || true
    git config --global user.email "test@example.com" 2>/dev/null || true
    
    log_success "Test environment setup complete"
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
    
    if (cd "$dir" && git rev-parse --is-inside-work-tree >/dev/null 2>&1 && [[ "$(git branch --show-current)" == "$branch" ]]); then
        log_success "$msg"
        return 0
    else
        log_error "$msg (FAILED - not a worktree or wrong branch)"
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

# Test runner function
run_test() {
    local test_name="$1"
    local test_function="$2"
    
    echo "DEBUG: run_test called with name='$test_name' function='$test_function'"
    echo "DEBUG: TESTS_RUN before increment: $TESTS_RUN"
    
    TESTS_RUN=$((TESTS_RUN + 1))
    echo "DEBUG: TESTS_RUN after increment: $TESTS_RUN"
    
    log "Running test: $test_name"
    
    # Create isolated test environment
    echo "DEBUG: Creating test work directory"
    local test_work_dir="$WORK_DIR/test_$(date +%s%N)"
    echo "DEBUG: Test work dir: $test_work_dir"
    mkdir -p "$test_work_dir"
    echo "DEBUG: Test work dir created"
    
    # Run test in subshell to isolate environment
    if (cd "$test_work_dir" && "$test_function" 2>&1); then
        log_success "Test passed: $test_name"
        TESTS_PASSED=$((TESTS_PASSED + 1))
        echo "DEBUG: TESTS_PASSED incremented to $TESTS_PASSED"
    else
        local exit_code=$?
        log_error "Test failed: $test_name (exit code: $exit_code)"
        TESTS_FAILED=$((TESTS_FAILED + 1))
        FAILED_TESTS+=("$test_name")
        echo "DEBUG: TESTS_FAILED incremented to $TESTS_FAILED"
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
    echo "=================================================="
    echo "Test Results Summary"
    echo "=================================================="
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
    
    echo "=================================================="
    
    if [[ $TESTS_FAILED -eq 0 ]]; then
        log_success "All tests passed!"
        return 0
    else
        log_error "Some tests failed!"
        return 1
    fi
}

# Trap to ensure cleanup on exit
trap cleanup EXIT