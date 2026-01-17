# Makefile for daft

# Variables
SCRIPTS_DIR := src/legacy
TESTS_DIR := tests
LEGACY_TESTS_DIR := tests/legacy
INTEGRATION_TESTS_DIR := tests/integration
TEMP_DIR := /tmp/git-worktree-tests
INTEGRATION_TEMP_DIR := /tmp/git-worktree-integration-tests

# Default target
.PHONY: all
all: test

# Test targets
.PHONY: test test-all test-legacy test-integration test-rust test-unit test-clone test-checkout test-checkout-branch test-checkout-branch-from-default test-init test-prune test-framework test-simple

test: test-all
	@echo "All tests completed"

test-all: test-unit test-legacy test-integration
	@echo "Running all tests (unit + legacy + integration)..."

test-legacy:
	@echo "Running legacy shell script tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_all.sh

test-integration: build-rust
	@echo "Running Rust integration tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_all.sh

test-rust: test-integration
	@echo "Rust integration tests completed"

# Legacy test targets
.PHONY: test-legacy-framework test-legacy-clone test-legacy-checkout test-legacy-checkout-branch test-legacy-checkout-branch-from-default test-legacy-init test-legacy-prune test-legacy-simple

test-framework: test-legacy-framework
	@echo "Running legacy test framework tests..."

test-legacy-framework:
	@echo "Running legacy test framework tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_framework.sh

test-clone: test-legacy-clone
	@echo "Running legacy clone tests..."

test-legacy-clone:
	@echo "Running legacy clone tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_clone.sh

test-checkout: test-legacy-checkout
	@echo "Running legacy checkout tests..."

test-legacy-checkout:
	@echo "Running legacy checkout tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_checkout.sh

test-checkout-branch: test-legacy-checkout-branch
	@echo "Running legacy checkout-branch tests..."

test-legacy-checkout-branch:
	@echo "Running legacy checkout-branch tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_checkout_branch.sh

test-checkout-branch-from-default: test-legacy-checkout-branch-from-default
	@echo "Running legacy checkout-branch-from-default tests..."

test-legacy-checkout-branch-from-default:
	@echo "Running legacy checkout-branch-from-default tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_checkout_branch_from_default.sh

test-init: test-legacy-init
	@echo "Running legacy init tests..."

test-legacy-init:
	@echo "Running legacy init tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_init.sh

test-prune: test-legacy-prune
	@echo "Running legacy prune tests..."

test-legacy-prune:
	@echo "Running legacy prune tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_prune.sh

test-simple: test-legacy-simple
	@echo "Running legacy simple validation tests..."

test-legacy-simple:
	@echo "Running legacy simple validation tests..."
	@cd $(LEGACY_TESTS_DIR) && ./test_simple.sh

# Integration test targets
.PHONY: test-integration-clone test-integration-checkout test-integration-checkout-branch test-integration-checkout-branch-from-default test-integration-init test-integration-prune

test-integration-clone: build-rust
	@echo "Running Rust integration clone tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_clone.sh

test-integration-checkout: build-rust
	@echo "Running Rust integration checkout tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_checkout.sh

test-integration-checkout-branch: build-rust
	@echo "Running Rust integration checkout-branch tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_checkout_branch.sh

test-integration-checkout-branch-from-default: build-rust
	@echo "Running Rust integration checkout-branch-from-default tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_checkout_branch_from_default.sh

test-integration-init: build-rust
	@echo "Running Rust integration init tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_init.sh

test-integration-prune: build-rust
	@echo "Running Rust integration prune tests..."
	@cd $(INTEGRATION_TESTS_DIR) && ./test_prune.sh

# Rust build targets
.PHONY: build-rust clean-rust test-unit

build-rust:
	@echo "Building Rust binaries..."
	@cargo build --release

clean-rust:
	@echo "Cleaning Rust build artifacts..."
	@cargo clean

test-unit:
	@echo "Running Rust unit tests..."
	@cargo test --lib

# Individual test runner with verbose output
.PHONY: test-verbose test-legacy-verbose test-integration-verbose
test-verbose: test-legacy-verbose test-integration-verbose
	@echo "All verbose tests completed"

test-legacy-verbose:
	@echo "Running legacy tests with verbose output..."
	@export VERBOSE=1 && cd $(LEGACY_TESTS_DIR) && ./test_all.sh

test-integration-verbose: build-rust
	@echo "Running integration tests with verbose output..."
	@export VERBOSE=1 && cd $(INTEGRATION_TESTS_DIR) && ./test_all.sh

# Clean up test artifacts
.PHONY: clean clean-tests
clean: clean-tests clean-rust dev-clean
	@echo "All artifacts cleaned"

clean-tests:
	@echo "Cleaning up test artifacts..."
	@rm -rf $(TEMP_DIR)
	@rm -rf $(INTEGRATION_TEMP_DIR)
	@echo "Test artifacts cleaned"

# Setup development environment
.PHONY: setup setup-legacy setup-rust
setup: setup-legacy setup-rust
	@echo "Development environment setup completed"

setup-legacy:
	@echo "Setting up legacy development environment..."
	@chmod +x $(SCRIPTS_DIR)/*
	@chmod +x $(LEGACY_TESTS_DIR)/*.sh
	@echo "Legacy scripts made executable"
	@echo "Add $(PWD)/$(SCRIPTS_DIR) to your PATH to use the legacy scripts"
	@echo "  export PATH=\"$(PWD)/$(SCRIPTS_DIR):\$$PATH\""

setup-rust: build-rust
	@echo "Setting up Rust development environment..."
	@chmod +x $(INTEGRATION_TESTS_DIR)/*.sh
	@echo "Creating symlinks in target/release/..."
	@cd target/release && \
		ln -sf daft git-worktree-clone && \
		ln -sf daft git-worktree-init && \
		ln -sf daft git-worktree-checkout && \
		ln -sf daft git-worktree-checkout-branch && \
		ln -sf daft git-worktree-checkout-branch-from-default && \
		ln -sf daft git-worktree-prune && \
		ln -sf daft git-worktree-carry && \
		ln -sf daft git-daft
	@echo "✓ Development environment ready!"
	@echo ""
	@echo "Binary size: $$(stat -f '%z' target/release/daft 2>/dev/null | awk '{printf "%.0f KB", $$1/1024}')"
	@echo ""
	@echo "Add to PATH for Git integration:"
	@echo "  export PATH=\"$(PWD)/target/release:\$$PATH\""
	@echo ""
	@echo "Quick test:"
	@echo "  ./target/release/daft"
	@echo "  ./target/release/git-worktree-clone --help"

# Development workflow targets
.PHONY: dev dev-setup dev-clean dev-test dev-verify

# Alias for setup-rust (clearer intent)
dev-setup: setup-rust

# Quick development cycle: setup + verify
dev: dev-setup dev-verify
	@echo "✓ Development environment ready and verified!"

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
		git-daft
	@echo "✓ Symlinks removed (binary preserved)"

# Verify dev setup is working
dev-verify:
	@echo "Verifying development setup..."
	@test -f target/release/daft || (echo "✗ Binary not found" && exit 1)
	@test -L target/release/git-worktree-clone || (echo "✗ Symlinks not created" && exit 1)
	@./target/release/daft >/dev/null 2>&1 || (echo "✗ Direct invocation failed" && exit 1)
	@./target/release/git-worktree-clone --help >/dev/null 2>&1 || (echo "✗ Symlink invocation failed" && exit 1)
	@echo "✓ All checks passed"

# Full dev test: setup + run all tests
dev-test: dev-setup test
	@echo "✓ Development setup tested successfully!"

# Watch targets (requires cargo-watch: cargo install cargo-watch)
.PHONY: watch watch-unit watch-clippy watch-check

watch: watch-unit

watch-unit:
	@if command -v cargo-watch >/dev/null 2>&1; then \
		cargo watch -c -x "test --lib"; \
	else \
		echo "cargo-watch not installed. Install with: cargo install cargo-watch"; \
		exit 1; \
	fi

watch-clippy:
	@if command -v cargo-watch >/dev/null 2>&1; then \
		cargo watch -c -x "clippy -- -D warnings" -x "test --lib"; \
	else \
		echo "cargo-watch not installed. Install with: cargo install cargo-watch"; \
		exit 1; \
	fi

watch-check:
	@if command -v cargo-watch >/dev/null 2>&1; then \
		cargo watch -c -x check; \
	else \
		echo "cargo-watch not installed. Install with: cargo install cargo-watch"; \
		exit 1; \
	fi

# Validate scripts (basic syntax check)
.PHONY: validate validate-legacy validate-rust
validate: validate-legacy validate-rust
	@echo "All validation completed"

validate-legacy:
	@echo "Validating legacy shell scripts..."
	@for script in $(SCRIPTS_DIR)/*; do \
		if [ -f "$$script" ]; then \
			echo "Validating $$script"; \
			bash -n "$$script" || exit 1; \
		fi; \
	done
	@for script in $(LEGACY_TESTS_DIR)/*.sh; do \
		if [ -f "$$script" ]; then \
			echo "Validating $$script"; \
			bash -n "$$script" || exit 1; \
		fi; \
	done
	@echo "Legacy scripts validated successfully"

validate-rust:
	@echo "Validating Rust code and integration tests..."
	@cargo check
	@for script in $(INTEGRATION_TESTS_DIR)/*.sh; do \
		if [ -f "$$script" ]; then \
			echo "Validating $$script"; \
			bash -n "$$script" || exit 1; \
		fi; \
	done
	@echo "Rust code and integration tests validated successfully"

# Run linting tools if available
.PHONY: lint lint-legacy lint-rust
lint: lint-legacy lint-rust
	@echo "All linting completed"

lint-legacy:
	@echo "Running shellcheck on legacy scripts (if available)..."
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck $(SCRIPTS_DIR)/* $(LEGACY_TESTS_DIR)/*.sh; \
	else \
		echo "shellcheck not available, skipping legacy lint..."; \
	fi

lint-rust:
	@echo "Running Rust linting..."
	@cargo clippy -- -D warnings
	@cargo fmt --check
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck $(INTEGRATION_TESTS_DIR)/*.sh; \
	else \
		echo "shellcheck not available, skipping integration test lint..."; \
	fi

# Performance tests
.PHONY: test-perf test-perf-legacy test-perf-integration
test-perf: test-perf-legacy test-perf-integration
	@echo "All performance tests completed"

test-perf-legacy:
	@echo "Running legacy performance tests..."
	@cd $(LEGACY_TESTS_DIR) && time ./test_all.sh

test-perf-integration: build-rust
	@echo "Running integration performance tests..."
	@cd $(INTEGRATION_TESTS_DIR) && time ./test_all.sh

# Test with different shells
.PHONY: test-bash test-zsh
test-bash:
	@echo "Testing with bash..."
	@bash -c "cd $(LEGACY_TESTS_DIR) && ./test_all.sh"

test-zsh:
	@echo "Testing with zsh..."
	@if command -v zsh >/dev/null 2>&1; then \
		zsh -c "cd $(LEGACY_TESTS_DIR) && ./test_all.sh"; \
	else \
		echo "zsh not available, skipping..."; \
	fi

# Shell completions
.PHONY: install-completions gen-completions-bash gen-completions-zsh gen-completions-fish test-completions

install-completions: build-rust
	@echo "Installing shell completions..."
	@./target/release/daft completions bash --install
	@./target/release/daft completions zsh --install
	@./target/release/daft completions fish --install
	@echo "✓ Completions installed successfully!"
	@echo ""
	@echo "Restart your shell or source your shell config file to enable completions"

gen-completions-bash: build-rust
	@echo "Generating bash completions..."
	@for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-checkout-branch-from-default git-worktree-prune; do \
		echo "  $$cmd"; \
		./target/release/daft completions bash --command="$$cmd" > /tmp/completion-$$cmd.bash; \
	done
	@echo "✓ Bash completions generated in /tmp/"

gen-completions-zsh: build-rust
	@echo "Generating zsh completions..."
	@for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-checkout-branch-from-default git-worktree-prune; do \
		echo "  $$cmd"; \
		./target/release/daft completions zsh --command="$$cmd" > /tmp/completion-_$$cmd.zsh; \
	done
	@echo "✓ Zsh completions generated in /tmp/"

gen-completions-fish: build-rust
	@echo "Generating fish completions..."
	@for cmd in git-worktree-clone git-worktree-init git-worktree-checkout git-worktree-checkout-branch git-worktree-checkout-branch-from-default git-worktree-prune; do \
		echo "  $$cmd"; \
		./target/release/daft completions fish --command="$$cmd" > /tmp/completion-$$cmd.fish; \
	done
	@echo "✓ Fish completions generated in /tmp/"

test-completions: build-rust
	@echo "Testing shell completions..."
	@if [ -f $(INTEGRATION_TESTS_DIR)/test_completions.sh ]; then \
		$(INTEGRATION_TESTS_DIR)/test_completions.sh; \
	else \
		echo "Completion tests not yet implemented"; \
	fi

# CI simulation
.PHONY: ci
ci: setup validate test
	@echo "CI simulation completed successfully"

# Help target
.PHONY: help
help:
	@echo "Available targets:"
	@echo ""
	@echo "Build targets:"
	@echo "  build-rust                    - Build Rust binaries"
	@echo "  clean-rust                    - Clean Rust build artifacts"
	@echo ""
	@echo "Development targets:"
	@echo "  dev                           - Quick setup + verify (recommended for dev)"
	@echo "  dev-setup                     - Build binary and create symlinks in target/release/"
	@echo "  dev-clean                     - Remove development symlinks"
	@echo "  dev-verify                    - Verify development setup is working"
	@echo "  dev-test                      - Full dev setup + run all tests"
	@echo ""
	@echo "Watch targets (requires: cargo install cargo-watch):"
	@echo "  watch                         - Watch and run unit tests on changes (alias for watch-unit)"
	@echo "  watch-unit                    - Watch and run unit tests on changes"
	@echo "  watch-clippy                  - Watch and run clippy + unit tests on changes"
	@echo "  watch-check                   - Watch and run cargo check on changes"
	@echo ""
	@echo "Test targets:"
	@echo "  all                           - Run all tests (default)"
	@echo "  test                          - Run all tests (legacy + integration)"
	@echo "  test-all                      - Run all tests (legacy + integration)"
	@echo "  test-legacy                   - Run legacy shell script tests"
	@echo "  test-integration              - Run Rust integration tests"
	@echo "  test-rust                     - Run Rust integration tests"
	@echo "  test-unit                     - Run Rust unit tests (cargo test)"
	@echo ""
	@echo "Legacy test targets:"
	@echo "  test-legacy-framework         - Run legacy test framework tests"
	@echo "  test-legacy-clone             - Run legacy clone tests"
	@echo "  test-legacy-checkout          - Run legacy checkout tests"
	@echo "  test-legacy-checkout-branch   - Run legacy checkout-branch tests"
	@echo "  test-legacy-checkout-branch-from-default - Run legacy checkout-branch-from-default tests"
	@echo "  test-legacy-init              - Run legacy init tests"
	@echo "  test-legacy-prune             - Run legacy prune tests"
	@echo "  test-legacy-simple            - Run legacy simple validation tests"
	@echo ""
	@echo "Integration test targets:"
	@echo "  test-integration-clone        - Run Rust integration clone tests"
	@echo "  test-integration-checkout     - Run Rust integration checkout tests"
	@echo "  test-integration-checkout-branch - Run Rust integration checkout-branch tests"
	@echo "  test-integration-checkout-branch-from-default - Run Rust integration checkout-branch-from-default tests"
	@echo "  test-integration-init         - Run Rust integration init tests"
	@echo "  test-integration-prune        - Run Rust integration prune tests"
	@echo ""
	@echo "Compatibility targets (legacy):"
	@echo "  test-framework                - Run legacy test framework tests"
	@echo "  test-clone                    - Run legacy clone tests"
	@echo "  test-checkout                 - Run legacy checkout tests"
	@echo "  test-checkout-branch          - Run legacy checkout-branch tests"
	@echo "  test-checkout-branch-from-default - Run legacy checkout-branch-from-default tests"
	@echo "  test-init                     - Run legacy init tests"
	@echo "  test-prune                    - Run legacy prune tests"
	@echo "  test-simple                   - Run legacy simple validation tests"
	@echo ""
	@echo "Other test targets:"
	@echo "  test-verbose                  - Run tests with verbose output"
	@echo "  test-legacy-verbose           - Run legacy tests with verbose output"
	@echo "  test-integration-verbose      - Run integration tests with verbose output"
	@echo "  test-perf                     - Run performance tests"
	@echo "  test-perf-legacy              - Run legacy performance tests"
	@echo "  test-perf-integration         - Run integration performance tests"
	@echo "  test-bash                     - Test with bash shell"
	@echo "  test-zsh                      - Test with zsh shell"
	@echo ""
	@echo "Setup and validation:"
	@echo "  setup                         - Setup development environment"
	@echo "  setup-legacy                  - Setup legacy development environment"
	@echo "  setup-rust                    - Setup Rust development environment"
	@echo "  validate                      - Validate all code"
	@echo "  validate-legacy               - Validate legacy shell script syntax"
	@echo "  validate-rust                 - Validate Rust code and integration tests"
	@echo "  lint                          - Run all linting tools"
	@echo "  lint-legacy                   - Run shellcheck on legacy scripts"
	@echo "  lint-rust                     - Run Rust linting (clippy + fmt)"
	@echo ""
	@echo "Shell completions:"
	@echo "  install-completions           - Install shell completions for bash/zsh/fish"
	@echo "  gen-completions-bash          - Generate bash completions to /tmp/"
	@echo "  gen-completions-zsh           - Generate zsh completions to /tmp/"
	@echo "  gen-completions-fish          - Generate fish completions to /tmp/"
	@echo "  test-completions              - Test completion generation"
	@echo ""
	@echo "Cleanup:"
	@echo "  clean                         - Clean up all artifacts"
	@echo "  clean-tests                   - Clean up test artifacts"
	@echo "  clean-rust                    - Clean up Rust build artifacts"
	@echo ""
	@echo "CI/CD:"
	@echo "  ci                            - Run CI simulation"
	@echo "  help                          - Show this help message"