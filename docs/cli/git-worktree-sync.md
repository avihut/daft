---
title: git-worktree-sync
description: Synchronize worktrees with remote (prune + update all)
---

# git worktree-sync

Synchronize worktrees with remote (prune + update all)

::: tip
This command is also available as `daft sync`. See [daft sync](./daft-sync.md).
:::

## Description

Synchronizes all worktrees with the remote in a single command.

This is equivalent to running `daft prune` followed by `daft update --all`:

  1. Prune: fetches with --prune, removes worktrees and branches for deleted
     remote branches, executes lifecycle hooks for each removal.
  2. Update: pulls all remaining worktrees from their remote tracking branches.
  3. Rebase (--rebase BRANCH): rebases all remaining worktrees onto BRANCH.
     Best-effort: conflicts are immediately aborted and reported.
  4. Push (--push): pushes all branches to their remote tracking branches.
     Branches without an upstream are skipped. Push failures are reported as
     warnings; they do not cause sync to fail. Use --force-with-lease with
     --push to force-push rebased branches.

If you are currently inside a worktree that gets pruned, the shell is redirected
to a safe location (project root by default, or as configured via
daft.prune.cdTarget).

Resource governing: parallel pushes with a pre-push hook are memory-governed.
Concurrency is capped (default max(2, cores/4); `--jobs N` overrides,
`--no-throttle` disables), admissions pause under memory pressure, each hook's
peak memory is learned across runs, and under sustained pressure the newest
push is frozen — then killed and retried — instead of exhausting the machine.
Every push unit gets a wall-clock budget (`daft.sync.pushTimeout`, default
30m). `daft.sync.pushHookStrategy batched` pushes all branches in one
`git push` so the hook runs once with every ref.

Cancellation: the first Ctrl+C (or SIGTERM) cancels gracefully — no new work
starts and every running git subprocess is torn down. A pre-push hook and all
of its descendants are killed by process group, reaching even stages that moved
to their own process groups or were stopped by terminal job control; an
interrupted rebase is aborted to restore the worktree; and sync prints partial
results and exits 130. A second Ctrl+C force-kills anything still running and
exits immediately.

For fine-grained control over either phase, use `daft prune` and `daft update`
separately.

## Usage

```
git worktree-sync [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-v, --verbose` | Increase verbosity (-v for hook details, -vv for full sequential output) |  |
| `-f, --prune-dirty` | Force removal of worktrees with uncommitted changes |  |
| `--force` | Hidden deprecated alias for --prune-dirty |  |
| `--rebase <BRANCH>` | Rebase all branches onto BRANCH after updating |  |
| `--autostash` | Automatically stash and unstash uncommitted changes before/after rebase |  |
| `--push` | Push all branches to their remotes after syncing |  |
| `--force-with-lease` | Use --force-with-lease when pushing (requires --push) |  |
| `--no-verify` | Skip the repo's pre-push hook when pushing (requires --push) |  |
| `--include <INCLUDE>` | Include additional branches in rebase/push (email, branch name, or 'unowned') |  |
| `--stat <STAT>` | Statistics mode: summary or lines (default: from git config daft.sync.stat, or summary) |  |
| `--columns <COLUMNS>` | Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col. Available: branch, path, size, base, changes, remote, pr, age, annotation, owner, hash, last-commit |  |
| `--sort <SORT>` | Sort order (comma-separated). +col ascending, -col descending. Columns: branch, path, size, base, changes, remote, age, owner, hash, activity, commit |  |
| `--jobs <N>` | Cap concurrent pushes when a pre-push hook is present (requires --push; default: from daft.governor.jobs, or max(2, cores/4)) |  |
| `--no-throttle` | Disable the push resource governor for this run (requires --push) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-prune](./git-worktree-prune.md)
- [git-worktree-fetch](./git-worktree-fetch.md)
- [git-worktree-push](./git-worktree-push.md)

