---
title: daft-shell-init
description: Generate shell wrapper functions for daft commands
---

# daft shell-init

Generate shell wrapper functions for daft commands

## Description

Generate shell wrapper functions that enable automatic cd into new worktrees.

By default, when daft commands create or switch to a worktree, the process
changes directory but the parent shell remains in the original directory. These
wrappers solve this by reading the CD target from a temp file and using the
shell's builtin cd.

Add to your shell config:

```bash
# Bash (~/.bashrc)
eval "$(daft shell-init bash)"

# Zsh (~/.zshrc)
eval "$(daft shell-init zsh)"

# Fish (~/.config/fish/config.fish)
daft shell-init fish | source
```

With short aliases (gwco, gwcob, etc.):

```bash
eval "$(daft shell-init bash --aliases)"
```

The generated output includes:

- Shell wrapper functions for all `git-worktree-*` commands
- A `git` wrapper that intercepts `git worktree-*` subcommands
- A `daft` wrapper that intercepts `daft worktree-*` subcommands
- Shortcut wrappers for all shortcut styles (gwtco, gwco, gclone, etc.)
- Default-branch shortcuts (gwtcm, gwtcbm, gwcobd, gcbdw)
- Shell completions for all commands

## Usage

```
daft shell-init <SHELL> [OPTIONS]
```

## Arguments

| Argument  | Description                    | Required |
| --------- | ------------------------------ | -------- |
| `<SHELL>` | Target shell (bash, zsh, fish) | Yes      |

## Options

| Option      | Description                            | Default |
| ----------- | -------------------------------------- | ------- |
| `--aliases` | Include short aliases (gwco, gwcob, etc.) |         |

## Global Options

| Option         | Description               |
| -------------- | ------------------------- |
| `-h`, `--help` | Print help information    |
| `-V`, `--version` | Print version information |

## How It Works

The wrappers use a temp file mechanism for shell cd:

1. Before running a daft command, the wrapper creates a temp file
2. The temp file path is passed via the `DAFT_CD_FILE` environment variable
3. The daft command writes the target directory to this file
4. After the command finishes, the wrapper reads the file and calls `cd`
5. The temp file is cleaned up

This approach keeps stdout clean for normal command output.

## See Also

- [Shell Integration](../getting-started/shell-integration.md)
- [daft-setup](./daft-setup.md)
- [Shortcuts](../guide/shortcuts.md)
