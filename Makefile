# Makefile for git-worktree-workflow

# Variables
SCRIPTS_DIR := scripts
TESTS_DIR := tests
TEMP_DIR := /tmp/git-worktree-tests

# Default target
.PHONY: all
all: test

# Test targets
.PHONY: test test-all test-clone test-checkout test-checkout-branch test-checkout-branch-from-default test-init test-prune test-framework

test: test-all
	@echo "All tests completed"

test-all:
	@echo "Running all tests..."
	@cd $(TESTS_DIR) && ./test_all.sh

test-framework:
	@echo "Running test framework tests..."
	@cd $(TESTS_DIR) && ./test_framework.sh

test-clone:
	@echo "Running clone tests..."
	@cd $(TESTS_DIR) && ./test_clone.sh

test-checkout:
	@echo "Running checkout tests..."
	@cd $(TESTS_DIR) && ./test_checkout.sh

test-checkout-branch:
	@echo "Running checkout-branch tests..."
	@cd $(TESTS_DIR) && ./test_checkout_branch.sh

test-checkout-branch-from-default:
	@echo "Running checkout-branch-from-default tests..."
	@cd $(TESTS_DIR) && ./test_checkout_branch_from_default.sh

test-init:
	@echo "Running init tests..."
	@cd $(TESTS_DIR) && ./test_init.sh

test-prune:
	@echo "Running prune tests..."
	@cd $(TESTS_DIR) && ./test_prune.sh

# Individual test runner with verbose output
.PHONY: test-verbose
test-verbose:
	@echo "Running tests with verbose output..."
	@export VERBOSE=1 && cd $(TESTS_DIR) && ./test_all.sh

# Clean up test artifacts
.PHONY: clean
clean:
	@echo "Cleaning up test artifacts..."
	@rm -rf $(TEMP_DIR)
	@echo "Test artifacts cleaned"

# Setup development environment
.PHONY: setup
setup:
	@echo "Setting up development environment..."
	@chmod +x $(SCRIPTS_DIR)/*
	@chmod +x $(TESTS_DIR)/*.sh
	@echo "Scripts made executable"
	@echo "Add $(PWD)/$(SCRIPTS_DIR) to your PATH to use the scripts"
	@echo "  export PATH=\"$(PWD)/$(SCRIPTS_DIR):\$$PATH\""

# Validate scripts (basic syntax check)
.PHONY: validate
validate:
	@echo "Validating shell scripts..."
	@for script in $(SCRIPTS_DIR)/*; do \
		if [ -f "$$script" ]; then \
			echo "Validating $$script"; \
			bash -n "$$script" || exit 1; \
		fi; \
	done
	@for script in $(TESTS_DIR)/*.sh; do \
		if [ -f "$$script" ]; then \
			echo "Validating $$script"; \
			bash -n "$$script" || exit 1; \
		fi; \
	done
	@echo "All scripts validated successfully"

# Run shellcheck if available
.PHONY: lint
lint:
	@echo "Running shellcheck (if available)..."
	@if command -v shellcheck >/dev/null 2>&1; then \
		shellcheck $(SCRIPTS_DIR)/* $(TESTS_DIR)/*.sh; \
	else \
		echo "shellcheck not available, skipping..."; \
	fi

# Performance tests
.PHONY: test-perf
test-perf:
	@echo "Running performance tests..."
	@cd $(TESTS_DIR) && time ./test_all.sh

# Test with different shells
.PHONY: test-bash test-zsh
test-bash:
	@echo "Testing with bash..."
	@bash -c "cd $(TESTS_DIR) && ./test_all.sh"

test-zsh:
	@echo "Testing with zsh..."
	@if command -v zsh >/dev/null 2>&1; then \
		zsh -c "cd $(TESTS_DIR) && ./test_all.sh"; \
	else \
		echo "zsh not available, skipping..."; \
	fi

# CI simulation
.PHONY: ci
ci: setup validate test
	@echo "CI simulation completed successfully"

# Help target
.PHONY: help
help:
	@echo "Available targets:"
	@echo "  all                           - Run all tests (default)"
	@echo "  test                          - Run all tests"
	@echo "  test-all                      - Run all tests"
	@echo "  test-framework                - Run test framework tests"
	@echo "  test-clone                    - Run clone tests"
	@echo "  test-checkout                 - Run checkout tests"
	@echo "  test-checkout-branch          - Run checkout-branch tests"
	@echo "  test-checkout-branch-from-default - Run checkout-branch-from-default tests"
	@echo "  test-init                     - Run init tests"
	@echo "  test-prune                    - Run prune tests"
	@echo "  test-verbose                  - Run tests with verbose output"
	@echo "  test-perf                     - Run performance tests"
	@echo "  test-bash                     - Test with bash shell"
	@echo "  test-zsh                      - Test with zsh shell"
	@echo "  setup                         - Setup development environment"
	@echo "  validate                      - Validate shell script syntax"
	@echo "  lint                          - Run shellcheck (if available)"
	@echo "  clean                         - Clean up test artifacts"
	@echo "  ci                            - Run CI simulation"
	@echo "  help                          - Show this help message"