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

`<repo>` is a catalog name, a uuid, or a path: `.`, a subdirectory, or an
absolute or relative directory. Catalog names win over paths, so `./api` is
the spelling that insists on a directory. A bare name the catalog does not
know is retried as a path, and must then name a repository root — only a
spelled-out path walks up to the repository a subdirectory belongs to. With no
argument the repo containing the current directory is used.

`--purge` additionally deletes the git dir and every checked-out worktree, and
everything below describes that mode alone:

  * Confirmation is asked unless `-y` is given. `-y` applies to `--purge` only
    — the default removal asks nothing, because it destroys nothing.
  * For each worktree, the worktree-pre-remove and worktree-post-remove
    lifecycle hooks are run when the repository is daft-managed and trusted.
  * Hook failures do not abort the deletion; failed hooks are summarized after
    the operation completes. The repo is removed regardless.
  * worktree-post-remove fires AFTER the worktree directory has been deleted —
    $DAFT_WORKTREE_PATH points at a path that no longer exists. Hook scripts
    that need to inspect the worktree must do so in worktree-pre-remove.
  * Paths that are not inside a Git repository are refused: there are no files
    to delete. The default removal has no such requirement — it drops a
    catalog entry whatever state the recorded directory is in.

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

