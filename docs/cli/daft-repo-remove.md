---
title: daft repo remove
description: Remove a repository from the repo catalog
---

# `daft repo remove`

Removes a repository from the [repo catalog](/graph/repo-catalog): the entry is
tombstoned and nothing on disk is touched. With `--purge` it also deletes the
bare directory and every checked-out worktree, running `worktree-pre-remove`
and `worktree-post-remove` lifecycle hooks for each worktree when the repo is
daft-managed and trusted.

## Usage

    daft repo remove [<repo>] [--purge] [--force | -y] [--dry-run] [-v]

| Argument / flag | Description                                                    |
| --------------- | -------------------------------------------------------------- |
| `<repo>`        | Catalog name, uuid, or a repo path (default: the current repo). |
| `--purge`       | Also delete the git dir and every worktree.                     |
| `--force`, `-y` | Skip the confirmation prompt. `--purge` only.                   |
| `--dry-run`     | Print the plan for whichever mode is in effect.                 |
| `-v`            | Show hook details inline.                                       |
| `-vv`           | Force the sequential (non-TUI) output path.                     |

## Addressing a repository

`<repo>` takes the same shapes as
[`daft repo info`](/cli/daft-repo-info): a catalog name, a uuid, or a path —
`.`, a subdirectory, or an absolute or relative directory.

Resolution is catalog-first. A bare word is looked up as a catalog name, so
`./client` is the spelling that insists on a directory when both could match.
A bare word the catalog does not know is retried as a path; when that happens
under `--purge`, the confirmation says so before anything is deleted:

    Remove repo at ~/src/old-repo?
      ('old-repo' is not in the catalog — resolved as a directory)
      This will delete 2 worktrees and the repo. [y/N]

With `-y` the same note goes to stderr. If the catalog cannot be read at all,
the command fails rather than falling through to the filesystem — an outage
never silently becomes "delete the directory of that name instead".

## Default: catalog only

- Tombstones the catalog entry. Nothing on disk changes, no hooks run, and no
  confirmation is asked.
- Reversible: registration is ambient, so the entry returns the next time daft
  runs inside the kept repo, and `daft clone <name>` restores removed entries
  by name.
- Works when the recorded directory is already gone, dropping the stale entry —
  the by-name counterpart of `daft doctor --fix`.
- Does **not** write `DAFT_CD_FILE`: nothing was deleted, so your working
  directory is still valid.

## `--purge`

- Resolves the git dir via `git rev-parse --git-common-dir`. Refuses paths that
  are not inside a Git repository.
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
  for the git dir, and tombstones the catalog entry.
- If invoked from inside the removed repo, writes a safe target path to
  `DAFT_CD_FILE` so the shell wrapper `cd`s out of the deleted directory.
- When the target is addressed by catalog name and the recorded directory is
  already gone, the error points at the plain (catalog-only) form.

## Confirmation

`--purge` prompts before deletion:

    Remove repo at ~/code/myproject? This will delete 3 worktrees and the repo. [y/N]

Only `y` proceeds; anything else aborts. With `--force` (or `-y`) the prompt is
skipped, and in a non-TTY context without it the command exits with an error
rather than proceeding silently. The default removal never prompts — it
destroys nothing — which is why `-y` is rejected without `--purge`.

## Examples

    # Stop cataloging the repo containing the current directory, keep the files
    daft repo remove

    # Same, by catalog name, from anywhere
    daft repo remove myproject

    # Drop a stale catalog entry whose directory is already gone
    daft repo remove old-project

    # Preview either mode
    daft repo remove --dry-run
    daft repo remove --purge --dry-run ~/code/myproject

    # Delete the repository and all its worktrees
    daft repo remove --purge ~/code/myproject

    # …non-interactively
    daft repo remove --purge --force myproject
