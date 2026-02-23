---
title: daft update
description: Update worktree branches from remote
---

# daft update

Update worktree branches from remote

## Usage

```
daft update [OPTIONS] [TARGETS] [PULL_ARGS]
```

This is equivalent to `git worktree-fetch`. All options and arguments are
the same.

## Description

Updates worktree branches from their remote tracking branches. Targets can
use refspec syntax (`source:destination`) to update a worktree from a
different remote branch:

- `daft update master` -- pulls master via `git pull --ff-only`
- `daft update master:test` -- fetches `origin/master` and resets the test worktree to it
- `daft update` -- pulls the current worktree's tracking branch
- `daft update --all` -- pulls all worktrees

Same-branch mode (source equals destination) uses `git pull` with configurable
options. Cross-branch mode uses `git fetch` + `git reset --hard` and ignores
pull flags.

Worktrees with uncommitted changes are skipped unless `--force` is specified.

## See Also

- [git worktree-fetch](./git-worktree-fetch.md) for full options reference
