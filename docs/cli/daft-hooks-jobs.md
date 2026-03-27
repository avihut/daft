---
title: daft-hooks-jobs
description: Manage background hook jobs
---

# daft hooks jobs

Manage background hook jobs

## Description

List, inspect, cancel, retry, and clean up background hook jobs.

When hooks include jobs with `background: true`, they run asynchronously
via a coordinator process after the command returns. This command provides
visibility and control over those background jobs.

When run without a subcommand, lists all background jobs for the current
repository grouped by status (running, completed, failed).

## Usage

```
daft hooks jobs [OPTIONS] [COMMAND]
```

## Subcommands

| Command | Description |
|---------|-------------|
| `logs <job>` | View the output log for a background job |
| `cancel <job>` | Cancel a running background job |
| `cancel --all` | Cancel all running background jobs |
| `retry <job>` | Re-run a failed background job |
| `clean` | Remove logs older than the retention period |

## Options

| Option | Description | Default |
|--------|-------------|---------|
| `--all-repos` | Show jobs across all repositories | |
| `--worktree <path>` | Filter to a specific worktree | |
| `--json` | Output in JSON format | |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Examples

```bash
# List all background jobs
daft hooks jobs

# View output from a specific job
daft hooks jobs logs warm-build-cache

# Cancel a running job
daft hooks jobs cancel warm-build-cache

# Cancel all running jobs
daft hooks jobs cancel --all

# Re-run a failed job
daft hooks jobs retry warm-build-cache

# Clean up old logs
daft hooks jobs clean
```

## See Also

- [Hooks guide](../guide/hooks.md#background-jobs)
- [git-daft-hooks](./git-daft-hooks.md)
