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

### Shell Installer (recommended)

The shell installer downloads the correct binary for your architecture and
creates all required command symlinks:

```bash
curl --proto '=https' --tlsv1.2 -LsSf \
  https://github.com/avihut/daft/releases/latest/download/daft-installer.sh | sh
```

This installs `daft` and all command symlinks (`git-worktree-clone`,
`git-worktree-checkout`, etc.) to `~/.cargo/bin`.

If `~/.cargo/bin` is not in your PATH, add it:

```bash
# Bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc

# Zsh
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
```

Supported architectures: x86_64 (Intel/AMD) and aarch64 (ARM64).

### apt (Debian/Ubuntu)

Download and install the `.deb` package from
[GitHub Releases](https://github.com/avihut/daft/releases/latest):

```bash
# For x86_64
curl -fsSL https://github.com/avihut/daft/releases/latest/download/daft_amd64.deb \
  -o /tmp/daft.deb
sudo dpkg -i /tmp/daft.deb

# For aarch64
curl -fsSL https://github.com/avihut/daft/releases/latest/download/daft_arm64.deb \
  -o /tmp/daft.deb
sudo dpkg -i /tmp/daft.deb
```

This installs `daft`, all command symlinks, and man pages to `/usr/bin`.

### dnf (Fedora/RHEL)

Install the `.rpm` package directly from
[GitHub Releases](https://github.com/avihut/daft/releases/latest):

```bash
# For x86_64
sudo dnf install \
  https://github.com/avihut/daft/releases/latest/download/daft.x86_64.rpm

# For aarch64
sudo dnf install \
  https://github.com/avihut/daft/releases/latest/download/daft.aarch64.rpm
```

### AUR (Arch Linux)

Install using your preferred AUR helper:

```bash
# Using paru
paru -S daft-bin

# Using yay
yay -S daft-bin
```

### Nix

Install from the flake:

```bash
# Add to your profile
nix profile install github:avihut/daft

# Or run without installing
nix run github:avihut/daft -- --version
```

For NixOS, add to your `flake.nix` inputs and `environment.systemPackages`.

### Manual Installation

Download the appropriate archive from
[GitHub Releases](https://github.com/avihut/daft/releases/latest):

| Architecture | Archive                                 |
| ------------ | --------------------------------------- |
| x86_64       | `daft-x86_64-unknown-linux-gnu.tar.xz`  |
| aarch64      | `daft-aarch64-unknown-linux-gnu.tar.xz` |

```bash
# Example for x86_64
curl -fsSL https://github.com/avihut/daft/releases/latest/download/daft-x86_64-unknown-linux-gnu.tar.xz \
  | tar xJ

# Move to a directory in your PATH
sudo mv daft /usr/local/bin/

# Create command symlinks
cd /usr/local/bin
for cmd in git-worktree-clone git-worktree-init git-worktree-checkout \
           git-worktree-checkout-branch git-worktree-prune git-worktree-carry \
           git-worktree-fetch git-worktree-flow-adopt git-worktree-flow-eject \
           git-daft daft-remove daft-rename; do
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
mise run dev:setup
```

## Verify Installation

After installing, verify everything is working:

```bash
daft doctor
```

This runs health checks on your installation and reports any issues with
actionable suggestions.

You can also verify individual commands:

```bash
daft --version
git worktree-clone --help
```

## Post-Install: Shell Integration

For the best experience, enable shell integration so that daft can automatically
`cd` you into new worktrees:

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
