---
title: daft-setup
description: Add daft shell integration to your shell config
---

# daft setup

Add daft shell integration to your shell config

## Description

Automatically adds the daft shell-init line to your shell configuration file.

This enables automatic cd into new worktrees when using daft commands.

The command will:
  1. Detect your shell (bash, zsh, or fish)
  2. Find the appropriate config file (~/.bashrc, ~/.zshrc, or ~/.config/fish/config.fish)
  3. Check if daft is already configured (won't add duplicates)
  4. Create a backup of your config file
  5. Append the shell-init line

Examples:
  daft setup              # Interactive setup with confirmation
  daft setup --force      # Skip confirmation and re-add if already configured
  daft setup --dry-run    # Show what would be done without making changes

## Usage

```
daft setup [OPTIONS]
```

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `-f, --force` | Skip confirmation and re-add if already configured |  |
| `--dry-run` | Show what would be done without making changes |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

