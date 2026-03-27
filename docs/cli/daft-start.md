---
title: daft start
description: Create a new branch and worktree
---

# daft start

Create a new branch and worktree

## Usage

```
daft start [OPTIONS] <BRANCH_NAME> [BASE_BRANCH_NAME]
```

This is equivalent to `git worktree-checkout -b`. All options and arguments
are the same as `git worktree-checkout` with `-b` implied.

## Description

Creates a new branch and a corresponding worktree in a single operation.
The new branch is based on the current branch, or on `<BASE_BRANCH_NAME>`
if specified.

By default, daft does not push the new branch to the remote. To enable pushing
and upstream tracking, set `daft.checkout.push true` or use
`daft config remote-sync --on`. You can also pass `--local` to skip remote
operations for a single invocation regardless of config.

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH_NAME>` | Name of the new branch to create | Yes |
| `<BASE_BRANCH_NAME>` | Branch to use as the base; defaults to the current branch | No |

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `--local` | Skip all remote operations (no fetch, no push) for this invocation | |
| `-c, --carry` | Apply uncommitted changes from the current worktree to the new one | |
| `--no-carry` | Do not carry uncommitted changes | |
| `-x, --exec <EXEC>` | Run a command in the worktree after setup completes (repeatable) | |
| `--no-cd` | Do not change directory to the new worktree | |
| `-v, --verbose` | Show detailed progress | |
| `-q, --quiet` | Suppress non-error output | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [daft go](./daft-go.md) to open an existing branch
- [daft config](./daft-config.md) to configure remote sync behavior
- [git worktree-checkout](./git-worktree-checkout.md) for full options reference
