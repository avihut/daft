# Hooks Jobs Bulk Retry — Design Spec

**Sub-project:** B of the hooks-jobs redesign basket **Parent:**
[`2026-04-11-hooks-jobs-basket-overview.md`](./2026-04-11-hooks-jobs-basket-overview.md)
**Depends on:**
[`2026-04-11-universal-hook-logging.md`](./2026-04-11-universal-hook-logging.md)
(sub-project A, complete) **Branch:** `feat/background-hook-jobs` (stacked on
A's commits)

## Goal

Replace the single-job `daft hooks jobs retry <job>` with a bulk retry family
that scales from "retry everything that just failed" down to "retry this one
job," disambiguated by the shape of the positional argument. All forms default
to the current worktree. Cross-worktree retry is explicitly deferred to
sub-project C.

## Motivation

Sub-project A made every hook invocation visible in `daft hooks jobs`, including
the failures users actually want to re-run. The next gap, documented in the
basket overview (§ gap 4), is that today's `retry <job>` only accepts one job at
a time. In practice users want bigger strokes:

- "Retry everything that failed just now" — common after a flaky-network
  checkout.
- "Retry everything that failed in the last `worktree-post-create`" — common
  when tweaking a hook definition.
- "Retry everything in that specific invocation" — for pinning to an exact run
  when several happened in succession.

A's `InvocationMeta` and `JobMeta` give us enough metadata to resolve each of
these forms to a concrete set of jobs and re-dispatch them coherently — as long
as we also persist `needs:` (a new requirement B introduces).

## CLI surface

Four positional forms plus three escape-hatch flags.

```text
daft hooks jobs retry                       # most recent invocation in current worktree
daft hooks jobs retry <hook-name>           # most recent invocation of that hook type in this worktree
daft hooks jobs retry <inv-prefix>          # specific invocation by short ID prefix
daft hooks jobs retry <job-name>            # single job (today's behavior, tightened)

daft hooks jobs retry --hook <name>         # force hook-scope interpretation
daft hooks jobs retry --inv <prefix>        # force invocation-scope interpretation
daft hooks jobs retry --job <name>          # force job-scope interpretation
```

Clap enforces mutual exclusion of the three flags. When a flag is present, the
positional argument (if any) is the flag's value and shape disambiguation is
skipped.

**Shape disambiguation** (in order, first match wins):

1. Empty arg → `LatestInvocation`.
2. Arg exactly matches a known hook type → `HookType`. Known set: `post-clone`,
   `worktree-pre-create`, `worktree-post-create`, `worktree-pre-remove`,
   `worktree-post-remove`.
3. Arg matches `^[0-9a-f]{2,8}$` → `InvocationPrefix` (prefix match on
   `invocation_id`).
4. Otherwise → `JobName`.

A user can defeat the heuristic at any time with an explicit flag:
`retry --job post-clone` retries a job literally named `post-clone` rather than
a hook of that type.

## Retry set selection

Given a selected invocation, which jobs get re-dispatched?

**Included:**

- `JobStatus::Failed` — the obvious case.
- `JobStatus::Cancelled` — jobs cancelled because a dep failed. Leaving them out
  would mean a successful dep retry still leaves the original goal unmet.

**Excluded:**

- `JobStatus::Completed` — already succeeded.
- `JobStatus::Skipped` — per-job `skip:`/`only:` matched. This is declared user
  intent, not a failure. Retrying it would silently violate the condition. If a
  skip was wrong, the right fix is to edit the condition, not to bypass it from
  the retry command.
- `JobStatus::Running` / `JobStatus::Pending` — still in flight. Retry refuses
  with an error:
  `Invocation a3f2 still has running jobs. Wait for it to finish, or cancel it.`

**Empty retry set** (selected invocation has nothing failed or cancelled): exit
0 with a friendly message,
`No failed jobs in invocation a3f2 (worktree-post-create, 3 min ago). Nothing to retry.`

**Single-job form tightening:** today's `retry <job-name>` silently re-runs a
completed job. In B, it refuses with
`Job 'db-migrate' in a3f2 is not in a retryable state (status: completed). Use 'daft hooks run' to re-fire the full hook.`

**DAG preservation within the subset** (Q4 answer):

1. Load the `JobSpec`s for every job in the retry set from their `JobMeta`.
2. For each spec, `needs.retain(|dep| retry_set.contains(dep))` — drop edges
   pointing outside the subset.
3. Pass the pruned specs to the runner's DAG executor as a fresh invocation.

This guarantees: no re-running of successful work, no dangling `needs:`
references, no reordering surprises within the retried subset. The prior success
of a dropped dep is treated as implicit satisfaction (it succeeded at some
point, which is the same guarantee a fresh run would give).

## Invocation selection per form

### `retry` (no args) — `LatestInvocation`

1. `list_invocations_for_worktree(current_branch)` → sorted by `created_at`
   desc.
2. Pick the first entry. No filtering by kind — retry invocations are eligible
   targets, matching the Q1 "keep hammering until green" intent.
3. No invocations at all →
   `No invocations found in worktree feature/x. Run a hook first.`
4. Selected invocation has empty retry set → empty-retry message from above.

### `retry <hook-name>` — `HookType`

1. `list_invocations_for_worktree(current_branch)`, filter to
   `hook_type == <hook-name>`.
2. Pick the most recent. No filtering by `trigger_command`: both automatic
   firings and `daft hooks run <name>` invocations match (Q7 answer a — "user
   said that hook type, not that event source").
3. Zero matches →
   `No invocations of worktree-post-create in worktree feature/x.`

### `retry <inv-prefix>` — `InvocationPrefix`

1. `find_invocations_by_prefix(current_branch, prefix)`.
2. Zero matches → `No invocation matching prefix 'a3f' in worktree feature/x.`
3. One match → use it.
4. Multiple matches → ambiguity error showing each candidate's short ID,
   trigger, hook type, and age, asking for a longer prefix.

### `retry <job-name>` — `JobName`

1. Find the most recent invocation in the current worktree containing a job with
   that name in Failed or Cancelled state.
2. Construct a one-element retry set. A DAG of size 1 is trivially well-formed.
3. No such job → `No failed job named 'db-migrate' in worktree feature/x.`

**Worktree identification** reuses `get_current_branch()` from
`src/core/repo.rs:51`, matching how `InvocationMeta.worktree` was populated in
sub-project A.

## Execution semantics

### Invocation grouping (Q3 answer a)

One new invocation containing all retried jobs. Fresh UUID, fresh 4-char short
ID. Its `InvocationMeta`:

```text
invocation_id:   <new UUID>
hook_type:       <copied from source invocation>
worktree:        <current branch>
trigger_command: "hooks jobs retry <form>"   # e.g., "hooks jobs retry a3f2"
created_at:      <now>
```

`trigger_command` is the human-readable form typed by the user (or the
equivalent canonical form if flags were used: e.g., `retry --inv a3f2` still
records as `hooks jobs retry a3f2`). The listing in `daft hooks jobs` renders
this string under the age, making the provenance self-documenting.

A `parent_invocation_id` for retry provenance is deliberately not added in B —
sub-project C may introduce it when cross-worktree retry chains make it
relevant.

### Foreground-first, background-second (Q2 answer a)

Split the retry set by `meta.background`:

- `fg_set` = jobs whose original ran in the foreground.
- `bg_set` = jobs whose original ran in the background.

Run in two phases within a single new invocation:

1. **Foreground phase.** Call `executor::runner::run_jobs` on `fg_set` with a
   `BufferingLogSink` pointing at the new invocation (reusing the sink wiring
   sub-project A added to `yaml_executor.rs`). Output streams inline; logs are
   captured. Failures during this phase do not short-circuit — the phase
   continues so the user sees every fg result.
2. **Background phase.** If `bg_set` is non-empty, fork the coordinator with the
   same new `invocation_id` and dispatch the bg specs through the existing
   `run_single_background_job` path. Command returns immediately after the fork.

On return, print a one-line summary:

```text
Retried 3 jobs in invocation b7c1 (2 foreground done, 1 background running). Check status: daft hooks jobs
```

If only fg or only bg jobs were present, the summary collapses to the relevant
half.

### Cross-phase `needs:` edges

A bg job's `needs:` may point to a fg job in the same retry set. Since the fg
phase runs synchronously and completes before the bg phase dispatches, the dep
is already resolved by the time the coordinator sees the bg DAG. When we prune
the bg DAG for dispatch, we drop `needs:` edges pointing into the fg set (their
gate is already decided).

This does mean a bg job whose fg dep failed in phase 1 still gets dispatched in
phase 2 — and will likely fail the same way or run without its prerequisite.
That mirrors how the original invocation would have behaved had the fg dep
failed (the runner's cancellation logic would have marked the bg job
`Cancelled`, which B then re-classifies as retry-eligible). The user got what
they asked for: "retry everything that didn't succeed." If they want a stricter
"don't retry downstream unless upstream passes" semantic, they can run two
passes manually.

## Parser design

### Retry positional disambiguation

New function in `src/commands/hooks/jobs.rs`:

```rust
enum RetryTarget {
    LatestInvocation,
    HookType(String),
    InvocationPrefix(String),
    JobName(String),
}

fn retry_target_from_arg(arg: Option<&str>, flags: RetryFlags) -> Result<RetryTarget>;
```

`RetryFlags` carries `--hook`, `--inv`, `--job` — at most one may be present.
When a flag is set, the function returns the corresponding variant directly.
When no flag is set, shape disambiguation runs against the positional.

Known hook types are a `const &[&str]` in `src/commands/hooks/jobs.rs` (or
imported from `src/hooks/types.rs` if the list lives there — to be confirmed at
implementation time).

### `JobAddress::parse` two-segment fix (B-1 fold-in)

Current parser at `src/commands/hooks/jobs.rs:122-145` handles:

- `x` → `JobOnly { job: x }`
- `x:y` → `InvocationJob { inv: x, job: y }`
- `x:y:z` → `Full { worktree: x, inv: y, job: z }`

Add a fourth variant:

```rust
enum JobAddress {
    JobOnly { job: String },
    InvocationJob { inv_prefix: String, job: String },
    WorktreeJob { worktree: String, job: String },  // NEW
    Full { worktree: String, inv_prefix: String, job: String },
}
```

**Two-segment disambiguation rule:** when parsing `x:y`, if `x` contains `/`,
interpret it as `WorktreeJob`. Otherwise `InvocationJob`. The rule is
deterministic, not heuristic: daft branch names are always `<category>/<name>`,
and invocation IDs are hex-only.

Edge case: a branch literally named `a3f2` (no slash) would parse as
`InvocationJob`. This is pathological and the three-segment form
`a3f2::db-migrate` is the escape hatch.

Update `resolve_job_address()` to handle `WorktreeJob`: find the most recent
invocation in the given worktree containing a job with that name (same logic as
`retry <job-name>`). The `Logs`, `Cancel`, and `Retry` subcommands all dispatch
through `resolve_job_address`, so all three pick up the fix.

**Important:** `retry feature/x:db-migrate` resolves via `WorktreeJob`, but when
the target worktree is not the current worktree, B refuses with
`Cross-worktree retry is not yet supported. Run this command from inside feature/x, or see 'daft help hooks jobs retry'.`
This guard is specific to the `Retry` subcommand; `Logs` and `Cancel` work
across worktrees in B via the new parser variant.

**Help text:** update the `logs`, `cancel`, and `retry` subcommand help to list
all four address forms (`name`, `inv:name`, `worktree:name`,
`worktree:inv:name`).

## Shell completions

`retry <TAB>` offers a merged, categorized list of retryable targets in the
current worktree. Candidates are filtered to those that actually have failed or
cancelled jobs — completing to a clean invocation is a dead end.

**Three helpers** in `src/commands/complete.rs`:

```rust
fn complete_retryable_hooks(current_branch: &str, prefix: &str) -> Vec<Candidate>;
fn complete_retryable_invocations(current_branch: &str, prefix: &str) -> Vec<Candidate>;
fn complete_retryable_jobs(current_branch: &str, prefix: &str) -> Vec<Candidate>;
```

Each produces zsh-`_describe`-style tab-separated `value\tdescription` lines:

- Hook: `worktree-post-create\thook — 2 failed across 1 invocation`
- Invocation: `a3f2\tinvocation — worktree-post-create, 2 failed, 3 min ago`
- Job: `db-migrate\tjob — failed in a3f2, 3 min ago`

The dispatch arm in `complete.rs`:

```rust
("hooks-jobs-retry", 1) => {
    let branch = get_current_branch_or_empty();
    let mut out = complete_retryable_hooks(&branch, word);
    out.extend(complete_retryable_invocations(&branch, word));
    out.extend(complete_retryable_jobs(&branch, word));
    print_completions(out);
}
```

**Shell wiring updates:**

- `src/commands/completions/zsh.rs` — emit a `_describe` call per category with
  category group headers (`Hooks`, `Invocations`, `Jobs`).
- `src/commands/completions/bash.rs` — merged `COMPREPLY=(...)`, no grouping
  (bash `compgen` doesn't do groups).
- `src/commands/completions/fish.rs` — three
  `complete -c daft -n '__fish_daft_retry_target'` registrations.
- `src/commands/completions/fig.rs` — add `retry` under the existing
  `hooks jobs` subcommand spec.

`logs` and `cancel` completion behavior is unchanged: they still complete to job
addresses only. Sub-project A's `complete_job_addresses` helper is untouched.

**Cost:** completion runs a filesystem scan of the current worktree's invocation
directory. In practice a worktree has tens of invocations, not thousands;
sub-10ms is the target. If a user has 10k invocations, they have bigger problems
than completion latency.

## Data & storage

### New `JobMeta` field

```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JobMeta {
    // ... existing fields from sub-project A ...
    #[serde(default)]
    pub needs: Vec<String>,
}
```

Written at job-start time in two sites:

- `BufferingLogSink::on_job_start` (foreground jobs) — the sink already has the
  `JobSpec` in hand.
- `run_single_background_job` in `src/coordinator/process.rs` (background jobs)
  — same.

Both sites persist `meta.needs = spec.needs.clone()` at start, before the first
output line arrives.

`#[serde(default)]` makes it back-compat: invocations written under sub-project
A (no `needs:` field) read back as empty, which is safe for listings but means
those older invocations can't be bulk-retried with DAG fidelity. That's
acceptable — A landed yesterday, we're not trying to retry six-month-old
invocations.

### New `LogStore` read methods

```rust
impl LogStore {
    pub fn list_invocations_for_worktree(
        &self,
        worktree: &str,
    ) -> Result<Vec<InvocationMeta>>;

    pub fn find_invocations_by_prefix(
        &self,
        worktree: &str,
        prefix: &str,
    ) -> Result<Vec<InvocationMeta>>;

    pub fn list_jobs_in_invocation(
        &self,
        invocation_id: &str,
    ) -> Result<Vec<JobMeta>>;
}
```

`list_invocations_for_worktree` and `find_invocations_by_prefix` share a common
"scan worktree invocation dir, read all `invocation.json` files" pass plus
filtering. `list_jobs_in_invocation` may already exist in some form from
sub-project A's listing path; if so, reuse rather than duplicate.

Results are sorted by `created_at` descending.

### No new write methods

Retry constructs a new `invocation_id` and dispatches through the existing write
path:

1. `write_invocation_meta(new_id, InvocationMeta { ... })` — already exists.
2. For fg jobs: `BufferingLogSink` wired to `new_id` handles `on_job_start`,
   `on_job_output`, `on_job_complete`, producing per-job `meta.json` and log
   files. No changes needed.
3. For bg jobs: forked coordinator with `new_id` dispatches through
   `run_single_background_job`, which writes its own per-job meta/log. No
   changes needed.

### Deleted worktrees

`list_invocations_for_worktree` takes a branch-name string. It does not check
whether the worktree still exists on disk — that's C's concern. B only resolves
against the current branch, which exists by definition.

## CLI entry point refactor

Today's `retry_job` in `src/commands/hooks/jobs.rs` handles exactly one job. It
becomes a private helper `retry_single_job` and is called from a new top-level
`retry_command(args: &RetryArgs) -> Result<()>`:

```rust
pub fn retry_command(args: &RetryArgs) -> Result<()> {
    let target = retry_target_from_arg(args.positional.as_deref(), args.flags())?;
    let (source_inv, retry_set) = resolve_retry_set(target, &current_branch()?)?;
    if retry_set.is_empty() {
        print_empty_retry_message(&source_inv);
        return Ok(());
    }
    let (fg_specs, bg_specs) = split_by_mode(retry_set);
    let new_inv = LogStore::new_invocation_id();
    write_retry_invocation_meta(&new_inv, &source_inv, target_str)?;
    if !fg_specs.is_empty() {
        run_foreground_retry(&fg_specs, &new_inv)?;
    }
    if !bg_specs.is_empty() {
        dispatch_background_retry(&bg_specs, &new_inv)?;
    }
    print_retry_summary(&new_inv, fg_specs.len(), bg_specs.len());
    Ok(())
}
```

This function is the only place that constructs a retry invocation. The
single-job legacy path collapses into the general case: `retry_single_job(name)`
is now "call `retry_command` with `RetryTarget::JobName(name)`."

## Non-goals for sub-project B

- **Cross-worktree retry.** Sub-project C. B refuses it explicitly.
- **Retry for deleted worktrees.** Sub-project C.
- **`parent_invocation_id` provenance.** Sub-project C, if needed.
- **Listing filters** (`--status failed`, `--hook <name>`). Nice-to-have,
  tracked in the basket follow-up list.
- **Force-run skipped jobs.** Explicitly rejected in Q8.
- **Non-Unix background fallback sink parity.** Sub-project C cleanup.
- **`JobMeta::skipped` constructor dedup.** Separate cleanup.

## Dependencies on sub-project A

- `JobStatus::Failed` / `JobStatus::Cancelled` / `JobStatus::Skipped` recorded
  by A's universal logging.
- `invocation.json` written by A — B relies on `hook_type`, `worktree`,
  `created_at`, `trigger_command`.
- `JobMeta.command`, `working_dir`, `env`, `background` — A guarantees these are
  present for both fg and bg jobs, so `JobSpec` reconstruction works uniformly.
- `BufferingLogSink` — B reuses the same sink to capture fg retry output.
- `complete_job_addresses` completion helper — B extends the dispatch site but
  leaves this helper untouched.
- `get_current_branch()` — reused for worktree identification.

## Testing

### Unit tests (~15–20 new)

- `retry_target_from_arg`: empty → Latest, each known hook → HookType, hex
  prefixes of lengths 2/4/8 → InvocationPrefix, word → JobName, each flag
  overrides shape, flag + positional together consumes positional as flag value.
- `JobAddress::parse`: two-segment with slash → WorktreeJob, two-segment without
  → InvocationJob, three-segment unchanged, pathological no-slash branch name
  `a3f2`.
- `resolve_job_address` → `WorktreeJob` variant finds the right invocation.
- `build_retry_set(invocation_jobs: &[JobMeta]) -> (Vec<JobSpec>, Vec<String>)`
  — pure function: all green → empty, mixed failed/cancelled/completed → correct
  subset, `needs:` outside subset pruned, `needs:` inside subset preserved,
  single-element set.
- `list_invocations_for_worktree` / `find_invocations_by_prefix` against a temp
  `LogStore` with synthetic records: filtering, sorting, zero-match,
  multi-match.
- Completion helpers: hooks with zero failures excluded, invocation candidates
  filtered by failure presence, job candidates drawn from latest failing
  invocation only.

### Integration scenarios (7 new, in `tests/manual/scenarios/hooks/`)

1. **`retry-empty.yml`** — Hook with a guaranteed-fail job, run it, run `retry`
   with no args, verify a new invocation appears and fails again. Fix the job,
   retry a third time, verify the third invocation is green. Listing shows three
   chronological invocations.

2. **`retry-hook-name.yml`** — Two different hook types each fire and produce
   failures. `retry worktree-post-create` picks only the post-create, leaves the
   other untouched. Then run `daft hooks run worktree-post-create` manually,
   then `retry worktree-post-create` — verify the manual one is picked (most
   recent, regardless of trigger).

3. **`retry-invocation-prefix.yml`** — Run a hook twice producing two
   invocations, read short IDs from `daft hooks jobs`, retry the _older_ one
   explicitly by prefix. Also test 1-char prefix matching both → expect
   ambiguity error listing both candidates.

4. **`retry-mixed-fg-bg.yml`** — Hook with one fg job (fails), one bg job
   (fails), one successful fg job. Retry re-runs only the two failures, in one
   new invocation. Verify: fg failure output inline, bg failure visible in
   `daft hooks jobs` after return, successful fg job untouched.

5. **`retry-needs-pruning.yml`** — Hook with DAG `a → b → c`. Run 1: `a`
   succeeds, `b` fails, `c` cancelled. Retry. Verify new invocation has `b` and
   `c`, `b.needs = []`, `c.needs = [b]`, `a` not re-run. Fix `b` to succeed on
   retry, verify `c` then runs successfully.

6. **`retry-job-name.yml`** — Single-job retry still works unchanged, plus the
   new tightening: attempting to retry a `Completed` or `Skipped` job errors out
   with the expected message.

7. **`retry-address-two-segment.yml`** — Tests the B-1 fold-in.
   `daft hooks jobs logs feature/x:db-migrate` (from outside `feature/x`)
   resolves to the latest matching invocation and prints its logs.
   `daft hooks jobs retry feature/x:db-migrate` (from outside `feature/x`)
   refuses with the "cross-worktree not yet supported" message.

## File inventory

| File                                              | Change                                                                                                                                                                                                                                        |
| ------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/commands/hooks/jobs.rs`                      | New `RetryTarget`, `retry_target_from_arg`, `retry_command`, `resolve_retry_set`, `split_by_mode`, `run_foreground_retry`, `dispatch_background_retry`; extended `JobAddress` enum with `WorktreeJob`; tightened single-job retry state check |
| `src/commands/hooks/mod.rs` or routing            | Update clap `Retry` subcommand: positional → `Option<String>`, add `--hook` / `--inv` / `--job` mutually-exclusive flags                                                                                                                      |
| `src/commands/complete.rs`                        | New `complete_retryable_hooks` / `complete_retryable_invocations` / `complete_retryable_jobs` helpers; new `("hooks-jobs-retry", 1)` dispatch arm                                                                                             |
| `src/commands/completions/{bash,zsh,fish,fig}.rs` | Wire `retry` completion, categorized in zsh                                                                                                                                                                                                   |
| `src/coordinator/log_store.rs`                    | New `list_invocations_for_worktree`, `find_invocations_by_prefix`; `JobMeta` gains `needs: Vec<String>` with `#[serde(default)]`                                                                                                              |
| `src/executor/log_sink.rs`                        | `BufferingLogSink::on_job_start` persists `spec.needs` into `JobMeta.needs`                                                                                                                                                                   |
| `src/coordinator/process.rs`                      | `run_single_background_job` persists `spec.needs` into `JobMeta.needs` at start                                                                                                                                                               |
| `tests/manual/scenarios/hooks/retry-*.yml`        | 7 new integration scenarios                                                                                                                                                                                                                   |

## Open design items deferred to implementation

- **Known-hook-types source of truth.** The `retry_target_from_arg` known-hook
  list and any existing enum in `src/hooks/types.rs` must share one definition.
  To be confirmed during implementation; trivial.
- **`KNOWN_HOOK_TYPES` vs. runtime registration.** Daft's hook types are
  compile-time. If that ever becomes dynamic, the disambiguation set will need
  to be resolved at runtime. Out of scope for B.
- **Short-ID uniqueness under extreme load.** The 4-char prefix is lifted from
  A's listing and has not been stress-tested against thousands of invocations in
  one worktree. The ambiguity-error path in `InvocationPrefix` resolution
  handles collisions gracefully, so B doesn't need to pre-solve this.
