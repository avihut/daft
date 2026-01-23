# justfile for daft
# Run `just --list` to see available recipes

# Variables
tests_dir := "tests"
integration_tests_dir := "tests/integration"
integration_temp_dir := "/tmp/git-worktree-integration-tests"

# Default recipe
default: test

# ============================================================================
# Aliases
# ============================================================================

alias t := test
alias b := build
alias l := lint

# ============================================================================
# Build Recipes
# ============================================================================

# Build Rust binaries (release mode)
build:
    @echo "Building Rust binaries..."
    cargo build --release

# Alias for build (backwards compatibility with Makefile)
build-rust: build

# ============================================================================
# Development Recipes
# ============================================================================

# Quick development cycle: setup + verify (recommended for dev)
dev: dev-setup dev-verify
    @echo "Development environment ready and verified!"

# Build binary and create symlinks in target/release/
dev-setup: setup-rust

# Remove development symlinks (keeps binary)
dev-clean:
    @echo "Cleaning development symlinks..."
    @cd target/release && rm -f \
        git-worktree-clone \
        git-worktree-init \
        git-worktree-checkout \
        git-worktree-checkout-branch \
        git-worktree-checkout-branch-from-default \
        git-worktree-prune \
        git-worktree-carry \
        git-worktree-fetch \
        git-daft \
        gwtclone gwtinit gwtco gwtcb gwtcbm gwtprune gwtcarry gwtfetch \
        gwco gwcob gwcobd \
        gclone gcw gcbw gcbdw gprune
    @echo "Symlinks removed (binary preserved)"

# Verify dev setup is working
dev-verify:
    @echo "Verifying development setup..."
    @test -f target/release/daft || (echo "Binary not found" && exit 1)
    @test -L target/release/git-worktree-clone || (echo "Symlinks not created" && exit 1)
    @./target/release/daft >/dev/null 2>&1 || (echo "Direct invocation failed" && exit 1)
    @./target/release/git-worktree-clone --help >/dev/null 2>&1 || (echo "Symlink invocation failed" && exit 1)
    @echo "All checks passed"

# Full dev test: setup + run all tests
dev-test: dev-setup test
    @echo "Development setup tested successfully!"

# ============================================================================
# Test Recipes
# ============================================================================

# Run all tests (default)
test: test-all
    @echo "All tests completed"

# Run all tests (unit + integration)
test-all: test-unit test-integration
    @echo "Running all tests (unit + integration)..."

# Run Rust unit tests
test-unit:
    @echo "Running Rust unit tests..."
    cargo test --lib

# Run Rust integration tests (alias)
test-rust: test-integration
    @echo "Rust integration tests completed"

# Run Rust integration tests
test-integration: build
    @echo "Running Rust integration tests..."
    @cd {{integration_tests_dir}} && ./test_all.sh

# ============================================================================
# Individual Integration Test Recipes
# ============================================================================

# Run integration clone tests
test-integration-clone: build
    @echo "Running Rust integration clone tests..."
    @cd {{integration_tests_dir}} && ./test_clone.sh

# Run integration checkout tests
test-integration-checkout: build
    @echo "Running Rust integration checkout tests..."
    @cd {{integration_tests_dir}} && ./test_checkout.sh

# Run integration checkout-branch tests
test-integration-checkout-branch: build
    @echo "Running Rust integration checkout-branch tests..."
    @cd {{integration_tests_dir}} && ./test_checkout_branch.sh

# Run integration checkout-branch-from-default tests
test-integration-checkout-branch-from-default: build
    @echo "Running Rust integration checkout-branch-from-default tests..."
    @cd {{integration_tests_dir}} && ./test_checkout_branch_from_default.sh

# Run integration init tests
test-integration-init: build
    @echo "Running Rust integration init tests..."
    @cd {{integration_tests_dir}} && ./test_init.sh

# Run integration prune tests
test-integration-prune: build
    @echo "Running Rust integration prune tests..."
    @cd {{integration_tests_dir}} && ./test_prune.sh

# Run integration shell-init tests
test-integration-shell-init: build
    @echo "Running Rust integration shell-init tests..."
    @cd {{integration_tests_dir}} && ./test_shell_init.sh

# Run integration setup tests
test-integration-setup: build
    @echo "Running Rust integration setup tests..."
    @cd {{integration_tests_dir}} && ./test_setup.sh

# Run integration config tests
test-integration-config: build
    @echo "Running Rust integration config tests..."
    @cd {{integration_tests_dir}} && ./test_config.sh

# Run integration hooks tests
test-integration-hooks: build
    @echo "Running Rust integration hooks tests..."
    @cd {{integration_tests_dir}} && ./test_hooks.sh

# Run integration fetch tests
test-integration-fetch: build
    @echo "Running Rust integration fetch tests..."
    @cd {{integration_tests_dir}} && ./test_fetch.sh

# ============================================================================
# Verbose Test Recipes
# ============================================================================

# Run tests with verbose output
test-verbose: test-integration-verbose
    @echo "All verbose tests completed"

# Run integration tests with verbose output
test-integration-verbose: build
    @echo "Running integration tests with verbose output..."
    @VERBOSE=1 && cd {{integration_tests_dir}} && ./test_all.sh

# ============================================================================
# Performance Test Recipes
# ============================================================================

# Run performance tests
test-perf: test-perf-integration
    @echo "All performance tests completed"

# Run integration performance tests
test-perf-integration: build
    @echo "Running integration performance tests..."
    @cd {{integration_tests_dir}} && time ./test_all.sh

# ============================================================================
# Setup Recipes
# ============================================================================

# Setup development environment
setup: setup-rust
    @echo "Development environment setup completed"

# Setup Rust development environment
setup-rust: build
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Setting up Rust development environment..."
    chmod +x {{integration_tests_dir}}/*.sh
    echo "Creating symlinks in target/release/..."
    cd target/release
    # Core command symlinks
    ln -sf daft git-worktree-clone
    ln -sf daft git-worktree-init
    ln -sf daft git-worktree-checkout
    ln -sf daft git-worktree-checkout-branch
    ln -sf daft git-worktree-checkout-branch-from-default
    ln -sf daft git-worktree-prune
    ln -sf daft git-worktree-carry
    ln -sf daft git-worktree-fetch
    ln -sf daft git-daft
    # Git-style shortcuts (default for development)
    ln -sf daft gwtclone
    ln -sf daft gwtinit
    ln -sf daft gwtco
    ln -sf daft gwtcb
    ln -sf daft gwtcbm
    ln -sf daft gwtprune
    ln -sf daft gwtcarry
    ln -sf daft gwtfetch
    cd ../..
    echo "Development environment ready!"
    echo ""
    # Cross-platform binary size display
    if [[ "$OSTYPE" == "darwin"* ]]; then
        size=$(stat -f '%z' target/release/daft 2>/dev/null || echo "0")
    else
        size=$(stat -c '%s' target/release/daft 2>/dev/null || echo "0")
    fi
    echo "Binary size: $(awk "BEGIN {printf \"%.0f KB\", $size/1024}")"
    echo ""
    echo "Add to PATH for Git integration:"
    echo "  export PATH=\"$PWD/target/release:\$PATH\""
    echo ""
    echo "Quick test:"
    echo "  ./target/release/daft"
    echo "  ./target/release/git-worktree-clone --help"
    echo "  ./target/release/gwtco --help  # Git-style shortcut"

# ============================================================================
# Validation Recipes
# ============================================================================

# Validate all code
validate: validate-rust
    @echo "All validation completed"

# Validate Rust code and integration tests
validate-rust:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Validating Rust code and integration tests..."
    cargo check
    for script in {{integration_tests_dir}}/*.sh; do
        if [ -f "$script" ]; then
            echo "Validating $script"
            bash -n "$script" || exit 1
        fi
    done
    echo "Rust code and integration tests validated successfully"

# ============================================================================
# Lint Recipes
# ============================================================================

# Run all linting tools
lint: lint-rust
    @echo "All linting completed"

# Run Rust linting (clippy + fmt + shellcheck)
lint-rust:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Running Rust linting..."
    cargo clippy -- -D warnings
    cargo fmt --check
    if command -v shellcheck >/dev/null 2>&1; then
        shellcheck {{integration_tests_dir}}/*.sh
    else
        echo "shellcheck not available, skipping integration test lint..."
    fi

# ============================================================================
# Watch Recipes (requires: cargo install cargo-watch)
# ============================================================================

# Watch and run unit tests on changes (alias for watch-unit)
watch: watch-unit

# Watch and run unit tests on changes
watch-unit:
    #!/usr/bin/env bash
    if command -v cargo-watch >/dev/null 2>&1; then
        cargo watch -c -x "test --lib"
    else
        echo "cargo-watch not installed. Install with: cargo install cargo-watch"
        exit 1
    fi

# Watch and run clippy + unit tests on changes
watch-clippy:
    #!/usr/bin/env bash
    if command -v cargo-watch >/dev/null 2>&1; then
        cargo watch -c -x "clippy -- -D warnings" -x "test --lib"
    else
        echo "cargo-watch not installed. Install with: cargo install cargo-watch"
        exit 1
    fi

# Watch and run cargo check on changes
watch-check:
    #!/usr/bin/env bash
    if command -v cargo-watch >/dev/null 2>&1; then
        cargo watch -c -x check
    else
        echo "cargo-watch not installed. Install with: cargo install cargo-watch"
        exit 1
    fi

# ============================================================================
# Shell Completion Recipes
# ============================================================================

# Install shell completions for bash/zsh/fish
install-completions: build
    @echo "Installing shell completions..."
    @./target/release/daft completions bash --install
    @./target/release/daft completions zsh --install
    @./target/release/daft completions fish --install
    @echo "Completions installed successfully!"
    @echo ""
    @echo "Restart your shell or source your shell config file to enable completions"

# Generate bash completions to /tmp/
gen-completions-bash: build
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Generating bash completions..."
    for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-checkout-branch-from-default git-worktree-prune; do
        echo "  $cmd"
        ./target/release/daft completions bash --command="$cmd" > /tmp/completion-$cmd.bash
    done
    echo "Bash completions generated in /tmp/"

# Generate zsh completions to /tmp/
gen-completions-zsh: build
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Generating zsh completions..."
    for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-checkout-branch-from-default git-worktree-prune; do
        echo "  $cmd"
        ./target/release/daft completions zsh --command="$cmd" > /tmp/completion-_$cmd.zsh
    done
    echo "Zsh completions generated in /tmp/"

# Generate fish completions to /tmp/
gen-completions-fish: build
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Generating fish completions..."
    for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-checkout-branch-from-default git-worktree-prune; do
        echo "  $cmd"
        ./target/release/daft completions fish --command="$cmd" > /tmp/completion-$cmd.fish
    done
    echo "Fish completions generated in /tmp/"

# Test shell completions
test-completions: build
    #!/usr/bin/env bash
    echo "Testing shell completions..."
    if [ -f {{integration_tests_dir}}/test_completions.sh ]; then
        {{integration_tests_dir}}/test_completions.sh
    else
        echo "Completion tests not yet implemented"
    fi

# ============================================================================
# Man Page Recipes
# ============================================================================

# Generate man pages to man/ directory
gen-man: build
    @echo "Generating man pages..."
    @mkdir -p man
    @./target/release/daft man --output-dir=man
    @echo "Man pages generated in man/"

# Install man pages to system location
install-man: build
    @echo "Installing man pages..."
    @./target/release/daft man --install
    @echo ""
    @echo "Note: You may need to run 'mandb' or restart your shell for man to find the new pages"

# ============================================================================
# Cleanup Recipes
# ============================================================================

# Clean up all artifacts
clean: clean-tests clean-rust dev-clean
    @echo "All artifacts cleaned"

# Clean up test artifacts
clean-tests:
    @echo "Cleaning up test artifacts..."
    @rm -rf {{integration_temp_dir}}
    @echo "Test artifacts cleaned"

# Clean Rust build artifacts
clean-rust:
    @echo "Cleaning Rust build artifacts..."
    cargo clean

# ============================================================================
# CI Recipes
# ============================================================================

# Run CI simulation
ci: setup validate test
    @echo "CI simulation completed successfully"

# ============================================================================
# Help
# ============================================================================

# Show available recipes
help:
    @echo "Available recipes:"
    @echo ""
    @echo "Build recipes:"
    @echo "  build                             - Build Rust binaries"
    @echo "  clean-rust                        - Clean Rust build artifacts"
    @echo ""
    @echo "Development recipes:"
    @echo "  dev                               - Quick setup + verify (recommended for dev)"
    @echo "  dev-setup                         - Build binary and create symlinks in target/release/"
    @echo "  dev-clean                         - Remove development symlinks"
    @echo "  dev-verify                        - Verify development setup is working"
    @echo "  dev-test                          - Full dev setup + run all tests"
    @echo ""
    @echo "Watch recipes (requires: cargo install cargo-watch):"
    @echo "  watch                             - Watch and run unit tests on changes (alias for watch-unit)"
    @echo "  watch-unit                        - Watch and run unit tests on changes"
    @echo "  watch-clippy                      - Watch and run clippy + unit tests on changes"
    @echo "  watch-check                       - Watch and run cargo check on changes"
    @echo ""
    @echo "Test recipes:"
    @echo "  test                              - Run all tests (default)"
    @echo "  test-all                          - Run all tests (unit + integration)"
    @echo "  test-unit                         - Run Rust unit tests (cargo test)"
    @echo "  test-integration                  - Run Rust integration tests"
    @echo "  test-rust                         - Run Rust integration tests"
    @echo ""
    @echo "Integration test recipes:"
    @echo "  test-integration-clone            - Run Rust integration clone tests"
    @echo "  test-integration-checkout         - Run Rust integration checkout tests"
    @echo "  test-integration-checkout-branch  - Run Rust integration checkout-branch tests"
    @echo "  test-integration-checkout-branch-from-default - Run Rust integration checkout-branch-from-default tests"
    @echo "  test-integration-init             - Run Rust integration init tests"
    @echo "  test-integration-prune            - Run Rust integration prune tests"
    @echo "  test-integration-shell-init       - Run Rust integration shell-init tests"
    @echo "  test-integration-config           - Run Rust integration config tests"
    @echo "  test-integration-hooks            - Run Rust integration hooks tests"
    @echo "  test-integration-fetch            - Run Rust integration fetch tests"
    @echo ""
    @echo "Other test recipes:"
    @echo "  test-verbose                      - Run tests with verbose output"
    @echo "  test-integration-verbose          - Run integration tests with verbose output"
    @echo "  test-perf                         - Run performance tests"
    @echo "  test-perf-integration             - Run integration performance tests"
    @echo ""
    @echo "Setup and validation:"
    @echo "  setup                             - Setup development environment"
    @echo "  setup-rust                        - Setup Rust development environment"
    @echo "  validate                          - Validate all code"
    @echo "  validate-rust                     - Validate Rust code and integration tests"
    @echo "  lint                              - Run all linting tools"
    @echo "  lint-rust                         - Run Rust linting (clippy + fmt)"
    @echo ""
    @echo "Shell completions:"
    @echo "  install-completions               - Install shell completions for bash/zsh/fish"
    @echo "  gen-completions-bash              - Generate bash completions to /tmp/"
    @echo "  gen-completions-zsh               - Generate zsh completions to /tmp/"
    @echo "  gen-completions-fish              - Generate fish completions to /tmp/"
    @echo "  test-completions                  - Test completion generation"
    @echo ""
    @echo "Man pages:"
    @echo "  gen-man                           - Generate man pages to man/"
    @echo "  install-man                       - Install man pages to system location"
    @echo ""
    @echo "Cleanup:"
    @echo "  clean                             - Clean up all artifacts"
    @echo "  clean-tests                       - Clean up test artifacts"
    @echo "  clean-rust                        - Clean up Rust build artifacts"
    @echo ""
    @echo "CI/CD:"
    @echo "  ci                                - Run CI simulation"
    @echo "  help                              - Show this help message"
    @echo ""
    @echo "Aliases:"
    @echo "  t                                 - Alias for test"
    @echo "  b                                 - Alias for build"
    @echo "  l                                 - Alias for lint"
