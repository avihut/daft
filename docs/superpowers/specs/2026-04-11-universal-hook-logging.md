# Universal Hook Invocation Logging — Design

**Status:** Draft **Date:** 2026-04-11 **Context:** Sub-project A of a larger
redesign basket. Sub-projects B (bulk retry shorthands) and C (cross-worktree
retry for cleanup) are deferred to their own specs.

## Motivation

`daft hooks jobs` currently gives a biased picture of what actually ran during
hook execution. Concretely, two gaps:

1. **Foreground jobs are invisible.** In `src/hooks/yaml_executor/mod.rs:244`,
   foreground jobs flow through `runner::run_jobs(&fg_specs, …)` synchronously
   and their output streams to the presenter. Nothing is written to `LogStore`.
   After the command returns, there is no record that the jobs ever ran.
2. **Hooks with only foreground jobs create no invocation record.** Lines
   246–250 early-return when `bg_specs.is_empty()`, so a hook that declares only
   foreground work (the common shape for remove hooks) writes _nothing_ — no
   `invocation.json`, no per-job meta, nothing in the listing.

As a result, `worktree-pre-remove` and `worktree-post-remove` hooks are
completely absent from the listing, and any worktree-post-create hook that
happens to have no background work looks like it never fired. The user cannot
debug cleanup failures or inspect what a past hook run actually printed.

This spec fixes the visibility problem. It does **not** add new retry modes or
cross-worktree retry UX — those land in sub-projects B and C.

## Non-goals

- **Bulk retry / shorthand retry commands.** Sub-project B.
- **Cross-worktree retry, especially for removed worktrees.** Sub-project C.
- **New listing filters** (`--failed`, `--running`, etc.). If post-A noise is a
  real problem, a filter lands in B.
- **Display layer changes** beyond rendering skipped jobs. The existing
  `list_jobs` renderer already supports mixed fg/bg invocations via the
  `background` flag.
- **Coordinator-free architecture for background jobs.** Background execution
  keeps its current forked-coordinator model.

## Design

### 1. Architecture / write path

Today's `yaml_executor::execute_yaml_hook_with_logging` has two paths:
foreground jobs run in-process via the runner with no log-store interaction, and
background jobs are dispatched to a forked coordinator that writes all records.

After this feature, the main process becomes the writer for everything that runs
in-process, and the coordinator only writes records for the jobs it is actually
running.

New flow in `yaml_executor`:

```
1. Compute repo_hash, invocation_id, trigger_command, hook_type, worktree.
2. Run filter pipeline; obtain (kept_specs, skipped_with_reasons).
3. Partition kept_specs into (fg_specs, bg_specs).
4. store.write_invocation_meta(...)  ← moved from coordinator to main.
5. For each skipped_with_reasons entry, write a sparse JobMeta + a log file
   containing the reason (see §4).
6. runner::run_jobs(&fg_specs, …, Some(&mut sink))  ← new sink parameter.
      Sink writes per-fg-job meta.json + log file atomically at completion.
7. If bg_specs nonempty, fork coordinator with the same invocation_id.
      The coordinator only writes bg job records; its existing call to
      write_invocation_meta is removed (main wrote it at step 4).
```

**The `LogSink` trait** (new, lives near the runner — exact module placement
decided during plan writing):

```rust
pub trait LogSink {
    fn on_job_start(&mut self, spec: &JobSpec);
    fn on_job_output(&mut self, spec: &JobSpec, chunk: &[u8]);
    fn on_job_complete(&mut self, spec: &JobSpec, result: &JobResult);
}
```

**`BufferingLogSink`** is the concrete implementation used for foreground jobs:

- Holds an in-memory `Vec<u8>` per in-flight job and a reference to `LogStore`.
- `on_job_start`: allocate the buffer; do not touch disk.
- `on_job_output`: append the chunk to the buffer.
- `on_job_complete`: atomically write the log file and `meta.json` together.
  Drop the buffer.
- `Drop`: if a buffer is still held when the sink is dropped, do not write
  anything. Crash mid-foreground-job leaves zero record for that job.

This is the "atomic at completion" choice from the design conversation (option
b). It avoids stale `running` records entirely; the trade-off is that a
main-process crash mid-job leaves no record, which is acceptable because
foreground job execution is visible live to the user.

**Runner integration.** `runner::run_jobs` gains an `Option<&mut dyn LogSink>`
parameter. Where it currently reads merged stdout/stderr chunks and forwards
them to the presenter, it fans the same chunks into the sink when present.
Existing callers that pass `None` are unaffected. `yaml_executor` passes
`Some(sink)`.

**Main and coordinator write disjoint files.** `invocation.json` is written
exactly once, by the main process, before any job starts. The main process (via
the sink) writes `<inv_id>/<fg_name>/meta.json` and `<inv_id>/<fg_name>/log` for
each foreground job. The coordinator writes `<inv_id>/<bg_name>/meta.json` and
`<inv_id>/<bg_name>/log` for each background job. No races.

### 2. Data model

`JobMeta` already has every field the feature needs: `name`, `background`,
`status`, `exit_code`, `started_at`, `finished_at`, `working_dir`, `command`,
`env`, `hook_type`, `worktree`. No new fields for foreground jobs.

One new variant on `JobStatus`:

- `JobStatus::Skipped` — added for jobs filtered out before execution. Sparse
  meta entries (no timestamps, no command, no env) carry this status.

`InvocationMeta` is unchanged.

**Subtleties for foreground jobs:**

- `started_at` and `finished_at` are both populated at write time, since the
  sink writes atomically after the job finishes.
- Promoted jobs (declared `background: true` in YAML but demoted to the
  foreground partition because a foreground job depends on them) are recorded
  with `background: false`. Rationale: meta reflects _how the job actually ran_,
  not what it was declared as. The live promotion warning already tells the user
  in the console at execution time.

### 3. Display — no changes needed for fg/bg

The existing `list_jobs` renderer already handles mixed foreground/background
invocations:

- `↻` blue prefix for jobs with `meta.background == true`.
- No prefix for foreground.
- Same status icons (`✓`, `✗`, `⟳`), same timing columns.

A typical post-A listing for a fresh worktree:

```
2m ago — worktree-post-create                          [c9d4]
  Job                Status         Started    Duration
  pnpm-install       ✓ completed    12:01:00   0:18
  generate-env       ✓ completed    12:01:18   0:02
  ↻ warm-build       ⟳ running      12:01:20   0:45
```

And for a worktree removed yesterday:

```
1d ago — worktree-pre-remove                           [a3f2]
  Job                Status         Started    Duration
  drop-db-volumes    ✓ completed    09:15:02   0:08
  unregister-hosts   ✗ failed       09:15:10   0:03
```

Both views require no code changes in the display layer — they are correct by
construction once the data is recorded.

### 4. Skip-reason capture

When the filter pipeline rejects a job (because a `when:` condition evaluated
false, or the expression failed to evaluate), the feature records it as a sparse
entry and stores the reason in the job's log file.

**Filter pipeline refactor.** Wherever `when:` conditions are currently
evaluated (in `job_adapter` or adjacent), the return type changes from "kept
specs" to `(kept_specs, skipped_with_reasons)`. Each skipped entry carries the
job name and a reason string. The reason format falls out of whatever the
`when:` evaluator produces and should make the cause obvious — e.g.,
`when: $env.NO_INSTALL != "true" → false`, or `when: <expr> → error: <err>` for
evaluation failures.

**Write behavior.** For each skipped entry, `yaml_executor` writes:

- `meta.json` with `name`, `hook_type`, `worktree`, `background` (as declared in
  YAML), `status = Skipped`. No `started_at`, `finished_at`, `command`, or
  `env`.
- `log` file containing the reason string.

**Display.** Skipped jobs render as regular rows with dim `— skipped` in the
Status column. No second line, no extra column:

```
5m ago — worktree-post-create                          [c9d4]
  Job              Status         Started    Duration
  pnpm-install     — skipped      —          —
  generate-env     — skipped      —          —
```

**Investigation.** The user runs `daft hooks jobs logs <job>` to see the reason.
Since the reason lives in the job's log file, no special-casing is needed:

```
$ daft hooks jobs logs pnpm-install
when: $env.NO_INSTALL != "true" → false
```

Composite addressing (`inv:job`, `worktree:inv:job`) works for skipped jobs
exactly like any other.

### 5. Edge cases

- **Hook with zero runnable jobs.** `invocation.json` is still written. The
  listing shows the invocation with a `(no jobs declared)` placeholder where the
  jobs table would be. Rationale: the user wants to see that the hook fired even
  when nothing executed, so they can investigate why.

- **Main process crashes mid-foreground-job.** `invocation.json` was written at
  step 4, so the invocation is visible. Jobs that finished before the crash have
  records; the in-flight job has none (the buffering sink had not yet written).
  The listing shows the invocation with a partial job list. This is an honest,
  if odd, reflection of what happened, and the crash was visible live anyway.

- **Main process crashes before step 4.** Nothing is recorded. Indistinguishable
  from "daft was never run". Fine.

- **Remove hooks with worktree deletion.** `worktree-pre-remove` and
  `worktree-post-remove` run while the worktree still exists. `LogStore` lives
  under `DAFT_STATE_DIR`, outside the worktree, so records survive the deletion.
  After deletion, the listing shows the invocation under the now-stale branch
  name — that is historically accurate and what the user wants.

- **`post-clone`.** Same path as every other hook type, no special-casing.
  `hook_type` in the meta records `post-clone`.

- **`DAFT_NO_BACKGROUND_JOBS` set.** This environment-variable escape hatch
  makes background jobs run inline. After this feature, those inline-run bg jobs
  flow through the foreground sink and are recorded with `background: false`,
  appearing in the listing as regular foreground rows. The escape hatch remains
  fully visible in the listing rather than creating a silent gap.

- **Promoted bg → fg.** Recorded with `background: false`. See §2.

- **Filter pipeline evaluation error on a `when:` expression.** Treated as a
  skip with the error as the reason (`when: <expr> → error: <err>`). Same
  display treatment as a normal false-result skip.

- **Empty YAML (no jobs declared at all).** `invocation.json` is written; zero
  per-job entries. Listing shows `(no jobs declared)`.

### 6. Testing

**Unit tests:**

- `BufferingLogSink` — output accumulates in the buffer, writes atomic log +
  meta on `on_job_complete`, drops without writing if the sink is dropped with a
  buffer still in flight (crash simulation).
- Filter pipeline — given a mix of pass/fail `when:` conditions plus one eval
  error, the pipeline returns the correct `(kept, skipped_with_reasons)`
  partition and reason strings capture the condition and its evaluated value or
  error.
- `JobMeta` serialization — `Skipped` variant round-trips through JSON; sparse
  fields stay `None`.

**Integration tests** (YAML manual scenarios under
`tests/manual/scenarios/hooks/`):

- **Foreground-only hook.** Post-create hook with three foreground jobs, all
  succeeding. Assert `invocation.json` plus three foreground meta records appear
  in the log store. Assert `daft hooks jobs` lists the invocation with three
  completed rows and no `↻` prefix.
- **Mixed fg + bg.** Post-create hook with two foreground and two background
  jobs. Assert both sets land under the same `invocation_id`, no write races.
  Assert the listing shows the mix with correct `↻` prefixes on the two bg rows.
- **All-skipped hook.** Every job has a `when:` that evaluates to false. Assert
  `invocation.json` appears, each skipped job has sparse meta plus a log file
  containing the reason. Assert `daft hooks jobs logs <name>` prints the reason.
- **Empty YAML.** Hook file declares zero jobs. Assert `invocation.json`
  appears; listing shows the invocation with `(no jobs declared)`.
- **Remove hook.** `worktree-pre-remove` has only foreground jobs; one fails.
  Remove the worktree afterwards. Assert the invocation and both job records
  persist in the log store and the failure is visible under the (now-stale)
  branch name. This is the direct regression for the observed bug.
- **Promoted bg → fg.** Hook declares a background job that a foreground job
  depends on. Assert the promoted job is recorded with `background: false` and
  appears without `↻` in the listing.

**Regression gate:** an explicit integration assertion that, for a
foreground-only post-create hook, `daft checkout` followed by `daft hooks jobs`
shows the invocation — locking in the fix against regressions.

## File inventory

| File                                                          | Change                                                                                                                                                                                          |
| ------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/coordinator/log_store.rs`                                | Add `JobStatus::Skipped` variant. Helper for writing sparse meta + log file for skipped jobs. Existing `JobMeta` otherwise unchanged.                                                           |
| `src/executor/runner.rs`                                      | Add `Option<&mut dyn LogSink>` parameter to `run_jobs`. Fan output chunks to sink when present.                                                                                                 |
| `src/executor/log_sink.rs` (new, path TBD in plan)            | `LogSink` trait and `BufferingLogSink` implementation.                                                                                                                                          |
| `src/coordinator/process.rs`                                  | Remove `write_invocation_meta` call from the coordinator's `run_all_with_cancel` — main process writes it now.                                                                                  |
| `src/hooks/yaml_executor/mod.rs`                              | Write `invocation.json` unconditionally before dispatch. Construct a `BufferingLogSink` and pass it to `run_jobs`. Write sparse skipped records. Remove the `bg_specs.is_empty()` early return. |
| `src/hooks/job_adapter.rs` (or wherever `when:` is evaluated) | Filter pipeline refactor: return `(kept_specs, skipped_with_reasons)` instead of just `kept_specs`.                                                                                             |
| `src/commands/hooks/jobs.rs`                                  | Render `Skipped` rows with `— skipped` status. Render `(no jobs declared)` placeholder for invocations with zero job entries.                                                                   |
| `tests/manual/scenarios/hooks/…`                              | New integration scenarios listed in §6.                                                                                                                                                         |
