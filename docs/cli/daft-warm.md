---
title: daft warm
description: Copy declared build caches into a worktree
---

# daft warm

Copy declared build caches into a worktree

## Usage

```
daft warm [OPTIONS] [TARGET]
```

This is equivalent to `git worktree-warm`. All options and arguments are the
same.

## Description

Replicates the paths declared under `copy:` in daft.yml from one worktree into
another — the manual re-run of the copy stage that worktree creation performs
automatically.

By default the current worktree is warmed from the default branch's worktree;
naming a worktree warms that one from where you stand. Entries that already
exist in the target are left alone, so repeat runs are a no-op; `--force`
replaces them.

## See Also

- [git worktree-warm](./git-worktree-warm.md) for full options reference
