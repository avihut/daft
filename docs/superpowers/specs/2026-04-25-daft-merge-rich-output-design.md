---
title: daft merge — rich output and hook parity
date: 2026-04-25
status: design
supersedes:
  - 2026-04-24-daft-merge-design.md (sections "Output" and "Hooks" — for the
    cleanup phase of `-r` / `-rb` only)
---

# daft merge — rich output and hook parity design

## Summary

`daft merge` is functionally complete but renders at a lower level of polish
than the rest of daft. Compared to `daft remove`, the merge command emits raw
`println!` lines, streams git's own stdout unstyled, and — most importantly —
**fails to fire the `worktree-pre-remove` / `worktree-post-remove` hooks** when
it cleans up a source worktree with `-r` / `-rb`. This design fixes all three.

The merge phase gets a buffered, spinner-driven presentation that matches
`daft remove`'s feel. The cleanup phase is hoisted out of
`core::worktree::merge` and delegated to the existing `branch_delete::execute`
pipeline, which already provides hook brackets, sink-routed step messages, and
the styled `Deleted X (worktree, local branch)` summary line.

`pre-merge` and `post-merge` already render through the rich hook-box presenter
(via `MergeHookRunner` + `HookExecutor` + `CliPresenter::auto`, see
`src/commands/merge.rs:820-898`); no change is needed there. The user's sandbox
screenshot showed no `pre-merge` / `post-merge` boxes because they had no such
hooks configured — once one is added, it renders the same way the
`worktree-pre-remove` direnv-revoke box does.

This refinement supersedes the corresponding output and cleanup-hook sections of
the [2026-04-24 daft merge design](2026-04-24-daft-merge-design.md). All other
parts of that spec — and the entire
[2026-04-25 squash + cleanup refinement](2026-04-25-daft-merge-squash-cleanup-design.md)
— stand.

## Motivation

Concrete observation from a sandbox session:

```
❯ daft merge test --squash -rb
Updating a6633f6..01aaa5b
Fast-forward
Squash commit -- not updating HEAD
 new-file | 0
 1 file changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 new-file
[feature 8931f31] Squashed commit of the following:
 1 file changed, 0 insertions(+), 0 deletions(-)
 create mode 100644 new-file
Removing worktree at /private/tmp/.../test/tax-analyzer.test...
Deleting branch test...
Squash merged and cleaned up test.
```

versus the same user's `daft remove` output for the same source:

```
❯ daft remove test
┌──────────────────────────────────────────────┐
│ daft hooks v1.7.2  hook: worktree-pre-remove │
└──────────────────────────────────────────────┘
┃  direnv-revoke ❯
┃  No output
────────────────────────────────────────
summary: (done in 106ms)
  ✔ direnv-revoke (106ms)
Deleted test (worktree, local branch)
```

Three concrete gaps stand out:

1. **`worktree-pre-remove` / `worktree-post-remove` hooks do not fire** when
   `daft merge -r` / `-rb` removes the source worktree. The user's
   `direnv-revoke` hook silently does not run. This is a hook-contract bug, not
   a stylistic gap: any hook the user added under `worktree-*-remove` is
   bypassed by the merge cleanup path. (`pre-merge` / `post-merge` are wired
   correctly and already render through the hook-box presenter — they just
   weren't configured by the user, so nothing rendered for them.)
2. **Git's stdout is unstyled and verbose**: `Updating ...`, `Fast-forward`,
   `Squash commit -- not updating HEAD`, `[feature 8931f31] Squashed commit ...`
   stream through to the user. `daft remove` produces zero git noise.
3. **daft's own progress lines are raw `println!`s**:
   `Removing worktree at ...`, `Deleting branch ...`,
   `Squash merged and cleaned up test.` print without spinner integration,
   without the styled hook box, and without the final
   `Deleted X (worktree, local branch)` summary line that `daft remove`
   produces.

The user's request is for `daft merge` to ship at the same level of polish as
`daft remove`. The cleanup-hook gap also needs fixing regardless of polish — it
is a behavioral inconsistency users will hit the moment they configure a
`worktree-pre-remove` hook.

## Behavior changes

### Cleanup is delegated to `branch_delete::execute`

When `daft merge -r` (or `-rb`) cleans up a source, daft invokes
`branch_delete::execute` for that source instead of calling
`git.worktree_remove` and `git.branch_delete` directly. The single delegation
buys, in one stroke:

- `worktree-pre-remove` and `worktree-post-remove` hooks fire correctly, with
  the same hook box, spinner, and summary rendering used by `daft remove`.
- Step messages (`Removing worktree at ...`, `Deleting branch ...`) flow through
  `OutputSink::on_step`, integrating with the spinner and respecting the
  presenter's formatting rules.
- The final `Deleted <source> (worktree, local branch)` line is produced by
  `branch_delete::execute`'s existing styled output path.
- `--quiet`, `--verbose`, and any future global output flags work uniformly.

`BranchDeleteParams` gains one new field — `keep_local_branch: bool` — that
mirrors the existing `remote_only` knob. When `true`, validation skips the
branch-deletion checks (merged-into-default, in-sync-with-remote) and the
deletion phase skips Step 4 (local branch delete) and Step 2 (remote delete).
Steps 1, 3, and 5 (pre-remove hook, worktree remove, post-remove hook) still
run. This handles the merge-only case `daft merge -r` (without `-b`), where the
user removes the source worktree but keeps the local branch.

Flag matrix used by merge cleanup:

| Merge cleanup case       | `force` | `keep_local_branch` | `delete_remote` | `command_label` |
| ------------------------ | ------- | ------------------- | --------------- | --------------- |
| `-r` without `-b`        | `true`  | `true`              | `false`         | `"merge"`       |
| `-rb` (regular merge)    | `true`  | `false`             | `false`         | `"merge"`       |
| `-rb` (squash committed) | `true`  | `false`             | `false`         | `"merge"`       |

`force = true` is set uniformly for all merge cleanup paths because
`plan_cleanup` has already validated reachability against the _actual_ merge
target (`target_branch`, which may be a non-default branch like `develop`).
`branch_delete::execute`'s own validation hardcodes the _default_ branch
(`main`/`master`) for its merged-into-default check, which would be wrong for
cross-target merges. Setting `force=true` bypasses that redundant + incorrect
check; the planner's reachability check against the real target is the source of
truth.

The pre-validation safety chain established in the
[squash + cleanup design](2026-04-25-daft-merge-squash-cleanup-design.md) is
preserved: pre-validation runs in merge before `branch_delete::execute` is
invoked, so the transactional "validate-then-mutate" guarantee continues to
hold. The stability check (source SHA equality) also runs in merge, before
delegation. The squash-committed path's justified-`-D` is preserved by the
combination of `plan_cleanup`'s SHA-stability check and the
`force=true`/`keep_local_branch=false` flag combination.

`command_label = "merge"` (a new field on `BranchDeleteParams`) flows into the
hook context as `DAFT_COMMAND`, so `worktree-pre-remove` /
`worktree-post-remove` hook scripts can distinguish merge cleanup from
standalone `daft remove` (which sets `command_label = "branch-delete"`).

### Cleanup execution moves from `core::worktree::merge` to `commands::merge::run`

Today `execute_cleanup` lives in `core::worktree::merge` and calls git directly.
After this change, `core::worktree::merge` returns a structured `CleanupIntent`
(or extends the existing `MergeOutcome` to carry the cleanup plan) and the
command-layer caller — `commands::merge::run` — invokes `branch_delete::execute`
for each source. This keeps `core::worktree::merge` free of `&mut dyn Output` /
`HookExecutor` plumbing and matches the layering already used by other commands
(`commands` owns I/O; `core` owns logic).

Pre-validation stays where it is — it does not need an output sink, only a git
handle. The flow becomes:

1. `core::worktree::merge` runs the merge, runs the squash commit if applicable,
   captures source SHAs, runs cleanup pre-validation, runs the stability check,
   and returns a `CleanupPlan` (or `Skipped` reason).
2. `commands::merge::run` consumes the plan. If `Skipped`, it prints the
   appropriate state-aware line and exits. If `CleanupPlan(items)`, it iterates
   items and dispatches each through `branch_delete::execute` with
   `force = true` (uniformly — see flag matrix and rationale above).
3. After all items succeed, `commands::merge::run` prints the final state-aware
   success line via the existing `output.success(...)` path.

If any `branch_delete::execute` call fails, the merge command surfaces that
failure with the same context wrapping today's `execute_cleanup` produces
(`already removed: ...; failed to remove ...`). The transactional guarantee
remains: pre-validation has already ensured every step _can_ succeed; a Phase 2
failure is a race or filesystem error, not a logic bug.

### Pre-merge / post-merge hooks (no change — already rich)

Verified in `src/commands/merge.rs:820-898`: `MergeHookRunner` already builds a
`HookExecutor` and dispatches through
`CliPresenter::auto(&HookOutputConfig::default())`, which is the same plumbing
`branch_delete::execute` uses. When the user configures a `pre-merge` or
`post-merge` hook, the box renders the same way the `worktree-pre-remove` box
renders for `daft remove`. The plan should include one regression scenario
asserting on the box rendering for these hooks (see "Test changes" below) but no
implementation work is required for this surface.

### Merge phase: buffer git stdout, suppress on success

`git merge`, `git merge --squash`, and the post-squash `git commit` all stream
useful-on-failure but noisy-on-success output. After this change:

- daft starts a spinner with a phase-appropriate label
  (`Merging <src> into <target>...`, `Squashing <src> into <target>...`,
  `Committing squash...`).
- Git is invoked with stdout/stderr captured into a buffer.
- On success, the spinner finishes, then a single styled step message renders
  (see "State-aware step messages" below); the buffer is discarded.
- On failure, the spinner finishes first, **then** the captured buffer is dumped
  verbatim to stderr (writing to a not-yet-stopped spinner mangles output via
  carriage-return overwrites), and finally the error path runs as today. The
  user still sees git's full diagnostic output when they need it.
- With `--verbose` (or `DAFT_VERBOSE=1`), the buffer is dumped after the spinner
  finishes regardless of success or failure — same semantics as `--verbose`
  already has elsewhere in daft.

This matches `daft remove`'s "zero git noise on the happy path" feel without
losing diagnostic value when something breaks.

### State-aware step messages

The buffer-and-suppress approach pairs with single-line styled step messages
that summarize what just happened. These replace the multi-line git output:

| State                                        | Step message                                                  |
| -------------------------------------------- | ------------------------------------------------------------- |
| Fast-forward succeeded                       | `Fast-forwarded <target> to <short-sha>`                      |
| Regular merge commit succeeded               | `Merged <source> into <target> (commit <short-sha>)`          |
| Squash + commit succeeded                    | `Squashed <source> into <target> (commit <short-sha>)`        |
| Squash + `--no-commit` (or `commit = false`) | `Squash staged on <target>`                                   |
| Already up to date                           | `<target> is already up to date with <source>` (existing-ish) |

These are emitted via `output.step(...)` (or the equivalent sink call) before
cleanup runs. The final summary line stays as designed in the squash + cleanup
spec (`Squash merged and cleaned up <source>.`, `Merge complete.`, etc.) and is
emitted via `output.success(...)` so it picks up the same styling as
`daft remove`'s `Deleted X (...)`.

For multi-source merges (octopus), one step message is emitted per source after
the merge completes, in the order the sources were specified.

### Editor pause/resume around `git commit`

When the squash commit step opens `$EDITOR`, the spinner must pause cleanly so
the editor owns the terminal. The infrastructure already exists:

- `CliOutput::pause_spinner` / `resume_spinner` (`src/output/cli.rs:252` /
  `:262`) handle the underlying terminal state.
- `CommandBridge::run_hook` already brackets hook execution with `pause_spinner`
  / `resume_spinner` (`src/core/progress.rs:110` / `:112`).

The merge command's commit step adopts the same bracketing: pause before
`git commit` is invoked when an editor will open, resume after it returns. The
TTY guard from the squash spec gates whether an editor _can_ open in the first
place; this section is purely about presentation when one will.

`-y` already implies `--no-edit` (per the squash spec), so the editor path is
not entered in non-interactive flows; the pause/resume is a safety net for the
interactive path.

### Verbose escape hatch

`daft merge` already exposes `--verbose` (`src/commands/merge.rs:243`). After
this change:

- Without `--verbose`: git stdout is buffered and discarded on success; only
  daft's styled step + summary lines are visible.
- With `--verbose`: the captured git buffer is dumped to stderr after the
  spinner finishes (success or failure). The styled step + summary lines still
  render. Verbose output is for power users who want to see git's view of what
  happened (commit SHAs, file change counts, fast-forward range).
- `DAFT_VERBOSE=1` continues to behave the same as the flag.

`--quiet` is similarly unchanged in semantics; it suppresses the styled step
messages **and** the final success line, matching how `daft remove --quiet`
behaves (per `src/output/cli.rs:18` — `result()` is "always shown unless
quiet"). Errors and warnings still print.

## Architecture

### Layer boundaries

```
commands::merge::run
├── output: &mut dyn Output
├── HookExecutor (for pre-merge / post-merge)
└── invokes:
    ├── core::worktree::merge::execute_start
    │   ├── runs merge / squash / commit (with captured I/O)
    │   ├── fires pre-merge / post-merge via MergeHookRunner
    │   └── returns StartOutcome { ..., captured_git_output, was_fast_forward,
    │       merge_commit_sha, ... }
    ├── core::worktree::merge::plan_cleanup
    │   ├── runs pre-validation + stability check
    │   └── returns Vec<CleanupItem>
    └── for each item in plan:
        ├── builds BranchDeleteParams from item (with appropriate
        │   force / keep_local_branch / delete_remote per the matrix above)
        ├── builds CommandBridge { output, executor: HookExecutor::new(...) }
        └── branch_delete::execute(params, &mut bridge)
            ├── validates (skips branch-deletion checks if keep_local_branch)
            ├── for each validated branch:
            │   ├── fires worktree-pre-remove
            │   ├── removes worktree (sink-routed step + spinner)
            │   ├── (skipped if keep_local_branch) deletes local branch
            │   └── fires worktree-post-remove
            └── returns BranchDeleteResult
```

`core::worktree::merge` does not gain an `Output` parameter. The buffer-and-
suppress wrapping for git invocations happens at the `commands::merge::run`
layer — the command starts the spinner, calls into core with a captured-IO git
handle, finishes the spinner, then delegates cleanup.

If the captured-IO git handle would require a wider refactor than warranted, an
acceptable fallback is to pass a small `OutputBridge` to core that exposes only
`start_spinner` / `finish_spinner` / `step` (a strict subset of `Output`) — but
the preferred path is for `core` to remain output-free.

### `CleanupPlan` shape

The plan returned by `core::worktree::merge::execute` is a list of items, one
per source being cleaned up. Each item carries:

- `source: String` — the original source spec (for messages).
- `worktree_path: Option<PathBuf>` — present iff `-r` was effective for this
  source.
- `branch_name: Option<String>` — present iff `-b` was effective and the source
  resolves to a local branch.
- `force_delete: bool` — true iff the squash + stability chain justified `-D`
  for this source's branch.

This mirrors today's `CleanupWork` struct in `execute_cleanup`; the change is
that the struct travels back to `commands::merge::run` instead of being consumed
in-place.

### Pre-validation stays in core

The transactional guarantee depends on pre-validation completing before any
mutation. Pre-validation logic — checking for unmerged branches, dirty source
worktrees, the stability check — is git-only and stays in
`core::worktree::merge`. If it fails, core returns an error and no `CleanupPlan`
is produced; `commands::merge::run` prints the error and exits before delegating
anything.

### Spinner / output ownership

`commands::merge::run` owns the merge-phase spinner. It starts the spinner
before calling core, finishes it after core returns. The cleanup delegation loop
runs outside the merge-phase spinner — `branch_delete::execute` owns its own
spinner lifecycle. This avoids nested-spinner state and lets each phase present
at its natural cadence.

### Phase sequencing

The full execution order is:

1. `pre-merge` hook box renders (if configured) — driven by `MergeHookRunner`
   inside `core::worktree::merge`, no spinner active.
2. Merge-phase spinner starts (`Merging X into Y...` / `Squashing X into Y...`).
3. `git merge` (or `git merge --squash`) runs with captured I/O.
4. If squash + commit: spinner pauses, `git commit` opens editor (if applicable)
   or runs non-interactively, spinner resumes.
5. Merge-phase spinner finishes.
6. State-aware step message renders.
7. `post-merge` hook box renders (if configured).
8. For each cleanup item: `branch_delete::execute` runs, owning its own spinner
   and rendering its own `worktree-pre-remove` / `worktree-post-remove` hook
   boxes plus the `Deleted X (...)` line.
9. Final state-aware success line renders via `output.success(...)`.

No two spinners are ever concurrently active. Hook boxes never render with a
spinner spinning — `CommandBridge::run_hook` already brackets executor calls
with `pause_spinner` / `resume_spinner`.

### `HookExecutor` instantiation per cleanup item

`branch_delete::execute`'s existing call site
(`src/commands/branch_delete.rs:105-106`) constructs a fresh
`HookExecutor::new(HooksConfig::default())` for each invocation. The merge
cleanup loop adopts the same pattern: one new `HookExecutor` per `CleanupPlan`
item. This is consistent with how merge's own `MergeHookRunner` already
constructs its executor (`src/commands/merge.rs:847`, also
`HooksConfig::default()`), so the hook discovery surface is uniform across the
whole merge command.

Reusing a single `HookExecutor` across the loop is a possible future
optimization but has no observable user benefit today; the per-item pattern
keeps the merge cleanup code structurally identical to `daft remove`'s call
site, which is the readability win.

## Hook interactions

This design changes the **rendering** of three hook firings and adds two new
ones (the `worktree-*-remove` pair on the cleanup path). It does not change when
any hook fires beyond the `worktree-*-remove` fix.

| Hook                                        | Fires when                              | Rendering         |
| ------------------------------------------- | --------------------------------------- | ----------------- |
| `pre-merge`                                 | unchanged (existing trigger)            | now: hook box     |
| `post-merge`                                | unchanged (existing trigger)            | now: hook box     |
| `worktree-pre-remove` (per source removed)  | new: when `-r` / `-rb` cleans up source | hook box (always) |
| `worktree-post-remove` (per source removed) | new: when `-r` / `-rb` cleans up source | hook box (always) |

`worktree-pre-remove` failures emit a warning and cleanup continues — inheriting
the existing behavior of `daft remove`, where `worktree-pre-remove` defaults to
`FailMode::Warn` (`src/hooks/mod.rs`). Users who want abort semantics can set
`fail_mode: abort` on the hook in `daft.yml`. The merge commit on `<target>`
stays in place regardless — daft's "merge result is never rolled back" rule
continues to hold.

`worktree-post-remove` is invoked from `branch_delete::execute` after the
worktree has been removed. There is a known pre-existing limitation in
`daft remove`'s post-remove path where the now-deleted worktree is used as the
cwd for the hook script invocation, causing the spawn to fail silently. This
limitation is shared between `daft remove` and `daft merge -r` / `-rb` and is
out of scope for this design.

`post-merge` failures continue to log warnings without rolling back the merge or
cancelling cleanup — same as before.

## Configuration interactions

No new config keys. Existing keys still control merge behavior; this design only
changes presentation and routing of cleanup. The `worktree-pre-remove` /
`worktree-post-remove` hooks discovered by the HookExecutor are the same files
used by `daft remove` — there is no merge-specific override mechanism, and none
is needed.

## Edge cases

| Case                                                      | Behavior                                                                                       |
| --------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `-r` / `-rb` with multi-source                            | One `branch_delete::execute` call per source, in order; each gets its own hook box and spinner |
| `worktree-pre-remove` hook fails on source #1 of N        | Source #1 cleanup skipped, errors surfaced; sources #2..N still attempted                      |
| `worktree-pre-remove` hook fails on the only source       | Cleanup error surfaced; merge commit remains; non-zero exit                                    |
| User runs merge without `-r` / `-rb`                      | No cleanup phase; no `worktree-*-remove` hooks; same merge-phase rendering as before           |
| `--verbose`                                               | Captured git buffer dumped to stderr after spinner; styled lines still render                  |
| `--quiet`                                                 | Step messages suppressed; final success line still prints                                      |
| Non-TTY (`!stdout.is_terminal()`)                         | Spinner degrades to plain step messages (existing presenter behavior)                          |
| Editor opens during squash commit                         | Spinner pauses, editor takes terminal, spinner resumes after editor exits                      |
| Editor aborted (empty message)                            | Buffer dumped (so user sees git's "Aborting commit" message); cleanup skipped per squash spec  |
| Source has worktree but no local branch (commit-or-other) | `branch_delete::execute` removes worktree only; no branch deletion attempt                     |
| `branch_delete::execute` Phase 2 fails (race condition)   | Error surfaced with same context-wrapping today's `execute_cleanup` produces                   |

## Non-goals

- **Reformatting `daft remove`'s own output.** This design brings `daft merge`
  up to `daft remove`'s level; `daft remove` is the reference, not a target for
  changes.
- **A new `--no-hooks` flag for merge cleanup.** If users want to suppress
  `worktree-*-remove` hooks specifically when invoked via merge, the existing
  hook trust / disable mechanisms apply uniformly. Adding a merge-specific
  bypass would split the hook-contract surface for no clear gain.
- **Reordering cleanup to be branch-first / worktree-second.** The
  worktree-first / branch-second order is preserved (matches `daft remove`).
- **Streaming git stdout in real time on the happy path.** Buffer-and-suppress
  trades real-time visibility for consistent presentation; `--verbose` is the
  escape hatch for the rare case where real-time is wanted.
- **Reformatting `git merge`'s conflict-resolution output.** Conflict output
  paths still surface git's diagnostics directly — they are diagnostic by
  nature.

## Test changes (high-level — detailed in the plan)

Existing scenarios that pass through this code path were inventoried; the churn
is small.

**Scenarios currently asserting on changing surfaces:**

- `tests/manual/scenarios/merge/squash-rb.yml` (line 32, asserts on
  `Squash merged and cleaned up feature/test-feature`) — keeps passing; the
  final success line is unchanged.
- `tests/manual/scenarios/merge/continue-squash-staged.yml` (line 52, asserts on
  `Squash merged and cleaned up`) — keeps passing for the same reason.
- `tests/manual/scenarios/merge/no-target-worktree-ff.yml` (line 41, asserts on
  `Fast-forwarded feat-no-wt`) — already daft-emitted; keeps passing.

No scenarios currently assert on git's raw stdout (`Updating ...`,
`Fast-forward`, `Squash commit -- not updating HEAD`, `[feature ...]`).

**Add:**

- `merge-pre-post-merge-hooks-render-rich.yml` — regression scenario asserting
  on the hook box rendering for `pre-merge` / `post-merge` (this surface is
  already correct, the test pins it down).
- `merge-fires-worktree-remove-hooks.yml` — `daft merge X -r` with a
  `worktree-pre-remove` hook configured: hook fires, hook output is captured,
  source worktree is gone after.
- `merge-pre-remove-hook-warns.yml` — `worktree-pre-remove` hook exits non-
  zero: source #1 cleanup is skipped with a clear error; merge commit stays;
  non-zero exit.
- `merge-rich-output-on-success.yml` — `daft merge X` (regular, no cleanup):
  asserts on the styled step message and absence of git's raw stdout. Uses
  `output_not_contains` for `Updating ` and `Fast-forward`.
- `merge-verbose-shows-git-output.yml` — `daft merge X --verbose`: asserts on
  presence of git's raw stdout in the verbose dump.
- `merge-multi-source-rb-hooks-each.yml` — `-rb` with two sources: each source's
  `worktree-pre-remove` / `worktree-post-remove` fires once.

**Update:**

- Existing scenarios that rely on raw `Removing worktree at <path>...` /
  `Deleting branch <name>...` lines (none today, but verify during plan
  authoring) get migrated to assert on the sink-routed step messages emitted by
  `branch_delete::execute`.

**Add unit tests:**

- `commands::merge::run` delegates each `CleanupPlan` item to
  `branch_delete::execute` with the right `force_delete` flag.
- `core::worktree::merge::execute` returns a `CleanupPlan` (not a side effect)
  with correctly-populated items for `-r`, `-rb`, and squash + `-rb` paths.
- Pre-validation failure short-circuits before any plan is returned.
- Spinner pause/resume bracketing is invoked around the squash-commit editor
  open (regression test, mirroring the existing
  `run_hook_brackets_executor_with_spinner_pause_resume` test in `progress.rs`).

## Risks

- **Cross-platform editor pause/resume edge cases.** The existing pause/resume
  works for hook execution; editor invocation is similar but not identical
  (long-lived foreground process, terminal mode changes). The plan must manually
  verify on macOS and Linux with both `vi` and `code --wait`.
- **`branch_delete::execute` validation overlap.** It runs its own validation
  (unmerged check, dirty worktree check). For the squash + stability path we
  pass `force = true`, which skips the unmerged check; the dirty-worktree check
  still runs. Since merge already pre-validates the same conditions, this is
  redundant but not incorrect — both checks pass or both fail. Worth documenting
  in the plan; not worth restructuring `branch_delete::execute` to skip them.
- **Hook ordering perception.** Users may expect `worktree-pre-remove` to fire
  before `pre-merge` (they remove the worktree first conceptually). The actual
  order — `pre-merge` → merge → `post-merge` → `worktree-pre-remove` (per
  source) → remove → `worktree-post-remove` — is what the underlying operations
  require, and is consistent with how `daft remove` would order them if invoked
  separately. The plan should document this clearly in `docs/guide/hooks.md`.
