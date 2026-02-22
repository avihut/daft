---
title: daft adopt
description: Convert a traditional repository to worktree-based layout
---

# daft adopt

Convert a traditional repository to worktree-based layout

## Usage

```
daft adopt [OPTIONS] [REPOSITORY_PATH]
```

This is equivalent to `git worktree-flow-adopt`. All options and arguments
are the same.

## Description

Converts your existing Git repository from the traditional layout to daft's
worktree-based layout:

    Before:                    After:
    my-project/                my-project/
    +-- .git/                  +-- .git/        (bare repository)
    +-- src/                   +-- main/        (worktree)
    +-- README.md                  +-- src/
                                   +-- README.md

Your uncommitted changes are preserved. The command is safe to run -- if
anything fails, your repository is restored to its original state.

## See Also

- [daft eject](./daft-eject.md) to convert back
- [git worktree-flow-adopt](./git-worktree-flow-adopt.md) for full options reference
