---
title: Contributing
description: Guidelines for contributing to daft
---

Thank you for your interest in contributing to daft!

## Development Setup

1. **Clone the repository** using daft's worktree layout:

   ```bash
   git worktree-clone git@github.com:avihut/daft.git
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
   git worktree-checkout -b daft-XX/feature-name
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
- **Integration tests:** `mise run test:integration`

Run the full CI simulation locally:

```bash
mise run ci
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

## Testing

```bash
mise run test              # Run all tests
mise run test:unit         # Rust unit tests only
mise run test:integration  # End-to-end tests
```

See [CLAUDE.md](https://github.com/avihut/daft/blob/master/CLAUDE.md) in the
repository for the complete testing architecture.

## Adding a New Command

1. Create `src/commands/<name>.rs` with a clap `Args` struct (include `about`,
   `long_about`, `arg(help)` attributes)
2. Add the module to `src/commands/mod.rs`
3. Add routing in `src/main.rs`
4. Add to `COMMANDS` array and `get_command_for_name()` in `xtask/src/main.rs`
5. Add to help output in `src/commands/docs.rs` (`get_command_categories()`)
6. Run `mise run man:gen` and `mise run docs:cli:gen` and commit the generated
   files
7. Add integration tests in `tests/integration/`

## License

By contributing, you agree that your contributions will be licensed under the
MIT License.
