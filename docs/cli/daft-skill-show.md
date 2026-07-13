---
title: daft skill show
description: Print the embedded agent skill to stdout
---

# `daft skill show`

Prints the [agent skill](/reference/agent-skill) embedded in the daft binary.
In a terminal the skill is rendered with daft's markdown styling and shown
through a pager; piped or redirected it is emitted raw, with no decoration and
no color, so it composes.

Use it to inspect exactly what [`daft skill install`](/reference/cli/daft-skill-install)
would write, or to install the skill manually for an agent whose skills
directory daft does not know. The redirected copy is byte-identical to what
`daft skill install` writes and carries the binary's `daft_version` frontmatter
stamp, so manual installs stay covered by the `daft doctor` freshness check.

## Usage

    daft skill show [--no-pager]

## Options

- `--no-pager` — print the rendered skill straight to the terminal without a
  pager.

## Examples

    daft skill show                 # rendered and paged in a terminal
    daft skill show --no-pager      # rendered, no pager
    mkdir -p <skills-root>/daft-worktree-workflow
    daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md   # raw

## See also

- [`daft skill install`](/reference/cli/daft-skill-install) — the managed
  install path
- [Agent skill](/reference/agent-skill) — what the skill teaches and why
