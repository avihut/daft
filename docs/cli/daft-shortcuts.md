---
title: daft-shortcuts
description: Manage command shortcut symlinks
---

# daft shortcuts

Manage command shortcut symlinks

## Description

Manage shortcut symlinks for daft commands.

Shortcuts provide short aliases for frequently used commands:
  - Git style:    gwtclone, gwtco, gwtcb, gwtprune, gwtcarry, gwtfetch, gwtinit, gwtbd
  - Shell style:  gwco, gwcob
  - Legacy style: gclone, gcw, gcbw, gprune

Default-branch shortcuts (gwtcm, gwtcbm, gwcobd, gcbdw) are available
via shell integration only (daft shell-init).

Examples:
  daft setup shortcuts                    # Show current status
  daft setup shortcuts list               # List all shortcut styles
  daft setup shortcuts enable git         # Enable git-style shortcuts
  daft setup shortcuts disable legacy     # Disable legacy shortcuts
  daft setup shortcuts only shell         # Enable only shell shortcuts

## Usage

```
daft shortcuts
```

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

