---
title: git-daft-repo-link
description: Declare a relation from this repo to another
---

# git daft-repo-link

Declare a relation from this repo to another

## Description

Adds a directed relation from the current repo to a target, writing a
well-formed entry into the current worktree's daft.yml `relations:` list. The
manifest is committed and team-shared; relations power `git daft exec
--related`, `git daft start --with-related`, and the `git daft repo info`
Relations section.

The target is resolved to a remote URL — the portable key relations match on —
in this order: a catalog repo name (or a repo path daft has cataloged), then a
path to a git repo on disk, then a remote URL used as-is. A URL that isn't
cloned yet is fine: the edge resolves as "not cloned" until it is.

Linking is idempotent. Re-linking an existing edge is a no-op; passing --name
or --kind updates that edge in place. Editing only touches the `relations:`
block — comments and formatting elsewhere in daft.yml are preserved. Commit the
result to share the relation with your team.

## Usage

```
git daft-repo-link [OPTIONS] <TARGET>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGET>` | Catalog repo name, a repo path, or a remote URL to link to | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--name <LABEL>` | Friendly label for the edge (defaults to the URL's last path segment) |  |
| `--kind <KIND>` | Free-form relationship kind (e.g. consumer, library) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

