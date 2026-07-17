---
title: daft skill uninstall
description: Remove the installed agent skill
---

# `daft skill uninstall`

Removes an [agent skill](/reference/agent-skill) previously written by
[`daft skill install`](/reference/cli/daft-skill-install) (the
`daft-worktree-workflow` skill).

Removal is safe by construction: only a `SKILL.md` whose frontmatter marks it
as the daft skill is deleted, and the `daft-worktree-workflow` directory is
removed only when nothing else is left inside it — files you keep beside the
skill are preserved. A missing skill is a no-op, not an error, so it can be run
blindly.

## Usage

    daft skill uninstall [--project | --dir <path>] [-q] [-v]

| Flag              | Description                                                          |
| ----------------- | ------------------------------------------------------------------- |
| _(none)_          | Remove from Claude Code's user-global `~/.claude/skills/`.           |
| `--project`       | Remove from the current worktree's `.claude/skills/` instead.        |
| `--dir <path>`    | Remove from this skills root (for agents other than Claude Code).    |
| `-q`, `--quiet`   | Suppress the result line.                                            |
| `-v`, `--verbose` | Show detailed progress.                                             |

`--project` and `--dir` are mutually exclusive. A file that does not look like
the daft skill (its frontmatter names a different skill) is left in place with
an error, never deleted silently.

## Examples

    daft skill uninstall                     # user-global (Claude Code)
    daft skill uninstall --project           # this worktree's .claude/skills/
    daft skill uninstall --dir ~/.config/agent/skills

## See also

- [`daft skill install`](/reference/cli/daft-skill-install) — the install /
  update path
- [Agent skill](/reference/agent-skill) — what the skill teaches and why
- [`daft doctor`](/reference/cli/daft-doctor) — the freshness check
