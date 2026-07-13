---
title: git-daft-skill-show
description: Print the embedded agent skill to stdout
---

# git daft-skill-show

Print the embedded agent skill to stdout

## Description

Prints the agent skill embedded in this daft binary (the repository's
SKILL.md, skill name `daft-worktree-workflow`) to stdout, with no
decoration and no color.

Use it to inspect exactly what `git daft skill install` would write, or to
install the skill manually for an agent whose skills directory daft does
not know:

    daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md

The printed copy carries the `daft_version` frontmatter stamp of this
binary, so manual installs stay covered by the `git daft doctor` freshness
check.

## Usage

```
git daft-skill-show
```

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

