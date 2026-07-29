---
title: git-worktree-warm
description: Copy declared build caches into a worktree
---

# git worktree-warm

Copy declared build caches into a worktree

::: tip
This command is also available as `daft warm`. See [daft warm](./daft-warm.md).
:::

## Description

Replicates the paths declared under `copy:` in daft.yml from one worktree
into another, so a worktree starts warm instead of rebuilding caches that
already exist next door.

This is the manual re-run of the copy stage that worktree creation performs
automatically. Use it for a worktree created before `copy:` was declared, or
to re-seed a cache that has since been rebuilt in the source worktree.

Naming no worktree warms the one you are standing in; naming one warms that
one. --from names the source outright and is never second-guessed. Without
it the source is ranked against what the target already holds: a worktree
sitting at the target's exact commit first, then the worktree you are
standing in, then the default branch's. Both the target and --from accept a
worktree directory name, a branch name, or a path under the project root.

Entries that already exist in the target are left alone, which makes repeat
runs a no-op; pass --force to replace them. Running --force while standing
inside a cache it replaces moves you to the target worktree's root, because
that directory is unlinked out from under your shell. On a filesystem that
supports copy-on-write (APFS, btrfs, XFS with reflink=1, OpenZFS 2.2+, ReFS)
the copy is near-free until the caches diverge.

Copy failures never fail the command: an entry that is tracked by git, too
large for its max_size, or unreadable is reported and the rest still copy.

## Usage

```
git worktree-warm [OPTIONS] [TARGET]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGET>` | Worktree to warm, by directory name, branch name, or path (default: the current worktree) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--from <worktree>` | Worktree to copy from, by directory name, branch name, or path (default: ranked — a worktree at the target's exact commit, then the current worktree, then the default branch's) |  |
| `-f, --force` | Replace entries that already exist in the target worktree |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-carry](./git-worktree-carry.md)
- [daft-shared](./daft-shared.md)
- [git-worktree-checkout](./git-worktree-checkout.md)

