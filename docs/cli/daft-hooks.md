---
title: daft-hooks
description: Manage repository trust for hook execution
---

# daft hooks

Manage repository trust for hook execution

## Description

Manage trust settings for repository hooks in .daft/hooks/.

Trust levels:
  deny     Do not run hooks (default)
  prompt   Prompt before each hook
  allow    Run hooks automatically

Trust applies to all worktrees. Without a subcommand, shows status.

## Usage

```
daft hooks [PATH]
```

## Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to check (defaults to current directory) | No |

## Subcommands

### trust

Trust repository to run hooks automatically

```
daft hooks trust [OPTIONS] [PATH]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to repository (defaults to current directory) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `-f, --force` | Do not ask for confirmation |  |

### prompt

Trust repository but prompt before each hook

```
daft hooks prompt [OPTIONS] [PATH]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to repository (defaults to current directory) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `-f, --force` | Do not ask for confirmation |  |

### deny

Revoke trust from the current repository

```
daft hooks deny [OPTIONS] [PATH]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to repository (defaults to current directory) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `-f, --force` | Do not ask for confirmation |  |

### status

Display trust status and available hooks

```
daft hooks status [OPTIONS] [PATH]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<PATH>` | Path to check (defaults to current directory) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `-s, --short` | Show compact one-line summary |  |

### run

Run a hook manually

```
daft hooks run [OPTIONS] [HOOK_TYPE]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<HOOK_TYPE>` | Hook type to run (omit to list available hooks) | No |

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--job <JOB>` | Run only the named job |  |
| `--tag <TAG>` | Run only jobs with this tag (repeatable) |  |
| `--dry-run` | Preview what would run without executing |  |
| `-v, --verbose` | Show verbose output including skipped jobs |  |

### install

Scaffold a daft.yml configuration with hook definitions

```
daft hooks install [HOOKS]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<HOOKS>` | Hook names to add (omit for all hooks) | No |

### validate

Validate the YAML hooks configuration

```
daft hooks validate
```

### dump

Dump the merged YAML hooks configuration

```
daft hooks dump
```

### migrate

Rename deprecated hook files to their new names

```
daft hooks migrate [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--dry-run` | Preview renames without making changes |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

