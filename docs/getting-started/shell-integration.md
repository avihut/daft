---
title: Shell Integration
description: Enable automatic directory changes when creating worktrees
---

# Shell Integration

## Why It's Needed

When daft creates a new worktree, the Rust binary changes directory internally -
but your parent shell stays in the original directory. Shell integration solves
this by wrapping daft commands so the shell follows along.

## Setup

### Bash / Zsh

Add to `~/.bashrc` or `~/.zshrc`:

```bash
eval "$(daft shell-init bash)"
```

### Fish

Add to `~/.config/fish/config.fish`:

```fish
daft shell-init fish | source
```

### With Short Aliases

Include short aliases like `gwco`, `gwcob`:

```bash
# Bash/Zsh
eval "$(daft shell-init bash --aliases)"

# Fish
daft shell-init fish --aliases | source
```

## How It Works

1. The shell wrapper creates a temporary file and passes its path via
   `DAFT_CD_FILE`
2. When this env var is set, daft writes the target directory to that file
3. After the command finishes, the wrapper reads the file and uses the shell's
   builtin `cd` to change directory
4. The temp file is cleaned up automatically

This means the binary does the heavy lifting (cloning, branching, etc.) and the
wrapper just handles the final `cd`. Because stdout is never captured, all
output streams to the terminal in real-time.

## Disabling Auto-CD

If you prefer to stay in your current directory after creating worktrees:

```bash
# Disable globally
git config --global daft.autocd false

# Disable for a specific repository
git config daft.autocd false
```

You can also use the `--no-cd` flag on individual commands:

```bash
git worktree-checkout-branch feature/auth --no-cd
```

## Shell Completions

daft provides tab completions for all commands:

```bash
# Install completions for all shells
daft completions bash --install
daft completions zsh --install
daft completions fish --install
```

Or generate to a specific location:

```bash
# Generate bash completions
daft completions bash > /path/to/completions/daft.bash

# Generate zsh completions
daft completions zsh > /path/to/_daft

# Generate fish completions
daft completions fish > /path/to/completions/daft.fish
```

Restart your shell after installing completions.
