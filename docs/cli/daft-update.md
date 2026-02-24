---
title: daft update
description: Update worktree branches from remote
---

# daft update

Update worktree branches from remote

## Usage

```
daft update [OPTIONS] [TARGETS] [PULL_ARGS]
```

## Description

Updates worktree branches from their remote tracking branches. Targets can
use refspec syntax (`source:destination`) to update a worktree from a
different remote branch.

**Same-branch mode** (source equals destination, or no refspec) uses `git pull`
with configurable options (`--rebase`, `--ff-only`, `--autostash`, and any
extra `PULL_ARGS`). The default pull strategy is `--ff-only`, configurable via
`git config daft.update.args`.

**Cross-branch mode** (source differs from destination, e.g. `master:test`)
uses `git fetch` + `git reset --hard` and ignores all pull flags. This is
useful for resetting a worktree to match a different remote branch.

Worktrees with uncommitted changes are skipped unless `--force` is specified.
Use `--dry-run` to preview what would be done without making changes.

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGETS>` | Target worktree(s) by name or refspec (`source:destination`) | No |
| `<PULL_ARGS>` | Additional arguments to pass to `git pull` (same-branch mode only) | No |

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `--all` | Update all worktrees | |
| `-f, --force` | Update even with uncommitted changes | |
| `--dry-run` | Show what would be done | |
| `--rebase` | Use `git pull --rebase` | |
| `--autostash` | Use `git pull --autostash` | |
| `--ff-only` | Only fast-forward (default) | |
| `--no-ff-only` | Allow merge commits | |
| `-v, --verbose` | Be verbose; show detailed progress | |
| `-q, --quiet` | Suppress non-error output | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Update the current worktree from its tracking branch
daft update

# Update a specific worktree
daft update feature/auth

# Update all worktrees
daft update --all

# Update with rebase instead of merge
daft update --rebase

# Reset the test worktree to match origin/master (cross-branch)
daft update master:test

# Preview what would happen
daft update --all --dry-run

# Force update even if worktrees have uncommitted changes
daft update --all --force
```

## See Also

- [git worktree-fetch](./git-worktree-fetch.md) for the underlying git-native command
- [daft sync](./daft-sync.md) to prune + update all in one step
