---
title: daft skill install
description: Install or update the agent skill for Claude Code
---

# `daft skill install`

Writes the [agent skill](/reference/agent-skill) embedded in the daft binary
(`daft-worktree-workflow`) into a skills directory, creating parent
directories as needed.

The embedded skill is version-matched to the binary by construction, so
re-running the command after upgrading daft is also the update path: install
== update. An existing copy is always overwritten unless it is already
identical. The command is non-interactive and never touches the network.

## Usage

    daft skill install [--project | --dir <path>] [-q] [-v]

| Flag              | Description                                                             |
| ----------------- | ----------------------------------------------------------------------- |
| _(none)_          | Install to Claude Code's user-global `~/.claude/skills/`.               |
| `--project`       | Install into the current worktree's `.claude/skills/` instead.          |
| `--dir <path>`    | Install under this skills root (for agents other than Claude Code).     |
| `-q`, `--quiet`   | Suppress the result line.                                               |
| `-v`, `--verbose` | Show detailed progress.                                                 |

`--project` and `--dir` are mutually exclusive. The `daft-worktree-workflow`
folder is always created inside the chosen root — the folder name is what
agents resolve the skill by. A project-level copy is meant to be committed, so
everyone who clones the repo gets the skill.

## Examples

    daft skill install                       # user-global (Claude Code)
    daft skill install --project             # this worktree's .claude/skills/
    daft skill install --dir ~/.config/agent/skills

## Freshness

`daft doctor` compares an installed copy's `daft_version` frontmatter stamp
against the embedded skill and warns when it is stale;
`daft doctor --fix` rewrites it with the same content this command installs.

## See also

- [Agent skill](/reference/agent-skill) — what the skill teaches and why
- [`daft skill show`](/reference/cli/daft-skill-show) — print the embedded
  skill (manual installs)
- [`daft doctor`](/reference/cli/daft-doctor) — the freshness check
