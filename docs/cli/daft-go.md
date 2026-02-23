---
title: daft go
description: Open an existing branch in a worktree
---

# daft go

Open an existing branch in a worktree

## Usage

```
daft go [OPTIONS] <BRANCH_NAME>
```

This is equivalent to `git worktree-checkout`. All options and arguments are
the same.

## Description

Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If a worktree for the branch already exists, the working
directory is changed to it.

With `--start` (or `-s`), if the branch does not exist locally or on the
remote, a new branch and worktree are created automatically. This can also be
enabled permanently with `git config daft.go.autoStart true`.

### Previous worktree (`-`)

Use `-` as the branch name to switch to the previous worktree, similar to
`cd -`. Each successful `daft go` or `daft start` records the source worktree,
so repeated `daft go -` toggles between the two most recent worktrees.

```
daft go main        # switch to main
daft go feature/x   # switch to feature/x (main is now "previous")
daft go -           # back to main
daft go -           # back to feature/x
```

Cannot be combined with `-b`/`--create-branch`.

## See Also

- [daft start](./daft-start.md) to create a new branch
- [git worktree-checkout](./git-worktree-checkout.md) for full options reference
