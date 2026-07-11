---
title: daft repo remove
description: Remove a Git repository and all its worktrees
---

# `daft repo remove`

Removes a Git repository — bare directory plus every checked-out worktree —
running `worktree-pre-remove` and `worktree-post-remove` lifecycle hooks for
each worktree when the repo is daft-managed and trusted. With `--keep-files`
it instead removes the repo from the [repo catalog](/graph/repo-catalog)
only, leaving everything on disk untouched.

## Usage

    daft repo remove [<path> | --repo <name>] [--keep-files] [--force | -y] [--dry-run] [-v]

| Argument / flag    | Description                                             |
| ------------------ | ------------------------------------------------------- |
| `<path>`           | Repo path or any directory inside it (default: cwd).    |
| `--repo <name>`    | Cataloged repo to remove; exclusive with `<path>`.      |
| `--keep-files`     | Drop the catalog entry only; touch nothing on disk.     |
| `--force`, `-y`    | Skip the interactive confirmation prompt.               |
| `--dry-run`        | Print the removal plan and exit without changes.        |
| `-v`               | Show hook details inline.                               |
| `-vv`              | Force the sequential (non-TUI) output path.             |

## Behavior

- Resolves the git dir via `git rev-parse --git-common-dir`.
  Refuses paths that are not inside a Git repository.
- Enumerates all checked-out worktrees via `git worktree list --porcelain`.
- For each worktree, runs `worktree-pre-remove` (if configured and trusted),
  removes the worktree, then runs `worktree-post-remove`.
- Hook failures **do not abort** the run. The repo is removed regardless;
  failed hooks appear in the post-run summary.
- `worktree-post-remove` fires **after** the worktree directory has been
  deleted — `$DAFT_WORKTREE_PATH` points at a directory that no longer exists
  on disk. Hook scripts that need to inspect the worktree must do so in
  `worktree-pre-remove` instead. `$DAFT_SOURCE_WORKTREE` (the main worktree)
  is still present at `post-remove` time unless it itself is the worktree
  being removed.
- After all worktrees are gone, removes the git dir and the project root if
  it is empty. **Does not** walk further up — the parent directory of the
  project root is user-owned and is left untouched. Drops the trust marker
  for the git dir.
- If invoked from inside the removed repo, writes a safe target path to
  `DAFT_CD_FILE` so the shell wrapper `cd`s out of the deleted directory.
- `--keep-files` tombstones the catalog entry and stops there: no hooks run,
  nothing on disk changes, and no confirmation is asked (the operation is
  reversible — registration is ambient, so the entry returns the next time
  daft runs inside the kept repo, and `daft clone <name>` restores removed
  entries by name). Combined with `--repo` it also works when the recorded
  directory is already gone, dropping the stale entry — the by-name
  counterpart of `daft doctor --fix`.
- `--repo <name>` resolves the target through the catalog instead of the
  filesystem. Full removal still requires the recorded directory to exist;
  when it doesn't, the error points at `--keep-files`.

## Confirmation

By default, prompts before deletion. When the repo is cataloged, the prompt
offers three choices — `y` removes everything, `k` keeps the files and only
drops the catalog entry, anything else aborts:

    Remove repo at ~/code/myproject? This will delete 3 worktrees and the repo.
    [y] remove  [k] keep files, only drop the catalog entry  [N] abort:

With `--force` (or `-y`) the prompt is skipped. In a non-TTY context without
`--force`, the command exits with an error rather than proceeding silently
(`--keep-files` is exempt: it never destroys anything, so it never prompts).

## Examples

    # Remove the repo containing the current directory
    daft repo remove

    # Remove a repo by path
    daft repo remove ~/code/myproject

    # Remove a cataloged repo by name
    daft repo remove --repo myproject -y

    # Stop cataloging the current repo but keep it on disk
    daft repo remove --keep-files

    # Drop a stale catalog entry whose directory is already gone
    daft repo remove --keep-files --repo old-project

    # Preview what would happen
    daft repo remove --dry-run ~/code/myproject

    # Remove non-interactively
    daft repo remove --force ~/code/myproject
