---
title: git-worktree-exec
description: Run a command across one or more worktrees
---

# git worktree-exec

Run a command across one or more worktrees

::: tip
This command is also available as `daft exec`. See [daft exec](./daft-exec.md).
:::

## Description

Runs one or more commands against one or more selected worktrees without
changing the current directory.

Targets may be given as positional branch or worktree-directory names, or
globs against branch names (e.g. 'feat/*'). Use --all to target every
worktree in the repository. Positionals and --all are mutually exclusive.

Commands are expressed either as a literal argv after --, or as one or
more -x shell strings. The two forms are mutually exclusive. Multiple -x
values run sequentially per worktree; a failure stops that worktree but
does not stop other worktrees.

When a single worktree is targeted, stdio is fully inherited, making
interactive programs (claude, vim, fzf) work the same as if you had cd'd
into the worktree first.

On an interactive terminal, multi-worktree runs render a live rail: one row
per worktree, filled in place, persisted as a receipt. Failed worktrees thread
their captured output under their row; pass -v to thread every worktree's
output (and, when stdout is redirected, dump successful worktrees' output too).
The flag has no effect on single-target runs (stdio is already inherited).

## Usage

```
git worktree-exec [OPTIONS] [TARGETS] [CMD]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGETS>` | Target worktree(s) by branch name, directory name, or glob | No |
| `<CMD>` | Trailing command vector after `--`. Mutually exclusive with `-x` | No |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--all` | Target every worktree in the repository |  |
| `--repo <REPO>` | Run in another cataloged repository (targets and --all apply there) |  |
| `--all-repos` | Run in every cataloged repository's default-branch worktree |  |
| `--related` | Run across this repo and its related repos (relations manifest), in each one's worktree for the current branch |  |
| `-x, --exec <CMD>` | Shell command to run (repeatable); runs via $SHELL -c |  |
| `--sequential` | Run worktrees one at a time and stop on first failure |  |
| `--keep-going` | Run worktrees one at a time and continue through failures |  |
| `--refresh-aliases` | Re-capture user shell aliases instead of using the cached snapshot |  |
| `-v, --verbose` | Thread each worktree's full output into the rail and dump captured output for successful worktrees too (no-op for single-target runs) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [git-worktree-sync](./git-worktree-sync.md)
- [git-worktree-list](./git-worktree-list.md)
- [git-worktree-carry](./git-worktree-carry.md)

