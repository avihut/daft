---
title: git-daft-repo-move
description: Move a repository, keeping its worktrees, trust and catalog entry intact
---

# git daft-repo-move

Move a repository, keeping its worktrees, trust and catalog entry intact

## Description

Relocates a repository on disk and moves everything keyed to its old path
along with it: git's worktree linkage, the trust grant, the layout override,
the catalog entry, and recorded worktree paths.

`<DEST>` follows `mv` semantics — an existing directory means "move into it",
anything else names the new repository directory outright. The parent has to
exist already.

Worktrees the layout placed outside the repository directory move too. Under
the default `sibling` layout that is all of them, so moving only the
repository directory would strand every worktree it has. A worktree you placed
somewhere yourself stays where you put it; its git linkage is still repaired.

--name also updates the catalog name. Without it the name follows the
directory when it was derived from the old one, and is left alone when you
chose it yourself.

Refuses before touching anything if the destination is occupied, sits inside
the repository, or lives on another filesystem.

## Usage

```
git daft-repo-move [OPTIONS] <REPO> <DEST>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<REPO>` | Repository to move: a catalog name, uuid, or path | Yes |
| `<DEST>` | Where to move it: a new path, or an existing directory to move into | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--name <NAME>` | Catalog name for the repository after the move |  |
| `--dry-run` | Print the full plan without touching anything |  |
| `-q, --quiet` | Suppress progress reporting |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

