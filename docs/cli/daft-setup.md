---
title: daft-setup
description: Add daft shell integration to your shell config
---

# daft setup

Add daft shell integration to your shell config

## Description

Automatically adds the `daft shell-init` line to your shell configuration file.
This enables automatic cd into new worktrees when using daft commands.

The command will:

1. Detect your shell (bash, zsh, or fish)
2. Find the appropriate config file (`~/.bashrc`, `~/.zshrc`, or
   `~/.config/fish/config.fish`)
3. Check if daft is already configured (won't add duplicates)
4. Create a backup of your config file
5. Append the shell-init line
6. Install git-style shortcuts (gwtco, gwtcb, etc.)

## Usage

```
daft setup [OPTIONS]
```

## Options

| Option           | Description                                                | Default |
| ---------------- | ---------------------------------------------------------- | ------- |
| `-f, --force`    | Skip confirmation and re-add if already configured         |         |
| `--dry-run`      | Show what would be done without making changes             |         |

## Global Options

| Option         | Description               |
| -------------- | ------------------------- |
| `-h`, `--help` | Print help information    |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Interactive setup with confirmation
daft setup

# Skip confirmation
daft setup --force

# Preview without making changes
daft setup --dry-run
```

---

## Subcommand: shortcuts

Manage command shortcut symlinks.

### Usage

```
daft setup shortcuts [SUBCOMMAND]
```

Without a subcommand, shows the current shortcut installation status.

### Shortcut Styles

| Style    | Shortcuts                                                             |
| -------- | --------------------------------------------------------------------- |
| `git`    | gwtclone, gwtinit, gwtco, gwtcb, gwtbd, gwtprune, gwtcarry, gwtfetch |
| `shell`  | gwco, gwcob                                                          |
| `legacy` | gclone, gcw, gcbw, gprune                                           |

Default-branch shortcuts (`gwtcm`, `gwtcbm`, `gwcobd`, `gcbdw`) are
shell-only functions provided by `daft shell-init`, not managed here.

### shortcuts list

List all shortcut styles and their aliases.

```
daft setup shortcuts list
```

### shortcuts status

Show currently installed shortcuts (default when no subcommand given).

```
daft setup shortcuts status
```

### shortcuts enable

Enable a shortcut style by creating symlinks.

```
daft setup shortcuts enable <STYLE> [OPTIONS]
```

| Argument / Option          | Description                        |
| -------------------------- | ---------------------------------- |
| `<STYLE>`                  | Style to enable (git, shell, legacy) |
| `--install-dir <DIR>`      | Override installation directory     |
| `--dry-run`                | Preview without making changes     |

### shortcuts disable

Disable a shortcut style by removing its symlinks.

```
daft setup shortcuts disable <STYLE> [OPTIONS]
```

| Argument / Option          | Description                          |
| -------------------------- | ------------------------------------ |
| `<STYLE>`                  | Style to disable (git, shell, legacy) |
| `--install-dir <DIR>`      | Override installation directory       |
| `--dry-run`                | Preview without making changes       |

### shortcuts only

Enable only the specified style and disable all others.

```
daft setup shortcuts only <STYLE> [OPTIONS]
```

| Argument / Option          | Description                                     |
| -------------------------- | ----------------------------------------------- |
| `<STYLE>`                  | Style to enable exclusively (git, shell, legacy) |
| `--install-dir <DIR>`      | Override installation directory                  |
| `--dry-run`                | Preview without making changes                  |

### Examples

```bash
# See what's installed
daft setup shortcuts status

# List all available styles
daft setup shortcuts list

# Enable git-style shortcuts
daft setup shortcuts enable git

# Switch to shell-style only
daft setup shortcuts only shell

# Preview changes
daft setup shortcuts only git --dry-run
```

## See Also

- [daft-shell-init](./daft-shell-init.md)
- [Shortcuts guide](../guide/shortcuts.md)
