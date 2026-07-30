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
  daft config list --global      Only what the shared scope sets
  daft config get <key>          Print one effective value
  daft config get <key> --origin Print it with the full layer-by-layer chain
  daft config get <key> --local  Print what this worktree's own scope sets
  daft config set <key> <value>  Change it for this worktree
  daft config set --global ...   Change it at the shared scope instead
  daft config unset <key>        Remove it, revealing whatever it was masking

--local and --global name one layer, and name the same layer whether you are
reading or writing: `get --local` prints what `set --local` would replace. For
a git-config setting those are the two files git gives the same flags for; for
a daft.yml setting they are the local overlay and the committed config; for the
worktree layout, the repository's own entry and the global default.

A read narrowed to one layer exits 1 when that layer is silent, the way
`git config --get` does, so a script can ask whether something is set here
rather than only what it resolves to.

Some settings only make sense together, and travel as a named behavior — one
name for the group and for the states it can be in:

  daft config get remote-sync    on, off, or custom
  daft config set remote-sync on Write every setting the state names

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
| `--global` | Read the shared scope alone, rather than what daft resolves |  |
| `--local` | Read this worktree's own scope alone, rather than what daft resolves |  |
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
| `<KEY>` | The setting or behavior to read | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--origin` | Show every layer's value and which one won |  |
| `--global` | Read the shared scope alone, rather than what daft resolves |  |
| `--local` | Read this worktree's own scope alone, rather than what daft resolves |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |

### set

Change a setting

```
daft config set [OPTIONS] <KEY> <VALUE>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<KEY>` | The setting or behavior to change | Yes |
| `<VALUE>` | The new value, or a behavior's state | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--global` | Target the shared scope rather than this worktree's own |  |
| `--local` | Target this worktree's own scope — the default |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |

### unset

Remove a setting, revealing whatever it was masking

```
daft config unset [OPTIONS] <KEY>
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<KEY>` | The setting or behavior to remove | Yes |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--global` | Target the shared scope rather than this worktree's own |  |
| `--local` | Target this worktree's own scope — the default |  |
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

