---
title: git-worktree-carry
description: Transfer uncommitted changes to other worktrees
---

# git worktree-carry

Transfer uncommitted changes to other worktrees

## Description

Transfers uncommitted changes (staged, unstaged, and untracked files) from
the current worktree to one or more target worktrees.

When a single target is specified without --copy, changes are moved: they
are applied to the target worktree and removed from the source. When --copy
is specified or multiple targets are given, changes are copied: they are
applied to all targets while remaining in the source worktree.

Targets may be specified by worktree directory name or by branch name. If
both a worktree and a branch have the same name, the worktree takes
precedence.

After transferring changes, the working directory is changed to the last
target worktree (or the only target, if just one was specified).

## Usage

```
git worktree-carry [OPTIONS] <TARGETS>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGETS>` | Target worktree(s) by directory name or branch name | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-c, --copy` | Copy changes instead of moving; changes remain in the source worktree |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-checkout-branch](./git-worktree-checkout-branch.md)
- [git-worktree-fetch](./git-worktree-fetch.md)

