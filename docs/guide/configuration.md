---
title: Configuration
description: All daft configuration options
---

# Configuration

daft reads configuration from Git's config system. Settings are loaded with standard Git priority: repository-local config overrides global config, which overrides built-in defaults.

## Setting Values

```bash
# Set for the current repository
git config daft.autocd false

# Set globally (all repositories)
git config --global daft.autocd false
```

## General Settings

| Key | Default | Description |
|-----|---------|-------------|
| `daft.autocd` | `true` | CD into new worktrees when using shell wrappers |
| `daft.remote` | `"origin"` | Default remote name for all operations |
| `daft.updateCheck` | `true` | Show notifications when a new daft version is available |
| `daft.experimental.gitoxide` | `false` | Use gitoxide for supported Git operations |

## Checkout Settings

| Key | Default | Description |
|-----|---------|-------------|
| `daft.checkout.push` | `true` | Push new branches to remote after creation |
| `daft.checkout.upstream` | `true` | Set upstream tracking for branches |
| `daft.checkout.carry` | `false` | Carry uncommitted changes when checking out existing branches |
| `daft.checkoutBranch.carry` | `true` | Carry uncommitted changes when creating new branches |

## Fetch Settings

| Key | Default | Description |
|-----|---------|-------------|
| `daft.fetch.args` | `"--ff-only"` | Default arguments passed to `git pull` in fetch operations |

## Prune Settings

| Key | Default | Description |
|-----|---------|-------------|
| `daft.prune.cdTarget` | `"root"` | Where to cd after pruning the current worktree. Values: `root` (project root) or `default-branch` (default branch worktree) |

## Multi-Remote Settings

| Key | Default | Description |
|-----|---------|-------------|
| `daft.multiRemote.enabled` | `false` | Enable multi-remote directory organization |
| `daft.multiRemote.defaultRemote` | `"origin"` | Default remote for new branches in multi-remote mode |

## Hooks Settings

| Key | Default | Description |
|-----|---------|-------------|
| `daft.hooks.enabled` | `true` | Master switch for all hooks |
| `daft.hooks.defaultTrust` | `"deny"` | Default trust level for unknown repositories (`deny`, `prompt`, or `allow`) |
| `daft.hooks.userDirectory` | `~/.config/daft/hooks/` | Path to user-global hooks directory |
| `daft.hooks.timeout` | `300` | Hook execution timeout in seconds |

### Per-Hook Settings

Each hook type can be configured individually. The hook name uses camelCase.

| Key | Default | Description |
|-----|---------|-------------|
| `daft.hooks.<hookName>.enabled` | `true` | Enable/disable a specific hook type |
| `daft.hooks.<hookName>.failMode` | varies | Behavior on failure: `abort` or `warn` |

Hook names: `postClone`, `postInit`, `worktreePreCreate`, `worktreePostCreate`, `worktreePreRemove`, `worktreePostRemove`.

Default fail modes:
- `worktreePreCreate`: `abort` (setup must succeed before creating worktree)
- All others: `warn` (don't block operations)

## Examples

```bash
# Don't push new branches automatically
git config daft.checkout.push false

# Use a different remote
git config daft.remote upstream

# Disable auto-cd globally
git config --global daft.autocd false

# After pruning, cd to default branch worktree instead of root
git config daft.prune.cdTarget default-branch

# Use rebase-style fetch by default
git config daft.fetch.args "--rebase"

# Disable hooks globally
git config --global daft.hooks.enabled false

# Make post-create hooks abort on failure
git config daft.hooks.worktreePostCreate.failMode abort
```
