---
title: daft-hooks-jobs
description: Manage background hook jobs
---

# daft hooks jobs

Manage background hook jobs

## Description

List, inspect, cancel, retry, and prune background hook jobs.

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
| `prune` | Remove old job records (invocations, metadata, logs) past retention |

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

### `prune` options

| Option | Description |
|--------|-------------|
| `--dry-run` | List candidates without removing anything. |
| `--older-than <DURATION>` | Override retention for this run (e.g., `30d`, `12h`). |

## Global Options

| Option | Description |
|--------|-------------|
| `-h`, `--help` | Print help information |
| `-V`, `--version` | Print version information |

## Listing columns

The default human-readable listing shows the following columns per job:

| Column | Description |
|--------|-------------|
| `Job` | Job name |
| `Status` | `running`, `completed`, `failed`, `cancelled`, or `skipped` |
| `Started` | Relative time since the job started (e.g., `3m ago`) |
| `Duration` | Elapsed wall-clock time. Compact format: `36ms` / `12s` / `1m32s` / `1h5m` / `2d3h`. Running jobs append `...`. |
| `Size` | Human-readable size of `output.log` (e.g., `4.2 KB`, `1.1 MB`). Renders as `—` for missing or zero-byte logs. |

## Structured Output

`daft hooks jobs` supports machine-readable output via `--format`: `json`,
`ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>`
for custom output.

The listing is a flat table — one row per job, each carrying its invocation
context (`invocation_id`, `invocation_short`, `worktree`, `hook_type`,
`trigger_command`, `invocation_created_at`) alongside job fields (`name`,
`status`, `background`, `started_at`, `finished_at`, `duration_secs`,
`exit_code`, `command`, `size_bytes`).

| Field | Type | Description |
|-------|------|-------------|
| `size_bytes` | int \| null | Bytes of `output.log` for this job. Null when the log file is absent. |

```sh
# Pipe to jq
daft hooks jobs --format json | jq '.[] | select(.status == "failed")'

# Pluck one field per row with cut
daft hooks jobs --format tsv --no-headers | cut -f2,7,8

# Custom template
daft hooks jobs --template '{% for j in items %}{{ j.name }}: {{ j.status }}
{% endfor %}'
```

See the [Output Formats guide](/reference/output-formats) for format details
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

# Prune old job records (invocations + metadata + logs) past retention
daft hooks jobs prune

# Preview what would be removed without touching anything
daft hooks jobs prune --dry-run

# Override retention for a one-off run
daft hooks jobs prune --older-than 30d
```

## Automatic cleanup

`daft` runs an automatic background cleanup once every 24 hours. On each
invocation, if the cache file at `$XDG_CONFIG_HOME/daft/log-clean.json` is
missing or stale, a detached `daft __clean-logs` child is spawned. The child
acquires a single-flight file lock and runs three layered passes per repo:

1. **Per-log truncation** — any `output.log` exceeding `max_log_size` (default
   10 MB) is truncated with a `[output truncated at N bytes]` footer.
2. **Retention sweep** — invocations older than the captured per-job
   `retention` (default 7 days) are removed, subject to the `keep_last`
   sanity floor (default 3 most-recent invocations per worktree).
3. **Per-repo budget** — if total disk usage still exceeds `max_total_size`
   (default 500 MB), oldest invocations are evicted LRU-style.

To disable automatic cleanup: `export DAFT_NO_LOG_CLEAN=1`. Manual pruning
is always available via `daft hooks jobs prune`.

Cleanup is auto-disabled in CI environments.

The most recent cleanup result is summarized as a footer line in the
`daft hooks jobs --all` listing:

```text
Last log cleanup 4h ago: removed 23 job log(s) (4.2 MB freed)
```

## See Also

- [Hooks guide](/hooks/yaml-reference#background-jobs)
- [Output Formats guide](/reference/output-formats)
- [git-daft-hooks](./git-daft-hooks.md)
