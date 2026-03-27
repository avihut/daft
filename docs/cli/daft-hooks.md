---
title: daft-hooks
description: Manage repository trust for hook execution
---

# daft hooks

Manage repository trust for hook execution

## Description

Manage trust settings for repository hooks in .daft/hooks/.

Trust levels:
  deny     Do not run hooks (default)
  prompt   Prompt before each hook
  allow    Run hooks automatically

Trust applies to all worktrees. Without a subcommand, shows status.

## Usage

```
daft hooks [PATH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to check (defaults to current directory) | No |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

