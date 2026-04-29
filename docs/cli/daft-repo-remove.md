---
title: daft repo remove
description: Remove a Git repository and all its worktrees
---

# `daft repo remove`

Removes a Git repository — bare directory plus every checked-out worktree —
running `worktree-pre-remove` and `worktree-post-remove` lifecycle hooks for
each worktree when the repo is daft-managed and trusted.

## Usage

    daft repo remove [<path>] [--force | -y] [--dry-run] [-v]

| Argument / flag    | Description                                             |
| ------------------ | ------------------------------------------------------- |
| `<path>`           | Repo path or any directory inside it (default: cwd).    |
| `--force`, `-y`    | Skip the interactive confirmation prompt.               |
| `--dry-run`        | Print the removal plan and exit without changes.        |
| `-v`               | Show hook details inline.                               |
| `-vv`              | Force the sequential (non-TUI) output path.             |

## Behavior

- Resolves the bare git directory via `git rev-parse --git-common-dir`.
  Refuses paths that are not inside a Git repository.
- Enumerates all checked-out worktrees via `git worktree list --porcelain`.
- For each worktree, runs `worktree-pre-remove` (if configured and trusted),
  removes the worktree, then runs `worktree-post-remove`.
- Hook failures **do not abort** the run. The repo is removed regardless;
  failed hooks appear in the post-run summary.
- After all worktrees are gone, removes the bare git directory and walks
  upward removing any now-empty parent directories. Drops the trust DB entry
  for the bare git path.
- If invoked from inside the removed repo, writes a safe target path to
  `DAFT_CD_FILE` so the shell wrapper `cd`s out of the deleted directory.

## Confirmation

By default, prompts before deletion. With `--force` (or `-y`), or in
non-interactive mode, the prompt is skipped. In a non-TTY context without
`--force`, the command exits with an error rather than proceeding silently.

## Examples

    # Remove the repo containing the current directory
    daft repo remove

    # Remove a repo by path
    daft repo remove ~/code/myproject

    # Preview what would happen
    daft repo remove --dry-run ~/code/myproject

    # Remove non-interactively
    daft repo remove --force ~/code/myproject
