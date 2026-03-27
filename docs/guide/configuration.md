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
| `daft.go.autoStart`          | `false`    | Auto-create worktree when branch not found in `daft go` |

## Layout Settings

Layout configuration uses `~/.config/daft/config.toml` (TOML format), not
`git config`. This is different from the other settings on this page.

See the [Layouts guide](./layouts.md) for detailed explanations of each layout.

| Key                | File                 | Description                                 |
| ------------------ | -------------------- | ------------------------------------------- |
| `defaults.layout`  | `config.toml`        | Global default layout name or template      |
| `layout`           | `daft.yml` (in repo) | Team-recommended layout for this repository |
| `[layouts.<name>]` | `config.toml`        | Custom layout definition                    |

### Global Default

```toml
# ~/.config/daft/config.toml
[defaults]
layout = "contained"
```

Or use the command: `daft layout default contained`

### Custom Layouts

```toml
# ~/.config/daft/config.toml
[layouts.my-team]
template = "../.worktrees/{{ repo }}/{{ branch | sanitize }}"

[layouts.isolated]
template = "~/worktrees/{{ repo }}/{{ branch | sanitize }}"
```

### Team Convention (daft.yml)

```yaml
# daft.yml (committed to repository)
layout: contained
```

## Remote Sync Settings

By default, daft does not contact the remote during worktree management. All
remote operations are opt-in. Use `daft config remote-sync` to toggle these
settings interactively, or set them directly with `git config`.

| Key                        | Default | Description                                                         |
| -------------------------- | ------- | ------------------------------------------------------------------- |
| `daft.checkout.fetch`      | `false` | Fetch from remote before creating a worktree for an existing branch |
| `daft.checkout.push`       | `false` | Push new branches to remote after creation                          |
| `daft.branchDelete.remote` | `false` | Delete the remote branch when removing a local branch               |

### Enabling Remote Sync

Use the interactive command to toggle remote sync settings:

```bash
daft config remote-sync         # Open interactive TUI
daft config remote-sync --on    # Enable all remote sync operations
daft config remote-sync --off   # Disable all remote sync operations
daft config remote-sync --status  # Show current settings
```

You can also set values directly:

```bash
# Enable all remote sync (opt in to old behavior)
git config daft.checkout.fetch true
git config daft.checkout.push true
git config daft.branchDelete.remote true
```

Per-command overrides are available via `--local` (skip remote operations) and
`--remote` (delete remote branch only, without removing local worktree or
branch).

## Checkout Settings

| Key                         | Default | Description                                                   |
| --------------------------- | ------- | ------------------------------------------------------------- |
| `daft.checkout.upstream`    | `true`  | Set upstream tracking for branches                            |
| `daft.checkout.carry`       | `false` | Carry uncommitted changes when checking out existing branches |
| `daft.checkoutBranch.carry` | `true`  | Carry uncommitted changes when creating new branches          |

## Update Settings

| Key                | Default       | Description                                                                    |
| ------------------ | ------------- | ------------------------------------------------------------------------------ |
| `daft.update.args` | `"--ff-only"` | Default arguments passed to `git pull` in update operations (same-branch mode) |

## List Settings

| Key                 | Default     | Description                                                                                                                            |
| ------------------- | ----------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.list.stat`    | `"summary"` | Default statistics mode for list command (`summary` or `lines`)                                                                        |
| `daft.list.columns` |             | Default column selection for list command (e.g., `branch,path,age` or `+size,-annotation`)                                             |
| `daft.list.sort`    |             | Default sort order for list command (e.g., `+branch`, `-activity`, `+owner,-size`). Sortable: branch, path, size, age, owner, activity |

## Prune Settings

| Key                   | Default     | Description                                                                                                                 |
| --------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------- |
| `daft.prune.cdTarget` | `"root"`    | Where to cd after pruning the current worktree. Values: `root` (project root) or `default-branch` (default branch worktree) |
| `daft.prune.stat`     | `"summary"` | Default statistics mode for prune command (`summary` or `lines`)                                                            |
| `daft.prune.columns`  |             | Default column selection for prune command                                                                                  |
| `daft.prune.sort`     |             | Default sort order for prune command (e.g., `+branch`, `-activity`)                                                         |

## Sync Settings

| Key                 | Default     | Description                                                        |
| ------------------- | ----------- | ------------------------------------------------------------------ |
| `daft.sync.stat`    | `"summary"` | Default statistics mode for sync command (`summary` or `lines`)    |
| `daft.sync.columns` |             | Default column selection for sync command                          |
| `daft.sync.sort`    |             | Default sort order for sync command (e.g., `+branch`, `-activity`) |

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
| `daft.hooks.trustPrune`    | `true`                  | Auto-prune stale entries from the trust database (background, once per 24h) |

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
# Enable remote sync (fetch on checkout, push on start, delete remote on remove)
daft config remote-sync --on

# Or enable individual settings
git config daft.checkout.fetch true
git config daft.checkout.push true
git config daft.branchDelete.remote true

# Use a different remote
git config daft.remote upstream

# Disable auto-cd globally
git config --global daft.autocd false

# After pruning, cd to default branch worktree instead of root
git config daft.prune.cdTarget default-branch

# Auto-create worktree when branch not found in daft go
git config daft.go.autoStart true

# Use rebase-style update by default
git config daft.update.args "--rebase"

# Disable hooks globally
git config --global daft.hooks.enabled false

# Make post-create hooks abort on failure
git config daft.hooks.worktreePostCreate.failMode abort
```

## Environment Variables

| Variable                  | Description                                                               |
| ------------------------- | ------------------------------------------------------------------------- |
| `DAFT_CD_FILE`            | Temp file path for shell wrapper CD communication (set by shell wrappers) |
| `DAFT_NO_HINTS`           | Set to suppress contextual hint messages                                  |
| `DAFT_NO_TRUST_PRUNE`     | Set to disable automatic trust database pruning                           |
| `DAFT_NO_UPDATE_CHECK`    | Set to disable version update notifications                               |
| `DAFT_NO_BACKGROUND_JOBS` | Set to promote all background hook jobs to foreground                     |
| `NO_COLOR`                | Standard variable to disable colored output                               |
| `PAGER`                   | Override the pager for `daft release-notes`                               |

## Git Hooks

daft's push operations are structural -- they manage branch topology as a
side-effect of worktree management, not as user-initiated code pushes. Because
of this, daft passes `--no-verify` on all `git push` calls, skipping any
`pre-push` hooks configured in the repository.

Remote operations are disabled by default. When enabled (via
`daft config remote-sync --on` or by setting the individual keys), this affects:

- **`daft start` / `git worktree-checkout -b`** -- pushes the new branch to set
  upstream tracking (controlled by `daft.checkout.push`)
- **`daft remove` / `git worktree-branch -d`** -- pushes `--delete` to remove
  the remote branch (controlled by `daft.branchDelete.remote`)
- **`daft multi-remote move --push`** -- pushes an existing branch to a new
  remote

If a push fails (due to network issues, auth errors, or remote rejection rules),
daft treats it as non-fatal: the local worktree and branch remain usable, and a
warning is shown with the manual recovery command.

Use `--local` on any worktree command to skip all remote operations for that
invocation, regardless of config.

::: tip This only applies to git's own hooks. daft's
[lifecycle hooks](./hooks.md) (configured in `daft.yml` or `.daft/hooks/`) are
always executed normally. :::
