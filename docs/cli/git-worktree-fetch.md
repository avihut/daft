---
title: git-worktree-fetch
description: Update worktree branches from their remote tracking branches
---

# git worktree-fetch

Update worktree branches from their remote tracking branches

::: tip
This command is also available as `daft update`. See [daft update](./daft-update.md).
:::

## Description

Updates worktree branches from their remote tracking branches.

Targets can use refspec syntax (source:destination) to update a worktree
from a different remote branch:

  Same-branch:   daft update master        (pulls master via git pull --ff-only)
  Cross-branch:  daft update master:test   (fetches origin/master, resets test to it)
  Current:       daft update               (pulls current worktree's tracking branch)
  All:           daft update --all         (pulls all worktrees)

Same-branch mode uses `git pull` with configurable options (--rebase,
--ff-only, --autostash, -- PULL_ARGS). Cross-branch mode uses `git fetch`
+ `git reset --hard` and ignores pull flags.

Worktrees with uncommitted changes are skipped unless --force is specified.
Use --dry-run to preview what would be done without making changes.

## Usage

```
git worktree-fetch [OPTIONS] [TARGETS] [PULL_ARGS]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGETS>` | Target worktree(s) by name or refspec (source:destination) | No |
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

