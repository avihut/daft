---
title: daft-layout
description: Manage worktree layouts
---

# daft layout

Manage worktree layouts

## Description

Manage worktree layouts for daft repositories.

Layouts control where worktrees are placed relative to the bare repository.
Built-in layouts:

  contained           Worktrees inside the repo directory (bare required)
  contained-classic   Like contained but default branch is a regular clone
  contained-flat      Like contained but branch slashes flattened to dashes
  sibling             Worktrees next to the repo directory (default)
  nested              Worktrees in a hidden subdirectory
  centralized         Worktrees in a global ~/worktrees/ directory

Use `daft layout list` to see all available layouts including custom ones
defined in your global config (~/.config/daft/config.toml).

Use `daft layout show` to see the resolved layout for the current repo.

Use `daft layout transform <layout>` to convert a repo between layouts.

Use `daft layout default` to view or change the global default layout.

## Usage

```
daft layout
```

## Subcommands

### list

List all available layouts

```
daft layout list
```

### show

Show the resolved layout for the current repo

```
daft layout show [PATH]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to a git repository (defaults to current directory) | No |

### transform

Transform the current repo to a different layout

```
daft layout transform [OPTIONS] <LAYOUT>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<LAYOUT>` | Target layout name or template | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `-f, --force` | Force transform even with uncommitted changes |  |
| `--dry-run` | Show plan without executing |  |
| `--include <BRANCH>` | Also relocate this non-conforming worktree (repeatable) |  |
| `--include-all` | Relocate all non-conforming worktrees |  |
| `-q, --quiet` | Operate quietly; suppress progress reporting |  |
| `-v, --verbose` | Show detailed hook execution |  |

### default

View or set the global default layout

```
daft layout default [OPTIONS] [LAYOUT]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<LAYOUT>` | Layout name or template to set as the global default | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--reset` | Remove the global default, reverting to built-in (sibling) |  |

### reset

Clear the stored layout for a repo

```
daft layout reset [PATH]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to a git repository (defaults to current directory) | No |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

