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

Grants full trust to the current repository, allowing hooks in
.daft/hooks/ to be executed automatically during worktree operations.

Use 'git daft hooks prompt' instead if you want to be prompted before
each hook execution.

Trust settings are stored in the daft config directory (trust.json)
and persist across sessions.

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

Grants conditional trust to the current repository. Hooks in
.daft/hooks/ will be executed, but you will be prompted for
confirmation before each hook runs.

Use 'git daft hooks trust' instead if you want hooks to run
automatically without prompting.

Trust settings are stored in the daft config directory (trust.json)
and persist across sessions.

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

Revokes trust from the current repository. After this command,
hooks will no longer be executed for this repository until trust
is granted again.

This sets the trust level to deny.

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

Display trust status and available hooks for the current repository.

Shows:
  level    Current trust level (deny, prompt, or allow)
  yaml     Hooks defined in daft.yml
  scripts  Executable scripts in .daft/hooks/
  commands Suggested commands to change trust

Use -s/--short for a compact one-line output.

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

Manually run a hook by name.

Executes the specified hook type as if it were triggered by a
worktree lifecycle event. Trust checks are bypassed since the
user is explicitly invoking the hook.

Use cases:
  re-run   Re-run a hook after a previous failure
  develop  Iterate on hook scripts during development
  bootstrapSet up worktrees that predate the hooks config

Use --dry-run to preview which jobs would run.
Use --job <name> to run a single job by name.
Use --tag <tag> to run only jobs with a specific tag.

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

Scaffold a daft.yml configuration with hook definitions.

Creates a daft.yml file with placeholder jobs for the specified hooks.
If no hook names are provided, all daft lifecycle hooks are scaffolded.

If a config file already exists, it is not modified. Instead, a YAML
snippet is printed for any missing hooks so you can add them manually.

Valid hook names:
  post-clone, worktree-pre-create, worktree-post-create,
  worktree-pre-remove, worktree-post-remove

```
daft hooks install [HOOKS]
```

#### Arguments

| Argument | Description | Required |
|----------|-------------|----------|
| `<HOOKS>` | Hook names to add (omit for all hooks) | No |

### validate

Validate the YAML hooks configuration

Validate the YAML hooks configuration file.

Loads and parses daft.yml (or equivalent), then runs semantic
validation checks including:
  version  min_version compatibility check
  modes    Mutually exclusive execution modes
  jobs     Each job has a run, script, or group
  groups   Group definitions are valid

Exits with code 1 if there are validation errors.

```
daft hooks validate
```

### dump

Dump the merged YAML hooks configuration

Load and display the fully merged YAML hooks configuration.

Merges all config sources (main file, extends, per-hook files,
local overrides) and outputs the final effective configuration
as YAML.

```
daft hooks dump
```

### jobs

Manage background hook jobs

```
daft hooks jobs [OPTIONS]
```

#### Options

| Option | Description | Default |
|--------|-------------|----------|
| `--all` | Show jobs across all worktrees |  |
| `--json` | Output in JSON format |  |

### migrate

Rename deprecated hook files to their new names

Rename deprecated hook files to their new canonical names.

In daft v1.x, worktree-scoped hooks were renamed with a 'worktree-' prefix:
  pre-createworktree-pre-create
  post-createworktree-post-create
  pre-removeworktree-pre-remove
  post-removeworktree-post-remove

This command must be run from within a worktree. It renames deprecated
hook files in the current worktree's .daft/hooks/ directory.

If both old and new names exist, the old file is skipped (conflict).
Resolve conflicts manually before re-running.

Use --dry-run to preview changes without renaming.

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

