---
title: daft repo move
description: Move a repository, keeping its worktrees, trust and catalog entry intact
---

# `daft repo move`

Relocates a repository on disk and moves everything keyed to its old path with
it: git's worktree linkage, the trust grant, the layout override, the
[repo catalog](/graph/repo-catalog) entry, and recorded worktree paths.

A plain `mv` breaks all of those and reports nothing. The catalog is the one
piece that heals itself — any daft command running inside the repo lazily
upserts the drifted path — which is exactly why the rest of the breakage stays
invisible: `daft repo list` looks correct while the repo is untrusted, has lost
its layout override, and has worktrees git can no longer resolve.

## Usage

    daft repo move <repo> <dest> [--name <name>] [--dry-run] [-q | -v]

| Argument / flag | Description                                                       |
| --------------- | ----------------------------------------------------------------- |
| `<repo>`        | Repository to move: a catalog name, uuid, or path.                |
| `<dest>`        | New path, or an existing directory to move into (`mv` semantics). |
| `--name <name>` | Catalog name for the repository after the move.                   |
| `--dry-run`     | Print the full plan and exit without changes.                     |
| `-q`            | Suppress progress reporting.                                      |
| `-v`            | Show detailed progress.                                           |

## Which worktrees move

Worktrees fall into three cases, decided before anything moves:

- **Inside the repository directory** — they travel with it. Contained, nested
  and bare layouts put every worktree here.
- **Outside it, but exactly where the layout would place them** — they move
  too, to the path the layout template predicts under the new directory name.
  The default `sibling` layout puts every worktree here, so moving only the
  repository directory would strand all of them.
- **Anywhere else** — you chose that path deliberately (`git daft start --at`),
  or the worktree is detached and unpredictable, so it stays where it is. Its
  git linkage is still repaired, and the summary says it was left behind.

## Behavior

- Resolves `<repo>` through the catalog first, then the filesystem. A catalog
  read failure is an error, never a silent fallback — a repository is not
  moved on a guess.
- Computes the whole plan up front and refuses before touching anything if the
  destination is occupied, is a file, has no parent directory, sits inside the
  repository, or lives on another filesystem.
- Moves the repository directory, then each worktree the layout places outside
  it. If any rename fails, the ones that already happened are moved back.
- Repairs every worktree's git linkage with `git worktree repair`, naming each
  one explicitly. Repair runs immediately after the last rename: until it does,
  git reports every relocated worktree as prunable.
- Re-keys the trust grant and layout override in `repos.json` to the new git
  dir. The grant moves intact — its level, timestamp, and fingerprint are
  preserved, because relocating a repository is not re-granting trust.
- Updates the catalog entry's path. The entry is keyed by the repository's
  uuid, which lives inside the git dir and travels with it, so job history and
  coordinator state follow the repository for free.
- Re-points recorded worktree identities and drops cached sizes.
- If invoked from inside the moved tree, writes the new location to
  `DAFT_CD_FILE` so the shell wrapper `cd`s along with it. A working directory
  outside the move is left alone.

## The catalog name

`--name` always wins. Without it, the name follows the directory when it was
derived from the old one — that is, when the catalog name still matches the old
directory's name — and is left alone when you chose it yourself. A name already
taken by another repository is kept rather than refused: by the time the name
is applied the directories have moved, and failing then would leave you with a
relocated repository and an error.

To change only the name, use [`daft repo rename`](/cli/daft-repo-rename).

## Examples

    # Move and rename in one operation
    daft repo move api ~/Work/api-gateway --name api-gateway

    # Move into an existing directory, keeping the name (mv semantics)
    daft repo move api ~/Work

    # Preview the full plan — directory moves, repairs, re-keys, catalog updates
    daft repo move api ~/Work/api-gateway --dry-run

    # Move a repository daft has never operated in, by path
    daft repo move ./scratch-repo ~/Projects/scratch
