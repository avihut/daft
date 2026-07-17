---
title: git-daft-skill-install
description: Install or update the agent skill for Claude Code
---

# git daft-skill-install

Install or update the agent skill for Claude Code

## Description

Writes the agent skill embedded in this daft binary (the repository's
SKILL.md, skill name `daft-worktree-workflow`) into a skills directory,
creating parent directories as needed.

The embedded skill is version-matched to the binary by construction, so
re-running the command after upgrading daft is also the update path:
install == update. An existing copy is always overwritten unless it is
already identical. The command is non-interactive and never touches the
network.

By default the skill lands in Claude Code's user-global skills directory
(~/.claude/skills/daft-worktree-workflow/SKILL.md). Use --project to
install into the current worktree's .claude/skills/ instead (commit it to
share the skill with everyone who clones the repo), or --dir to target
another agent's skills root; the daft-worktree-workflow folder is always
created inside the chosen root, because the folder name is what agents
resolve the skill by.

`git daft doctor` reports when an installed copy is stale relative to the
running binary, and `git daft doctor --fix` rewrites it with the same
content this command installs.

## Usage

```
git daft-skill-install [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--project` | Install into the current worktree's .claude/skills/ instead of ~/.claude/skills |  |
| `--dir <PATH>` | Install under this skills root (for agents other than Claude Code) |  |
| `-q, --quiet` | Suppress the result line |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

