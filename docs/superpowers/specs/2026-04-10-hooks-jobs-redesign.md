# Spec: `daft hooks jobs` Redesign

## Problem

The current `daft hooks jobs` output dumps all background jobs from every
worktree creation as a flat list grouped by status. This is unusable with
multiple worktrees: repeated job names with no context, no timestamps, no
duration, empty parentheses where metadata should be (hook_type and worktree are
never populated in JobMeta).

## Goal

Replace with a grouped, scannable, context-aware job listing. Jobs are grouped
by worktree and invocation (the event that triggered them). Users can drill into
specific jobs via a composite addressing scheme with shell completions at every
level.

---

## Data Model

### InvocationMeta

Stored as `{invocation_id}/invocation.json` in the log store. One per
coordinator launch.

| Field             | Type            | Description                                              |
| ----------------- | --------------- | -------------------------------------------------------- |
| `invocation_id`   | `String`        | 16-hex-char timestamp (existing format)                  |
| `trigger_command` | `String`        | What caused this invocation (see Trigger Commands below) |
| `hook_type`       | `String`        | Hook type name, e.g. `"worktree-post-create"`            |
| `worktree`        | `String`        | Branch name, e.g. `"feature/tax-calc"`                   |
| `created_at`      | `DateTime<Utc>` | When the invocation started                              |

### Trigger Commands

The `trigger_command` field captures the user-facing event that caused the
invocation:

| Event                              | trigger_command value              |
| ---------------------------------- | ---------------------------------- |
| `daft checkout feature/x`          | `"worktree-post-create"`           |
| `daft clone repo`                  | `"post-clone"`                     |
| `daft hooks run post-create`       | `"hooks run worktree-post-create"` |
| `daft hooks jobs retry db-migrate` | `"hooks jobs retry db-migrate"`    |

Automatic hooks use the hook type name. Manual runs and retries use the daft
subcommand that triggered them.

Derivation at dispatch time: if `ctx.command == "hooks-run"` then
`format!("hooks run {}", hook_name)`, otherwise just `hook_name`.

### JobMeta Additions

Two new fields on the existing `JobMeta` struct:

| Field         | Type                    | Description                       |
| ------------- | ----------------------- | --------------------------------- |
| `background`  | `bool`                  | Whether this was a background job |
| `finished_at` | `Option<DateTime<Utc>>` | When the job completed            |

The existing `hook_type` and `worktree` fields (currently always empty) will be
populated from `CoordinatorState` at job execution time.

No backward compatibility handling. This is new, unshipped code.

### CoordinatorState Additions

Three new fields, set via a `with_metadata()` builder:

| Field             | Type     | Source at dispatch time    |
| ----------------- | -------- | -------------------------- |
| `trigger_command` | `String` | Derived from `ctx.command` |
| `hook_type`       | `String` | `hook_name` parameter      |
| `worktree`        | `String` | `ctx.branch_name`          |

### Directory Structure

```
~/.local/state/daft/jobs/
  {repo_hash}/
    {invocation_id}/
      invocation.json
      {job_name}/
        meta.json
        output.log
```

Unchanged layout. `invocation.json` is the only new file.

---

## Display Format

### Default: Current Worktree

```
2h ago — worktree-post-create                            [c9d4]
  Job            Status         Started    Duration
  ↻ db-migrate   ✓ completed    12:01:00   3s
  ↻ db-seed      ✓ completed    12:01:03   2s
  ↻ warm-build   ✓ completed    12:01:00   40s

1h ago — hooks run worktree-post-create                  [e7f2]
  Job            Status         Started    Duration
  ↻ db-migrate   ✓ completed    13:05:12   3s
  ↻ db-seed      ✓ completed    13:05:15   2s
  ↻ warm-build   ✗ failed       13:05:12   40s

3 min ago — hooks jobs retry warm-build                  [a3f2]
  Job            Status         Started    Duration
  ↻ warm-build   ✗ failed       14:32:05   45s
```

### With --all: Multiple Worktrees

Top-level grouping by worktree name (bold):

```
feature/tax-calc
  2h ago — worktree-post-create                          [c9d4]
    Job            Status         Started    Duration
    ↻ db-migrate   ✓ completed    12:01:00   3s
    ...

feature/auth
  20 min ago — worktree-post-create                      [d8a3]
    Job            Status         Started    Duration
    ↻ db-migrate   ✓ completed    14:12:00   2s
    ...
```

### Rendering Details

- **Ordering**: Oldest invocations first within each worktree group. Worktrees
  ordered by their oldest invocation.
- **Invocation header**: `{relative_time} — {trigger_command}` with
  `[{short_id}]` right-aligned. Relative time uses `shorthand_from_seconds()`
  with "ago" suffix.
- **Short invocation ID**: First 4 hex characters of the invocation_id.
- **Column headers**: `dim_underline` style (matching `daft list`).
- **Background prefix**: Blue `↻` for background jobs. No prefix for foreground
  (future-proofing).
- **Status icons and colors**:
  - `✓ completed` — green
  - `✗ failed` — red
  - `⟳ running` — yellow
  - `— cancelled` — dim
  - `⟳ running (stale)` — yellow, when coordinator is gone but meta says Running
- **Started column**: Local time `HH:MM:SS` from `meta.started_at`.
- **Duration column**: `finished_at - started_at` for terminal jobs,
  `now - started_at` for running jobs.
- **Table rendering**: `tabled::Builder` with `Style::blank()`, matching
  existing daft table patterns.
- **No jobs**:
  `"No background job history for this worktree.\nUse --all to see jobs across all worktrees."`

---

## Job Addressing

### Composite Address Format

Full path: `worktree:invocation:job`

Example: `feat/tax-calc:c9d4:db-migrate`

### Parsing Rules

| Input                           | Interpretation                          |
| ------------------------------- | --------------------------------------- |
| `db-migrate`                    | job name only                           |
| `c9d4:db-migrate`               | invocation prefix + job name            |
| `feat/tax-calc:c9d4:db-migrate` | worktree + invocation prefix + job name |

Detection: split on `:` with `splitn(3, ':')`. Count of parts determines the
form.

### Resolution

- **Missing worktree**: Use current worktree branch from `get_current_branch()`.
- **Missing invocation**: Use the most recent invocation in that worktree
  containing the named job.
- **Invocation prefix matching**: Match invocation IDs whose hex string starts
  with the given prefix. Error on ambiguous match (multiple IDs share the
  prefix) with a suggestion to use more characters.

### --inv Flag

Alternative to inline invocation prefix:

```
daft hooks jobs logs db-migrate --inv c9d4
```

Equivalent to `c9d4:db-migrate`. When both are provided, `--inv` takes
precedence.

### Error Messages

**Ambiguous invocation prefix:**

```
Error: Ambiguous invocation ID 'a3' — matches:
  a3f2  worktree-post-create — 2h ago
  a3e1  hooks run worktree-post-create — 1h ago
Use more characters to disambiguate.
```

**Job not found:**

```
Error: No job named 'db-migrate' found in worktree 'feature/tax-calc'.
Available jobs: db-seed, warm-build
```

---

## CLI Interface

```
daft hooks jobs                            # list jobs for current worktree
daft hooks jobs --all                      # list all worktrees
daft hooks jobs --json                     # nested JSON output
daft hooks jobs --json --all               # nested JSON, all worktrees

daft hooks jobs logs <address>             # show job log
daft hooks jobs logs <address> --inv ID    # with explicit invocation

daft hooks jobs cancel <address>           # cancel a running job
daft hooks jobs cancel --all               # cancel all running jobs

daft hooks jobs retry <address>            # retry a failed job

daft hooks jobs clean                      # remove old logs (retention period)
```

### Logs Output Format

```
FAILED  db-migrate                                       [c9d4]
worktree:  feature/tax-calc
trigger:   worktree-post-create
started:   2h ago (2026-04-10 12:01:00)
duration:  2s
command:   set -a && . .env && set +a && pnpm dlx prisma migrate deploy

--- output ---
<full log contents>

Full log: ~/.local/state/daft/jobs/a8f2.../0019.../db-migrate/output.log
```

---

## Shell Completions

Uses the existing `daft __complete` infrastructure. New handler:
`hooks-jobs-job`.

### Context-Aware Completion

Completions adapt to what the user has typed based on the presence of colons.

**No colon — first level (job names + invocation IDs):**

```
$ daft hooks jobs logs <TAB>
db-migrate        ✗ failed — 3 min ago [a3f2]
db-seed           ✗ failed — 3 min ago [a3f2]
warm-build        ✗ failed — 3 min ago [a3f2]
a3f2              hooks jobs retry warm-build — 3 min ago (1 job)
c9d4              worktree-post-create — 2h ago (3 jobs)
e7f2              hooks run worktree-post-create — 1h ago (3 jobs)
```

Job names come first (from the latest invocation in current worktree), then
invocation IDs with descriptions.

**After `invocation:`:**

```
$ daft hooks jobs logs c9d4:<TAB>
c9d4:db-migrate   ✓ completed
c9d4:db-seed      ✓ completed
c9d4:warm-build   ✗ failed
```

**After `worktree:`:**

```
$ daft hooks jobs logs feat/tax-calc:<TAB>
feat/tax-calc:a3f2   hooks jobs retry — 3 min ago
feat/tax-calc:c9d4   worktree-post-create — 2h ago
feat/tax-calc:e7f2   hooks run — 1h ago
```

**After `worktree:invocation:`:**

```
$ daft hooks jobs logs feat/tax-calc:c9d4:<TAB>
feat/tax-calc:c9d4:db-migrate   ✓ completed
feat/tax-calc:c9d4:db-seed      ✓ completed
feat/tax-calc:c9d4:warm-build   ✗ failed
```

**`--inv` flag:**

```
$ daft hooks jobs logs db-migrate --inv <TAB>
a3f2   hooks jobs retry — 3 min ago
c9d4   worktree-post-create — 2h ago
e7f2   hooks run — 1h ago
```

Descriptions use tab-separated format. Shells that support descriptions show
them; others show just the value.

Same completions apply to `logs`, `retry`, and `cancel` subcommands.

### Implementation

Add `("hooks-jobs-job", 1) => complete_job_addresses(word)` to
`src/commands/complete.rs`.

The completion function:

1. Computes `repo_hash` from cwd
2. Opens `LogStore` for the repo
3. Reads invocations (filtered to current worktree unless prefix contains a
   worktree segment)
4. Returns completions appropriate to the current colon-level

Wire into bash/zsh/fish completion scripts for `logs`, `retry`, `cancel`
subcommands under `hooks jobs`.

---

## JSON Output

Mirrors the display hierarchy:

```json
{
  "worktrees": [
    {
      "name": "feature/tax-calc",
      "invocations": [
        {
          "id": "c9d4e7f2a3b10000",
          "short_id": "c9d4",
          "trigger_command": "worktree-post-create",
          "hook_type": "worktree-post-create",
          "created_at": "2026-04-10T12:01:00Z",
          "jobs": [
            {
              "name": "db-migrate",
              "background": true,
              "status": "completed",
              "exit_code": 0,
              "started_at": "2026-04-10T12:01:00Z",
              "finished_at": "2026-04-10T12:01:03Z",
              "duration_secs": 3,
              "command": "set -a && . .env && set +a && pnpm dlx prisma migrate deploy"
            }
          ]
        }
      ]
    }
  ]
}
```

Respects the same scoping: current worktree by default, all worktrees with
`--all`.

---

## Edge Cases

- **Stale running jobs**: When `meta.json` says `Running` but no coordinator
  socket exists, display as `⟳ running (stale)`.
- **Empty worktree**: When `get_current_branch()` fails (bare repo, detached
  HEAD), fall back to `--all` behavior with a warning.
- **Concurrent coordinators**: Multiple `daft checkout` commands running
  simultaneously create separate invocations with unique IDs. Each writes its
  own `invocation.json`. No conflict.
- **Clean command**: Respects worktree scope (current worktree unless `--all`).
  Removes entire invocation directories when all jobs within are past retention.

---

## Files to Modify

| File                               | Change                                                                                                                                                                                                                                                       |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/coordinator/log_store.rs`     | Add `InvocationMeta` struct. New LogStore methods: `write_invocation_meta`, `read_invocation_meta`, `list_invocations`, `list_invocations_for_worktree`, `list_jobs_in_invocation`. Add `background` and `finished_at` to `JobMeta`.                         |
| `src/coordinator/process.rs`       | Add `trigger_command`, `hook_type`, `worktree` to `CoordinatorState` with `with_metadata()`. Write `invocation.json` before spawning jobs. Propagate `hook_type`/`worktree` to `run_single_background_job`. Set `finished_at` and `background` on `JobMeta`. |
| `src/hooks/yaml_executor/mod.rs`   | At dispatch site (~line 276): derive `trigger_command` from `ctx.command`, call `with_metadata()` on `CoordinatorState`.                                                                                                                                     |
| `src/commands/hooks/jobs.rs`       | Add `JobAddress` parser and resolution. Rewrite `list_jobs()` for grouped display. Update `show_logs`, `retry_job`, `cancel_job` to use address resolution. Change `--all-repos` to `--all`. Add `--inv` flag to subcommands.                                |
| `src/commands/complete.rs`         | Add `hooks-jobs-job` match arm with `complete_job_addresses()`.                                                                                                                                                                                              |
| `src/commands/completions/bash.rs` | Wire `daft __complete hooks-jobs-job` for `logs`/`retry`/`cancel` under `hooks jobs`.                                                                                                                                                                        |
| `src/commands/completions/zsh.rs`  | Same, using `_describe` for descriptions.                                                                                                                                                                                                                    |
| `src/commands/completions/fish.rs` | Add `complete -c daft` entries for `hooks jobs` subcommands.                                                                                                                                                                                                 |
