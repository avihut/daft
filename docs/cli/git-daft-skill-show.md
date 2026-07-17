---
title: git-daft-skill-show
description: Print the embedded agent skill
---

# git daft-skill-show

Print the embedded agent skill

## Description

Prints the agent skill embedded in this daft binary (the repository's
SKILL.md, skill name `daft-worktree-workflow`).

In a terminal the skill is rendered with daft's markdown styling and shown
through a pager; piped or redirected it is emitted raw, with no decoration
and no color, so it composes:

    daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md

installs the skill manually for an agent whose skills directory daft does
not know, byte-identical to what `git daft skill install` would write. The
printed copy carries the `daft_version` frontmatter stamp of this binary,
so manual installs stay covered by the `git daft doctor` freshness check.

Pass --no-pager to print the rendered skill straight to the terminal
without a pager.

## Usage

```
git daft-skill-show [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--no-pager` | Print rendered output directly instead of through a pager |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

