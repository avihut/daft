---
title: git-worktree-flow-eject
description: Convert a worktree-based repository back to traditional layout
---

# git worktree-flow-eject

Convert a worktree-based repository back to traditional layout

::: tip
This command is also available as `daft eject`. See [daft eject](./daft-eject.md).
:::

## Description

WHAT THIS COMMAND DOES

Converts your worktree-based repository back to a traditional Git layout.
This removes all worktrees except one, and moves that worktree's files
back to the repository root.

  Before:                    After:
  my-project/                my-project/
  ├── .git/                  ├── .git/
  ├── main/                  ├── src/
  │   ├── src/               └── README.md
  │   └── README.md
  └── feature/auth/
      └── ...

By default, the remote's default branch (main, master, etc.) is kept.
Use --branch to specify a different branch.

HANDLING UNCOMMITTED CHANGES

- Changes in the target branch's worktree are preserved
- Other worktrees with uncommitted changes cause the command to fail
- Use --force to delete dirty worktrees (changes will be lost!)

EXAMPLES

  git worktree-flow-eject
      Eject to the default branch

  git worktree-flow-eject -b feature/auth
      Eject, keeping the feature/auth branch

  git worktree-flow-eject --force
      Eject even if other worktrees have uncommitted changes

## Usage

```
git worktree-flow-eject [OPTIONS] [REPOSITORY_PATH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPOSITORY_PATH>` | Path to the repository to convert (defaults to current directory) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-b, --branch <BRANCH>` | Branch to keep (defaults to remote's default branch) |  |
| `-f, --force` | Delete worktrees with uncommitted changes (changes will be lost!) |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `--dry-run` | Show what would be done without making any changes |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-flow-adopt](./git-worktree-flow-adopt.md)
- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-clone](./git-worktree-clone.md)

