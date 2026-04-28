# Hooks Jobs Cross-Worktree Retry — Design Spec

**Sub-project:** C of the hooks-jobs redesign basket **Parent:**
[`2026-04-11-hooks-jobs-basket-overview.md`](./2026-04-11-hooks-jobs-basket-overview.md)
**Depends on:**
[`2026-04-11-hooks-jobs-bulk-retry.md`](./2026-04-11-hooks-jobs-bulk-retry.md)
(sub-project B, complete) **Branch:** `feat/background-hook-jobs` (stacked on
A's and B's commits)

## Goal

Enable retrying failed jobs from any worktree — including deleted worktrees
whose directories and branches no longer exist — and add listing filters so
users can inspect job history across the full worktree landscape. Also folds in
all remaining cleanup items deferred from sub-projects A and B.

## Motivation

Sub-project B made bulk retry work within the current worktree. The remaining
gap is the original user story that started the whole redesign: _"re-running
failed cleanup jobs, which is also an issue since I don't see logs for the
remove hooks."_ A made the cleanup records visible. B handles same-worktree
retry. C handles the hard cases:

- You're in `main` and want to retry a failed job in `feature/x` without
  switching worktrees.
- `feature/x` was removed, its `worktree-pre-remove` cleanup hook failed, and
  the directory is gone. The job's recorded `working_dir` is a dead path.

C also adds listing filters (`--worktree`, `--status`, `--hook`) so users can
inspect specific slices of job history before deciding what to retry.

## CLI surface — cross-worktree retry

### `--worktree <name>` flag

New flag on `retry` that shifts the worktree context. Composes with all existing
retry forms:

```text
daft hooks jobs retry --worktree feature/x
daft hooks jobs retry --worktree feature/x worktree-post-create
daft hooks jobs retry --worktree feature/x a3f2
daft hooks jobs retry --worktree feature/x warm-cache
```

The flag resolves from the log store, not from live git state. If `feature/x`
has invocations recorded in the log store, it's a valid target — even if the
worktree directory and branch no longer exist.

### Composite address (lifting B's guard)

B's cross-worktree guard in `retry_command` currently bails with
_"Cross-worktree retry is not yet supported."_ C lifts this guard. The composite
address form works for single-job retry:

```text
daft hooks jobs retry feature/x:warm-cache
```

This is equivalent to `retry --worktree feature/x --job warm-cache`.

When both `--worktree` and a composite address with a worktree segment are used,
it's an error: _"Conflicting worktree: --worktree says 'feature/y' but address
says 'feature/x'."_

## Deleted worktree handling

When the retry set is computed and execution begins, each job's recorded
`working_dir` is checked:

- **Exists on disk** → run normally. This covers both same-worktree retry (B's
  behavior, unchanged) and cross-worktree retry where the target worktree is
  still live.
- **Missing** → refuse by default with a clear error:
  `Cannot retry job 'docker-down': working directory '/path/to/feature/x' no longer exists. Use --cwd to specify an alternative.`

### `--cwd <path>` flag

New flag on `retry` that overrides the working directory for all jobs in the
retry set. Intended for the deleted-worktree case where the user knows a
suitable alternative directory (the project root, a recreated worktree, a temp
directory, etc.):

```text
daft hooks jobs retry --worktree feature/x --cwd /tmp/cleanup
daft hooks jobs retry feature/x:docker-down --cwd .
```

`--cwd` is validated: the path must exist and be a directory. If not, bail
before execution.

`--cwd` applies uniformly to all jobs in the retry set. Per-job overrides are
not supported — for that, run single-job retries with different `--cwd` values.

## CLI surface — listing filters

Three new flags on `daft hooks jobs`, all combinable:

```text
daft hooks jobs --worktree feature/x
daft hooks jobs --status failed
daft hooks jobs --hook worktree-post-create
daft hooks jobs --worktree feature/x --status failed --hook worktree-pre-remove
```

### `--worktree <name>`

Filters invocations by the `worktree` field in `InvocationMeta`. Resolves from
the log store — works for deleted worktrees.

Without `--worktree` or `--all`, the listing defaults to the current worktree
(unchanged from B).

**Mutual exclusion with `--all`:** `--all` shows every worktree, `--worktree`
shows one specific worktree. Using both is an error.

### `--status <status>`

Filters to invocations that contain at least one job matching the given status.
Valid values: `failed`, `completed`, `running`, `cancelled`, `skipped`.

This is an invocation-level filter: if any job within the invocation matches,
the full invocation is shown with all its jobs. Filtering individual jobs within
an invocation would break the grouped display.

### `--hook <name>`

Filters invocations by `hook_type`. Accepts the known hook type names
(`post-clone`, `worktree-post-create`, etc.) and arbitrary strings (for
`hooks run <custom>` invocations and retry trigger commands).

### Combination semantics

Filters are ANDed. `--status failed --hook worktree-pre-remove` means
"invocations of `worktree-pre-remove` that have at least one failed job."

## Shell completions

### `--worktree` value completion

Candidates sourced from the log store — all distinct `worktree` values from
`InvocationMeta` records. Output follows the rich completion format
(`name\tgroup\tdescription`) established by the branch completion overhaul:

```text
feature/x       worktree · 3 failed, 2h ago
feature/auth    worktree · 1 failed, 20m ago
main            worktree · 0 failed, 1d ago
```

Two dispatch arms with different filtering:

- `("hooks-jobs-retry-worktree", 1)` — for `retry --worktree <TAB>`, filtered to
  worktrees with failures only (completing to a worktree with nothing to retry
  is a dead end).
- `("hooks-jobs-worktree", 1)` — for `daft hooks jobs --worktree <TAB>`,
  unfiltered (the listing is read-only, viewing a clean worktree is fine).

### `--status` value completion

Static list: `failed`, `completed`, `running`, `cancelled`, `skipped`. Hardcoded
in the shell scripts, no dynamic dispatch needed.

### `--hook` value completion

Candidates from the log store: distinct `hook_type` values from all invocations
in the current repo. Format:

```text
worktree-post-create    hook · 5 invocations
worktree-pre-remove     hook · 2 invocations
```

Dispatch arm: `("hooks-jobs-hook-filter", 1)`.

### Shell rendering

Follow the pattern from the rich branch completions:

- zsh: `compadd -V` groups with padded descriptions
- bash: `cut -f1` for values
- fish: `awk`-reformatted descriptions

## Cleanup fold-ins

### C-2: Non-Unix bg fallback sink parity

In `src/hooks/yaml_executor/mod.rs`, the `#[cfg(not(unix))]` fallback block
passes `None` for the sink to `run_jobs`. Fix: pass `Some(&fg_sink)`, matching
the `DAFT_NO_BACKGROUND_JOBS` path.

### C-3: `write_invocation_meta` `with_context`

In `src/coordinator/log_store.rs`, the `write_invocation_meta` method's
`fs::write` call lacks `.with_context(...)`. Add it for consistency with
`write_meta`.

### C-4: runner.rs LogSink path cleanup

In `src/executor/runner.rs`, replace repeated fully-qualified
`crate::executor::log_sink::LogSink` with `use super::log_sink::LogSink`.
Cosmetic.

### B-2: post-clone integration scenario

Add a `post-clone-visibility.yml` scenario in `tests/manual/scenarios/hooks/`
exercising `post-clone` hooks through `git-worktree-clone`. All existing
scenarios target `worktree-post-create` or `worktree-pre-remove` — this fills
the coverage gap.

### B-3: `JobMeta::skipped` constructor dedup

Both `yaml_executor/mod.rs` and `BufferingLogSink::on_job_runner_skipped`
construct sparse `JobMeta` for skipped jobs with a similar shape. Extract a
`JobMeta::skipped(name, hook_type, worktree, command, needs)` constructor to
avoid drift between the two sites.

## Dependencies on sub-projects A and B

- `InvocationMeta` with `worktree`, `hook_type`, `trigger_command` fields (A).
- `JobMeta` with `needs`, `background`, `command`, `working_dir` fields (A+B).
- `LogStore::list_invocations_for_worktree`, `find_invocations_by_prefix` (A+B).
- `RetryTarget`, `retry_target_from_arg`, `build_retry_set`, `retry_command`
  (B).
- `BufferingLogSink` and `fork_coordinator` wiring (A+B).
- Rich completion infrastructure: `format_entries_as_strings`, shell rendering
  patterns from `complete.rs` and `completions/{bash,zsh,fish}.rs`.

## Non-goals for sub-project C

- **Pre-remove hook veto** (letting a pre-remove hook failure block worktree
  removal). Pinned as a separate feature — it's a hook execution policy change,
  not a retry feature.
- **`parent_invocation_id` provenance** — tracking retry chains. Not needed for
  any C workflow.
- **Per-job `--cwd` overrides** — use single-job retries with different `--cwd`
  values instead.

## Testing

### Unit tests (~10-15 new)

- `resolve_retry_invocation` with `--worktree` override — resolves from log
  store for both live and deleted worktrees.
- `retry_command` with `--cwd` override — validates path, substitutes
  `working_dir` in all specs.
- Listing filter logic: `--status`, `--hook`, `--worktree` individually and
  combined (ANDed).
- `--worktree` + `--all` mutual exclusion.
- `JobMeta::skipped` constructor produces identical output to inline
  construction.
- Completion helpers: `complete_retry_worktrees` filtered vs unfiltered,
  `complete_hook_types`.

### Integration scenarios (4 new)

1. **`cross-worktree-retry.yml`** — Two worktrees, hook fails in one, retry from
   the other using `--worktree`. Also test the composite address form
   `feature/x:job-name`.

2. **`deleted-worktree-retry.yml`** — Create worktree, hook fails, remove the
   worktree, retry with `--worktree` and `--cwd`. Verify: retry without `--cwd`
   refuses, retry with `--cwd /tmp/scratch` runs the command there.

3. **`listing-filters.yml`** — Multiple invocations with different hook types
   and statuses. Verify `--status failed`, `--hook worktree-post-create`,
   `--worktree feature/x` each filter correctly, and combinations AND together.

4. **`post-clone-visibility.yml`** — The B-2 fold-in. Clone via
   `git-worktree-clone`, verify the post-clone hook's invocation and jobs appear
   in the listing.

### Existing scenarios

All 13 existing hooks scenarios (6 from A, 7 from B) must continue to pass.

## File inventory

| File                                              | Change                                                                                                                            |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `src/commands/hooks/jobs.rs`                      | Lift cross-worktree guard, add `--worktree` and `--cwd` to `Retry`, add `--worktree`/`--status`/`--hook` to listing, filter logic |
| `src/commands/complete.rs`                        | New `complete_retry_worktrees`, `complete_listing_worktrees`, `complete_hook_types` helpers; new dispatch arms                    |
| `src/commands/completions/{bash,zsh,fish,fig}.rs` | Wire new flag completions and dynamic dispatch for `--worktree`/`--status`/`--hook`                                               |
| `src/coordinator/log_store.rs`                    | `with_context` fix (C-3), `JobMeta::skipped` constructor (B-3)                                                                    |
| `src/executor/runner.rs`                          | LogSink use-path cleanup (C-4)                                                                                                    |
| `src/executor/log_sink.rs`                        | Use `JobMeta::skipped` constructor (B-3)                                                                                          |
| `src/hooks/yaml_executor/mod.rs`                  | Non-Unix sink fix (C-2), use `JobMeta::skipped` constructor (B-3)                                                                 |
| `tests/manual/scenarios/hooks/*.yml`              | 4 new integration scenarios                                                                                                       |
