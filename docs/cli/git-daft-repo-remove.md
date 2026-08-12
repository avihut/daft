---
title: git-daft-repo-remove
description: Remove a repository from the repo catalog
---

# git daft-repo-remove

Remove a repository from the repo catalog

## Description

Removes a repository from the repo catalog: the catalog entry is marked
removed, nothing on disk is touched, no hooks run, and no confirmation is
asked. The operation is reversible — daft re-registers repos it runs inside,
and removed entries are restorable by name with `git daft clone <name>`. A
stale entry whose recorded directory is already gone is dropped the same way.

`--purge` additionally deletes the git dir and every checked-out worktree. For
each worktree, the worktree-pre-remove and worktree-post-remove lifecycle
hooks are run when the repository is daft-managed and trusted, and the removal
asks for confirmation unless `-y` is given. `-y` applies to `--purge` only —
the default removal asks nothing.

`<repo>` is a catalog name, a uuid, or a path: `.`, a subdirectory, or an
absolute or relative directory. Catalog names win over paths, so `./api` is
the spelling that insists on a directory; a bare name that is not cataloged is
retried as a path. With no argument the repo containing the current directory
is used.

Hook failures do not abort removal; failed hooks are summarized after the
operation completes. The repo is removed regardless.

worktree-post-remove fires AFTER the worktree directory has been deleted —
$DAFT_WORKTREE_PATH points at a path that no longer exists. Hook scripts that
need to inspect the worktree must do so in worktree-pre-remove.

Refuses to operate on paths that are not inside a Git repository.

## Usage

```
git daft-repo-remove [OPTIONS] [REPO]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPO>` | Catalog name, uuid, or a repo path — including . or a subdirectory (default: the current repo) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--purge` | Also delete the git dir and every worktree (destructive) |  |
| `-y, --force` | Skip the confirmation prompt (--purge only) |  |
| `--dry-run` | Print what would be removed without touching anything |  |
| `-v, --verbose` | Increase verbosity (-v hook details, -vv full sequential output) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

