# Exec Row Labels and Skip States — Design

## Goal

Revise the finalization rows emitted by `daft exec` so each row names the
command it ran (dropping the opaque `[N/M]` notation), and so the output
distinguishes between commands that were cancelled mid-flight and commands that
were never started. Layers on top of the prior "Worktree Exec UI Revision"
design (2026-04-22); reuses its live-progress panels and its compact-row
plumbing.

## Motivation

After the prior revision shipped, two usability gaps remain:

1. **Opaque `[N/M]` labels.** With a multi-command pipeline
   (`-x 'mise dev' -x 'mise fmt'`), each finalization row shows
   `✓  branch  [1/2]  (1.5s)`. To know what step that was the user has to scroll
   back to the `Commands` header and count. This is the only place in the output
   where the command identity is indirected.
2. **Invisible skipped work.** If a command fails (fail-fast at the worktree
   level) or the user hits Ctrl-C mid-run, commands that never started emit no
   row at all. The user can't distinguish "ran and failed" from "ran and was
   cancelled" from "never got its turn".

## Non-Goals

- Restructuring the live UI or the per-worktree panel layout. The panel stays
  one-spinner-per-worktree with a rolling tail; the only live change is that the
  spinner's displayed message surfaces the currently-running command.
- Grouping finalization rows by worktree (explored and rejected in brainstorming
  — the flat one-row-per-step layout is what we want).
- Adding a per-step `exit N` suffix to failure rows. Left as-is (see the prior
  spec's "Exit code plumbing" section).
- Changing the failed-output dump format (`render_failed_output_dump`).

## User-Visible Behavior (After)

All-success, multi-command pipeline:

```
────────────────────────────────────────────────────────────
Worktrees
  ✓  feat/execute-on-worktree  ❯ mise dev   (1.5s)
  ✓  feat/execute-on-worktree  ❯ mise fmt   (71ms)
  ✓  master                    ❯ mise dev   (1.9s)
  ✓  master                    ❯ mise fmt   (71ms)
```

Per-worktree fail-fast (default: step 1 fails, step 2 is skipped for that
worktree only; other worktrees continue normally):

```
  ✓  feat/execute-on-worktree  ❯ mise dev   (1.5s)
  ✓  feat/execute-on-worktree  ❯ mise fmt   (71ms)
  ✗  daft-330/feat/merge       ❯ mise dev   (3.0s)
  ○  daft-330/feat/merge       ❯ mise fmt   skipped
```

Cancellation (Ctrl-C while `master`'s `mise dev` is running;
`feat/background- hook-jobs` hadn't launched yet):

```
  ✓  feat/execute-on-worktree  ❯ mise dev   (1.5s)
  ⊘  feat/execute-on-worktree  ❯ mise fmt   cancelled after 0.4s
  ⊘  master                    ❯ mise dev   cancelled after 1.2s
  ○  master                    ❯ mise fmt   skipped
  ○  feat/background-hook-jobs ❯ mise dev   skipped
  ○  feat/background-hook-jobs ❯ mise fmt   skipped
```

Single-command pipeline (unchanged — every row already names its command
implicitly via the sole pipeline entry, and the row is identical in shape
because `mise dev` just appears after `❯` like any other label):

```
  ✓  feat/execute-on-worktree  ❯ mise dev   (1.5s)
  ✓  master                    ❯ mise dev   (1.9s)
```

Live phase: the per-worktree spinner's message now shows the current command
after the branch name, so the user sees the same label live that they'll see in
finalization.

```
┃  ⠋  feat/execute-on-worktree  ❯ mise fmt
┃
┃  [build] fmt — nothing to format
┃
```

## Header Block

The prior spec keeps a `Commands` block at the top:

```
Commands
  1. mise dev
  2. mise fmt
────────────────────────────────────────
Worktrees
```

Under this revision, every row names its command inline, so the `Commands`
listing is redundant. **Drop the `Commands` block and the numbered list.** Keep
the divider + `Worktrees` label as a lightweight section header:

```
────────────────────────────────────────
Worktrees
  ✓  branch  ❯ cmd  (dur)
```

This also sidesteps the question of whether to renumber / relabel items in the
header for single- vs multi-command pipelines.

## State Vocabulary

Four finalization states (shown in column 1):

| Glyph | State     | Meaning                                                               |
| ----- | --------- | --------------------------------------------------------------------- |
| `✓`   | success   | Step ran and exited 0.                                                |
| `✗`   | failed    | Step ran and exited non-zero, no cancellation in flight.              |
| `⊘`   | cancelled | Step was running when the cancellation flag escalated (SIGTERM/KILL). |
| `○`   | skipped   | Step was never started — fail-fast upstream, or cancel before launch. |

Right-column text, after the command label:

- Success/failure: `(1.5s)` — elapsed wall-clock for that step.
- Cancelled: `cancelled after 1.2s`.
- Skipped: `skipped`.

## Architecture

### Presenter / renderer surface changes

`JobPresenter` (`src/executor/presenter.rs`) gains two new event methods:

```rust
pub trait JobPresenter: Send + Sync {
    // existing:
    fn on_job_start(&self, name: &str, total: Option<usize>, preview: Option<&str>);
    fn on_job_output(&self, name: &str, line: &str);
    fn on_job_success(&self, name: &str, duration: Duration);
    fn on_job_failure(&self, name: &str, duration: Duration);

    // new:
    fn on_job_cancelled(&self, name: &str, duration: Duration);
    fn on_job_skipped(&self, name: &str, preview: Option<&str>);
}
```

`CliPresenter` and the in-process test presenter implement both. The two
`HookProgressRenderer` flavors (`interactive.rs`, `plain.rs`) grow matching
`finish_job_cancelled` / `finish_job_skipped_with_preview` entry points. The
existing `finish_job_skipped` used for hook-skip scenarios stays — it's called
from `src/output/hook_progress/mod.rs` with its own semantics; we're adding a
sibling that carries the command preview so it composes with the compact row.

### Row format

`format_compact_row` (`src/output/hook_progress/formatting.rs`) gains a
`command_preview: Option<&str>` and a `state: RowState` where `RowState` is:

```rust
pub enum RowState {
    Success { duration: Duration },
    Failure { duration: Duration },
    Cancelled { duration: Duration },
    Skipped,
}
```

Layout (monospace, single space between columns):

```
  <glyph>  <branch>  ❯ <command>  <right>
```

- `<branch>` is padded to the longest branch name in the current exec run. Width
  is known to the renderer (it owns the target list at
  `HookProgressRenderer::new`; add a `set_name_column_width(usize)` setter
  called once from `run_with_progress` after target resolution).
- `❯ <command>` uses the same preview string the spinner uses
  (`CommandSpec::display()`). Left unpadded — width varies with the command,
  which is fine for a flat list.
- `<right>` is the state-specific suffix from the table above.

Color (when stderr is a TTY): `✓` green, `✗` red, `⊘` yellow, `○` dim/gray. No
color in plain mode.

### Step name / preview plumbing

The current scheme of encoding the step into `job_name`
(`"{branch} [{i+1}/{n}]"`) no longer needs to be the primary identity. The
renderer already receives `preview` in `on_job_start`; switch the job name back
to being per-worktree-per-step keyed with a stable synthetic id, and have the
presenter store `preview` on `JobState` so finalization can format the row with
the real command text.

Concretely in `run_pipeline_streaming`:

```rust
let job_name = if pipeline.len() > 1 {
    format!("{}#{}", base_name, idx)   // stable unique key; NOT user-visible
} else {
    base_name.to_string()
};
let preview = spec.display();
presenter.on_job_start(&job_name, None, Some(&preview));
```

`HookProgressRenderer::start_job` already accepts `preview: Option<&str>`. It
currently uses it in the hook header; under `compact_finalization`, store it on
`JobState` instead and render it at finalization time via `format_compact_row`.
The live spinner message becomes `format!("{branch}  ❯ {preview}")` (the branch
name is recoverable from the stored display name on `JobState` — we add a field
`display_name` alongside the existing `name` so the synthetic `#idx` suffix is
never shown).

### Skipped-row emission

Two sources of skips, both handled inline (not post-run):

1. **Fail-fast within a pipeline.** After the `break` following a
   `presenter.on_job_failure(...)` inside `run_pipeline_streaming`, iterate the
   unrun steps and call `presenter.on_job_skipped(&job_name, Some(&preview))`
   for each. This emits one row per unrun step for the just-aborted worktree, in
   pipeline order, immediately after the failure row.
2. **Cancellation between commands.** When the top-of-loop
   `cancel .is_cancelled()` check fires, the in-flight step is already done (or
   there isn't one yet). Iterate the unrun steps (including the current `idx`)
   and emit `on_job_skipped` for each.

For entire worktrees that never launched because parallel workers saw cancel
before they grabbed a slot, the outer `run_parallel` / `run_sequential` helpers
build synthetic skipped outcomes. Today they simply don't include the worktree
in `outcomes`. Fix: after the scheduler exits, walk `targets` and for every
target that doesn't have an outcome (and we observed cancel), call
`presenter.on_job_skipped` once per pipeline step for that worktree. Do this
from `run_with_progress` so both scheduler paths share the logic.

### `WorktreeOutcome` shape

`WorktreeOutcome` (`src/core/worktree/exec/mod.rs:115`) currently stores
`last_command_index`, `exit_code`, `cancelled`. Keep these — they're enough for
`aggregate_exit_code` and for the failed-output dump. The new per-step row
emission is driven by live events, not by post-run outcome inspection; no struct
changes are needed. The never-launched-worktrees case is handled inside
`run_with_progress` by diffing `targets` against the outcomes it collected; skip
rows are emitted before returning.

### Cancellation observability

`CancelFlag::level()` distinguishes soft (1, SIGTERM sent) from hard (2, SIGKILL
sent). The `⊘` state and `cancelled after X` suffix apply to both — we don't
surface the distinction in the row. `cancel.is_cancelled()` already returns true
for level ≥ 1, which is what the fail-fast loop checks.

### Plain (non-TTY / piped) renderer

`PlainHookRenderer` gets the same `on_job_cancelled` / `on_job_skipped`
branches, emitting the plain-text form of the row (no color, same glyphs and
layout). Pipe-consumer tools (e.g., capturing exec output in a script) get one
row per step in deterministic order.

## Data Flow

```
parse Args
  ↓
resolve_targets (unchanged)
  ↓
single-target passthrough?  →  inherit stdio (unchanged)
  ↓
run_with_progress:
  1. set_name_column_width from max target branch length
  2. drop Commands header; print divider + "Worktrees" label
  3. CliPresenter with compact_finalization = true
  4. run_parallel / run_sequential drive run_pipeline_streaming
     → per step: on_job_start(name, preview) → spinner shows "branch ❯ cmd"
     → on step completion: on_job_success | on_job_failure | on_job_cancelled
     → on fail-fast / cancel: on_job_skipped for every unrun step in pipeline
  5. for each target NOT in outcomes (cancelled before dispatch):
       emit on_job_skipped rows for every pipeline step
  6. return ExecReport
  ↓
render_failed_output_dump (unchanged)
  ↓
exit aggregate_exit_code
```

## Testing

- **`src/output/hook_progress/formatting.rs`** — new unit tests for
  `format_compact_row` covering all four states, with and without a
  `command_preview`.
- **`src/output/hook_progress/interactive.rs`** — tests using the hidden
  renderer to drive a sequence `start → output → cancelled/skipped` and assert
  the recorded compact row shape. The existing trailer test stays.
- **`src/output/hook_progress/plain.rs`** — mirror tests for plain mode.
- **`src/core/worktree/exec/mod.rs`** — unit test that `run_pipeline_streaming`
  emits `on_job_skipped` for unrun steps after a fail-fast, and after a mid-loop
  cancel. Use a fake presenter that records event order.
- **`src/core/worktree/exec/progress_renderer.rs`** — integration test covering
  the worktree-never-launched path: build two targets, cancel before dispatch,
  assert each unstarted target yields one skip row per pipeline step.
- **`tests/integration/test_worktree_exec.sh`** — add substring assertions for
  `skipped` and (if feasible to exercise) `⊘`.
- **`tests/manual/scenarios/worktree-exec/*.yml`** — add a scenario with a
  two-command pipeline where the first command fails, asserting the second row's
  `skipped` suffix.

## Documentation

- `CHANGELOG.md`: under `[Unreleased]` → `### Changed`:
  - "`daft exec` finalization rows now name the command inline
    (`✓ branch ❯ cmd (1.5s)`) instead of `[N/M]`, and surface cancelled vs
    skipped steps distinctly."
- `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md`, and
  `docs/guide/running-commands-across-worktrees.md`: update the sample output
  blocks to the new row format; add a short note on the state glyphs.
- `man/` regenerated (no CLI-surface changes, but help text in `exec.rs` may
  grow an example showing a cancelled run; regenerate via `mise run man:gen`).
- `SKILL.md`: scan for any sample output using `[1/2]`; update to inline form.

## Risk / Rollback

- **Risk:** the presenter trait change (`on_job_cancelled`, `on_job_skipped`)
  touches every implementer. Mitigated by landing all impls in the same commit
  and by the compiler enforcing exhaustiveness.
- **Risk:** `JobState` growing a `display_name` field (distinct from the
  synthetic `name` key) introduces a rename-trap if future call sites forget
  which to use in user-visible strings. Mitigated by keeping the two presenter
  entry points that accept names as `&str` (so all sites pass the id) and by
  having a single renderer-local helper format the display line, so there is
  exactly one place the display string is composed.
- **Risk:** skipped-row emission at the end of `run_with_progress` for
  never-launched targets races with the teardown of live bars on cancel.
  Mitigated by emitting these rows only after the scheduler has joined all
  workers.
- **Rollback:** single-commit revert restores `[N/M]` labels and the
  no-skipped-rows behavior. No on-disk format, no CLI flags, no config. The
  presenter trait additions are compatible: reverting leaves call sites
  unchanged because the added methods are only called from exec paths.

## Out-of-Scope Follow-Ups

- Smart truncation of very long command strings in the compact row (currently:
  let the terminal wrap).
- A `--summary` flag to also print a post-run count-by-state summary line.
- Per-step exit code surfacing in the failure row
  (`✗ branch ❯ cmd (1.2s) exit 101`).
- Operation-table ratatui TUI for exec (deferred separately).
