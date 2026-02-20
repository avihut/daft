---
title: Configuration
description: All daft configuration options
---

# Configuration

daft reads configuration from Git's config system. Settings are loaded with
standard Git priority: repository-local config overrides global config, which
overrides built-in defaults.

## Setting Values

```bash
# Set for the current repository
git config daft.autocd false

# Set globally (all repositories)
git config --global daft.autocd false
```

## General Settings

| Key                          | Default    | Description                                             |
| ---------------------------- | ---------- | ------------------------------------------------------- |
| `daft.autocd`                | `true`     | CD into new worktrees when using shell wrappers         |
| `daft.remote`                | `"origin"` | Default remote name for all operations                  |
| `daft.updateCheck`           | `true`     | Show notifications when a new daft version is available |
| `daft.experimental.gitoxide` | `false`    | Use gitoxide for supported Git operations               |

## Checkout Settings

| Key                         | Default | Description                                                   |
| --------------------------- | ------- | ------------------------------------------------------------- |
| `daft.checkout.push`        | `true`  | Push new branches to remote after creation                    |
| `daft.checkout.upstream`    | `true`  | Set upstream tracking for branches                            |
| `daft.checkout.carry`       | `false` | Carry uncommitted changes when checking out existing branches |
| `daft.checkoutBranch.carry` | `true`  | Carry uncommitted changes when creating new branches          |

## Fetch Settings

| Key               | Default       | Description                                                |
| ----------------- | ------------- | ---------------------------------------------------------- |
| `daft.fetch.args` | `"--ff-only"` | Default arguments passed to `git pull` in fetch operations |

## Prune Settings

| Key                   | Default  | Description                                                                                                                 |
| --------------------- | -------- | --------------------------------------------------------------------------------------------------------------------------- |
| `daft.prune.cdTarget` | `"root"` | Where to cd after pruning the current worktree. Values: `root` (project root) or `default-branch` (default branch worktree) |

## Multi-Remote Settings

| Key                              | Default    | Description                                          |
| -------------------------------- | ---------- | ---------------------------------------------------- |
| `daft.multiRemote.enabled`       | `false`    | Enable multi-remote directory organization           |
| `daft.multiRemote.defaultRemote` | `"origin"` | Default remote for new branches in multi-remote mode |

## Hooks Settings

| Key                        | Default                 | Description                                                                 |
| -------------------------- | ----------------------- | --------------------------------------------------------------------------- |
| `daft.hooks.enabled`       | `true`                  | Master switch for all hooks                                                 |
| `daft.hooks.defaultTrust`  | `"deny"`                | Default trust level for unknown repositories (`deny`, `prompt`, or `allow`) |
| `daft.hooks.userDirectory` | `~/.config/daft/hooks/` | Path to user-global hooks directory                                         |
| `daft.hooks.timeout`       | `300`                   | Hook execution timeout in seconds                                           |

### Per-Hook Settings

Each hook type can be configured individually. The hook name uses camelCase.

| Key                              | Default | Description                            |
| -------------------------------- | ------- | -------------------------------------- |
| `daft.hooks.<hookName>.enabled`  | `true`  | Enable/disable a specific hook type    |
| `daft.hooks.<hookName>.failMode` | varies  | Behavior on failure: `abort` or `warn` |

Hook names: `postClone`, `worktreePreCreate`, `worktreePostCreate`,
`worktreePreRemove`, `worktreePostRemove`.

Default fail modes:

- `worktreePreCreate`: `abort` (setup must succeed before creating worktree)
- All others: `warn` (don't block operations)

### Hook Output Settings

| Key                            | Default | Description                             |
| ------------------------------ | ------- | --------------------------------------- |
| `daft.hooks.output.quiet`      | `false` | Suppress hook stdout/stderr             |
| `daft.hooks.output.timerDelay` | `5`     | Seconds before showing elapsed timer    |
| `daft.hooks.output.tailLines`  | `6`     | Rolling output lines per job (0 = none) |

### YAML Hooks Configuration

Hooks can also be configured through a `daft.yml` file for richer features
including multiple jobs, execution modes, job dependencies, and conditional
execution.

See the [Hooks guide](./hooks.md#yaml-configuration) for the complete `daft.yml`
reference.

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

## Environment Variables

| Variable               | Description                                                               |
| ---------------------- | ------------------------------------------------------------------------- |
| `DAFT_CD_FILE`         | Temp file path for shell wrapper CD communication (set by shell wrappers) |
| `DAFT_NO_HINTS`        | Set to suppress contextual hint messages                                  |
| `DAFT_NO_UPDATE_CHECK` | Set to disable version update notifications                               |
| `NO_COLOR`             | Standard variable to disable colored output                               |
| `PAGER`                | Override the pager for `daft release-notes`                               |

## Git Hooks

daft's push operations are structural -- they manage branch topology as a
side-effect of worktree management, not as user-initiated code pushes. Because
of this, daft passes `--no-verify` on all `git push` calls, skipping any
`pre-push` hooks configured in the repository.

This affects three commands:

- **`git worktree-checkout-branch`** -- pushes the new branch to set upstream
  tracking
- **`git worktree-branch-delete`** -- pushes `--delete` to remove the remote
  branch
- **`daft branch move --push`** -- pushes an existing branch to a new remote

If a push fails (due to network issues, auth errors, or remote rejection rules),
daft treats it as non-fatal: the local worktree and branch remain usable, and a
warning is shown with the manual recovery command.

To disable pushing entirely for new branches, set `daft.checkout.push` to
`false`.

::: tip This only applies to git's own hooks. daft's
[lifecycle hooks](./hooks.md) (configured in `daft.yml` or `.daft/hooks/`) are
always executed normally. :::
