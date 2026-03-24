---
title: git-worktree-clone
description: Clone a repository into a worktree-based directory structure
---

# git worktree-clone

Clone a repository into a worktree-based directory structure

::: tip
This command is also available as `daft clone`. See [daft clone](./daft-clone.md).
:::

## Description

Clones a repository into a directory structure optimized for worktree-based
development. The resulting layout is:

    <repository-name>/.git    (bare repository metadata)
    <repository-name>/<branch>  (worktree for the checked-out branch)

The command first queries the remote to determine the default branch (main,
master, or other configured default), then performs a bare clone and creates
the initial worktree. This structure allows multiple worktrees to be created
as siblings, each containing a different branch.

If the repository contains a .daft/hooks/ directory and the repository is
trusted, lifecycle hooks are executed. See git-daft(1) for hook management.

## Usage

```
git worktree-clone [OPTIONS] <REPOSITORY_URL>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPOSITORY_URL>` | The repository URL to clone (HTTPS or SSH) | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-b, --branch <BRANCH>` | Branch to check out (repeatable; use HEAD or @ for default branch) |  |
| `-n, --no-checkout` | Perform a bare clone only; do not create any worktree |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-a, --all-branches` | Create a worktree for each remote branch, not just the default |  |
| `--trust-hooks` | Trust the repository and allow hooks to run without prompting |  |
| `--no-hooks` | Do not run any hooks from the repository |  |
| `-r, --remote <REMOTE>` | Organize worktree under this remote folder (enables multi-remote mode) |  |
| `--no-cd` | Do not change directory to the new worktree |  |
| `--layout <LAYOUT>` | Worktree layout to use for this repository |  |
| `-x, --exec <EXEC>` | Run a command in the worktree after setup completes (repeatable) |  |

## Cloning Multiple Branches

The `-b` flag is repeatable — pass it multiple times to check out several
branches in a single clone operation. The values `HEAD` and `@` are accepted as
aliases for the remote's default branch (e.g. `main` or `master`).

### Layout-dependent behavior

How the branches are laid out depends on the worktree layout in use:

**Non-bare layouts (sibling, nested)**

When more than one branch is requested, daft always checks out the default
branch in the base worktree (the one created by the clone itself). Each
additional branch becomes a satellite worktree placed according to the active
layout. If you explicitly pass `HEAD` or `@` alongside other branches, the
default branch is still placed in the base worktree and the remaining branches
become satellites.

**Bare layouts (contained)**

No branch is singled out as the "primary" worktree. Only the branches you
explicitly request receive worktrees — the bare repository metadata lives at the
root, and each branch gets its own subdirectory.

### Examples

Clone and immediately check out two feature branches:

```sh
git worktree clone <url> -b feat-a -b feat-b
```

Clone with the default branch plus a feature branch (non-bare layout):

```sh
git worktree clone <url> -b @ -b feat-a
```

Here `@` resolves to the remote's default branch. In a sibling layout the
default branch occupies the base worktree and `feat-a` becomes a sibling
worktree next to it.

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-init](./git-worktree-init.md)
- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-flow-adopt](./git-worktree-flow-adopt.md)

