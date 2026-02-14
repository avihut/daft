---
title: git-worktree-init
description: Initialize a new repository in the worktree-based directory structure
---

# git worktree-init

Initialize a new repository in the worktree-based directory structure

## Description

Initializes a new Git repository using the same directory structure as
git-worktree-clone(1). The resulting layout is:

    <name>/.git      (bare repository metadata)
    <name>/<branch>  (worktree for the initial branch)

This structure is optimized for worktree-based development, allowing multiple
branches to be checked out simultaneously as sibling directories.

The initial branch name is determined by, in order of precedence: the -b
option, the init.defaultBranch configuration value, or "master" as a fallback.

If the repository contains a .daft/hooks/ directory (created manually after
init) and is trusted, lifecycle hooks are executed. See git-daft(1) for hook
management.

## Usage

```
git worktree-init [OPTIONS] <REPOSITORY_NAME>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPOSITORY_NAME>` | Name for the new repository directory | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--bare` | Create only the bare repository; do not create an initial worktree |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-b, --initial-branch <INITIAL_BRANCH>` | Use <name> as the initial branch instead of the configured default |  |
| `-r, --remote <REMOTE>` | Organize worktree under this remote folder (enables multi-remote mode) |  |
| `--no-cd` | Do not change directory to the new worktree |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-clone](./git-worktree-clone.md)
- [git-worktree-checkout-branch](./git-worktree-checkout-branch.md)
- [git-worktree-flow-adopt](./git-worktree-flow-adopt.md)

