# Exec Row Labels and Skip States Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the opaque `[N/M]` notation in `daft exec` finalization rows
with the actual command name, and surface cancelled vs skipped steps as distinct
finalization states.

**Architecture:** Extend the shared compact-row formatter with a command preview
and a four-state enum (success / failure / cancelled / skipped). Plumb the
preview into renderer state at `on_job_start` so `finish_*` calls can compose
the row. Add `on_job_cancelled` to `JobPresenter`, extend the existing
`on_job_skipped` signature with a trailing `command_preview: Option<&str>`
(backward compatible — all existing callers add `None`). Drop the `Commands`
header block now that every row names its command inline. Drive skipped-row
emission inline from `run_pipeline_streaming` (fail-fast and mid-loop cancel)
and from `run_with_progress` (worktrees never launched).

**Tech Stack:** Rust, indicatif, existing `HookProgressRenderer` /
`PlainHookRenderer` / `CliPresenter` plumbing.

**Spec:**
`docs/superpowers/specs/2026-04-24-exec-row-labels-and-skip-states-design.md`

---

## File Structure

**Modified files:**

- `src/output/hook_progress/formatting.rs` — `RowState` enum, extended
  `format_compact_row(name, preview, state, name_width, use_color)` with
  per-state suffix.
- `src/output/hook_progress/interactive.rs` — `JobState.command_preview`,
  `name_column_width` field, `set_name_column_width()`,
  `finish_job_cancelled()`, `finish_job_skipped()` signature extended, spinner
  message composes `name  ❯  preview`.
- `src/output/hook_progress/plain.rs` — matching changes (no spinner, so message
  format change N/A; just the finalization methods and state storage).
- `src/output/hook_progress/mod.rs` — `HookRenderer` enum delegates: new
  `finish_job_cancelled`, extended `finish_job_skipped` signature, new
  `set_name_column_width` delegate.
- `src/executor/presenter.rs` — `JobPresenter::on_job_skipped` signature
  extended with trailing `command_preview: Option<&str>`; new
  `fn on_job_cancelled(&self, name: &str, duration: Duration)`; `NullPresenter`
  updated; existing tests updated.
- `src/executor/cli_presenter.rs` — `CliPresenter` impl wires new/changed
  methods; existing tests updated.
- `src/output/tui/presenter.rs` — `TuiPresenter` impl (also implements
  `JobPresenter`) wires new/changed methods; existing tests updated.
- `src/core/worktree/exec/mod.rs` — `run_pipeline_streaming` drops `[i+1/n]`
  from job name, emits `on_job_cancelled` (instead of `on_job_failure`) when
  cancel observed, emits `on_job_skipped` for unrun steps after fail-fast and
  after cancel.
- `src/core/worktree/exec/progress_renderer.rs` — `run_with_progress` sets
  name-column width on the presenter's renderer, drops the `Commands` block from
  the header, and after the scheduler joins, emits skipped rows for any targets
  that never launched.
- `src/core/worktree/exec/list_renderer.rs` — `render_header` trimmed: keep
  divider + `Worktrees` label, drop the `Commands` block and the numbered list.
- `tests/integration/test_worktree_exec.sh` — add assertions for inline command
  names and skipped rows.
- `tests/manual/scenarios/worktree-exec/*.yml` — new scenario for multi-command
  pipeline with a failing first step.
- `CHANGELOG.md`, `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md`,
  `docs/guide/running-commands-across-worktrees.md` — wording + sample output
  updates; `man/` regen.

---

## Task 1: `RowState` enum + extended `format_compact_row`

**Files:**

- Modify: `src/output/hook_progress/formatting.rs`

Introduces a four-state row format that every finalization path funnels through,
with the command preview as first-class input. This is the foundation everything
else depends on.

- [ ] **Step 1: Replace `compact_row_tests` with new-shape tests (failing)**

Replace the existing `compact_row_tests` module at
`src/output/hook_progress/formatting.rs:185-245` with:

```rust
#[cfg(test)]
mod compact_row_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn success_row_with_preview_plain() {
        let row = format_compact_row(
            "master",
            Some("mise dev"),
            RowState::Success {
                duration: Duration::from_millis(1900),
            },
            12,
            false,
        );
        // "  ✓  master        ❯ mise dev  (1.9s)"
        assert!(row.contains("\u{2713}"), "expected ✓, got: {row:?}");
        assert!(row.contains("master"), "missing branch: {row:?}");
        assert!(row.contains("\u{276f} mise dev"), "missing preview: {row:?}");
        assert!(row.contains("(1.9s)"), "missing elapsed: {row:?}");
    }

    #[test]
    fn failure_row_with_preview_plain() {
        let row = format_compact_row(
            "feat/dirty",
            Some("cargo build"),
            RowState::Failure {
                duration: Duration::from_millis(1200),
            },
            12,
            false,
        );
        assert!(row.contains("\u{2717}"), "expected ✗, got: {row:?}");
        assert!(row.contains("feat/dirty"));
        assert!(row.contains("\u{276f} cargo build"));
        assert!(row.contains("(1.2s)"));
    }

    #[test]
    fn cancelled_row_with_preview_plain() {
        let row = format_compact_row(
            "master",
            Some("mise dev"),
            RowState::Cancelled {
                duration: Duration::from_millis(1200),
            },
            12,
            false,
        );
        assert!(row.contains("\u{2298}"), "expected ⊘, got: {row:?}");
        assert!(row.contains("master"));
        assert!(row.contains("\u{276f} mise dev"));
        assert!(
            row.contains("cancelled after 1.2s"),
            "missing cancelled suffix: {row:?}"
        );
    }

    #[test]
    fn skipped_row_with_preview_plain() {
        let row = format_compact_row(
            "daft-330/feat/merge",
            Some("mise fmt"),
            RowState::Skipped,
            20,
            false,
        );
        assert!(row.contains("\u{25cb}"), "expected ○, got: {row:?}");
        assert!(row.contains("daft-330/feat/merge"));
        assert!(row.contains("\u{276f} mise fmt"));
        assert!(row.ends_with("skipped"), "expected 'skipped' suffix: {row:?}");
    }

    #[test]
    fn name_is_padded_to_requested_width() {
        let row = format_compact_row(
            "a",
            Some("cmd"),
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            10,
            false,
        );
        // Two leading spaces + sigil + two spaces + name padded to 10 chars =
        // "  ✓  a         ❯ cmd  (1.0s)"
        assert!(
            row.contains("a         "),
            "branch must be left-padded to 10 chars, got: {row:?}"
        );
    }

    #[test]
    fn preview_none_omits_arrow_segment_plain() {
        let row = format_compact_row(
            "master",
            None,
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            10,
            false,
        );
        assert!(
            !row.contains("\u{276f}"),
            "no preview ⇒ no arrow, got: {row:?}"
        );
        assert!(row.contains("master"));
        assert!(row.contains("(1.0s)"));
    }

    #[test]
    fn row_has_leading_indent() {
        let row = format_compact_row(
            "x",
            None,
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            4,
            false,
        );
        assert!(
            row.starts_with("  "),
            "expected 2-space leading indent, got: {row:?}"
        );
    }

    #[test]
    fn colored_success_uses_green_sigil() {
        let row = format_compact_row(
            "x",
            Some("cmd"),
            RowState::Success {
                duration: Duration::from_secs(1),
            },
            4,
            true,
        );
        assert!(
            row.contains(crate::styles::GREEN),
            "colored success row should include GREEN, got: {row:?}"
        );
    }

    #[test]
    fn colored_cancelled_uses_yellow_sigil() {
        let row = format_compact_row(
            "x",
            Some("cmd"),
            RowState::Cancelled {
                duration: Duration::from_secs(1),
            },
            4,
            true,
        );
        assert!(
            row.contains(YELLOW),
            "colored cancelled row should include YELLOW, got: {row:?}"
        );
    }

    #[test]
    fn colored_skipped_uses_dark_grey() {
        let row = format_compact_row("x", Some("cmd"), RowState::Skipped, 4, true);
        assert!(
            row.contains(DARK_GREY),
            "colored skipped row should include DARK_GREY, got: {row:?}"
        );
    }
}
```

- [ ] **Step 2: Run the tests and verify they fail (compile error)**

Run:
`cargo test -p daft --lib output::hook_progress::formatting::compact_row_tests -- --nocapture 2>&1 | head -30`
Expected: compile error — `RowState` undefined, `format_compact_row` signature
mismatch.

- [ ] **Step 3: Implement `RowState` and the new `format_compact_row` + shim the
      old signature**

Replace the existing `format_compact_row` at
`src/output/hook_progress/formatting.rs:157-183` with:

````rust
/// Lifecycle state of a finalized row (one per pipeline step).
#[derive(Debug, Clone, Copy)]
pub(super) enum RowState {
    Success { duration: Duration },
    Failure { duration: Duration },
    Cancelled { duration: Duration },
    Skipped,
}

/// Render a finalized per-step row for compact-finalization mode.
///
/// Shape (monospace):
/// ```text
///   <glyph>  <name padded to name_width>  ❯ <preview>  <right>
/// ```
/// When `command_preview` is `None`, the `❯ <preview>` segment is omitted.
/// `<right>` is the state-specific suffix: `(1.5s)` for success/failure,
/// `cancelled after 1.2s` for cancelled, `skipped` for skipped.
pub(super) fn format_compact_row(
    name: &str,
    command_preview: Option<&str>,
    state: RowState,
    name_width: usize,
    use_color: bool,
) -> String {
    let (sigil, color_code) = match state {
        RowState::Success { .. } => ("\u{2713}", styles::GREEN),
        RowState::Failure { .. } => ("\u{2717}", styles::RED),
        RowState::Cancelled { .. } => ("\u{2298}", YELLOW),
        RowState::Skipped => ("\u{25cb}", DARK_GREY),
    };
    let right = match state {
        RowState::Success { duration } | RowState::Failure { duration } => {
            format!("({})", format_duration(duration))
        }
        RowState::Cancelled { duration } => {
            format!("cancelled after {}", format_duration(duration))
        }
        RowState::Skipped => "skipped".to_string(),
    };

    let name_part = format!("{:<w$}", name, w = name_width);
    let preview_segment = command_preview
        .map(|p| format!("  \u{276f} {p}"))
        .unwrap_or_default();

    if use_color {
        format!(
            "  {color_code}{sigil}  {name_part}{}{preview_segment}  {GREY}{right}{}",
            styles::RESET,
            styles::RESET,
        )
    } else {
        format!("  {sigil}  {name_part}{preview_segment}  {right}")
    }
}

/// Compatibility shim over the 4-arg signature used by existing callers.
/// Will be removed in Task 3 once renderers migrate to the full API.
pub(super) fn format_compact_row_legacy(
    name: &str,
    success: bool,
    duration: Duration,
    use_color: bool,
) -> String {
    let state = if success {
        RowState::Success { duration }
    } else {
        RowState::Failure { duration }
    };
    format_compact_row(name, None, state, 24, use_color)
}
````

- [ ] **Step 3b: Redirect old callers through the shim**

At `src/output/hook_progress/interactive.rs:363-370`, change
`super::formatting::format_compact_row(name, success, duration, self.use_color)`
to
`super::formatting::format_compact_row_legacy(name, success, duration, self.use_color)`.

At `src/output/hook_progress/plain.rs:85`, the same rename.

This preserves existing behavior exactly — the shim always passes `None` preview
and `24` width, matching today's output. Task 3 deletes the shim once renderers
call the full API directly.

- [ ] **Step 4: Run the tests and verify they pass**

Run:
`cargo test -p daft --lib output::hook_progress::formatting::compact_row_tests`
Expected: 10 tests pass.

- [ ] **Step 5: Run the full formatting test module to ensure nothing else
      broke**

Run: `cargo test -p daft --lib output::hook_progress::formatting` Expected: all
tests in this module pass.

- [ ] **Step 6: Commit**

```bash
git add src/output/hook_progress/formatting.rs \
        src/output/hook_progress/interactive.rs \
        src/output/hook_progress/plain.rs
git commit -m "$(cat <<'EOF'
feat(hook_progress): add RowState and extend format_compact_row

The compact finalization row now accepts an optional command preview and
a four-state enum (Success/Failure/Cancelled/Skipped). Preview renders as
a `❯ cmd` segment between name and right-column; omitted when `None`.
Per-state suffix: `(dur)`, `cancelled after dur`, or `skipped`.

Existing interactive + plain renderers route through a temporary 4-arg
shim so this commit is buildable on its own. Task 3 deletes the shim
when renderers start passing previews through.
EOF
)"
```

---

## Task 2: Extend `JobPresenter` trait

**Files:**

- Modify: `src/executor/presenter.rs`
- Modify: `src/executor/cli_presenter.rs`
- Modify: `src/output/tui/presenter.rs`

Add `on_job_cancelled`; extend `on_job_skipped` with a trailing
`command_preview: Option<&str>`. Update all three impls and their tests.

- [ ] **Step 1: Update the trait definition**

At `src/executor/presenter.rs:35`, change:

```rust
    fn on_job_skipped(&self, name: &str, reason: &str, duration: Duration, show_duration: bool);
```

to:

```rust
    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    );

    /// A job was cancelled by SIGINT while still running.
    fn on_job_cancelled(&self, name: &str, duration: Duration);
```

- [ ] **Step 2: Update `NullPresenter`**

At `src/executor/presenter.rs:75-82`, change the `on_job_skipped` impl:

```rust
    fn on_job_skipped(
        &self,
        _name: &str,
        _reason: &str,
        _duration: Duration,
        _show_duration: bool,
        _command_preview: Option<&str>,
    ) {
    }

    fn on_job_cancelled(&self, _name: &str, _duration: Duration) {}
```

- [ ] **Step 3: Update the `NullPresenter` test at `presenter.rs:108-119`**

Change the call inside `null_presenter_methods_are_no_ops`:

```rust
    p.on_job_skipped("job", "reason", Duration::from_secs(0), false, None);
    p.on_job_cancelled("job", Duration::from_secs(1));
```

- [ ] **Step 4: Update `CliPresenter`**

At `src/executor/cli_presenter.rs:83-86`, change:

```rust
    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
    }

    fn on_job_cancelled(&self, name: &str, duration: Duration) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.finish_job_cancelled(name, duration);
    }
```

(`HookRenderer::finish_job_skipped` gains the trailing arg in Task 5;
`HookRenderer::finish_job_cancelled` is new there too. This file will not
compile until Tasks 3–5 land. Same TDD-vs-always-green tradeoff as Task 1 — ship
the three as a unit.)

- [ ] **Step 5: Update `CliPresenter` test at `cli_presenter.rs:166-177`**

In `skipped_maps_to_skipped_status`, change the call:

```rust
    presenter.on_job_skipped("lint", "no files changed", Duration::from_millis(10), false, None);
```

- [ ] **Step 6: Update `TuiPresenter`**

At `src/output/tui/presenter.rs:128`, change the `on_job_skipped` impl signature
to include the trailing `command_preview: Option<&str>` (ignored — the TUI does
not render per-step previews):

```rust
    fn on_job_skipped(
        &self,
        name: &str,
        reason: &str,
        duration: Duration,
        _show_duration: bool,
        _command_preview: Option<&str>,
    ) {
        // existing body unchanged
```

Add a new method after it:

```rust
    fn on_job_cancelled(&self, name: &str, duration: Duration) {
        // Same shape as on_job_failure — emit a JobCompleted event with
        // Failed status. The cancellation distinction is surfaced at the
        // exec renderer layer, not here.
        self.on_job_failure(name, duration);
    }
```

- [ ] **Step 7: Update `TuiPresenter` test at `tui/presenter.rs:393-398`**

```rust
    presenter.on_job_skipped("lint", "no files", Duration::ZERO, false, None);
```

- [ ] **Step 8: Build — must compile with existing call sites temporarily
      broken**

Run: `cargo check -p daft 2>&1 | head -60` Expected: errors only in
`src/output/hook_progress/*` (they still call the old `finish_job_skipped`
signature and don't define `finish_job_cancelled`). Tasks 3–5 fix those.

- [ ] **Step 9: Commit (bundled with Tasks 3–5 below)**

Hold this commit until Tasks 3–5 land together — the tree must build after the
bundle.

---

## Task 3: Interactive renderer — store preview, add cancelled, extend skipped

**Files:**

- Modify: `src/output/hook_progress/interactive.rs`

Store `command_preview` on `JobState` so it's available at finalization. Add
`finish_job_cancelled`. Extend `finish_job_skipped` signature. Add a
`name_column_width` field defaulted to 24 for backward compatibility.

- [ ] **Step 1: Extend `JobState` with a preview field**

At `src/output/hook_progress/interactive.rs:11-18`, add `command_preview`:

```rust
struct JobState {
    spinner: ProgressBar,
    separator: Option<ProgressBar>,
    tail_lines: Vec<ProgressBar>,
    trailer: Option<ProgressBar>,
    output_buffer: Vec<String>,
    start_time: Instant,
    command_preview: Option<String>,
}
```

- [ ] **Step 2: Add `name_column_width` to `HookProgressRenderer`**

At `src/output/hook_progress/interactive.rs:20-32`, add the field and a setter.
Change the struct:

```rust
pub struct HookProgressRenderer {
    mp: MultiProgress,
    jobs: HashMap<String, JobState>,
    config: HookOutputConfig,
    finished_jobs: Vec<JobResultEntry>,
    use_color: bool,
    pipe_str: String,
    arrow_str: String,
    spinner_style: ProgressStyle,
    spinner_style_with_timer: ProgressStyle,
    tail_style: ProgressStyle,
    trailer_style: ProgressStyle,
    name_column_width: usize,
}
```

In `create()` at `interactive.rs:88-101`, initialize `name_column_width: 24`.

Add the setter near the other public methods (after `print_header`, around line
108):

```rust
    /// Override the branch-name column width used in compact finalization
    /// rows. Default is 24 (matches `list_renderer::render_outcome`).
    pub fn set_name_column_width(&mut self, width: usize) {
        self.name_column_width = width;
    }
```

- [ ] **Step 3: Populate `command_preview` in `start_job_with_description`**

At `src/output/hook_progress/interactive.rs:173-183`, change the `jobs.insert`
block to store preview:

```rust
        self.jobs.insert(
            name.to_string(),
            JobState {
                spinner,
                separator: None,
                tail_lines: Vec::new(),
                trailer: Some(trailer),
                output_buffer: Vec::new(),
                start_time: Instant::now(),
                command_preview: command_preview.map(str::to_string),
            },
        );
```

- [ ] **Step 4: Migrate `finish_job` compact branch to new
      `format_compact_row`**

At `src/output/hook_progress/interactive.rs:362-370`, replace the compact branch
body so it reads `command_preview` from state and uses the success/failure
`RowState`:

```rust
        if self.config.compact_finalization {
            let preview = state.command_preview.as_deref();
            let row_state = if success {
                super::formatting::RowState::Success { duration }
            } else {
                super::formatting::RowState::Failure { duration }
            };
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    preview,
                    row_state,
                    self.name_column_width,
                    self.use_color,
                ))
                .ok();
        } else {
```

- [ ] **Step 5: Add `finish_job_cancelled`**

Insert after `finish_job_failure` (around `interactive.rs:286`):

```rust
    pub fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        let Some(state) = self.jobs.remove(name) else {
            return;
        };

        if let Some(ref sep) = state.separator {
            self.mp.remove(sep);
        }
        for pb in &state.tail_lines {
            self.mp.remove(pb);
        }
        if let Some(ref trailer) = state.trailer {
            self.mp.remove(trailer);
        }
        self.mp.remove(&state.spinner);

        if self.config.compact_finalization {
            let preview = state.command_preview.as_deref();
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    preview,
                    super::formatting::RowState::Cancelled { duration },
                    self.name_column_width,
                    self.use_color,
                ))
                .ok();
        }

        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Failed,
            duration,
        });
    }
```

(Uses `JobOutcome::Failed` for `JobResultEntry` — `take_finished_jobs()`
consumers care about success-vs-not, not about the cancel/fail distinction. Add
a new `JobOutcome::Cancelled` variant only if a downstream consumer needs it;
defer.)

- [ ] **Step 6: Extend `finish_job_skipped` signature**

At `src/output/hook_progress/interactive.rs:288-335`, change the signature to
accept `command_preview: Option<&str>` and branch on compact mode:

```rust
    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        use super::formatting::YELLOW;

        // State may not exist: exec emits skip rows for steps that never
        // ran. Hook flow always has state at this point (start_job ran
        // first). Handle both.
        let stored_preview = if let Some(state) = self.jobs.remove(name) {
            if let Some(ref sep) = state.separator {
                self.mp.remove(sep);
            }
            for pb in &state.tail_lines {
                self.mp.remove(pb);
            }
            if let Some(ref trailer) = state.trailer {
                self.mp.remove(trailer);
            }
            self.mp.remove(&state.spinner);
            state.command_preview
        } else {
            None
        };

        if self.config.compact_finalization {
            let preview = command_preview.or(stored_preview.as_deref());
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    preview,
                    super::formatting::RowState::Skipped,
                    self.name_column_width,
                    self.use_color,
                ))
                .ok();
        } else {
            // Hook-style inline skip line — unchanged from today.
            let msg = if self.use_color {
                format!(
                    "{}  {ORANGE}{name}{} {DARK_GREY}(skip){} {YELLOW}{reason}{}",
                    self.pipe_str,
                    styles::RESET,
                    styles::RESET,
                    styles::RESET
                )
            } else {
                format!("{}  {name} (skip) {reason}", self.pipe_str)
            };
            self.mp.println(msg).ok();
        }

        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Skipped {
                reason: reason.to_string(),
                show_duration,
            },
            duration,
        });
    }
```

- [ ] **Step 7: Build**

Run: `cargo check -p daft 2>&1 | head -40` Expected: errors in `plain.rs` and
`mod.rs` only (still using old signature) — interactive compiles.

---

## Task 4: Plain renderer — same method set

**Files:**

- Modify: `src/output/hook_progress/plain.rs`

- [ ] **Step 1: Add `command_preview` tracking**

Plain renderer doesn't have a JobState struct — it keeps flat fields. Add a
per-job preview map. At `src/output/hook_progress/plain.rs:9-16`, extend the
struct:

```rust
#[derive(Default)]
pub struct PlainHookRenderer {
    output_lines: Vec<String>,
    finished_jobs: Vec<JobResultEntry>,
    jobs_with_output: std::collections::HashSet<String>,
    verbose: bool,
    compact_finalization: bool,
    name_column_width: usize,
    previews: std::collections::HashMap<String, String>,
}
```

Initialize `name_column_width: 24` in `new()` / `with_verbose()` —
`..Self::default()` already yields 0, so set explicitly:

```rust
    pub fn new() -> Self {
        Self {
            name_column_width: 24,
            ..Self::default()
        }
    }

    pub fn with_verbose(verbose: bool) -> Self {
        Self {
            verbose,
            name_column_width: 24,
            ..Self::default()
        }
    }
```

Add the setter:

```rust
    pub fn set_name_column_width(&mut self, width: usize) {
        self.name_column_width = width;
    }
```

- [ ] **Step 2: Record preview on start**

At `src/output/hook_progress/plain.rs:50-71`, at the end of
`start_job_with_description`:

```rust
        if let Some(cmd) = command_preview {
            self.previews.insert(name.to_string(), cmd.to_string());
        }
```

- [ ] **Step 3: Migrate `finish_job` compact branch**

At `src/output/hook_progress/plain.rs:79-99`, rewrite:

```rust
    fn finish_job(&mut self, name: &str, success: bool, duration: Duration) {
        if self.compact_finalization {
            let preview = self.previews.remove(name);
            let state = if success {
                super::formatting::RowState::Success { duration }
            } else {
                super::formatting::RowState::Failure { duration }
            };
            eprintln!(
                "{}",
                super::formatting::format_compact_row(
                    name,
                    preview.as_deref(),
                    state,
                    self.name_column_width,
                    false,
                )
            );
        } else if !self.jobs_with_output.contains(name) {
            eprintln!("\u{2503}  No output");
        }
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: if success {
                JobOutcome::Success
            } else {
                JobOutcome::Failed
            },
            duration,
        });
    }
```

- [ ] **Step 4: Add `finish_job_cancelled`**

Add after `finish_job_failure`:

```rust
    pub fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        if self.compact_finalization {
            let preview = self.previews.remove(name);
            eprintln!(
                "{}",
                super::formatting::format_compact_row(
                    name,
                    preview.as_deref(),
                    super::formatting::RowState::Cancelled { duration },
                    self.name_column_width,
                    false,
                )
            );
        }
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Failed,
            duration,
        });
    }
```

- [ ] **Step 5: Extend `finish_job_skipped`**

Rewrite at `src/output/hook_progress/plain.rs:109-125`:

```rust
    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        let stored = self.previews.remove(name);
        let preview = command_preview.or(stored.as_deref());
        if self.compact_finalization {
            eprintln!(
                "{}",
                super::formatting::format_compact_row(
                    name,
                    preview,
                    super::formatting::RowState::Skipped,
                    self.name_column_width,
                    false,
                )
            );
        } else {
            eprintln!("\u{2503}  {name} (skip) {reason}");
        }
        self.finished_jobs.push(JobResultEntry {
            name: name.to_string(),
            outcome: JobOutcome::Skipped {
                reason: reason.to_string(),
                show_duration,
            },
            duration,
        });
    }
```

- [ ] **Step 6: Build**

Run: `cargo check -p daft 2>&1 | head -30` Expected: errors in `mod.rs` only
(the `HookRenderer` enum still delegates with the old signature).

---

## Task 5: `HookRenderer` enum + wire into `CliPresenter`

**Files:**

- Modify: `src/output/hook_progress/mod.rs`
- Modify: `src/executor/cli_presenter.rs` (no further changes — Task 2 already
  wrote the right calls)

- [ ] **Step 1: Extend `HookRenderer` delegates**

At `src/output/hook_progress/mod.rs:117-130`, change the `finish_job_skipped`
delegate signature:

```rust
    pub fn finish_job_skipped(
        &mut self,
        name: &str,
        reason: &str,
        duration: Duration,
        show_duration: bool,
        command_preview: Option<&str>,
    ) {
        match self {
            HookRenderer::Progress(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
            }
            HookRenderer::Plain(r) => {
                r.finish_job_skipped(name, reason, duration, show_duration, command_preview);
            }
        }
    }
```

Add new `finish_job_cancelled` delegate after `finish_job_failure` (around
`mod.rs:115`):

```rust
    pub fn finish_job_cancelled(&mut self, name: &str, duration: Duration) {
        match self {
            HookRenderer::Progress(r) => r.finish_job_cancelled(name, duration),
            HookRenderer::Plain(r) => r.finish_job_cancelled(name, duration),
        }
    }
```

Add `set_name_column_width` delegate:

```rust
    pub fn set_name_column_width(&mut self, width: usize) {
        match self {
            HookRenderer::Progress(r) => r.set_name_column_width(width),
            HookRenderer::Plain(r) => r.set_name_column_width(width),
        }
    }
```

- [ ] **Step 2: Sanity check — no other `finish_job_skipped` call sites**

Verified at plan-writing time that the only non-renderer call sites for
`finish_job_skipped` are `CliPresenter` (already updated in Task 2) and the
`HookRenderer` enum delegate (updated in Step 1 above). Re-verify before
building:

```bash
rg 'finish_job_skipped' src/ --files-with-matches
```

Expected files: `src/output/hook_progress/{interactive,plain,mod}.rs` and
`src/executor/cli_presenter.rs`. If any other file appears, it's a new call site
that needs the trailing `None` or `Some(&preview)` argument.

- [ ] **Step 3: Full build**

Run: `cargo build -p daft 2>&1 | tail -20` Expected: clean build.

- [ ] **Step 4: Run the full unit-test suite**

Run: `mise run test:unit 2>&1 | tail -30` Expected: all tests pass (including
the new compact-row tests from Task 1).

- [ ] **Step 5: Commit Tasks 2–5 as a single bundle**

Tasks 2, 3, 4 and this task (5) are tightly coupled: the trait signature change
in Task 2 and the renderer-side changes in Tasks 3–4 must land together for the
tree to build. This final commit closes the bundle.

```bash
git add src/output/hook_progress/ src/executor/presenter.rs src/executor/cli_presenter.rs src/output/tui/presenter.rs
git commit -m "$(cat <<'EOF'
refactor(hook_progress): plumb command preview + cancelled/skipped states

Extends the compact row renderer with a four-state enum
(Success/Failure/Cancelled/Skipped), optional command preview, and
configurable name-column width. Interactive + Plain + HookRenderer enum
all gain finish_job_cancelled and the extended finish_job_skipped
signature. JobPresenter trait grows on_job_cancelled and its on_job_skipped
now takes a trailing command_preview (existing hook callers pass None).

No user-visible behavior change yet — `run_with_progress` still calls the
old row format via the fallback (command_preview = None, width = 24).

The format_compact_row_legacy shim introduced in Task 1 is deleted here;
interactive and plain renderers now call the full API directly with
previews pulled from their JobState storage.
EOF
)"
```

- [ ] **Step 6: Delete the `format_compact_row_legacy` shim**

Back in `src/output/hook_progress/formatting.rs`, delete the shim added in Task
1 Step 3. Since Tasks 3 and 4 (above) migrated interactive.rs and plain.rs to
the full 5-arg API, the shim is unused. Confirm with
`rg format_compact_row_legacy src/` — zero matches. Amend the bundle commit to
include this cleanup (or land it as a trailing commit — preference).

---

## Task 6: Spinner shows current command in live mode

**Files:**

- Modify: `src/output/hook_progress/interactive.rs`

The spinner message today shows only the branch name; the style appends a
trailing arrow. Under this revision the spinner should show `branch  ❯  command`
so the live UI matches the finalized row. Hooks (no preview) stay on the old
shape `branch ❯`.

- [ ] **Step 1: Move the trailing arrow out of the style template**

At `src/output/hook_progress/interactive.rs:64-78`, change both style templates
to drop the trailing arrow:

```rust
        let spinner_style = ProgressStyle::with_template(&format!(
            "{pipe_str}  {{spinner}} {{msg}}"
        ))
        .unwrap()
        .tick_chars(
            "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
        );

        let spinner_style_with_timer = ProgressStyle::with_template(&format!(
            "{pipe_str}  {{spinner}} {{msg}} [{{elapsed_precise}}]"
        ))
        .unwrap()
        .tick_chars(
            "\u{2807}\u{2819}\u{2839}\u{2838}\u{283c}\u{2834}\u{2826}\u{2827}\u{2807}\u{280f}",
        );
```

- [ ] **Step 2: Compose the spinner message to include arrow + preview**

At `src/output/hook_progress/interactive.rs:122-127`, replace the display-name
construction with:

```rust
        let display_name = match command_preview {
            Some(cmd) if self.use_color => format!(
                "{ORANGE}{name}{}  {arrow}  {DARK_GREY}{cmd}{}",
                styles::RESET,
                styles::RESET,
                arrow = self.arrow_str,
            ),
            Some(cmd) => format!("{name}  \u{276f}  {cmd}"),
            None if self.use_color => format!(
                "{ORANGE}{name}{}  {arrow}",
                styles::RESET,
                arrow = self.arrow_str,
            ),
            None => format!("{name}  \u{276f}"),
        };
        spinner.set_message(display_name);
```

The `None` branches preserve the pre-revision hook shape (`┃  ⠋ hookname ❯`) so
hook output is visually unchanged.

- [ ] **Step 3: Drop the now-unused `command_preview` sub-bar block**

At `src/output/hook_progress/interactive.rs:146-161`, the verbose-mode sub-bar
that previously displayed the command preview below the spinner (`cmd_bar`) is
now redundant — the preview is in the spinner's message. Delete that block. Keep
the `description` sub-bar (lines 130-144) — descriptions are distinct from
command previews. After deletion, `last_bar` is still whatever was set in the
description block (or the spinner), and the trailer insertion still works.

Concretely, delete lines 146-161 and reset `last_bar` logic so the remaining
flow is:

```rust
        let mut last_bar = spinner.clone();
        if let Some(desc) = description {
            let desc_bar = self.mp.insert_after(&last_bar, ProgressBar::new_spinner());
            // … existing description body …
            last_bar = desc_bar;
        }

        // Trailer is a blank spacer bar …
        let trailer = self.mp.insert_after(&last_bar, ProgressBar::new_spinner());
```

- [ ] **Step 4: Build + test**

Run: `cargo test -p daft --lib output::hook_progress 2>&1 | tail -20` Expected:
all tests pass. The existing
`active_job_has_trailing_spacer_for_vertical_separation` test still holds.

- [ ] **Step 5: Commit**

```bash
git add src/output/hook_progress/interactive.rs
git commit -m "$(cat <<'EOF'
feat(hook_progress): live spinner shows `branch ❯ command`

The interactive spinner's message now composes `name  ❯  preview` when a
command preview is provided (exec flow), preserving the pre-revision
`name  ❯` shape for hooks (preview is None). The verbose-mode sub-bar
that used to render the preview below the spinner is removed — the
preview is now inline.
EOF
)"
```

---

## Task 7: `run_pipeline_streaming` — drop `[N/M]`, emit cancel and skip

**Files:**

- Modify: `src/core/worktree/exec/mod.rs`

- [ ] **Step 1: Add a test that exercises fail-fast skip emission**

Add to the existing `#[cfg(test)] mod tests` at the bottom of
`src/core/worktree/exec/mod.rs` (find the current test module; if the file
already has tests, append — otherwise create a new
`#[cfg(test)] mod streaming_tests` block):

```rust
#[cfg(test)]
mod streaming_skip_emission_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct EventRecorder {
        events: Mutex<Vec<String>>,
    }

    impl EventRecorder {
        fn arc() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn log(&self, event: &str) {
            self.events.lock().unwrap().push(event.to_string());
        }

        fn take(&self) -> Vec<String> {
            std::mem::take(&mut self.events.lock().unwrap())
        }
    }

    impl crate::executor::presenter::JobPresenter for EventRecorder {
        fn on_phase_start(&self, _: &str) {}
        fn on_job_start(&self, name: &str, _: Option<&str>, preview: Option<&str>) {
            self.log(&format!("start:{name}:{}", preview.unwrap_or("")));
        }
        fn on_job_output(&self, _: &str, _: &str) {}
        fn on_job_success(&self, name: &str, _: std::time::Duration) {
            self.log(&format!("success:{name}"));
        }
        fn on_job_failure(&self, name: &str, _: std::time::Duration) {
            self.log(&format!("failure:{name}"));
        }
        fn on_job_cancelled(&self, name: &str, _: std::time::Duration) {
            self.log(&format!("cancelled:{name}"));
        }
        fn on_job_skipped(
            &self,
            name: &str,
            _reason: &str,
            _duration: std::time::Duration,
            _show: bool,
            preview: Option<&str>,
        ) {
            self.log(&format!("skipped:{name}:{}", preview.unwrap_or("")));
        }
        fn on_message(&self, _: &str) {}
        fn on_phase_complete(&self, _: std::time::Duration) {}
        fn take_results(&self) -> Vec<crate::executor::JobResult> {
            Vec::new()
        }
    }

    #[test]
    fn fail_fast_emits_skipped_for_unrun_steps() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: "branch-a".into(),
        };
        let pipeline = vec![
            CommandSpec::Argv(vec!["false".into()]),
            CommandSpec::Argv(vec!["echo".into(), "never".into()]),
        ];
        let recorder = EventRecorder::arc();
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = Arc::clone(&recorder);

        let outcome = run_pipeline_streaming(&target, &pipeline, "", &presenter, &CancelFlag::new())
            .unwrap();

        assert!(!outcome.succeeded(), "first step should fail");
        let events = recorder.take();
        let starts: Vec<&String> = events.iter().filter(|e| e.starts_with("start:")).collect();
        assert_eq!(starts.len(), 1, "only first step should start: {events:?}");
        assert!(
            events.iter().any(|e| e == "failure:branch-a"),
            "missing failure event: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "skipped:branch-a:echo never"),
            "missing skipped event for step 2: {events:?}"
        );
    }

    #[test]
    fn pre_cancel_emits_skipped_for_all_steps() {
        // Exercises the top-of-loop `cancel.is_cancelled()` check: pre-
        // escalate the flag before calling run_pipeline_streaming so the
        // first iteration bails immediately and emits skip rows for every
        // pipeline step. Race-free (no sleeps).
        let dir = tempfile::TempDir::new().unwrap();
        let target = ResolvedTarget {
            worktree_path: dir.path().to_path_buf(),
            branch_name: "branch-b".into(),
        };
        let pipeline = vec![
            CommandSpec::Argv(vec!["echo".into(), "one".into()]),
            CommandSpec::Argv(vec!["echo".into(), "two".into()]),
        ];
        let recorder = EventRecorder::arc();
        let presenter: Arc<dyn crate::executor::presenter::JobPresenter> = Arc::clone(&recorder);

        let cancel = CancelFlag::new();
        cancel.escalate();

        let outcome = run_pipeline_streaming(&target, &pipeline, "", &presenter, &cancel).unwrap();

        assert!(outcome.cancelled, "expected cancelled outcome");
        let events = recorder.take();
        let starts = events.iter().filter(|e| e.starts_with("start:")).count();
        assert_eq!(starts, 0, "no steps should start when pre-cancelled: {events:?}");
        assert!(
            events.iter().any(|e| e == "skipped:branch-b:echo one"),
            "missing skipped event for step 1: {events:?}"
        );
        assert!(
            events.iter().any(|e| e == "skipped:branch-b:echo two"),
            "missing skipped event for step 2: {events:?}"
        );
    }
}
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run:
`cargo test -p daft --lib core::worktree::exec::streaming_skip_emission_tests 2>&1 | tail -30`
Expected: both tests FAIL — the code doesn't emit skip events yet, and cancel
routes through `on_job_failure` not `on_job_cancelled`.

- [ ] **Step 3: Drop `[i+1/n]` from job name**

At `src/core/worktree/exec/mod.rs:549-553`, change:

```rust
        let job_name = base_name.to_string();
```

(Remove the `if pipeline.len() > 1` branch — the step identity is now conveyed
via the `preview` arg, not the job name. Because per-worktree pipelines are
serial, the HashMap key can simply be the branch name; the previous step's state
is always removed by its own `finish_*` call before the next `on_job_start`.)

- [ ] **Step 4: Route cancel through `on_job_cancelled`**

At `src/core/worktree/exec/mod.rs:610-614`, change:

```rust
        if cancel.is_cancelled() {
            cancelled = true;
            presenter.on_job_cancelled(&job_name, cmd_elapsed);
            for step in pipeline.iter().skip(idx + 1) {
                presenter.on_job_skipped(
                    &job_name,
                    "",
                    Duration::ZERO,
                    false,
                    Some(&step.display()),
                );
            }
            break;
        }
```

- [ ] **Step 5: Emit skipped rows for unrun steps after fail-fast**

At `src/core/worktree/exec/mod.rs:615-620`, change the failure branch:

```rust
        if exit_code == 0 {
            presenter.on_job_success(&job_name, cmd_elapsed);
        } else {
            presenter.on_job_failure(&job_name, cmd_elapsed);
            for step in pipeline.iter().skip(idx + 1) {
                presenter.on_job_skipped(
                    &job_name,
                    "",
                    Duration::ZERO,
                    false,
                    Some(&step.display()),
                );
            }
            break;
        }
```

- [ ] **Step 6: Emit skipped rows when top-of-loop cancel check fires**

At `src/core/worktree/exec/mod.rs:541-546`, replace the top-of-loop cancel
check:

```rust
        if cancel.is_cancelled() {
            cancelled = true;
            for step in pipeline.iter().skip(idx) {
                presenter.on_job_skipped(
                    &base_name.to_string(),
                    "",
                    Duration::ZERO,
                    false,
                    Some(&step.display()),
                );
            }
            break;
        }
```

Note: `idx` is the declared loop counter (`for (idx, spec)`). `base_name` is in
scope. The `skip(idx)` emits rows for the current step and all later ones.

- [ ] **Step 7: Run the unit tests**

Run:
`cargo test -p daft --lib core::worktree::exec::streaming_skip_emission_tests 2>&1 | tail -30`
Expected: both tests pass.

- [ ] **Step 8: Run the full exec module tests**

Run: `cargo test -p daft --lib core::worktree::exec 2>&1 | tail -30` Expected:
all pass.

- [ ] **Step 9: Commit**

```bash
git add src/core/worktree/exec/mod.rs
git commit -m "$(cat <<'EOF'
feat(exec): emit cancelled and skipped events per pipeline step

run_pipeline_streaming no longer encodes `[i+1/n]` into the job name —
step identity is conveyed via the command preview passed to on_job_start.
When a step is cancelled mid-flight, fires on_job_cancelled (not
on_job_failure) and emits on_job_skipped for every remaining step in the
pipeline. Fail-fast (non-zero exit) also emits on_job_skipped for
remaining steps. Top-of-loop cancel emits skips for the current and all
remaining steps.
EOF
)"
```

---

## Task 8: `run_with_progress` — drop Commands header + emit skips for unlaunched targets

**Files:**

- Modify: `src/core/worktree/exec/progress_renderer.rs`
- Modify: `src/core/worktree/exec/list_renderer.rs`

- [ ] **Step 1: Trim `render_header`**

At `src/core/worktree/exec/list_renderer.rs:13-28`, reduce to divider + label:

```rust
pub fn render_header<W: Sink>(sink: &mut W, _pipeline: &[CommandSpec]) -> std::io::Result<()> {
    writeln!(
        sink,
        "────────────────────────────────────────────────────────────"
    )?;
    writeln!(sink, "Worktrees")?;
    Ok(())
}
```

(The `_pipeline` arg is kept for signature compatibility with existing callers;
adjust if a warning fires.)

- [ ] **Step 2: Pass target names to the renderer for column-width sizing**

At `src/core/worktree/exec/progress_renderer.rs:109-115`, after the
`CliPresenter::auto(&cfg)` line, obtain the underlying renderer and set the
column width. Because `CliPresenter` owns its `HookRenderer` behind a mutex,
expose a pass-through setter:

In `src/executor/cli_presenter.rs`, add a public method:

```rust
    pub fn set_name_column_width(&self, width: usize) {
        let mut r = self.renderer.lock().expect("CliPresenter mutex poisoned");
        r.set_name_column_width(width);
    }
```

Back in `progress_renderer.rs`, after creating the presenter:

```rust
    let cfg = HookOutputConfig {
        compact_finalization: true,
        ..HookOutputConfig::default()
    };
    let presenter_concrete = CliPresenter::auto(&cfg);
    let max_name = targets
        .iter()
        .map(|t| t.branch_name.len())
        .max()
        .unwrap_or(24);
    presenter_concrete.set_name_column_width(max_name);
    let presenter: Arc<dyn JobPresenter> = presenter_concrete;
```

(`CliPresenter::auto` already returns `Arc<CliPresenter>` — calling
`set_name_column_width` on it before coercion is the reason for the temporary
binding.)

- [ ] **Step 3: Emit skips for never-launched targets after scheduler join**

At `src/core/worktree/exec/progress_renderer.rs:117-122` (after the `match mode`
block), before the `Ok(ExecReport { … })` return, add:

```rust
    let dispatched: std::collections::HashSet<_> = outcomes
        .iter()
        .map(|o| o.target.worktree_path.clone())
        .collect();
    for target in targets {
        if dispatched.contains(&target.worktree_path) {
            continue;
        }
        for step in pipeline {
            presenter.on_job_skipped(
                &target.branch_name,
                "",
                std::time::Duration::ZERO,
                false,
                Some(&step.display()),
            );
        }
    }
```

- [ ] **Step 4: Update the progress renderer's sole unit test**

At `src/core/worktree/exec/progress_renderer.rs:198-211`, the
`run_with_progress_single_target_success` test still passes as-is (single
target, no cancel). No changes required.

- [ ] **Step 5: Build + unit tests**

Run: `mise run test:unit 2>&1 | tail -20` Expected: all pass.

- [ ] **Step 6: Manual smoke test (if available)**

Run:
`cargo build --release && ./target/release/daft exec --all -x 'true' -x 'false' 2>&1 | head -40`

Expected output (truncated): every row shows the inline command; the first
step's `false` failure causes the second `false` to be suppressed for that
worktree and rendered as `○ branch ❯ false skipped`.

(Skip this step in CI; run locally if possible.)

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/exec/progress_renderer.rs \
        src/core/worktree/exec/list_renderer.rs \
        src/executor/cli_presenter.rs
git commit -m "$(cat <<'EOF'
feat(exec): drop Commands header; emit skips for never-launched targets

Every finalization row now names its command inline, making the `Commands`
listing at the top redundant — trim the header to divider + `Worktrees`
label. After the scheduler joins, emit one `on_job_skipped` per pipeline
step for every target that never received an outcome (cancel before
dispatch). Plumb the max branch-name width through `CliPresenter` so rows
align.
EOF
)"
```

---

## Task 9: Integration tests + YAML scenarios + docs

**Files:**

- Modify: `tests/integration/test_worktree_exec.sh`
- Create: `tests/manual/scenarios/worktree-exec/fail-fast-multi-step.yml`
  (filename; adjust to match existing naming convention)
- Modify: `CHANGELOG.md`
- Modify: `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md`,
  `docs/guide/running-commands-across-worktrees.md`
- Regenerate: `man/` files

- [ ] **Step 1: Add integration-test assertion for inline command names**

Find the existing multi-target success case in
`tests/integration/test_worktree_exec.sh` and add an assertion after the command
produces output. Grep for an existing assertion pattern first:

```bash
rg -n 'assert_output_contains' tests/integration/test_worktree_exec.sh | head -10
```

Then add a check that the output contains `❯ echo` (or whichever command the
existing test runs) — the exact glyph is `\xe2\x9d\xaf`. Use the helper pattern
already in the file.

(If the existing tests do not produce a multi-command pipeline, add a new test
case that runs `daft exec --all -x 'true' -x 'echo hi'` in the test fixture and
asserts `✓ <branch> ❯ true`, `✓ <branch> ❯ echo hi` appear.)

- [ ] **Step 2: Add an integration-test assertion for skipped rows**

Extend with: `daft exec --all -x 'false' -x 'echo never'` — assert stdout
contains `✗ <branch> ❯ false` and `○ <branch> ❯ echo never skipped`.

- [ ] **Step 3: Run integration tests**

Run: `mise run test:integration 2>&1 | tail -30` Expected: all pass.

- [ ] **Step 4: Add a YAML manual-test scenario**

Create `tests/manual/scenarios/worktree-exec/fail-fast-skipped.yml`. Use an
adjacent existing scenario as a template — open
`tests/manual/scenarios/worktree-exec/` first:

```bash
ls tests/manual/scenarios/worktree-exec/
```

Follow the schema used there. The scenario should set up ≥2 worktrees, run
`daft exec --all -x 'false' -x 'echo never'`, and assert:

- Stdout contains `✗` rows followed by `○ ... ❯ echo never  skipped` rows.
- Exit code is 1.

- [ ] **Step 5: Run the manual-test scenario**

Run: `mise run test:manual -- --ci worktree-exec` Expected: all `worktree-exec`
scenarios pass, including the new one.

- [ ] **Step 6: Update CHANGELOG.md**

Open `CHANGELOG.md`. Under `[Unreleased]` → `### Changed` (create the section if
absent), add:

```markdown
- `daft exec` finalization rows now name the command inline
  (`✓ branch ❯ cmd (1.5s)`) instead of `[N/M]`. Cancelled-mid-flight and
  never-started steps surface as distinct states (`⊘ cancelled after N` and
  `○ skipped`). The `Commands` block at the top of the output is dropped — every
  row is self-describing.
```

- [ ] **Step 7: Update docs sample output**

In `docs/cli/git-worktree-exec.md`, `docs/cli/daft-exec.md`, and
`docs/guide/running-commands-across-worktrees.md`, find any block that shows
`[1/2]` notation and replace with the new inline-command form. Also add a short
paragraph on the state glyphs (✓ ✗ ⊘ ○) somewhere reasonable in the guide.

Run `rg '\[1/2\]' docs/` to find all instances; update each.

- [ ] **Step 8: Regenerate man pages**

Run: `mise run man:gen`

Commit the regenerated `man/` diff alongside the doc updates.

- [ ] **Step 9: Run `mise run ci` end-to-end locally**

Run: `mise run ci 2>&1 | tail -40` Expected: green.

- [ ] **Step 10: Commit**

```bash
git add tests/ docs/ CHANGELOG.md man/
git commit -m "$(cat <<'EOF'
test(exec): cover inline command names + skipped rows

Adds integration-test and manual (YAML) coverage for the new
row format. Updates CHANGELOG, docs sample output, and regenerated
man pages.
EOF
)"
```

---

## Self-Review Checklist (for the executor)

Before handing off to code review:

1. `rg '\[1/2\]|\[N/M\]' src/ docs/` — no residual occurrences of the old
   bracket notation.
2. `rg 'on_job_skipped\(' src/` — every call site passes 5 args (trailing `None`
   or `Some(&preview)`).
3. `rg 'on_job_cancelled' src/` — the new method is wired in every
   `JobPresenter` impl.
4. `mise run fmt:check && mise run clippy && mise run test:unit && mise run test:integration && mise run man:verify`
   — all green.
5. Exit codes unchanged: `daft exec --all -- true` returns 0;
   `daft exec --all -- false` returns 1; interactive pass-through (single
   target) unchanged.
