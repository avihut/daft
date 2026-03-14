# Test Suite

Two test systems: **bash integration tests** (legacy) and **YAML manual tests**
(declarative, preferred for new tests).

## Structure

```
tests/
├── integration/              # Bash integration tests
│   ├── test_all.sh           # Master runner (all suites)
│   ├── test_framework.sh     # Shared assertions and helpers
│   └── test_*.sh             # Per-command test suites
├── manual/
│   ├── scenarios/            # YAML test scenarios
│   │   ├── checkout/         # One directory per command
│   │   ├── clone/
│   │   ├── init/
│   │   └── ...               # 18 command directories
│   └── fixtures/
│       └── repos/            # Shared repo templates
│           └── standard-remote.yml
└── README.md
```

## Running Tests

```bash
# All tests (unit + integration matrix)
mise run test

# Unit tests only
mise run test:unit

# Bash integration tests (full matrix: default + gitoxide)
mise run test:integration

# YAML manual tests (all 252 scenarios)
mise run test:manual -- --ci

# YAML tests for a specific command
mise run test:manual -- --ci tests/manual/scenarios/checkout/

# YAML tests by namespace
mise run test:manual -- --ci checkout:basic

# Interactive mode (step through with prompts)
mise run test:manual -- checkout:basic

# List all available scenarios
mise run test:manual -- --list
```

## Benchmarks

```bash
# TUI benchmark: bash vs YAML side-by-side (live table with spinners)
mise run bench:tests:integration

# Parallel mode (bash + YAML for same suite run concurrently)
mise run bench:tests:integration -- --parallel

# YAML-only benchmark with timing
mise run bench:tests:manual
```

## YAML Manual Test Framework

### Scenario format

Each `.yml` file in `tests/manual/scenarios/` defines a test scenario:

```yaml
name: Checkout basic
description: Checkout existing branch from cloned repo

repos:
  - name: test-repo
    use_fixture: standard-remote # shared template

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/main"

  - name: Checkout develop branch
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/develop"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/develop"
          branch: develop
```

### Available assertions

| Assertion             | Description                                      |
| --------------------- | ------------------------------------------------ |
| `exit_code`           | Expected exit code                               |
| `dirs_exist`          | Directories that must exist                      |
| `files_exist`         | Files that must exist                            |
| `files_not_exist`     | Files that must NOT exist                        |
| `file_contains`       | File must contain substring                      |
| `file_not_contains`   | File must NOT contain substring                  |
| `output_contains`     | Command stdout+stderr must contain substring     |
| `output_not_contains` | Command stdout+stderr must NOT contain substring |
| `is_git_worktree`     | Directory is a git worktree on expected branch   |
| `branch_exists`       | Branch exists in a repo                          |

### Variables

| Variable         | Description                                                           |
| ---------------- | --------------------------------------------------------------------- |
| `$WORK_DIR`      | Sandbox working directory                                             |
| `$BASE_DIR`      | Sandbox root (parent of work/ and remotes/)                           |
| `$BINARY_DIR`    | Path to built daft binaries                                           |
| `$REMOTE_<NAME>` | Path to generated bare repo (name uppercased, hyphens to underscores) |

### Shared fixtures

Place reusable repo templates in `tests/manual/fixtures/repos/`. Reference them
with `use_fixture`:

```yaml
repos:
  - name: my-repo
    use_fixture: standard-remote # loads standard-remote.yml with {{NAME}} substitution
```

### Path convention

All assertion paths and `cwd` values must use `$WORK_DIR/` prefix for correct
resolution:

```yaml
# Correct
dirs_exist:
  - "$WORK_DIR/test-repo/develop"

# Wrong (resolves relative to cwd, causes double-nesting)
dirs_exist:
  - "test-repo/develop"
```

## Adding New Tests

### YAML scenario (preferred)

1. Create `tests/manual/scenarios/<command>/<test-name>.yml`
2. Use `standard-remote` fixture or define inline repos
3. Run: `mise run test:manual -- --ci <command>:<test-name>`

### Bash integration test

1. Add test function to `tests/integration/test_<command>.sh`
2. Register in `run_<command>_tests()` function
3. Run: `cd tests/integration && bash ./test_<command>.sh`

## CI Integration

The GitHub Actions workflow runs both test systems for each matrix entry
(default + gitoxide config):

1. Bash: `test_all.sh` with `GIT_CONFIG_GLOBAL`
2. YAML: `xtask manual-test --ci` with same config
3. Shell completions: `test_completions.sh`
4. Help commands: verify all binaries respond to `--help`
