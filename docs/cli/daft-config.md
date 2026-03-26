---
title: daft config
description: Configure daft settings
---

# daft config

Configure daft settings

## Usage

```
daft config <SUBCOMMAND>
```

## Subcommands

| Subcommand     | Description                              |
| -------------- | ---------------------------------------- |
| `remote-sync`  | Configure remote sync behavior           |

## daft config remote-sync

Toggle whether daft contacts the remote during worktree management.

By default, daft is local-first: `daft start` does not push new branches,
`daft go` does not fetch, and `daft remove` does not delete the remote branch.
Use `remote-sync` to opt in to remote operations globally or per-repository.

### Usage

```
daft config remote-sync [OPTIONS]
```

### Options

| Option     | Description                                             |
| ---------- | ------------------------------------------------------- |
| `--on`     | Enable all remote sync operations (fetch, push, delete) |
| `--off`    | Disable all remote sync operations                      |
| `--status` | Show current remote sync settings                       |
| `--global` | Write to global git config instead of local             |

### Description

Running `daft config remote-sync` without arguments opens an interactive TUI
where you can toggle each setting individually:

- **fetch on checkout** (`daft.checkout.fetch`) -- fetch from remote before
  creating a worktree for an existing branch
- **push on start** (`daft.checkout.push`) -- push new branches to the remote
  after creation, setting upstream tracking
- **delete remote on remove** (`daft.branchDelete.remote`) -- delete the remote
  branch when removing a local branch

All three settings default to `false`.

### Examples

```bash
# Interactive toggle (opens TUI)
daft config remote-sync

# Enable everything at once (opt in to remote sync globally)
daft config remote-sync --on --global

# Disable for the current repository only
daft config remote-sync --off

# Check what is currently enabled
daft config remote-sync --status
```

### Per-command overrides

You can bypass the config settings on individual commands:

- `--local` -- skip all remote operations for this invocation
- `--remote` -- (remove/branch -d only) delete the remote branch only, without
  touching the local worktree or branch

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## See Also

- [Configuration guide](../guide/configuration.md) for all settings
- [daft start](./daft-start.md) -- create a new branch and worktree
- [daft remove](./daft-remove.md) -- delete branches and their worktrees
