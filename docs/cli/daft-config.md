---
title: daft-config
description: Browse and change daft settings
---

# daft config

Browse and change daft settings

## Description

Browse and change daft settings.

Every daft setting in one place, with the value it currently has and the
layer that decided it. Settings live in several stores — git config, the
repository's daft.yml, the global config — resolved through different
precedence chains; this command hides that split behind one list of keys.

  daft config                    Open the settings browser
  daft config list               Every setting, its value, and where it came from
  daft config list --modified    Only the settings something sets
  daft config get <key>          Print one effective value
  daft config get <key> --origin Print it with the full layer-by-layer chain
  daft config set <key> <value>  Change it for this worktree
  daft config set --global ...   Change it at the shared scope instead
  daft config unset <key>        Remove it, revealing whatever it was masking

Values are validated against the setting's own type before anything is
written, so a bad enum or column spec is refused where you typed it rather
than at the next command that reads it.

## Usage

```
daft config
```

## Subcommands

### list

List every setting with its value and origin

```
daft config list [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--modified` | Only settings something actually sets |  |
| `--category <NAME>` | Only settings in this category (checkout, merge, hooks, ...) |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |

### get

Print one setting's effective value

```
daft config get [OPTIONS] <KEY>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<KEY>` | The setting to read | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--origin` | Show every layer's value and which one won |  |

### set

Change a setting

```
daft config set [OPTIONS] <KEY> <VALUE>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<KEY>` | The setting to change | Yes |
| `<VALUE>` | The new value | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--global` | Write at the shared scope rather than this worktree's own |  |

### unset

Remove a setting, revealing whatever it was masking

```
daft config unset [OPTIONS] <KEY>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<KEY>` | The setting to remove | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--global` | Remove at the shared scope rather than this worktree's own |  |

### remote-sync

Configure remote sync behavior

```
daft config remote-sync [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--on` | Enable all remote sync operations |  |
| `--off` | Disable all remote sync operations |  |
| `--status` | Show current remote sync settings |  |
| `--global` | Write to global git config instead of local |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

