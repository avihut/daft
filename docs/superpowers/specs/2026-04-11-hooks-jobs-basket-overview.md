# Hooks Jobs Redesign — Basket Overview

**Status:** Sub-project A complete. Sub-project B in brainstorming. Sub-project
C deferred. **Branch:** `feat/background-hook-jobs` (all three sub-projects
stack on this branch)

## Motivation

The `daft hooks jobs` command evolved from a background-coordinator feature and
inherited several gaps that surfaced as soon as users tried to use it for real
debugging:

1. **Foreground jobs were invisible.** Only the forked coordinator wrote to the
   log store. Anything that ran in the main process — which includes the common
   case of fg-only hooks and all remove hooks — left no trace.
2. **Skipped jobs were invisible (and in fact weren't being skipped).** Per-job
   `skip:` / `only:` conditions were parsed from YAML but never evaluated. Users
   could declare them and watch nothing happen.
3. **Empty-jobs hooks were invisible.** A hook that declared no runnable jobs
   produced no log record, so users couldn't tell the hook had fired at all.
4. **Single-job retry only.** `daft hooks jobs retry <job>` reconstructed
   exactly one job at a time. There was no way to say "retry all failed from the
   last run," let alone "retry the failed cleanup jobs from that worktree I just
   deleted."
5. **Bad listing UX.** Cross-worktree view was a flat status-grouped table that
   became unreadable with more than a couple of worktrees.

The redesign decomposes into three independently-shippable sub-projects. Each
produces a working, useful feature on its own and unblocks the next.

## Sub-project A — Universal Hook Invocation Logging ✅ Complete

**Spec:** `docs/superpowers/specs/2026-04-11-universal-hook-logging.md`
**Plan:** `docs/superpowers/plans/2026-04-11-universal-hook-logging.md`
**Status:** Shipped in 25 commits on `feat/background-hook-jobs`, ending at
commit `2db5294`. 43/43 integration scenario steps pass. 1034 unit tests pass.

Fixes gaps #1, #2, #3 above. Every hook run now writes an `invocation.json`,
foreground job output gets captured by a new `BufferingLogSink`, skipped jobs
produce sparse records with a reason, empty hooks appear in the listing with a
`(no jobs declared)` placeholder, and per-job `skip:` / `only:` evaluation is
activated. The whole `daft hooks jobs` listing reflects what actually happened.

Deferred items surfaced during implementation are catalogued in
`~/.claude/projects/-Users-avihu-Projects-daft/memory/project_hooks_jobs_followups.md`
for pickup by B and C.

## Sub-project B — Bulk Retry Shorthands (current)

**Fixes gap #4** — the single-job retry limitation, in the in-repo scope
(same-worktree retries). Cross-worktree retry — especially for deleted worktrees
— is explicitly deferred to C.

### User-stated requirements

From the original conversation:

> "I think there should be a shorthand for running all of the failed jobs from
> an invocation with shorthands for running failed jobs from hooks and the most
> recent run."

> "Maybe an empty retry just retries the failed jobs from the last invocation,
> and a retry hook-name retries all of the failed jobs from the last hook run of
> that name. All are automatically assumed to be related to the worktree they're
> running on."

### Tentative scope

- **`daft hooks jobs retry`** (no args) — retry all failed jobs from the most
  recent invocation in the current worktree.
- **`daft hooks jobs retry <hook-name>`** — retry all failed jobs from the most
  recent invocation of that hook name in the current worktree.
- **`daft hooks jobs retry <invocation-id>`** — retry all failed jobs from a
  specific invocation.
- **`daft hooks jobs retry <job-name>`** — retry a single job (today's
  behavior).
- Disambiguate the single-token form (hook vs. invocation vs. job) via smart
  parsing, with explicit flags (`--hook`, `--inv`, `--job`) as escape hatches.

### Open design questions (for brainstorming)

1. **What counts as "most recent invocation" for the empty-retry case?** Most
   recent of any kind (including retry invocations), most recent hook-type only,
   or most recent with any failed jobs? The "keep hammering until green"
   workflow argues for "most recent period."

2. **Foreground vs. background for retried jobs.** Today's retry hardcodes
   `background: true`. After sub-project A, a failed foreground job has
   `meta.background: false`. Should retry honor the original execution mode, or
   always re-dispatch as background?

3. **One invocation or many?** Bulk retry should probably create one new
   invocation containing all retried jobs (preserving any `needs:` relationships
   between them), rather than N independent invocations.

4. **Preserving DAG relationships.** Today's `retry_job` builds a `JobSpec` with
   empty `needs:`. Bulk retry needs to reconstruct any dependencies that existed
   in the original invocation, at least among the subset being retried.

5. **Disambiguation rules for single-token positional arg.** Proposed
   heuristics:

   - Matches a known hook type (`worktree-post-create`, `worktree-pre-remove`,
     `post-clone`, etc.) → hook scope.
   - All-hex short prefix (`a3f2`, etc.) → invocation scope.
   - Otherwise → job scope (today's behavior).
   - Explicit flags override: `--hook`, `--inv`, `--job`.

6. **Composite addressing gap from sub-project A follow-up (B-1).** Today
   `daft hooks jobs logs worktree:job` doesn't work — must use `worktree::job`
   with empty invocation. This is a UX wart that's relevant to retry addressing
   too. Decision: fold this fix into sub-project B, or defer to C.

7. **Scope of `retry <hook-name>` — strictly automatic hook invocations, or also
   manual `hooks run <name>` invocations?** The hook-name form is syntactically
   ambiguous between "the automatic hook" and "any invocation with that
   hook_type."

8. **What about skipped jobs?** `retry` targets failed jobs by default. Should
   there be a way to force-run skipped jobs (bypass their `skip:` condition)?
   Probably no — but worth confirming.

9. **Shell completions.** After sub-project A, the existing
   `complete_job_addresses` helper resolves job names. Bulk retry means new
   completion categories (hook names, invocation IDs) need to be supported.

### Dependencies on A

- Uses `JobStatus::Failed` / `JobStatus::Skipped` recorded by A's universal
  logging.
- Uses `invocation.json` written by A to identify invocation scope.
- Reconstruction pulls from `JobMeta.command`, `working_dir`, `env` which A
  ensures are present for both fg and bg jobs.

### Non-goals for B

- **Cross-worktree retry.** Sub-project C.
- **Retry for deleted-worktree jobs.** Sub-project C.
- **Listing filters** (`--status failed`, `--hook <name>`). Nice-to-have, not
  required for bulk retry.
- **`JobMeta::skipped` constructor refactor.** Separate cleanup.
- **Non-Unix fallback parity for the sink.** Separate cleanup.

## Sub-project C — Cross-Worktree Retry (deferred)

**Fixes the hardest scenario:** retrying failed cleanup jobs for a worktree that
no longer exists. This is the original user story that surfaced during
sub-project A (_"re-running failed cleanup jobs, which is also an issue since I
don't see logs for the remove hooks"_) — A made the cleanup records visible, B
handles the simple cases, C handles the gnarly cases.

### Tentative scope

- Cross-worktree retry via composite addressing (`<worktree>:<inv>:<job>` or
  similar).
- Handling deleted worktrees: detect that the stored `working_dir` no longer
  exists, decide what to do (refuse? resurrect? run in a replacement location?
  run with a synthetic env that makes sense for cleanup?).
- Fix for the composite addressing gap B-1 if sub-project B deferred it.
- Non-Unix fallback parity (the bg coordinator path that still passes `None` for
  the sink).

### Open questions

- What does "retry cleanup for `feature/x`" mean when `feature/x` has been
  removed and its directory is gone? There is no obvious place to run the
  command. Options:
  - Refuse with an informative error.
  - Let the user pass an explicit `--cwd` for the retry.
  - Synthesize a temp directory that mirrors the structure the hook expected.
  - Only retry cleanup jobs whose recorded `command` is idempotent and has no
    `cwd` dependency — but we can't detect that automatically.
- What if the branch is also deleted? The `invocation.json`'s `worktree` field
  is the branch name, which may no longer exist in git.

C is explicitly a design conversation that needs its own brainstorming pass.
Don't try to design it during B.

## Cross-cutting constraints

- **Backward compatibility** — the redesign branch is pre-1.0 for this feature
  surface. Breaking changes to `daft hooks jobs` CLI are acceptable where they
  improve the UX; no backwards-compat shims.
- **Single branch** — all three sub-projects stack on
  `feat/background-hook-jobs`. Each sub-project lands as a coherent batch of
  commits with its own spec + plan + integration scenarios. The full branch
  ships as one PR or multiple, user's choice.
- **Integration scenarios** — each sub-project adds new
  `tests/manual/scenarios/hooks/*.yml` entries. Sub-project A added 6;
  sub-projects B and C will add more.

## Running list of deferred items

Tracked in
`~/.claude/projects/-Users-avihu-Projects-daft/memory/project_hooks_jobs_followups.md`.
Current candidates:

**For sub-project B:**

- Composite addressing two-segment form (B-1)
- `post-clone` hook visibility scenario (B-2)
- `JobMeta::skipped` constructor dedup (B-3)
- Listing filters (B-4)

**For sub-project C:**

- Non-Unix bg fallback passes `None` for sink (C-2)
- `write_invocation_meta` `with_context` (C-3)
- `runner.rs` fully-qualified `LogSink` path cleanup (C-4)
