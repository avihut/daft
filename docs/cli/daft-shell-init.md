---
title: daft-shell-init
description: Generate shell wrapper functions for daft commands
---

# daft shell-init

Generate shell wrapper functions for daft commands

## Description

Generate shell wrapper functions that enable automatic cd into new worktrees.

By default, when daft commands create or switch to a worktree, the process changes
directory but the parent shell remains in the original directory. These wrappers
solve this by reading the CD target from a temp file and using the shell's builtin cd.

Add to your shell config:
  Bash (~/.bashrc):  eval "$(daft shell-init bash)"
  Zsh  (~/.zshrc):   eval "$(daft shell-init zsh)"
  Fish (~/.config/fish/config.fish): daft shell-init fish | source

With short aliases (gwco, etc.):
  eval "$(daft shell-init bash --aliases)"

## Usage

```
daft shell-init [OPTIONS] <SHELL>
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<SHELL>` | Target shell (bash, zsh, or fish) | Yes |

## Options

| Option | Description | Default |
|--------|-------------|----------|
| `--aliases` | Include short aliases (gwco, etc.) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [daft-setup](./daft-setup.md)
- [daft-shortcuts](./daft-shortcuts.md)

