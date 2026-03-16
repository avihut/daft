---
title: daft sync
description: Synchronize worktrees with remote (prune + update + push)
---

# daft sync

Synchronize worktrees with remote (prune + update all)

## Usage

```
daft sync [OPTIONS]
```

## Description

Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. **Prune**: fetches with `--prune`, removes worktrees and branches for
     deleted remote branches, executes lifecycle hooks for each removal.
  2. **Update**: pulls all remaining worktrees from their remote tracking
     branches.
  3. **Rebase** (`--rebase BRANCH`): rebases all remaining worktrees onto
     BRANCH. Best-effort: conflicts are immediately aborted and reported.
  4. **Push** (`--push`): pushes all branches to their remote tracking branches.
     Branches without an upstream are skipped. Push failures are reported as
     warnings; they do not cause sync to fail. Use `--force-with-lease` with
     `--push` to force-push rebased branches.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
`daft.prune.cdTarget`).

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.

### Ownership-gated rebase and push

When `--rebase` or `--push` is specified, daft applies these operations only to
branches you own. A branch is considered yours when its tip commit author email
matches your `git config user.email`.

Branches owned by others are still fetched and updated (pulled), but they are
not rebased or pushed. This protects teammates' branches from unintended rewrites
when multiple people share a repository.

The summary table shown after sync includes an **Owner** column displaying the
tip commit author email for each branch, making it easy to see who owns what.

### Controlling which branches are rebased and pushed

Use `--include` to opt additional branches into the rebase and push phases. The
flag is repeatable and accepts three value forms:

| Value | Effect |
|-------|--------|
| `unowned` | Include all branches regardless of owner |
| `alice@example.com` | Include all branches owned by that email |
| `feature/my-branch` | Include that specific branch by name |

Examples:

```bash
# Rebase and push your own branches plus a specific colleague's branch
daft sync --rebase main --push --include bob@example.com

# Rebase and push every branch (no ownership filtering)
daft sync --rebase main --push --include unowned

# Include one specific branch by name
daft sync --rebase main --push --include feature/shared-work
```

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-v, --verbose` | Increase verbosity (`-v` for hook details, `-vv` for full sequential output) | |
| `-f, --prune-dirty` | Force removal of worktrees with uncommitted changes | |
| `--rebase <BRANCH>` | Rebase all branches onto BRANCH after updating | |
| `--autostash` | Automatically stash/unstash uncommitted changes during rebase (requires `--rebase`) | |
| `--push` | Push all branches to their remotes after syncing | |
| `--force-with-lease` | Use `--force-with-lease` when pushing (requires `--push`) | |
| `--include <VALUE>` | Include additional branches in rebase/push: `unowned`, an email address, or a branch name. Repeatable. | |
| `--stat <STAT>` | Statistics mode: `summary` or `lines` (default: from git config `daft.sync.stat`, or `summary`) | |
| `--columns <COLUMNS>` | Columns to display in the summary table (comma-separated). Replace mode: `branch,path,age`. Modifier mode: `+col,-col`. The status column is always shown. | |

::: info
The `--force` flag is a deprecated alias for `--prune-dirty` and will be removed
in a future release.
:::

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Prune stale worktrees and update all remaining ones
daft sync

# Sync with hook details shown in the TUI
daft sync -v

# Sync with full sequential output (no TUI)
daft sync -vv

# Sync and rebase all worktrees onto main
daft sync --rebase main

# Sync, rebase onto main, and autostash uncommitted changes
daft sync --rebase main --autostash

# Sync and push all branches to their remotes
daft sync --push

# Full workflow: sync, rebase onto main, and push (force-with-lease for rebased branches)
daft sync --rebase main --push --force-with-lease

# Force sync even if worktrees have uncommitted changes
daft sync --prune-dirty

# Rebase and push your branches plus a teammate's branches
daft sync --rebase main --push --include alice@example.com

# Rebase and push all branches regardless of owner
daft sync --rebase main --push --include unowned

# Show the Owner column in the summary table
daft sync --columns +owner
```

## See Also

- [git worktree-sync](./git-worktree-sync.md) for the underlying git-native command
- [daft prune](./daft-prune.md) to prune stale worktrees only
- [daft update](./daft-update.md) to update branches only
