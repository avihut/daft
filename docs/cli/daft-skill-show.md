---
title: daft skill show
description: Print the embedded agent skill to stdout
---

# `daft skill show`

Prints the [agent skill](/reference/agent-skill) embedded in the daft binary
to stdout — raw, with no decoration and no color.

Use it to inspect exactly what [`daft skill install`](/reference/cli/daft-skill-install)
would write, or to install the skill manually for an agent whose skills
directory daft does not know. The printed copy carries the binary's
`daft_version` frontmatter stamp, so manual installs stay covered by the
`daft doctor` freshness check.

## Usage

    daft skill show

## Examples

    daft skill show | less
    mkdir -p <skills-root>/daft-worktree-workflow
    daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md

## See also

- [`daft skill install`](/reference/cli/daft-skill-install) — the managed
  install path
- [Agent skill](/reference/agent-skill) — what the skill teaches and why
