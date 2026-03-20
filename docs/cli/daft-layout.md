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

  contained     Worktrees inside the repo directory (bare required)
  sibling       Worktrees next to the repo directory (default)
  nested        Worktrees in a hidden subdirectory
  centralized   Worktrees in a global ~/worktrees/ directory

Use `daft layout list` to see all available layouts including custom ones
defined in your global config (~/.config/daft/config.toml).

Use `daft layout show` to see the resolved layout for the current repo.

Use `daft layout transform <layout>` to convert a repo between layouts.

## Usage

```
daft layout
```

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

