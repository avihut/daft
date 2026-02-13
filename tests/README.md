# Git Worktree Workflow Test Suite

This directory contains comprehensive integration tests for the daft toolkit's
Rust binary implementation.

## Test Structure

```
tests/
├── integration/         # Rust integration tests
│   ├── test_all.sh      # Master test runner for integration tests
│   ├── test_framework.sh # Test framework for integration tests
│   ├── test_clone.sh    # Integration tests for git-worktree-clone
│   ├── test_init.sh     # Integration tests for git-worktree-init
│   ├── test_checkout.sh # Integration tests for git-worktree-checkout
│   ├── test_checkout_branch.sh # Integration tests for git-worktree-checkout-branch
│   ├── test_checkout_branch_from_default.sh # Integration tests for git-worktree-checkout-branch-from-default
│   ├── test_prune.sh    # Integration tests for git-worktree-prune
│   └── test_simple.sh   # Simple validation tests for Rust binaries
└── README.md           # This file
```

## Integration Tests

The integration tests validate the Rust binary implementations by:

- Building the Rust binaries with `cargo build --release`
- Testing the binaries as they would run in real-world scenarios
- Testing advanced features and edge cases
- Validating error handling and security

**Environment**: Uses compiled Rust binaries from `target/release/` directory

## Running Tests

### Using Mise (Recommended)

```bash
# Run all tests (unit + integration)
mise run test

# Run only unit tests
mise run test-unit

# Run only integration tests
mise run test-integration

# Run specific test suites
mise run test-integration-clone
mise run test-integration-init
mise run test-integration-checkout

# Run with verbose output
mise run test-verbose
mise run test-integration-verbose
```

### Direct Execution

```bash
# Integration tests
cd tests/integration && ./test_all.sh

# Individual test files
cd tests/integration && ./test_clone.sh
cd tests/integration && ./test_init.sh
```

## Test Framework Features

The test suite uses a sophisticated test framework with:

- **Isolated test environments**: Each test runs in its own temporary directory
- **Mock remote repositories**: Create realistic Git repositories for testing
- **Comprehensive assertions**: File existence, directory structure, Git state
  validation
- **Cleanup on exit**: Automatic cleanup of test artifacts
- **Colored output**: Clear success/failure indication
- **Performance tracking**: Timing for performance-sensitive operations
- **Error handling**: Proper cleanup on test failures

## Test Coverage

### Integration Tests

- **80+ test scenarios** covering all Rust binary commands
- **Real-world workflows**: Clone, checkout, branch creation, pruning
- **Error handling**: Invalid inputs, missing dependencies, cleanup
- **Security testing**: Path traversal prevention
- **Performance validation**: Timing and resource usage
- **Cross-platform compatibility**: Linux, macOS testing

## Test Environment

### Prerequisites

- **Git**: Version 2.5+ (for worktree support)
- **Bash**: Version 4.0+ (for shell script execution)
- **Rust**: Version 1.70+ (for building test targets)
- **Standard Unix tools**: `awk`, `basename`, `dirname`, `sed`, `cut`

### Optional Dependencies

- **direnv**: For environment setup testing
- **shellcheck**: For shell script linting

### Temporary Directories

- Integration tests: `/tmp/git-worktree-integration-tests`

## CI/CD Integration

The test suite is integrated with GitHub Actions and provides:

- **Automated testing**: Integration tests run on every PR
- **Multi-platform support**: Ubuntu and macOS testing
- **Performance monitoring**: Track test execution times
- **Artifact collection**: Test results and logs for debugging
- **Dependency validation**: Ensure all required tools are available

## Development Workflow

### Adding New Tests

1. Add to appropriate `tests/integration/test_*.sh` file
2. Update test runners: Ensure new tests are called in `run_*_tests()` functions
3. Test isolation: Each test should clean up after itself

### Test Development Guidelines

- **Descriptive names**: Use clear, descriptive test function names
- **Proper assertions**: Use framework assertion functions
- **Error handling**: Tests should handle failures gracefully
- **Documentation**: Document complex test scenarios
- **Cleanup**: Always clean up test artifacts

### Debugging Failed Tests

1. **Run individual tests**: Use specific test targets for faster iteration
2. **Verbose output**: Use `-verbose` targets for detailed output
3. **Manual execution**: Run test scripts directly for debugging
4. **Test isolation**: Check `/tmp` directories for test artifacts
