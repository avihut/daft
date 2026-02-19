---
title: git-worktree-clone
description: Clone a repository into a worktree-based directory structure
---

# git worktree-clone

Clone a repository into a worktree-based directory structure

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
| `-b, --branch <BRANCH>` | Check out <branch> instead of the remote's default branch |  |
| `-n, --no-checkout` | Perform a bare clone only; do not create any worktree |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-a, --all-branches` | Create a worktree for each remote branch, not just the default |  |
| `--trust-hooks` | Trust the repository and allow hooks to run without prompting |  |
| `--no-hooks` | Do not run any hooks from the repository |  |
| `-r, --remote <REMOTE>` | Organize worktree under this remote folder (enables multi-remote mode) |  |
| `--no-cd` | Do not change directory to the new worktree |  |
| `-x, --exec <EXEC>` | Run a command in the worktree after setup completes (repeatable) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-init](./git-worktree-init.md)
- [git-worktree-checkout](./git-worktree-checkout.md)
- [git-worktree-flow-adopt](./git-worktree-flow-adopt.md)

