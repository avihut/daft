---
title: Installation
description: Install daft on macOS, Linux, or Windows
---

# Installation

## macOS (Homebrew)

The recommended way to install on macOS:

```bash
brew install avihut/tap/daft
```

This installs the `daft` binary, all command symlinks, and man pages.

## Windows

Run the PowerShell installer:

```powershell
irm https://github.com/avihut/daft/releases/latest/download/daft-installer.ps1 | iex
```

## Linux

Download the latest binary from [GitHub Releases](https://github.com/avihut/daft/releases/latest):

```bash
# Download and extract
curl -fsSL https://github.com/avihut/daft/releases/latest/download/daft-x86_64-unknown-linux-gnu.tar.gz | tar xz

# Move to a directory in your PATH
sudo mv daft /usr/local/bin/

# Create command symlinks
cd /usr/local/bin
for cmd in git-worktree-clone git-worktree-init git-worktree-checkout \
           git-worktree-checkout-branch git-worktree-checkout-branch-from-default \
           git-worktree-prune git-worktree-carry git-worktree-fetch \
           git-worktree-flow-adopt git-worktree-flow-eject git-daft; do
  sudo ln -sf daft "$cmd"
done
```

## From Source

Build from source using Cargo:

```bash
git clone https://github.com/avihut/daft.git
cd daft
cargo build --release
```

Add the binary directory to your PATH and create symlinks:

```bash
export PATH="$PWD/target/release:$PATH"

# Create symlinks (or use the mise task)
mise run dev-setup
```

## Verify Installation

After installing, verify everything is working:

```bash
daft doctor
```

This runs health checks on your installation and reports any issues with actionable suggestions.

You can also verify individual commands:

```bash
daft --version
git worktree-clone --help
```

## Post-Install: Shell Integration

For the best experience, enable shell integration so that daft can automatically `cd` you into new worktrees:

```bash
# Bash/Zsh: Add to your shell config
eval "$(daft shell-init bash)"

# Fish
daft shell-init fish | source
```

See [Shell Integration](./shell-integration.md) for full details.

## Requirements

- **Git** 2.5+ (for worktree support)
- **Rust** 1.70+ (only needed when building from source)
