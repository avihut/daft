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

2. **Create a feature branch** (a worktree, with daft):

   ```bash
   daft start daft-XX/feature-name
   ```

3. **Make changes** following the commit conventions above

4. **Run quality checks**:

   ```bash
   mise run fmt
   mise run clippy
   mise run test
   ```

5. **Submit a pull request** with a conventional commit title

## Merging

`master` is protected by a ruleset (the intent is in
[`.github/rulesets/`](.github/rulesets/)); these are the rules you will meet:

- **Squash only.** Every PR lands as one commit whose subject is the PR title
  (so keep the title in conventional-commit form — it drives the changelog and
  the version bump) and whose body is the PR's commit messages. No merge
  commits, no rebase merges; history stays linear.
- **One required check, `ci-gate`.** It fans in every job of
  `.github/workflows/test.yml`; jobs that do not apply to your change are
  skipped, and skipped counts as green. If your PR touches only docs, expect
  only the docs jobs to run.
- **Up to date with `master`.** When `master` moves under you, rebase and
  force-push (`git push --force-with-lease`) or press **Update branch**; CI
  re-runs on the result. The tree CI tested is the tree that lands — the same
  rule `daft merge` enforces locally with `ff: only`.
- **Review threads resolved.** Every conversation on the PR must be resolved
  before merging.
- **Dependabot PRs merge themselves.** Patch and minor dependency updates are
  auto-merged once `ci-gate` is green; major bumps wait for a maintainer. Every
  GitHub Action in the workflows is pinned to a commit SHA
  (`scripts/check-actions-pinned.sh`), so a Dependabot bump is a reviewed,
  immutable commit and not a moving tag. If you add an action, pin it —
  `scripts/pin-actions.sh` does it for you.

There are no required approvals: daft has one maintainer, and the gate that does
the work is the CI check. That will change when there is a second pair of hands.

## Code Quality Requirements

Before submitting, ensure:

- [ ] All tests pass: `mise run test`
- [ ] No clippy warnings: `mise run clippy`
- [ ] Code is formatted: `mise run fmt:check`
- [ ] Documentation is updated if needed

## Testing

The project has a two-tier testing architecture:

```bash
# Run all tests
mise run test

# Run specific test suites
mise run test:unit          # Rust unit tests
mise run test:integration   # End-to-end tests
```

## Getting Help

- Open an issue for bugs or feature requests
- Check existing issues before creating new ones
- For questions, use GitHub Discussions

## License

By contributing, you agree that your contributions will be licensed under the
MIT License.
