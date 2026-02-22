---
title: daft clone
description: Clone a repository into a worktree-based directory structure
---

# daft clone

Clone a repository into a worktree-based directory structure

## Usage

```
daft clone [OPTIONS] <REPOSITORY_URL>
```

This is equivalent to `git worktree-clone`. All options and arguments are
the same.

## Description

Clones a repository into a directory structure optimized for worktree-based
development. The resulting layout is:

    <repository-name>/.git    (bare repository metadata)
    <repository-name>/<branch>  (worktree for the checked-out branch)

The command first queries the remote to determine the default branch (main,
master, or other configured default), then performs a bare clone and creates
the initial worktree.

## See Also

- [git worktree-clone](./git-worktree-clone.md) for full options reference
