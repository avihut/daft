---
title: git-daft-repo-unlink
description: Remove a relation from this repo
---

# git daft-repo-unlink

Remove a relation from this repo

## Description

Removes a directed relation declared in the current worktree's daft.yml. The
target is matched against existing entries first by friendly label, then by
resolving it (catalog name, repo path, or remote URL) to a remote URL and
matching on that — so `unlink` accepts the same forms as `link`, plus the label
shown by `git daft repo info`.

Unlinking an edge that isn't there is a friendly no-op, not an error. Only the
`relations:` block is touched; the rest of daft.yml is left intact. Commit the
result to share the change with your team.

## Usage

```
git daft-repo-unlink <TARGET>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<TARGET>` | Relation label, catalog repo name, repo path, or remote URL to unlink | Yes |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

