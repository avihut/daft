---
title: daft-activate
description: Activate daft in this shell
---

# daft activate

Activate daft in this shell

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
  daft activate              # Interactive activation with confirmation
  daft activate --force      # Skip confirmation and re-add if already configured
  daft activate --dry-run    # Show what would be done without making changes

## Usage

```
daft activate [OPTIONS]
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

## See Also

- [daft-shortcuts](./daft-shortcuts.md)
- [daft-shell-init](./daft-shell-init.md)

