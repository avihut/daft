---
title: git-worktree-prune
description: Remove worktrees and branches for deleted remote branches
---

# git worktree-prune

Remove worktrees and branches for deleted remote branches

::: tip
This command is also available as `daft prune`. See [daft prune](./daft-prune.md).
:::

## Description

Removes local branches whose corresponding remote tracking branches have been
deleted, along with any associated worktrees. This is useful for cleaning up
after branches have been merged and deleted on the remote.

The command first fetches from the remote with pruning enabled to update the
list of remote tracking branches. It then identifies local branches that were
tracking now-deleted remote branches, removes their worktrees (if any exist),
and finally deletes the local branches.

If you are currently inside a worktree that is about to be pruned, the command
handles this gracefully. In a bare-repo worktree layout (created by daft), the
current worktree is removed last and the shell is redirected to a safe location
(project root by default, or the default branch worktree if configured via
daft.prune.cdTarget). In a regular repository where the current branch is being
pruned, the command checks out the default branch before deleting the old branch.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.

## Usage

```
git worktree-prune [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Be verbose; show detailed progress |  |
| `-f, --force` | Force removal of worktrees with uncommitted changes or untracked files |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-fetch](./git-worktree-fetch.md)
- [git-worktree-flow-eject](./git-worktree-flow-eject.md)
- [git-worktree-branch](./git-worktree-branch.md)

