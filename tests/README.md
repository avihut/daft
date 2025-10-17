# Git Worktree Workflow Test Suite

This directory contains comprehensive test suites for both the legacy shell script implementation and the new Rust binary implementation of the daft toolkit.

## Test Structure

```
tests/
├── legacy/              # Legacy shell script tests
│   ├── test_all.sh      # Master test runner for legacy tests
│   ├── test_framework.sh # Test framework for legacy tests
│   ├── test_clone.sh    # Tests for git-worktree-clone shell script
│   ├── test_init.sh     # Tests for git-worktree-init shell script
│   ├── test_checkout.sh # Tests for git-worktree-checkout shell script
│   ├── test_checkout_branch.sh # Tests for git-worktree-checkout-branch shell script
│   ├── test_checkout_branch_from_default.sh # Tests for git-worktree-checkout-branch-from-default shell script
│   ├── test_prune.sh    # Tests for git-worktree-prune shell script
│   └── test_simple.sh   # Simple validation tests for legacy scripts
├── integration/         # Rust integration tests
│   ├── test_all.sh      # Master test runner for integration tests
│   ├── test_framework.sh # Test framework for integration tests
│   ├── test_clone.sh    # Integration tests for git-worktree-clone Rust binary
│   ├── test_init.sh     # Integration tests for git-worktree-init Rust binary
│   ├── test_checkout.sh # Integration tests for git-worktree-checkout Rust binary
│   ├── test_checkout_branch.sh # Integration tests for git-worktree-checkout-branch Rust binary
│   ├── test_checkout_branch_from_default.sh # Integration tests for git-worktree-checkout-branch-from-default Rust binary
│   ├── test_prune.sh    # Integration tests for git-worktree-prune Rust binary
│   └── test_simple.sh   # Simple validation tests for Rust binaries
└── README.md           # This file
```

## Test Types

### Legacy Tests (`tests/legacy/`)

The legacy tests validate the deprecated shell script implementations located in `src/legacy/`. These tests:

- Test the original shell script behavior
- Ensure backward compatibility during the migration
- Use the shell scripts directly via PATH
- Are maintained for regression testing and comparison

**Environment**: Uses shell scripts from `src/legacy/` directory

### Integration Tests (`tests/integration/`)

The integration tests validate the Rust binary implementations by:

- Building the Rust binaries with `cargo build --release`
- Testing the binaries as they would run in real-world scenarios
- Ensuring the Rust implementation matches the shell script behavior
- Testing advanced features and edge cases

**Environment**: Uses compiled Rust binaries from `target/release/` directory

## Running Tests

### Using Make (Recommended)

```bash
# Run all tests (legacy + integration)
make test

# Run only legacy tests
make test-legacy

# Run only integration tests
make test-integration

# Run specific test suites
make test-legacy-init
make test-integration-clone

# Run with verbose output
make test-verbose
make test-legacy-verbose
make test-integration-verbose
```

### Direct Execution

```bash
# Legacy tests
cd tests/legacy && ./test_all.sh

# Integration tests
cd tests/integration && ./test_all.sh

# Individual test files
cd tests/legacy && ./test_init.sh
cd tests/integration && ./test_clone.sh
```

## Test Framework Features

Both test suites use sophisticated test frameworks with:

- **Isolated test environments**: Each test runs in its own temporary directory
- **Mock remote repositories**: Create realistic Git repositories for testing
- **Comprehensive assertions**: File existence, directory structure, Git state validation
- **Cleanup on exit**: Automatic cleanup of test artifacts
- **Colored output**: Clear success/failure indication
- **Performance tracking**: Timing for performance-sensitive operations
- **Error handling**: Proper cleanup on test failures

## Test Coverage

### Legacy Tests
- **37+ test scenarios** covering all shell script commands
- **Real-world workflows**: Clone, checkout, branch creation, pruning
- **Error handling**: Invalid inputs, missing dependencies, cleanup
- **Cross-platform compatibility**: Linux, macOS testing

### Integration Tests
- **80+ test scenarios** covering all Rust binary commands
- **Enhanced coverage**: Security testing, performance validation
- **Edge cases**: Path traversal prevention, large repositories
- **Help and version**: CLI interface validation
- **Direnv integration**: Environment setup testing

## Key Differences

| Aspect | Legacy Tests | Integration Tests |
|--------|--------------|-------------------|
| **Target** | Shell scripts in `src/legacy/` | Rust binaries in `target/release/` |
| **Performance** | Baseline shell performance | Optimized Rust performance |
| **Features** | Original feature set | Enhanced features + compatibility |
| **Error Messages** | Shell script errors | Rust structured errors |
| **Help System** | Basic help text | Advanced clap-generated help |
| **Validation** | Shell script validation | Rust type safety + validation |

## Test Environment

### Prerequisites
- **Git**: Version 2.5+ (for worktree support)
- **Bash**: Version 4.0+ (for shell script execution)
- **Rust**: Version 1.70+ (for building integration test targets)
- **Standard Unix tools**: `awk`, `basename`, `dirname`, `sed`, `cut`

### Optional Dependencies
- **direnv**: For environment setup testing
- **shellcheck**: For shell script linting

### Temporary Directories
- Legacy tests: `/tmp/git-worktree-tests`
- Integration tests: `/tmp/git-worktree-integration-tests`

## CI/CD Integration

The test suite is integrated with GitHub Actions and provides:

- **Automated testing**: Both legacy and integration tests run on every PR
- **Multi-platform support**: Ubuntu and macOS testing
- **Performance monitoring**: Track test execution times
- **Artifact collection**: Test results and logs for debugging
- **Dependency validation**: Ensure all required tools are available

## Development Workflow

### Adding New Tests

1. **For Legacy Tests**: Add to appropriate `tests/legacy/test_*.sh` file
2. **For Integration Tests**: Add to appropriate `tests/integration/test_*.sh` file
3. **Update test runners**: Ensure new tests are called in `run_*_tests()` functions
4. **Test isolation**: Each test should clean up after itself

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

## Migration Notes

During the migration from shell scripts to Rust binaries:

1. **Legacy tests remain**: Ensure no regressions in existing functionality
2. **Integration tests expand**: New features and improvements are tested
3. **Compatibility validation**: Both implementations produce similar results
4. **Performance comparison**: Track performance improvements

The dual test suite approach ensures a smooth transition while maintaining confidence in both implementations.