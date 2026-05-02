# Background Hook Jobs

**Date:** 2026-03-27 **Branch:** feat/background-hook-jobs **Status:** Design

## Problem

When creating a new worktree, hook execution blocks until all jobs complete.
Long-running jobs like building the project from scratch delay the user from
starting work, even though the build result isn't needed immediately.

## Solution

Allow hook jobs to run in the background, returning the user to their shell
while a coordinator process manages the remaining work. The coordinator is a
single forked process that runs background jobs as threads, avoiding the orphan
and fork-bomb risks that detached process spawning creates.

## Configuration Schema

### Job-Level Fields

| Field               | Type                  | Default     | Description                                       |
| ------------------- | --------------------- | ----------- | ------------------------------------------------- |
| `background`        | `bool`                | `false`     | Run this job in the background                    |
| `background_output` | `"log"` \| `"silent"` | `"log"`     | Output and notification behavior                  |
| `log.path`          | `string`              | XDG default | Override log file location (absolute or relative) |
| `log.retention`     | `duration`            | inherited   | Override log retention for this job               |

### Hook-Level Fields

```yaml
hooks:
  worktree-post-create:
    background: true # default for all jobs in this hook
    jobs:
      - name: install deps
        run: pnpm install
        background: false # per-job override
      - name: warm build cache
        run: cargo build # inherits background: true from hook
```

### Top-Level / Global Fields

```yaml
log:
  retention: 14d # default 7d, configurable at any level
```

### `background_output` Modes

- **`log`** (default) — output always written to log file; one-line terminal
  notification on failure.
- **`silent`** — output written to log file only on failure; no terminal
  notification.

### `log.path`

- Absolute paths used as-is.
- Relative paths resolved from the worktree root.
- Template variables available (`{branch}`, `{worktree_path}`, etc.).
- When set, this job's output goes to the specified path instead of the XDG
  state directory.
- Retention and cleanup for custom paths is the user's responsibility —
  `daft hooks jobs clean` only touches the XDG directory.

### `log.retention`

Resolution order (lowest to highest precedence):

Duration format: integer followed by a unit suffix — `d` (days), `h` (hours),
`m` (minutes). Examples: `7d`, `24h`, `30m`.

1. Built-in default: `7d`
2. Global config: `$XDG_CONFIG_HOME/daft/config.yml`
3. Repository config: `daft.yml`
4. Local config: `daft-local.yml`
5. Per-job: `log.retention` on a job definition

### Environment Variables

| Variable                  | Purpose                                                    |
| ------------------------- | ---------------------------------------------------------- |
| `DAFT_IS_COORDINATOR`     | Set by coordinator; prevents recursive background spawning |
| `DAFT_NO_BACKGROUND_JOBS` | User-set; promotes all background jobs to foreground       |

`DAFT_NO_BACKGROUND_JOBS` is the escape hatch for debugging, CI, and testing.
When set, no coordinator is spawned — background jobs simply run in the
foreground like regular jobs.

## Coordinator Architecture

### Lifecycle

1. During hook execution, daft partitions jobs into foreground and background
   sets based on the DAG.
2. Foreground jobs run inline as today, blocking until complete.
3. If background jobs exist, daft forks once into a coordinator process.
4. The parent prints a summary and exits, returning the shell to the user.
5. The coordinator runs background jobs as threads. Shell commands (e.g.,
   `cargo build`) spawn as child processes of the coordinator, not as separate
   daft invocations.
6. When all jobs finish, the coordinator exits cleanly.

### Identity and Discovery

- **Socket:** `$XDG_STATE_HOME/daft/coordinator-<repo-hash>.sock`
- **PID file:** `$XDG_STATE_HOME/daft/coordinator-<repo-hash>.pid`
- When a coordinator is already running for a repo, additional coordinators use
  an invocation ID suffix on the socket name.

### Log Storage

```
$XDG_STATE_HOME/daft/jobs/<repo-hash>/<invocation-id>/
  ├── <job-name>/
  │   ├── meta.json      # job name, hook type, worktree, start time, status
  │   └── output.log     # combined stdout/stderr
  └── <job-name>/
      ├── meta.json
      └── output.log
```

- Scoped per repo via a short hash of the git common dir path.
- `daft hooks jobs clean` prunes logs older than the configured retention
  period.

### Fork Bomb Resilience (Three Layers)

This design is informed by a prior incident (commit `26f647e`) where `daft`
background tasks (`__check-update`, `__prune-trust`) recursively spawned each
other, creating an exponential fork bomb.

1. **Env var guard** — the coordinator sets `DAFT_IS_COORDINATOR=1` in its
   environment. Any `daft` invocation with this var set skips all background
   spawning. Same pattern as the `__` prefix fix from `26f647e`.
2. **Thread-based execution** — background jobs run as threads inside the
   coordinator, not as forked daft processes. Shell commands are child processes
   of the coordinator, not daft recursion points.
3. **Single PID cleanup** — killing the coordinator PID terminates all its
   threads and child processes. No orphan chains.

### Worktree Removal Integration

When a worktree is being removed (`worktree-pre-remove`):

1. Connect to any active coordinator sockets for the repo.
2. Send a cancel signal for jobs associated with the worktree being removed.
3. SIGTERM to child processes, grace period (default 5s, configurable via
   `cancel_grace_period` in `daft.yml`), then SIGKILL.
4. Print progress: `Stopping background job 'warm build cache'... done`

## DAG Integration

The `background` flag is orthogonal to `needs`. It only affects whether a job
blocks the daft command from returning — it does not change dependency
resolution.

### Runtime contract for `needs:` (binding)

The background coordinator is bound by the same dependency-ordering contract the
foreground runner provides. Stating it explicitly so the binary can be audited
against the spec:

1. A background job whose `needs:` list is non-empty MUST NOT begin executing
   its command until every listed dependency has reached a terminal state.
2. A dependency is **satisfied** only when it terminates with
   `JobStatus::Completed`.
3. If a dependency terminates `Failed`, `Cancelled`, or `Skipped` (e.g. routed
   through a `cancel_all` at-start gate), every transitive dependent MUST be
   marked as not-run (`NodeStatus::DepFailed`) and MUST NOT spawn a child
   process.
4. The scheduler MUST detect cycles and missing-dependency references in the
   background bucket and surface them as errors rather than silently fanning
   out.

The runtime SHOULD share the foreground scheduler primitive (`DagGraph` in
`src/executor/dag.rs`) so foreground and background share one source of truth
for ordering, parallelism, and failure cascading. Coordinator-specific behavior
(PID registration for `daft hooks jobs cancel`, log streaming to `output.log`,
`meta.json` lifecycle, silent-mode log deletion) lives in the per-job thread
body — not in the scheduler itself.

**Constraint — `std::thread::scope` post-`fork()`:** The coordinator runs in a
forked child (`libc::fork()` in `fork_coordinator`). On macOS, executing
`std::thread::scope` after `fork()` can deadlock because the malloc arenas
inherited from the parent are in an inconsistent state — the second
`format!`/allocation inside a scoped worker can hang. Therefore the bg scheduler
MUST NOT use `DagGraph::run_parallel` (which is built on `thread::scope`). The
coordinator instead uses `DagGraph` only for graph queries (`new`, `len`,
`dependencies_of`, `dependents_of`) and executes each wave with bare
`std::thread::spawn`/`JoinHandle::join` — the same primitive the pre-fix
coordinator used. The foreground runner is unaffected because it runs in the
parent process, not post-fork.

### Status mapping for cascade decisions

The bg per-job closure maps execution outcome to `NodeStatus` for the DAG
scheduler:

| Outcome                                              | `JobMeta.status` | `NodeStatus` returned to DAG |
| ---------------------------------------------------- | ---------------- | ---------------------------- |
| Command exited 0                                     | `Completed`      | `Succeeded`                  |
| Command exited non-zero                              | `Failed`         | `Failed`                     |
| Per-job cancel (`daft hooks jobs cancel <name>`)     | `Cancelled`      | `Failed`                     |
| `cancel_all` flipped before the closure ran          | `Cancelled`      | `Failed`                     |
| `cancel_all` flipped while the closure was executing | `Cancelled`      | `Failed`                     |

`Cancelled` and `Skipped` are mapped to `NodeStatus::Failed` for cascade
purposes because the dependency did not produce its work product, and any job
that declared `needs: [<dep>]` cannot be assumed to be safe to run. The on-disk
`JobMeta.status` (read by `daft hooks jobs`) preserves the precise distinction
between `Failed` and `Cancelled`; only the DAG cascade collapses them.

This intentionally diverges from the foreground rule that "skipped deps are
considered satisfied" (which exists because foreground `Skipped` is normally
caused by an `if:` condition voluntarily declining to run). In the bg
coordinator, `Skipped`/`Cancelled` always means "did not actually run" and must
propagate.

### Partitioning Algorithm

1. Build the full DAG as today (all jobs, foreground and background).
2. Resolve the DAG into an execution order respecting `needs`.
3. Walk the graph and partition into two phases:
   - **Foreground phase** — all jobs where `background: false`, plus any
     `background: true` jobs that are transitively depended on by a foreground
     job (the dependency forces them to complete before the command returns).
   - **Background phase** — remaining `background: true` jobs whose dependents
     are all also background.

### Foreground Promotion

When a background job is promoted to foreground due to a dependency chain, daft
prints a warning:

```
⚠ Job 'warm build cache' promoted to foreground (required by 'generate types')
```

This also surfaces in `daft hooks validate` as a configuration warning — not an
error, since it works correctly, but a signal that the config doesn't match
intent.

### Example

```yaml
jobs:
  - name: install deps
    run: pnpm install
  - name: warm build cache
    run: cargo build
    background: true
    needs: [install deps]
  - name: precompile assets
    run: pnpm build:assets
    background: true
    needs: [install deps]
  - name: generate types
    run: pnpm generate:types
    needs: [warm build cache]
```

Resolution:

- `install deps` — foreground, runs first.
- `warm build cache` — marked background, but `generate types` (foreground)
  depends on it, so promoted to foreground.
- `generate types` — foreground, runs after `warm build cache`.
- `precompile assets` — background, no foreground dependents, handed to
  coordinator.

Priority (`priority` field) is respected within each phase.

## Terminal Output

### Post-Hook Summary

After foreground jobs complete, if background jobs were dispatched:

```
✓ 3 hook jobs completed
⟳ 2 background jobs running — daft hooks jobs to manage
```

One line. The user takes it from there.

### Background Job Failure

When a background job fails (and `background_output` is `log`), the coordinator
prints to the terminal where the original command was run:

```
✗ Background job 'warm build cache' failed (exit 1) — daft hooks jobs logs warm-build-cache
```

**Edge cases:**

- **Terminal closed** — coordinator catches `EPIPE` on broken pipe, logs the
  failure to the job log file, and continues running remaining jobs. The user
  sees the failure via `daft hooks jobs`.
- **Multiple failures** — one line per job as they happen. No batching.
- **Silent mode** — no terminal output. Failure only visible via
  `daft hooks jobs`.

## CLI: `daft hooks jobs`

### Default Output (no arguments)

```
RUNNING (coordinator PID 48291)
  warm build cache     worktree-post-create   feat/login   2m 13s
  precompile assets    worktree-post-create   feat/login   1m 58s

COMPLETED (last 24h)
  install mise         post-clone             —            0m 4s

FAILED (last 24h)
  generate types       worktree-post-create   feat/api     0m 12s  exit 1
```

### Subcommands

| Command                        | Description                                   |
| ------------------------------ | --------------------------------------------- |
| `daft hooks jobs`              | List jobs (default: current repo, last 24h)   |
| `daft hooks jobs logs <job>`   | Stream or tail the job's output log           |
| `daft hooks jobs cancel <job>` | SIGTERM + grace period, then SIGKILL          |
| `daft hooks jobs cancel --all` | Cancel all running jobs for this repo         |
| `daft hooks jobs retry <job>`  | Re-run a failed job with its original context |
| `daft hooks jobs clean`        | Remove logs older than retention period       |

### Flags

- `--all-repos` — show jobs across all repos.
- `--worktree <path>` — filter to a specific worktree.
- `--json` — machine-readable output.

### Retry Behavior

`retry` reconnects to the running coordinator (or spawns a new one if none
exists) and re-submits the job with its original environment, working directory,
and hook context. The failed job's log is archived, and a new log starts.
