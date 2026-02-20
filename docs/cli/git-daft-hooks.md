---
title: git daft hooks
description: Manage repository trust and YAML hooks configuration
---

Manage repository trust for hook execution and YAML hooks configuration.

## Description

Manage trust settings for repository hooks in `.daft/hooks/` and YAML hooks
configured via `daft.yml`. Without a subcommand, shows the current trust status.

Trust levels control whether hooks run for a repository:

- **deny** — Do not run hooks (default)
- **prompt** — Prompt before each hook
- **allow** — Run hooks automatically

Trust applies to all worktrees in a repository.

## Usage

```
git daft hooks [SUBCOMMAND]
```

## Subcommands

### trust

Trust a repository to run hooks automatically.

```
git daft hooks trust [OPTIONS] [PATH]
git daft hooks trust list [--all]
git daft hooks trust reset [OPTIONS] [PATH]
git daft hooks trust reset all [OPTIONS]
```

| Argument / Option | Description                 | Default                 |
| ----------------- | --------------------------- | ----------------------- |
| `[PATH]`          | Path to repository          | `.` (current directory) |
| `-f, --force`     | Do not ask for confirmation |                         |

#### trust list

List all repositories with explicit trust settings.

```
git daft hooks trust list [OPTIONS]
```

| Option  | Description                                  |
| ------- | -------------------------------------------- |
| `--all` | Include repositories with `deny` trust level |

#### trust reset

Remove the trust entry for a repository, returning it to the default deny state.

```
git daft hooks trust reset [OPTIONS] [PATH]
```

| Argument / Option | Description                 | Default                 |
| ----------------- | --------------------------- | ----------------------- |
| `[PATH]`          | Path to repository          | `.` (current directory) |
| `-f, --force`     | Do not ask for confirmation |                         |

#### trust reset all

Clear all trust settings from the database.

```
git daft hooks trust reset all [OPTIONS]
```

| Option        | Description                 |
| ------------- | --------------------------- |
| `-f, --force` | Do not ask for confirmation |

### prompt

Trust a repository but prompt before each hook execution.

```
git daft hooks prompt [OPTIONS] [PATH]
```

| Argument / Option | Description                 | Default                 |
| ----------------- | --------------------------- | ----------------------- |
| `[PATH]`          | Path to repository          | `.` (current directory) |
| `-f, --force`     | Do not ask for confirmation |                         |

### deny

Revoke trust from the current repository. Hooks will no longer run until trust
is granted again.

```
git daft hooks deny [OPTIONS] [PATH]
```

| Argument / Option | Description                 | Default                 |
| ----------------- | --------------------------- | ----------------------- |
| `[PATH]`          | Path to repository          | `.` (current directory) |
| `-f, --force`     | Do not ask for confirmation |                         |

### status

Display trust status and available hooks for the current repository.

```
git daft hooks status [OPTIONS] [PATH]
```

| Argument / Option | Description                   | Default                 |
| ----------------- | ----------------------------- | ----------------------- |
| `[PATH]`          | Path to check                 | `.` (current directory) |
| `-s, --short`     | Show compact one-line summary |                         |

### migrate

Rename deprecated hook files to their new canonical names. Must be run from
within a worktree.

Renames:

- `pre-create` -> `worktree-pre-create`
- `post-create` -> `worktree-post-create`
- `pre-remove` -> `worktree-pre-remove`
- `post-remove` -> `worktree-post-remove`

```
git daft hooks migrate [OPTIONS]
```

| Option      | Description                            |
| ----------- | -------------------------------------- |
| `--dry-run` | Preview renames without making changes |

### install

Scaffold a `daft.yml` configuration with hook definitions. If the file already
exists, only hooks not already defined are added.

```
git daft hooks install [HOOKS...]
```

| Argument     | Description                                 |
| ------------ | ------------------------------------------- |
| `[HOOKS...]` | Hook names to scaffold (omit for all hooks) |

Valid hook names: `post-clone`, `post-init`, `worktree-pre-create`,
`worktree-post-create`, `worktree-pre-remove`, `worktree-post-remove`.

### validate

Validate the YAML hooks configuration file. Loads and parses `daft.yml` (or
equivalent), then runs semantic validation checks.

```
git daft hooks validate
```

Checks include:

- `min_version` compatibility
- Mutually exclusive execution modes
- Each job has a `run`, `script`, or `group`
- Group definitions are valid
- Job dependency cycles and unknown references

Exits with code 1 if there are validation errors.

### dump

Load and display the fully merged YAML hooks configuration. Merges all config
sources (main file, extends, per-hook files, local overrides) and outputs the
final effective configuration as YAML.

```
git daft hooks dump
```

## Global Options

| Option            | Description               |
| ----------------- | ------------------------- |
| `-h`, `--help`    | Print help information    |
| `-V`, `--version` | Print version information |

## Examples

```bash
# Quick setup: scaffold, edit, trust
git daft hooks install
# Edit daft.yml with your commands...
git daft hooks trust -f

# Check what hooks are configured
git daft hooks status

# List all trusted repositories
git daft hooks trust list

# Remove trust entry for the current repository
git daft hooks trust reset

# Clear all trust settings
git daft hooks trust reset all

# Validate before committing
git daft hooks validate

# See the merged config from all sources
git daft hooks dump

# Preview hook file migration
git daft hooks migrate --dry-run
```

## See Also

- [Hooks Guide](../guide/hooks.md) — Full hooks documentation including YAML
  configuration
- [Configuration](../guide/configuration.md) — Git config settings for hooks
