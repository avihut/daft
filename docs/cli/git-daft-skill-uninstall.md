---
title: git-daft-skill-uninstall
description: Remove the installed agent skill
---

# git daft-skill-uninstall

Remove the installed agent skill

## Description

Removes an agent skill previously written by `git daft skill install` (the
daft-worktree-workflow skill).

By default it removes the user-global copy
(~/.claude/skills/daft-worktree-workflow/). Use --project to remove the
current worktree's .claude/skills/ copy, or --dir to target another
agent's skills root.

Removal is safe by construction: only a SKILL.md whose frontmatter marks
it as the daft skill is deleted, and the daft-worktree-workflow directory
is removed only when nothing else is left inside it, so files you keep
beside the skill are preserved. A missing skill is a no-op, not an error.

## Usage

```
git daft-skill-uninstall [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--project` | Remove from the current worktree's .claude/skills/ instead of ~/.claude/skills |  |
| `--dir <PATH>` | Remove from this skills root (for agents other than Claude Code) |  |
| `-q, --quiet` | Suppress the result line |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

