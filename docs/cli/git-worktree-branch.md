---
title: git-worktree-branch
description: Delete or rename branches and their worktrees
---

# git worktree-branch

Delete or rename branches and their worktrees

::: tip
This command is also available as `daft remove` (delete, use `-f` to force)
or `daft rename` (rename with `-m`).
See [daft remove](./daft-remove.md) and [daft rename](./daft-rename.md).
:::

## Description

Manage branches and their associated worktrees. Supports deletion (-d/-D)
and renaming (-m).

DELETE MODE (-d / -D)

Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout(1) -b.

Use -d for a safe delete that checks whether each branch has been merged.
Use -D to force-delete branches regardless of merge status.

Arguments can be branch names or worktree paths. When a path is given
(absolute, relative, or "."), the branch checked out in that worktree is
resolved automatically. This is convenient when you are inside a worktree
and want to delete it without remembering the branch name.

Safety checks (with -d) prevent accidental data loss. The command refuses to
delete a branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch
  - has refined untracked daft files (daft.yml / daft.local.yml edited since
    daft seeded them) that the default branch's worktree does not cover —
    consolidate with daft-file(1) merge, or answer the interactive prompt

Use -D to override these safety checks. Forcing DISCARDS refined untracked
daft files — they are stashed under `<git-common-dir>/.daft/discarded/<branch>/`
and never merged into another worktree. For the default branch (e.g. main),
-D removes its worktree only — the local branch ref and remote branch are
always preserved.

All targeted branches are validated before any deletions begin. If any branch
fails validation without -D, the entire command aborts and no branches are
deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.

When remote deletion is enabled, the remote-branch delete pushes no content,
so the repo's pre-push hook is skipped by default (configurable via
daft.pushVerify: auto, always, or never; use always for hooks that gate
deletes by ref name). daft.pushVerify is the base setting every daft push
reads, so setting it also affects the branch-creation upstream push;
daft.checkout.pushVerify overrides it for that push alone. Pass --no-verify
to skip it unconditionally.

RENAME MODE (-m)

Renames a local branch and moves its associated worktree directory to match
the new branch name. If the branch has a remote tracking branch, the remote
branch is also renamed (push new name, delete old name) unless --no-remote
is specified. The new-name push honors the repo's pre-push hook; the old-name
delete pushes no content and skips it by default (configurable via
daft.pushVerify).

The source can be specified as a branch name or a path to an existing
worktree (absolute or relative).

If you are currently inside the worktree being renamed, the shell is
redirected to the new worktree location after the rename completes.

Empty parent directories left behind by the move are automatically cleaned up.

## Usage

```
git worktree-branch [OPTIONS] [BRANCHES]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCHES>` | Branches (delete mode) or source + new-name (rename mode) | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-d, --delete` | Delete branches (safe mode) |  |
| `-D, --force` | Force deletion even if not fully merged |  |
| `--local` | Only delete locally, keep remote branch |  |
| `--remote` | Only delete the remote branch, keep local worktree and branch |  |
| `-m, --move` | Rename a branch and move its worktree |  |
| `--no-remote` | Skip remote branch rename (only with -m) |  |
| `--no-verify` | Skip the repo's pre-push hook on remote operations |  |
| `--dry-run` | Preview changes without executing (only with -m) |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Be verbose; show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-checkout](./git-worktree-checkout.md)

