---
title: Agent Skill
description:
  Teach AI coding agents the daft worktree workflow with the
  daft-worktree-workflow skill
---

# Agent Skill

daft provides an [Agent Skill](https://github.com/anthropics/agent-skills) that
teaches AI coding agents the daft worktree workflow. The skill follows an open
standard and works with any agent that supports skills, including Claude Code,
Cursor, Windsurf, and others.

## What Is It

The `daft-worktree-workflow` skill is a `SKILL.md` file that agents load as
context when working in daft-managed repositories. It contains structured
knowledge about:

- The worktree-centric development philosophy
- All daft commands and when to use each one
- The `daft.yml` hooks system for automating worktree setup
- Environment tool detection (mise, direnv, nvm, pyenv, and more)
- How to suggest automation for projects that lack it
- Correct worktree-aware translations for common Git operations

When loaded, the agent understands that "create a branch" means
`git worktree-checkout-branch`, that each worktree needs its own dependency
install, and that `daft.yml` hooks can automate the setup process.

## Installation

### Via npx (recommended)

```bash
npx skills add avihut/daft
```

This clones the skill from the daft repository and installs it into your agent's
skills directory.

### Manual -- User-Global

Copy the `SKILL.md` file to your agent's skills directory. For example, with
Claude Code:

```bash
mkdir -p ~/.claude/skills/daft-worktree-workflow
curl -o ~/.claude/skills/daft-worktree-workflow/SKILL.md \
  https://raw.githubusercontent.com/avihut/daft/master/SKILL.md
```

The skill will be available in all sessions. Consult your agent's documentation
for the correct skills directory if using a different tool.

### Manual -- Project-Level

To include the skill for a specific project, copy it into the project's skills
directory:

```bash
mkdir -p .claude/skills/daft-worktree-workflow
cp /path/to/SKILL.md .claude/skills/daft-worktree-workflow/SKILL.md
```

Commit it to the repository so all contributors benefit.

## When the Skill Activates

The skill activates automatically when the agent finds it in its skills search
path. This happens when:

- The skill is installed via `npx skills add` or manually placed in the skills
  directory
- The user is working in a daft-managed repository (bare `.git/` with worktree
  siblings)
- The user asks about worktree workflows, daft commands, or environment
  isolation

The skill can also be invoked explicitly by the user.

## What the Skill Teaches

### Detecting daft Repositories

The agent learns to recognize the daft directory layout: a bare `.git/`
directory at the project root with branch worktrees as sibling directories.

### Command Translation

Instead of suggesting `git checkout -b`, the agent suggests
`git worktree-checkout-branch`. Instead of `git switch`, it suggests navigating
to the worktree directory. The skill maps common Git intents to their daft
equivalents.

### Hooks Automation

The skill covers the full `daft.yml` configuration format: hook types, execution
modes (parallel, piped, follow), job definitions, dependencies, template
variables, skip/only conditions, and trust management.

### Environment Tooling

When the agent encounters a daft repo, it checks for environment tool markers
(`.mise.toml`, `.envrc`, `.nvmrc`, `package.json`, `Cargo.toml`, etc.) and
suggests `daft.yml` hooks that automate tool setup for new worktrees.

### Per-Worktree Isolation

The skill emphasizes that each worktree is a fully isolated workspace.
Dependencies, build artifacts, and environment config are not shared. This means
`npm install` must run in each worktree, virtual environments must be created
separately, and so on.

## Manual Integration

If you prefer not to use the skills system, you can reference the skill content
directly in your agent's project instructions file (e.g., `CLAUDE.md` for Claude
Code). The `SKILL.md` file at the repository root is the source of truth. Copy
relevant sections into your project's instructions file.

## See Also

- [Worktree Workflow](./worktree-workflow.md) -- understanding the worktree
  development approach
- [Hooks](./hooks.md) -- full `daft.yml` reference and hook system documentation
- [Configuration](./configuration.md) -- all daft configuration options
- [Shell Integration](../getting-started/shell-integration.md) -- setting up
  shell wrappers
- [Shortcuts](./shortcuts.md) -- command alias management
