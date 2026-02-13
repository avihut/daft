# Contributing to daft

Thank you for your interest in contributing to daft! This document provides
guidelines for contributing to the project.

## Commit Message Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/)
for automatic changelog generation via git-cliff.

### Format

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

### Types

| Type       | Description                                         | Changelog Section |
| ---------- | --------------------------------------------------- | ----------------- |
| `feat`     | A new feature                                       | Features          |
| `fix`      | A bug fix                                           | Bug Fixes         |
| `docs`     | Documentation only changes                          | Documentation     |
| `style`    | Code style changes (formatting, etc.)               | Styling           |
| `refactor` | Code changes that neither fix bugs nor add features | Refactoring       |
| `perf`     | Performance improvements                            | Performance       |
| `test`     | Adding or correcting tests                          | Testing           |
| `chore`    | Maintenance tasks, dependency updates               | Miscellaneous     |
| `ci`       | CI/CD configuration changes                         | CI/CD             |

### Examples

```bash
# Feature with scope
feat(checkout): add --force flag for overwriting worktrees

# Bug fix
fix: resolve branch name parsing for names with slashes

# Documentation
docs: update installation instructions for Windows

# With issue reference in footer
feat: implement branch search in checkout command

Fixes #42

# Breaking change (note the ! after type)
feat!: change default branch detection algorithm

BREAKING CHANGE: Now checks remote HEAD instead of hardcoded "main"
```

## Branch Naming

Follow the convention: `daft-<issue-number>/<short-description>`

```bash
# Examples
daft-42/dark-mode
daft-15/branch-search
hotfix/critical-bug
```

## Pull Request Titles

Use conventional commit format for PR titles:

```
feat: add dark mode toggle
fix: resolve login timeout
docs: update installation guide
```

Issue references should be in the PR body, not the title.

## Development Workflow

1. **Fork the repository** (external contributors)

2. **Create a feature branch**:

   ```bash
   git worktree-checkout-branch daft-XX/feature-name
   ```

3. **Make changes** following the commit conventions above

4. **Run quality checks**:

   ```bash
   mise run fmt
   mise run clippy
   mise run test
   ```

5. **Submit a pull request** with a conventional commit title

## Code Quality Requirements

Before submitting, ensure:

- [ ] All tests pass: `mise run test`
- [ ] No clippy warnings: `mise run clippy`
- [ ] Code is formatted: `mise run fmt-check`
- [ ] Documentation is updated if needed

## Testing

The project has a two-tier testing architecture:

```bash
# Run all tests
mise run test

# Run specific test suites
mise run test-unit          # Rust unit tests
mise run test-integration   # End-to-end tests
```

## Getting Help

- Open an issue for bugs or feature requests
- Check existing issues before creating new ones
- For questions, use GitHub Discussions

## License

By contributing, you agree that your contributions will be licensed under the
MIT License.
