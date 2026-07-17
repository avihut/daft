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
- The daft command surface — always the short verbs (`daft go`, `daft start`,
  ...), with a recognition table so agents understand users who type the
  `git worktree-*` or shortcut spellings without ever emitting those forms
- The `daft.yml` hooks system for automating worktree setup
- Environment tool detection (mise, direnv, nvm, pyenv, and more) and suggesting
  automation for projects that lack it
- The machine-readable output contract (`--format json` fields, column and sort
  grammar)
- Worktree-aware translations for common Git intents

When loaded, the agent understands that "create a branch" means `daft start`,
that each worktree needs its own dependency install, and that `daft.yml` hooks
can automate the setup process.

The skill is embedded in the daft binary itself, so the copy daft installs
always documents the binary that installed it.

## Installation

### Via daft (recommended)

```bash
daft skill install
```

Writes the embedded skill to Claude Code's user-global skills directory
(`~/.claude/skills/daft-worktree-workflow/`). No network, no prompts.

**Install is update**: the skill is version-matched to the binary, so after
upgrading daft, run the same command again to refresh the installed copy.

Two more targets are available:

```bash
daft skill install --project      # this worktree's .claude/skills/ — commit it
daft skill install --dir <path>   # another agent's skills root
```

See [`daft skill install`](/reference/cli/daft-skill-install) for details.

### Via npx (alternative)

```bash
npx skills add avihut/daft
```

The [`skills` CLI](https://github.com/anthropics/agent-skills) clones the skill
from the daft repository's `master` branch and knows the skills directories of
many agents — useful when daft has no flag for yours. Re-running it is likewise
its update path.

### Manual

For any other setup, print the embedded skill and place it yourself:

```bash
mkdir -p <skills-root>/daft-worktree-workflow
daft skill show > <skills-root>/daft-worktree-workflow/SKILL.md
```

Unlike downloading `SKILL.md` from the repository, the printed copy always
matches the daft version you are running.

### Removing

```bash
daft skill uninstall                 # user-global copy
daft skill uninstall --project       # this worktree's .claude/skills/
daft skill uninstall --dir <path>    # another agent's skills root
```

Removal is safe by construction: only a `SKILL.md` whose frontmatter marks it as
the daft skill is deleted, and the `daft-worktree-workflow` directory is kept if
you left other files beside the skill. A missing skill is a no-op. See
[`daft skill uninstall`](/reference/cli/daft-skill-uninstall) for details.

## Freshness

Installed skill copies are point-in-time snapshots, and the CLI surface evolves.
daft ships a freshness contract so stale copies get noticed:

- `SKILL.md` carries a `daft_version` frontmatter stamp naming the daft version
  it documents. The stamp is regenerated in every release.
- `daft doctor` compares the installed copy's stamp (user-global always, plus a
  project-level copy when the repo has one) against the running binary: in sync
  passes, stale warns with a `daft skill install` hint, and a copy _newer_ than
  the binary suggests upgrading daft instead.
- `daft doctor --fix` rewrites a stale copy from the embedded skill.
- A skill that is not installed is reported only informationally
  (`daft doctor -v`) — it is never a warning.

The skill itself teaches agents a fallback for the stale case: if daft rejects a
documented command, rediscover the surface via `daft --help` and refresh the
skill with `daft skill install`.

## When the Skill Activates

The skill activates automatically when the agent finds it in its skills search
path. This happens when:

- The skill is installed via any of the routes above
- The user is working in a daft-managed repository (bare `.git/` with worktree
  siblings)
- The user asks about worktree workflows, daft commands, or environment
  isolation

The skill can also be invoked explicitly by the user.

## What the Skill Teaches

### Running daft

Agents run and recommend the short daft verbs, invoking the binary directly. The
`git worktree-*`, `daft worktree-*`, and shortcut spellings survive only in a
recognition table — agents translate user vocabulary into daft verbs without
emitting those forms. A rejected command triggers `--help` rediscovery instead
of a fall back to raw `git worktree` plumbing.

### Detecting daft Repositories

The agent learns to recognize daft layouts — a bare `.git/` directory with
worktree siblings in the contained layout, plus the sibling, nested, and
centralized placements.

### Command Translation

Instead of suggesting `git checkout -b`, the agent suggests `daft start`.
Instead of `git switch`, it navigates to the worktree directory. The skill maps
common Git intents to their daft equivalents, including cross-worktree merges
via `daft merge`.

### Hooks Automation

The skill covers the full `daft.yml` configuration format: hook types, execution
modes, job definitions, dependencies, background jobs, template variables,
skip/only conditions, trust management, and per-invocation `--skip-hooks`.

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

If you prefer not to use the skills system, reference the skill content directly
in your agent's project instructions file (e.g., `CLAUDE.md` for Claude Code).
`daft skill show` prints the authoritative content for your daft version — copy
relevant sections from there.

## See Also

- [`daft skill install`](/reference/cli/daft-skill-install) /
  [`daft skill uninstall`](/reference/cli/daft-skill-uninstall) /
  [`daft skill show`](/reference/cli/daft-skill-show) -- the CLI reference
- [`daft doctor`](/reference/cli/daft-doctor) -- the freshness check
- [Worktrees](/worktrees/) -- understanding the worktree development approach
- [Hooks](/hooks/) -- full `daft.yml` reference and hook system documentation
- [Configuration](./configuration.md) -- all daft configuration options
- [Shell Integration](../getting-started/shell-integration.md) -- setting up
  shell wrappers
