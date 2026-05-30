---
title: git-daft-repo-install
description: Install a starter daft.yml in the current worktree
---

# git daft-repo-install

Install a starter daft.yml in the current worktree

## Description

Creates a starter daft.yml at the current worktree root with a commented
skeleton covering the major sections (hooks, shared, layout). Modeled on
`lefthook install`.

This is the canonical name for the bootstrap; `daft install` is a top-level
alias that runs the same thing (so lefthook-style discovery keeps working).

If daft.yml already exists, the command refuses without modifying anything;
edit the existing file with your editor or a future `daft config` TUI.

No git side effects: daft does not write to .gitignore or .git/info/exclude.
Ignore rules are the user's responsibility.

## Usage

```
git daft-repo-install [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-q, --quiet` | Suppress progress reporting |  |
| `-v, --verbose` | Show detailed progress |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

