---
title: git-daft-repo-remove
description: Remove a Git repository and all its worktrees
---

# git daft-repo-remove

Remove a Git repository and all its worktrees

## Description

Removes a Git repository identified by `<path>` (or the current directory if
no path is given), including the git dir and every checked-out
worktree. For each worktree, the worktree-pre-remove and worktree-post-remove
lifecycle hooks are run when the repository is daft-managed and trusted.

`--repo <name>` addresses a cataloged repository by name instead of a path
and is mutually exclusive with the positional.

`--keep-files` removes the repository from the repo catalog only: nothing on
disk is touched, no hooks run, and no confirmation is asked (the operation
is reversible — daft re-registers repos it runs inside, and removed entries
are restorable by name with `git daft clone <name>`). Combined with `--repo`
it also works when the recorded directory is already gone, dropping the
stale entry. When the repository is cataloged, the interactive confirmation
offers this as the `k` choice.

Hook failures do not abort removal; failed hooks are summarized after the
operation completes. The repo is removed regardless.

worktree-post-remove fires AFTER the worktree directory has been deleted —
$DAFT_WORKTREE_PATH points at a path that no longer exists. Hook scripts that
need to inspect the worktree must do so in worktree-pre-remove.

Refuses to operate on paths that are not inside a Git repository.

## Usage

```
git daft-repo-remove [OPTIONS] [PATH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to the repo or any directory inside it (default: cwd) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--repo <NAME>` | Cataloged repository to remove (instead of a path) |  |
| `--keep-files` | Only remove the repo from the catalog; leave all files on disk |  |
| `-y, --force` | Skip the confirmation prompt |  |
| `--dry-run` | Print what would be removed without touching anything |  |
| `-v, --verbose` | Increase verbosity (-v hook details, -vv full sequential output) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

