---
title: git-worktree-push
description: Push a branch, running pre-push hooks in its own worktree
---

# git worktree-push

Push a branch, running pre-push hooks in its own worktree

::: tip
This command is also available as `daft push`. See [daft push](./daft-push.md).
:::

## Description

Pushes a branch with the repository's shared pre-push hook running in
the pushed branch's own worktree.

Plain `git push` fires the shared pre-push hook with the working
directory of whatever worktree you invoked it from. A hook that runs
tests, lints the working tree, or reads worktree-local configuration
therefore silently validates the wrong tree when you push another
worktree's branch. This command resolves the branch to its worktree
first and runs the push from there — that is the only thing it adds
over `git push`.

The push targets the branch's own upstream remote when it has one,
falling back to the `daft.remote` remote (default: origin) otherwise —
and a branch with no upstream is pushed with `--set-upstream` so
tracking gets configured. A branch with no checked-out worktree is
pushed from the current directory, like plain `git push`.

Only local branches can be pushed: tags and other refs are rejected
rather than handed to git as if they were branches.

Single-branch only: git fires pre-push once with one working directory,
so worktree-correct hook context is only well-defined for one branch.

## Usage

```
git worktree-push [OPTIONS] [BRANCH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH>` | Branch to push (default: the current worktree's branch) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--no-verify` | Skip the repo's pre-push hook |  |
| `--force-with-lease` | Use git push --force-with-lease |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-q, --quiet` | Suppress non-essential output |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-sync](./git-worktree-sync.md)
- [git-worktree-checkout](./git-worktree-checkout.md)

