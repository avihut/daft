---
title: git-worktree-fetch
description: Update worktree branches from their remote tracking branches
---

# git worktree-fetch

Update worktree branches from their remote tracking branches

::: tip
This command is also available as `daft fetch`. See [daft fetch](./daft-fetch.md).
:::

## Description

Updates worktree branches by pulling from their remote tracking branches.

For each target worktree, the command navigates to that directory and runs
`git pull` with the configured options. By default, only fast-forward updates
are allowed (--ff-only).

Targets can be specified by worktree directory name or branch name. If no
targets are specified and --all is not used, the current worktree is updated.

Worktrees with uncommitted changes are skipped unless --force is specified.
Use --dry-run to preview what would be done without making changes.

Arguments after -- are passed directly to git pull, allowing full control
over the pull behavior.

## Usage

```
git worktree-fetch [OPTIONS] [TARGETS] [PULL_ARGS]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGETS>` | Target worktree(s) by directory name or branch name | No |
| `<PULL_ARGS>` | Additional arguments to pass to git pull | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--all` | Update all worktrees |  |
| `-f, --force` | Update even with uncommitted changes |  |
| `--dry-run` | Show what would be done |  |
| `--rebase` | Use git pull --rebase |  |
| `--autostash` | Use git pull --autostash |  |
| `--ff-only` | Only fast-forward (default) |  |
| `--no-ff-only` | Allow merge commits |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-q, --quiet` | Suppress non-error output |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-carry](./git-worktree-carry.md)

