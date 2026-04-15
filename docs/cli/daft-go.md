---
title: daft go
description: Open an existing branch in a worktree
---

# daft go

Open an existing branch in a worktree

## Usage

```
daft go [OPTIONS] <BRANCH_NAME>
```

## Description

Creates a new worktree for an existing local or remote branch. The worktree
is placed at the project root level as a sibling to other worktrees, using
the branch name as the directory name.

If the branch exists only on the remote, a local tracking branch is created
automatically. If a worktree for the branch already exists, the working
directory is changed to it.

With `--start` (or `-s`), if the branch does not exist locally or on the
remote, a new branch and worktree are created automatically. This can also be
enabled permanently with `git config daft.go.autoStart true`.

### Previous worktree (`-`)

Use `-` as the branch name to switch to the previous worktree, similar to
`cd -`. Each successful `daft go` or `daft start` records the source worktree,
so repeated `daft go -` toggles between the two most recent worktrees.

```
daft go main        # switch to main
daft go feature/x   # switch to feature/x (main is now "previous")
daft go -           # back to main
daft go -           # back to feature/x
```

Cannot be combined with `-b`/`--create-branch`.

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<BRANCH_NAME>` | Name of the branch to check out; use `-` for previous worktree | Yes |

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `-s, --start` | Create a new worktree if the branch does not exist | |
| `--local` | Skip all remote operations (no fetch) for this invocation | |
| `-x, --exec <EXEC>` | Run a command in the worktree after setup (repeatable) | |
| `--no-cd` | Do not change directory to the new worktree | |
| `-c, --carry` | Apply uncommitted changes from the current worktree | |
| `--no-carry` | Do not carry uncommitted changes | |
| `-r, --remote <REMOTE>` | Remote for worktree organization (multi-remote mode) | |
| `-v, --verbose` | Be verbose; show detailed progress | |
| `-q, --quiet` | Suppress non-error output | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Check out an existing branch
daft go feature/auth

# Switch to the previous worktree (toggle)
daft go -

# Check out a branch, auto-creating if it doesn't exist
daft go -s feature/new-idea

# Check out and run a command after setup
daft go feature/auth -x 'npm install'

# Check out without changing directory
daft go feature/auth --no-cd
```

## Completion behavior

`daft go <TAB>` offers candidates grouped by type:

1. **Worktrees** — branches that already have a linked worktree. These
   are the primary navigation targets and are listed first. The branch
   you are currently sitting in is excluded.
2. **Local branches** — branches in `refs/heads/` that don't have a
   worktree yet. Selecting one of these will check it out into a new
   worktree.
3. **Remote branches** — branches in `refs/remotes/` that don't already
   exist locally. In single-remote mode the `<remote>/` prefix is
   stripped for readability; in multi-remote mode the full
   `<remote>/<branch>` form is preserved.

In zsh and fish, each candidate is annotated with the relative time of
its last commit (e.g. "3 days ago"). In zsh the three groups are
colored distinctly — worktrees in bright green, local branches in
bright blue, remote branches in dim gray. Bash shows a flat list in
the same group order but without colors or descriptions.

### Fetch-on-miss

If you type a prefix that doesn't match any local or already-fetched
remote ref, daft will run `git fetch` once (from the configured
default remote) and re-resolve, showing a spinner while the fetch
runs. This lets you tab-complete to a remote branch that exists
upstream but hasn't been pulled yet.

The fetch path is gated by a 30-second cooldown per repository, so
rapid keystrokes won't trigger repeated fetches. To disable the
feature entirely:

```sh
git config daft.go.fetchOnMiss false
```

## See Also

- [daft start](./daft-start.md) to create a new branch
- [daft config](./daft-config.md) to configure remote sync behavior
- [git worktree-checkout](./git-worktree-checkout.md) for the underlying git-native command
