---
title: daft-completions
description: Generate shell completion scripts for daft commands
---

# daft completions

Generate shell completion scripts for daft commands

## Description

Generate shell completion scripts that provide tab completion for daft commands,
including static flag completions and dynamic branch name completions.

Supported targets:

- **bash** -- Bash completion scripts using `_init_completion`
- **zsh** -- Zsh completion scripts using `compdef`/`compadd`
- **fish** -- Fish completion scripts using `complete`
- **fig** -- Fig/Amazon Q/Kiro completion specs (ESM format)

By default, generates completions for all worktree commands. Use `--command` to
generate for a specific command only.

Shell completions are also included automatically in the output of
`daft shell-init`, so most users do not need to run this command directly.

## Usage

```
daft completions <TARGET> [OPTIONS]
```

## Arguments

| Argument   | Description                                         | Required |
| ---------- | --------------------------------------------------- | -------- |
| `<TARGET>` | Target to generate completions for (bash, zsh, fish, fig) | Yes      |

## Options

| Option                   | Description                                              | Default |
| ------------------------ | -------------------------------------------------------- | ------- |
| `-c, --command <COMMAND>` | Specific command to generate completions for (default: all) |         |
| `-i, --install`          | Install completions to standard locations                |         |

## Global Options

| Option         | Description               |
| -------------- | ------------------------- |
| `-h`, `--help` | Print help information    |
| `-V`, `--version` | Print version information |

## Installation Locations

When using `--install`, completions are written to:

| Shell | Location                                     |
| ----- | -------------------------------------------- |
| bash  | `$XDG_DATA_HOME/bash-completion/completions/` (or `~/.local/share/`) |
| zsh   | `~/.zfunc/`                                  |
| fish  | `$XDG_CONFIG_HOME/fish/completions/` (or `~/.config/fish/`) |
| fig   | `~/.amazon-q/autocomplete/build/` or `~/.fig/autocomplete/build/` |

## Examples

```bash
# Generate all bash completions
daft completions bash

# Generate completions for a specific command
daft completions zsh --command git-worktree-checkout

# Install completions to standard locations
daft completions bash --install
daft completions zsh --install
daft completions fish --install
daft completions fig --install
```

## See Also

- [daft-shell-init](./daft-shell-init.md)
- [daft-setup](./daft-setup.md)
