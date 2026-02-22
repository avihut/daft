---
title: daft init
description: Initialize a new repository in the worktree-based directory structure
---

# daft init

Initialize a new repository in the worktree-based directory structure

## Usage

```
daft init [OPTIONS] <REPOSITORY_NAME>
```

This is equivalent to `git worktree-init`. All options and arguments are
the same.

## Description

Initializes a new Git repository using the same directory structure as
`daft clone`. The resulting layout is:

    <name>/.git      (bare repository metadata)
    <name>/<branch>  (worktree for the initial branch)

This structure is optimized for worktree-based development, allowing multiple
branches to be checked out simultaneously as sibling directories.

## See Also

- [git worktree-init](./git-worktree-init.md) for full options reference
