---
title: Contributing
description: Guidelines for contributing to daft
---

Thank you for your interest in contributing to daft!

## Development Setup

1. **Clone the repository** using daft's worktree layout:

   ```bash
   daft clone git@github.com:avihut/daft.git
   ```

2. **Build and set up the development environment:**

   ```bash
   mise run dev
   ```

   This builds the binary, creates all symlinks in `target/release/`, and
   verifies the setup.

3. **Add the binary to your PATH:**

   ```bash
   export PATH="$PWD/target/release:$PATH"
   ```

## Development Workflow

1. Create a feature branch:

   ```bash
   daft start daft-XX/feature-name
   ```

2. Make changes following the conventions below.

3. Run quality checks before committing:

   ```bash
   mise run fmt
   mise run clippy
   mise run test:unit
   ```

4. Submit a pull request targeting `master`.

## Code Quality Requirements

All PRs must pass these checks (enforced in CI):

- **Formatting:** `mise run fmt:check`
- **Linting:** `mise run clippy` (zero warnings)
- **Unit tests:** `mise run test:unit`
- **Integration tests:** `mise run test:integration` (bash + YAML, full matrix)

Run the full CI simulation locally:

```bash
mise run ci
```

## Testing

daft has two test systems that run in CI for each matrix entry (default +
gitoxide):

### YAML manual tests (preferred for new tests)

Declarative test scenarios in `tests/manual/scenarios/`. Each YAML file defines
repos to create, steps to run, and expectations to verify.

```bash
mise run test:manual -- --ci              # Run all 252 scenarios
mise run test:manual -- --ci checkout     # Run one command's tests
mise run test:manual -- checkout:basic    # Interactive mode for one scenario
mise run test:manual -- --list            # List all available scenarios
```

To add a new test, create a `.yml` file in the appropriate command directory:

```yaml
name: Checkout basic
repos:
  - name: test-repo
    use_fixture: standard-remote
steps:
  - name: Clone and checkout
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/main"
```

See `tests/README.md` for the full schema reference (assertions, variables,
fixtures, path conventions).

### Bash integration tests

Shell-based tests in `tests/integration/`. The master runner `test_all.sh`
executes all suites.

```bash
mise run test:integration              # Full matrix (default + gitoxide)
cd tests/integration && bash test_clone.sh   # Run one suite directly
```

### Dev sandbox

The sandbox provides an isolated environment for manual testing of daft
commands. Each worktree gets its own sandbox with a clean git identity and
locally-built binaries on PATH.

```bash
mise run sandbox                       # Enter the sandbox shell
mise run sandbox:setup                 # Install sandbox shell function into RC file
mise run sandbox:clean                 # Remove sandbox for this worktree
```

Inside the sandbox you can set up test scenarios and explore them interactively:

```bash
# Set up a test scenario and cd into its working directory
mise run test:manual -- --setup-only checkout:basic

# Reset the test environment to its initial state
mise run test:manual -- --setup-only --step 1 checkout:basic
```

### Benchmarks

Compare bash and YAML test performance with a live TUI table:

```bash
mise run bench:tests:integration               # Sequential (default)
mise run bench:tests:integration -- --parallel  # Parallel bash+YAML per suite
```

## Commit Messages

This project uses [Conventional Commits](https://www.conventionalcommits.org/)
for automatic changelog generation.

Format: `<type>[scope]: <description>`

| Type       | Description                  |
| ---------- | ---------------------------- |
| `feat`     | New feature                  |
| `fix`      | Bug fix                      |
| `docs`     | Documentation changes        |
| `style`    | Code style (no logic change) |
| `refactor` | Code refactoring             |
| `perf`     | Performance improvement      |
| `test`     | Adding/updating tests        |
| `chore`    | Maintenance tasks            |
| `ci`       | CI/CD changes                |

Examples:

```bash
feat(checkout): add --force flag for overwriting worktrees
fix: resolve branch name parsing for names with slashes
docs: update installation instructions
```

## Pull Request Guidelines

- PR titles use conventional commit format: `feat: add dark mode toggle`
- Issue references go in the PR body, not the title: `Fixes #42`
- All PRs target `master` and are squash-merged

## Branch Naming

Follow the convention: `daft-<issue-number>/<short-description>`

```
daft-42/dark-mode
daft-15/branch-search
```

## Adding a New Command

1. Create `src/commands/<name>.rs` with a clap `Args` struct (include `about`,
   `long_about`, `arg(help)` attributes)
2. Add the module to `src/commands/mod.rs`
3. Add routing in `src/main.rs`
4. Add to `COMMANDS` array and `get_command_for_name()` in `xtask/src/main.rs`
5. Add to help output in `src/commands/docs.rs` (`get_command_categories()`)
6. Run `mise run man:gen` and `mise run docs:cli:gen` and commit the generated
   files
7. Add YAML test scenarios in `tests/manual/scenarios/<name>/`
8. Add bash integration tests in `tests/integration/`

## License

By contributing, you agree that your contributions will be licensed under the
MIT License.
