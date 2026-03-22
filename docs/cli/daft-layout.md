---
title: daft-layout
description: Manage worktree layouts
---

# daft layout

Manage worktree layouts

::: tip
See the [Layouts guide](/guide/layouts) for detailed explanations of each layout
and when to use them.
:::

## Description

Manage worktree layouts for daft repositories. Layouts control where worktrees
are placed relative to the repository.

Built-in layouts:

| Layout | Template | Description |
|--------|----------|-------------|
| `contained` | `{{ repo_path }}/{{ branch }}` | Worktrees inside the repo directory |
| `sibling` | `{{ repo }}.{{ branch \| sanitize }}` | Worktrees next to the repo directory (default) |
| `nested` | `{{ repo }}/.worktrees/{{ branch \| sanitize }}` | Worktrees in a hidden subdirectory |
| `centralized` | `{{ daft_data_dir }}/worktrees/{{ repo }}/{{ branch \| sanitize }}` | Worktrees in a central data directory |

Without a subcommand, `daft layout` shows the resolved layout for the current
repository (same as `daft layout show`).

## Usage

```
daft layout [SUBCOMMAND]
```

## Subcommands

### show

Show the resolved layout for the current repository. This is the default when
no subcommand is given.

```
daft layout show
daft layout
```

Output shows the layout name, its template, and where the setting came from
(CLI flag, repo setting, daft.yml, global config, or default):

```
contained  {{ repo_path }}/{{ branch }}  (daft.yml)
```

Must be run from inside a Git repository.

### list

List all available layouts, including custom ones defined in your global config.

```
daft layout list
```

The current repository's layout is marked with an indicator. The global default
is annotated with `(default)`.

### transform

Convert the current repository to a different layout. Moves all worktrees to
their new locations and handles any necessary internal structure changes.

```
daft layout transform <LAYOUT>
```

| Argument | Description | Required |
|----------|-------------|----------|
| `<LAYOUT>` | Target layout name or template | Yes |

| Option | Description |
|--------|-------------|
| `-f, --force` | Proceed even if worktrees have uncommitted changes |

The special value `default` resolves to your global default layout.

```bash
# Convert to contained layout
daft layout transform contained

# Convert to your global default
daft layout transform default

# Use a custom template directly
daft layout transform '{{ repo_path }}/branches/{{ branch | sanitize }}'
```

Must be run from inside a Git repository.

### default

View or change the global default layout. The default is used when no
`--layout` flag, repo setting, or `daft.yml` layout is present.

```
daft layout default [LAYOUT]
```

| Argument | Description | Required |
|----------|-------------|----------|
| `[LAYOUT]` | Layout name or template to set as default | No |

| Option | Description |
|--------|-------------|
| `--reset` | Remove the global default, reverting to the built-in (sibling) |

```bash
# View current default
daft layout default

# Set a new default
daft layout default contained

# Reset to built-in (sibling)
daft layout default --reset
```

The default is stored in `~/.config/daft/config.toml`.

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [Layouts guide](/guide/layouts) — When to use each layout and how they work
- [git-worktree-clone](./git-worktree-clone.md) — `--layout` flag at clone time
- [git-worktree-init](./git-worktree-init.md) — `--layout` flag at init time
- [Configuration](/guide/configuration#layout-settings) — Layout configuration
  options
