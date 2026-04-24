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
|--------|-------------|----------|
| `--format <FORMAT>` | Output format. Mutually exclusive with --template |  |
| `--template <STR>` | Tera template string. Mutually exclusive with --format |  |
| `--no-headers` | Omit header row (tsv/csv only) |  |
| `--all` | Show jobs across all worktrees |  |
| `--worktree <name>` | Filter to a specific worktree (can be deleted) |  |
| `--status <status>` | Filter to invocations containing jobs with this status (`running`, `completed`, `failed`, `cancelled`, `skipped`) |  |
| `--hook <type>` | Filter to invocations of this hook type |  |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Structured Output

`daft hooks jobs` supports machine-readable output via `--format`: `json`,
`ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>`
for custom output.

The listing is a flat table — one row per job, each carrying its invocation
context (`invocation_id`, `invocation_short`, `worktree`, `hook_type`,
`trigger_command`, `invocation_created_at`) alongside job fields (`name`,
`status`, `background`, `started_at`, `finished_at`, `duration_secs`,
`exit_code`, `command`).

```sh
# Pipe to jq
daft hooks jobs --format json | jq '.[] | select(.status == "failed")'

# Pluck one field per row with cut
daft hooks jobs --format tsv --no-headers | cut -f2,7,8

# Custom template
daft hooks jobs --template '{% for j in items %}{{ j.name }}: {{ j.status }}
{% endfor %}'
```

See the [Output Formats guide](../guide/output-formats.md) for format details
and Tera syntax.

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
- [Output Formats guide](../guide/output-formats.md)
- [git-daft-hooks](./git-daft-hooks.md)
