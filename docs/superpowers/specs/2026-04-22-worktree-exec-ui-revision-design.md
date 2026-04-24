# Worktree Exec UI Revision — Design

## Goal

Fix the multi-worktree UX of `daft worktree-exec` so the user sees live
per-worktree feedback during a run. Reuse the existing running-windows visual
treatment already used for hooks, minus the hook branding. Preserve today's
compact finalization shape (`✓ branch (1.8s)`). Drop the separate `-v` mode —
there is one UI.

## Motivation

The shipped feature had two modes:

- **Default (list mode):** run silently, print a static table at the end. In
  practice this "hangs" from the user's point of view — no feedback while
  commands run.
- **`-v` mode (windows):** reused the hook renderer. Live per-worktree panels,
  good look — but carries the hook-branded header
  (`┌── daft hooks v1.7.2  hook: exec ──┐`), prints a full output dump inline
  per worktree, and ends with a `summary: (done in…)` block. None of that is
  right for exec.

The user likes the windowed running output but wants:

1. No hooks branding at the top.
2. Compact final row per worktree (same as the current end-of-run list-mode
   table).
3. No redundant summary block.
4. One UI — no `-v` split.

## Non-Goals

- Operation-table ratatui TUI (sync/prune's full-screen table). Deferred.
- Merging exec into the sync/prune DAG event bus. Deferred.
- Renaming `HookOutputConfig` / `HookProgressRenderer` to something neutral. Out
  of scope; internal names don't affect user-visible behavior.
- Single-target pass-through mode (one positional resolving to one worktree).
  Unchanged — stays fully stdio-inherited.

## User-Visible Behavior (After)

```
$ daft exec --all -- mise dev
────────────────────────────────────────────────────────────
Commands
  1. mise dev
────────────────────────────────────────────────────────────
⠋  master
│  Building Rust binaries...
│  …rolling tail…
⠋  feat/background-hook-jobs
│  …rolling tail…
…
```

During a run: one spinner row per worktree, with a rolling tail of output lines
beneath each (today's live-windows shape).

On per-worktree finish: the spinner and its tail are cleared in place, and a
permanent one-line row appears — exactly the row `list_renderer::render_outcome`
produces today:

```
✓  master                    (1.8s)
✓  feat/background-hook-jobs (2.3s)
✗  feat/dirty                (1.2s)  exit 101
```

After all worktrees finish, only if any failed: the failed-output dump appears
(today's `render_failed_output_dump`, unchanged).

No hooks header. No `summary:` block. No `-v` flag.

## Architecture

### What moves, what stays

| Component                                    | Change                                                                                                                                                                                                                                                                            |
| -------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/commands/exec.rs`                       | Drop `-v / --verbose` flag; drop the two-path branch; always call the progress renderer; drop the post-run static `render_header` + `render_outcome` loop.                                                                                                                        |
| `src/core/worktree/exec/windows_renderer.rs` | Rename → `progress_renderer.rs`. Function `run_with_live_windows` → `run_with_progress`. Stop calling `on_phase_start` (which prints the hook header); print `list_renderer::render_header(pipeline)` directly. Stop calling `on_phase_complete` (which prints the hook summary). |
| `src/core/worktree/exec/list_renderer.rs`    | Unchanged. `render_header`, `render_outcome`, `render_failed_output_dump` all remain — `render_outcome` becomes the finalization format for compact-mode rows.                                                                                                                    |
| `src/core/worktree/exec/mod.rs`              | Unchanged. `run_pipeline_streaming` still captures output; `run_scheduler` still exists (used by tests and the compact path).                                                                                                                                                     |
| `src/core/settings.rs`                       | Add `compact_finalization: bool` (default `false`) to `HookOutputConfig`.                                                                                                                                                                                                         |
| `src/output/hook_progress/interactive.rs`    | `HookProgressRenderer::finish_job` branches on `config.compact_finalization`: when true, print the `render_outcome`-equivalent line and skip the output dump and the trailing empty line.                                                                                         |
| `src/output/hook_progress/plain.rs`          | `PlainHookRenderer::finish_job_*` gains the same branch.                                                                                                                                                                                                                          |
| Hook command sites                           | Continue to use `HookOutputConfig::default()` — behavior unchanged (`compact_finalization: false`).                                                                                                                                                                               |

### Config shape

```rust
// src/core/settings.rs
pub struct HookOutputConfig {
    pub quiet: bool,
    pub timer_delay_secs: u32,
    pub tail_lines: u32,
    pub verbose: bool,
    /// When true, on job finish print a single compact row
    /// (`✓ name (dur)` or `✗ name (dur)  exit N`) and drop the inline
    /// output dump. Hooks leave this false. `daft exec` sets it true.
    pub compact_finalization: bool,
}
```

Default stays `false` — hooks' behavior is preserved exactly.

### Exec progress renderer shape

```rust
// src/core/worktree/exec/progress_renderer.rs

pub fn run_with_progress(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
    cancel: &CancelFlag,
) -> anyhow::Result<ExecReport> {
    // Print the Commands header directly — neutral, no hook branding.
    let mut stderr = std::io::stderr().lock();
    crate::core::worktree::exec::list_renderer::render_header(&mut stderr, pipeline)?;
    drop(stderr);

    let cfg = HookOutputConfig {
        compact_finalization: true,
        ..HookOutputConfig::default()
    };
    let presenter: Arc<dyn JobPresenter> = CliPresenter::auto(&cfg);

    // Do NOT call presenter.on_phase_start — that would print the hook header.
    // Do NOT call presenter.on_phase_complete — that would print the hook summary.

    let outcomes = match mode {
        ExecMode::Parallel    => run_parallel(targets, pipeline, &presenter, cancel)?,
        ExecMode::Sequential  => run_sequential(targets, pipeline, false, &presenter, cancel)?,
        ExecMode::KeepGoing   => run_sequential(targets, pipeline, true, &presenter, cancel)?,
    };

    Ok(ExecReport { outcomes, orphan_branches_skipped: Vec::new() })
}
```

The existing `run_parallel` / `run_sequential` worker helpers inside this file
stay unchanged — they already drive `run_pipeline_streaming` with the presenter.

### Compact-finalization branch in `HookProgressRenderer::finish_job`

```rust
fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
    let Some(state) = self.jobs.remove(name) else { return; };

    // Clear all bars (unchanged).
    if let Some(ref sep) = state.separator { sep.finish_and_clear(); }
    for pb in &state.tail_lines { pb.finish_and_clear(); }
    state.spinner.finish_and_clear();

    if self.config.compact_finalization {
        // One compact row; no output dump; no trailing blank line.
        self.mp.println(format_compact_row(name, success, duration, self.use_color)).ok();
    } else {
        // Existing hook-style: "│ name →" heading + output dump + blank line.
        // (current code, unchanged)
    }

    // Record for summary (unchanged — callers that want the summary keep calling print_summary).
    self.finished_jobs.push(JobResultEntry { name: name.to_string(), outcome: …, duration });
}
```

`format_compact_row` mirrors `list_renderer::render_outcome`'s visible string
exactly: `  ✓  name  (1.8s)` on success, `  ✗  name  (1.2s)  exit 101` on
failure. (Failure exit code is not in `finish_job`'s signature today; see "Exit
code plumbing" below.)

### Exit code plumbing

`render_outcome` includes the `exit N` suffix on failure.
`HookProgressRenderer::finish_job` has no exit code in its signature.

Options:

- **(a)** Keep the compact row's failure case as `✗ name (dur)` without the exit
  code. Users get the exit code from the failed-output dump at the end. Smallest
  change.
- **(b)** Add an optional exit code to `JobPresenter::on_job_failure` /
  `finish_job_failure`. Bigger API change.

Pick **(a)** for v1. The exit code is still visible in the failed-output dump
(which follows the failure header line `FAILED: branch (exit 101)`). Revisit if
users complain.

### PlainHookRenderer (non-TTY / pipe)

`PlainHookRenderer::finish_job_success/failure` already prints name-based lines.
Extend with the same `compact_finalization` branch: output the plain-text
equivalent of `render_outcome` (`✓ name (1.8s)`, `✗ name (1.2s)`) instead of the
multi-line form.

### Exec command layer

```rust
// src/commands/exec.rs

#[derive(Debug, Parser)]
pub struct Args {
    // … (unchanged options)

    // DROPPED: pub verbose: bool
}
```

In `run()`:

- Remove `init_logging(args.verbose)` (no `args.verbose` any more). Inline
  `init_logging(false)` if needed, or use whatever default daft uses. (Check
  existing convention — `daft list` / `daft sync` patterns.)
- Remove the `if args.verbose { … } else { run_scheduler(…) }` branch.
- Always call `progress_renderer::run_with_progress(…)`.
- Keep `render_failed_output_dump` after the run.
- Remove the post-run `list_renderer::render_header` + `render_outcome` loop —
  compact-row finalization already left those lines in place.

### Single-target pass-through

Unchanged. The `if targets.len() == 1` branch inheriting stdio stays intact.

## Data Flow

```
parse Args
  ↓
resolve_targets_with_orphans → Vec<ResolvedTarget>
  ↓
if targets.len() == 1 → pass-through with inherited stdio (unchanged)
  ↓
run_with_progress:
  1. render_header(pipeline)  → stderr, "Commands" block
  2. CliPresenter::auto(cfg { compact_finalization: true })
  3. run_parallel / run_sequential drive run_pipeline_streaming per target
     → per job: on_job_start, on_job_output (rolling tail), on_job_success/failure
     → presenter's HookProgressRenderer prints compact row on finish
  4. return ExecReport
  ↓
render_failed_output_dump(report) → stdout, only if failures exist
  ↓
exit aggregate_exit_code
```

## Testing

### New / changed tests

- **`src/core/settings.rs`** — no new test; the field default is covered by
  `HookOutputConfig::default()` usage in existing hook tests.
- **`src/output/hook_progress/interactive.rs`** — add a unit test that, with
  `compact_finalization: true`, `finish_job_success` does not record the "No
  output" / heading lines; the `take_finished_jobs` result still reflects the
  outcome. (Can't assert on stdout in-process easily; assert on the
  lines-written collector if the renderer has one, or use the hidden renderer
  path.)
- **`src/output/hook_progress/plain.rs`** — add a test mirroring the above for
  plain mode.
- **`src/core/worktree/exec/progress_renderer.rs`** — existing
  `run_with_live_windows_single_target_success` test migrates to
  `run_with_progress` with updated assertions.
- **`src/commands/exec.rs`** — drop tests referencing `--verbose`. Add a parse
  test asserting `--verbose` is rejected (regression guard).
- **`tests/integration/test_worktree_exec.sh`** — existing substring assertions
  on branch names + "✓"/"✗" glyphs continue to hold. Any assertion on the
  hook-style `│ name →` line is removed.
- **`tests/manual/scenarios/worktree-exec/*.yml`** — drop scenarios that
  exercise `-v`. Other scenarios pass as-is (they don't depend on the
  hook-branded header).

### Regression guards

- Hook tests in `src/output/hook_progress/*` pass unchanged (default config
  leaves behavior intact).
- `daft hooks run` visual output unchanged.
- CI:
  `mise run fmt:check && mise run clippy && mise run test:unit && mise run test:integration && mise run man:verify`
  all green.

## Documentation

- `man/git-worktree-exec.1`, `man/daft-exec.1` — regenerated (no more `-v`
  flag).
- `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md` — drop `-v` section;
  replace "list mode vs windows mode" copy with a single "live progress"
  paragraph.
- `docs/guide/running-commands-across-worktrees.md` — same copy changes.
- `CHANGELOG.md` — entry under `[Unreleased]` → `### Changed`:
  - "`daft exec` now shows live per-worktree progress during multi-worktree runs
    (removed the separate `-v` mode)."
- `SKILL.md` — scan for `-v` references under exec; remove.

## Risk / Rollback

- **Risk:** `compact_finalization` bug in `HookProgressRenderer::finish_job`
  could leak to hook output. Mitigated by default `false` and explicit unit
  tests on both branches.
- **Risk:** `mp.println` ordering with removed bars. Existing hook finalization
  already uses `mp.println` after `finish_and_clear` — the compact path follows
  the same pattern.
- **Rollback:** single-commit revert restores the `-v` flag and the two-path
  branch. No schema or config-file changes; compatible with older daft
  invocations that never used `-v` (positional/`--all` calls are still valid).

## Out-of-Scope Follow-Ups

- Operation-table ratatui TUI for exec (explored later, bigger scope).
- Rename `HookOutputConfig` / `HookProgressRenderer` → neutral names (pure
  refactor; can happen any time).
- Exit-code inclusion in the compact failure row.
