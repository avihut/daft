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

A branch is only in scope when something attests that it was on the remote:
git's own upstream tracking reports it gone, or daft recorded seeing it
there. Daft records every push it performs, and each prune run also records
any branch it finds tracking a remote branch of the same name while that
branch is still present - so branches published outside daft are covered
too, from the first prune run that sees them. Being absent from the remote
is not enough on its own, since that is equally true of a branch that was
just created and never pushed. A branch daft cannot place is left alone,
whatever its commits and whatever --force says; discard those with
`daft remove` instead. One consequence worth knowing: publishing a branch
outside daft without upstream tracking (`git push <remote> <branch>`, no -u)
leaves nothing for prune to observe, so it will not be reclaimed
automatically.

A deleted remote branch does not by itself prove the work was merged, so each
gone branch is verified against the default branch (regular or squash merge)
before anything is deleted; gone-but-unmerged branches are kept with a
warning. Squash merges are matched by content, not by patch text, so a squash
still counts as merged when an intervening commit shifted the surrounding
lines. A branch that no local check can place is checked against the forge as
a last resort: it counts as merged only if a freshly fetched pull request
reports merged, targets the default branch, and merged a commit that contains
the local branch tip. Only recent pull requests are visible to that check, so
a branch abandoned long ago can still be reported unmerged; delete it with
--force. Worktrees whose untracked daft files (daft.yml / daft.local.yml)
were refined since daft seeded them are also kept, with a pointer at
daft-file(1) merge for consolidation. --force overrides both: unmerged
branches are deleted and refined daft files are discarded to
`<git-common-dir>/.daft/discarded/<branch>/` — prune never writes another
worktree's files.

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
| `-v, --verbose` | Increase verbosity (-v for hook details, -vv for full sequential output) |  |
| `-f, --force` | Force removal of worktrees with uncommitted changes or untracked files |  |
| `--stat <STAT>` | Statistics mode: summary or lines (default: from git config daft.prune.stat, or summary) |  |
| `--columns <COLUMNS>` | Columns to display (comma-separated). Replace: name,path,age. Modify defaults: +col,-col. Available: name, path, size, base, changes, remote, pr, age, annotation, owner, hash, last-commit |  |
| `--sort <SORT>` | Sort order (comma-separated). +col ascending, -col descending. Columns: name, path, size, base, changes, remote, age, owner, hash, activity, commit |  |
| `--repo <REPO>` | Prune another cataloged repository |  |
| `--all-repos` | Prune every cataloged repository (current repo last) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-fetch](./git-worktree-fetch.md)
- [git-worktree-sync](./git-worktree-sync.md)
- [git-worktree-branch](./git-worktree-branch.md)

