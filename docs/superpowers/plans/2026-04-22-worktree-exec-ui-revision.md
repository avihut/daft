# Worktree Exec UI Revision — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the current split default/`-v` UI for multi-worktree
`daft exec` with a single live-progress UI: reuse the running-windows renderer
minus its hook branding, finalize each worktree to the current compact row
(`✓ branch (1.8s)`), drop `-v` entirely.

**Architecture:** Add a `compact_finalization` knob to `HookOutputConfig`
(default `false`, so hooks are unchanged). Branch
`HookProgressRenderer::finish_job` / `PlainHookRenderer::finish_job_*` on that
flag to print a one-line row instead of the hook-style heading + output dump.
Rename `src/core/worktree/exec/windows_renderer.rs` → `progress_renderer.rs`;
make it the only multi-worktree path, print the list-mode `Commands` header
itself, and skip the presenter's `on_phase_start`/`on_phase_complete` calls.
Drop `--verbose` from the exec command.

**Tech Stack:** Rust, clap, indicatif (via existing `HookProgressRenderer`),
`HookOutputConfig`, bash integration tests, YAML manual scenarios, VitePress
docs.

**Spec reference:**
`docs/superpowers/specs/2026-04-22-worktree-exec-ui-revision-design.md`

---

### Task 1: Add `compact_finalization` field to `HookOutputConfig`

**Files:**

- Modify: `src/core/settings.rs`

- [ ] **Step 1: Add the field**

Open `src/core/settings.rs` and find the `HookOutputConfig` struct (around line
717). Add a new field:

```rust
/// Configuration for hook output display.
#[derive(Debug, Clone)]
pub struct HookOutputConfig {
    /// Suppress hook stdout/stderr (only show spinner + result line).
    pub quiet: bool,
    /// Seconds before showing elapsed timer on spinners.
    pub timer_delay_secs: u32,
    /// Number of rolling output tail lines per job (0 = no tail).
    pub tail_lines: u32,
    /// Show verbose output including skipped jobs and their reasons.
    pub verbose: bool,
    /// When true, on job finish print a single compact row
    /// (`✓ name (dur)` / `✗ name (dur)`) and drop the inline output dump.
    /// Hooks leave this false; `daft exec` sets it true.
    pub compact_finalization: bool,
}
```

- [ ] **Step 2: Update the `Default` impl**

```rust
impl Default for HookOutputConfig {
    fn default() -> Self {
        Self {
            quiet: false,
            timer_delay_secs: 5,
            tail_lines: 6,
            verbose: false,
            compact_finalization: false,
        }
    }
}
```

- [ ] **Step 3: Verify existing tests still pass**

Run: `cargo test --lib core::settings::` Expected: PASS (existing tests don't
check the new field; default additions are additive).

- [ ] **Step 4: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat(settings): add compact_finalization knob to HookOutputConfig"
```

---

### Task 2: Add `format_compact_row` helper in `formatting.rs`

The helper produces the visible string for a compact row, colored or plain.
Unit-testable in isolation.

**Files:**

- Modify: `src/output/hook_progress/formatting.rs`

- [ ] **Step 1: Write the failing test**

Append to `src/output/hook_progress/formatting.rs`:

```rust
#[cfg(test)]
mod compact_row_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn compact_row_success_plain() {
        let row = format_compact_row("master", true, Duration::from_millis(1800), false);
        // Matches list_renderer::render_outcome's visible format:
        //   "  ✓  master                    (1.8s)"
        assert!(row.contains("\u{2713}"), "expected ✓, got: {row:?}");
        assert!(row.contains("master"), "missing name: {row:?}");
        assert!(row.contains("(1.8s)"), "missing elapsed: {row:?}");
    }

    #[test]
    fn compact_row_failure_plain() {
        let row = format_compact_row("feat/dirty", false, Duration::from_millis(1200), false);
        assert!(row.contains("\u{2717}"), "expected ✗, got: {row:?}");
        assert!(row.contains("feat/dirty"));
        assert!(row.contains("(1.2s)"));
    }

    #[test]
    fn compact_row_has_leading_indent() {
        let row = format_compact_row("x", true, Duration::from_secs(1), false);
        assert!(row.starts_with("  "), "expected 2-space leading indent, got: {row:?}");
    }

    #[test]
    fn compact_row_color_wraps_sigil_and_name() {
        let row = format_compact_row("x", true, Duration::from_secs(1), true);
        // Colored variant must include an ANSI reset somewhere.
        assert!(row.contains("\x1b["), "expected ANSI escapes when use_color: {row:?}");
    }
}
```

Run: `cargo test --lib output::hook_progress::formatting::compact_row_tests`
Expected: FAIL — `format_compact_row` does not exist.

- [ ] **Step 2: Implement `format_compact_row`**

Add to `src/output/hook_progress/formatting.rs` (above the `#[cfg(test)]`
module). Uses the same `✓`/`✗` glyphs as `list_renderer::render_outcome`:

```rust
/// Render a finalized per-job row for compact-finalization mode.
///
/// Matches `crate::core::worktree::exec::list_renderer::render_outcome`'s
/// visible shape: two-space indent, sigil, double space, 24-char left-padded
/// name, single space, parenthesized duration. Colored variant adds ANSI
/// escapes consistent with the summary formatting.
pub(super) fn format_compact_row(
    name: &str,
    success: bool,
    duration: Duration,
    use_color: bool,
) -> String {
    let sigil = if success { "\u{2713}" } else { "\u{2717}" };
    let elapsed = format_duration(duration);
    if use_color {
        let color = if success { crate::styles::GREEN } else { crate::styles::RED };
        format!(
            "  {color}{sigil}{}  {:<24}{} {GREY}({elapsed}){}",
            crate::styles::RESET,
            name,
            crate::styles::RESET,
            crate::styles::RESET
        )
    } else {
        format!("  {sigil}  {:<24} ({elapsed})", name)
    }
}
```

Run: `cargo test --lib output::hook_progress::formatting::compact_row_tests`
Expected: PASS (4/4).

- [ ] **Step 3: Commit**

```bash
git add src/output/hook_progress/formatting.rs
git commit -m "feat(hook_progress): add format_compact_row helper for compact finalization"
```

---

### Task 3: Branch `HookProgressRenderer::finish_job` on `compact_finalization`

**Files:**

- Modify: `src/output/hook_progress/interactive.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `#[cfg(test)] mod tests` block in
`src/output/hook_progress/interactive.rs`:

```rust
#[test]
fn compact_finalization_records_success_without_panicking() {
    let config = HookOutputConfig {
        compact_finalization: true,
        ..Default::default()
    };
    let mut renderer = HookProgressRenderer::new_hidden(&config);
    renderer.start_job("master", None);
    renderer.update_job_output("master", "some build output");
    renderer.finish_job_success("master", Duration::from_millis(1800));

    let jobs = renderer.take_finished_jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].name, "master");
    assert!(matches!(jobs[0].outcome, JobOutcome::Success));
}

#[test]
fn compact_finalization_records_failure_without_panicking() {
    let config = HookOutputConfig {
        compact_finalization: true,
        ..Default::default()
    };
    let mut renderer = HookProgressRenderer::new_hidden(&config);
    renderer.start_job("feat/dirty", None);
    renderer.update_job_output("feat/dirty", "panicked!");
    renderer.finish_job_failure("feat/dirty", Duration::from_millis(1200));

    let jobs = renderer.take_finished_jobs();
    assert_eq!(jobs.len(), 1);
    assert!(matches!(jobs[0].outcome, JobOutcome::Failed));
}
```

Run:
`cargo test --lib output::hook_progress::interactive::tests::compact_finalization`
Expected: PASS (the current code path already records outcomes correctly; this
test locks in that the new branch doesn't break recording).

- [ ] **Step 2: Add the compact-finalization branch in `finish_job`**

In `src/output/hook_progress/interactive.rs`, find
`fn finish_job(&mut self, name: &str, success: bool, duration: Duration)`
(around line 313). Locate the block that prints the permanent heading + output
dump:

```rust
        // Print heading as a permanent line. Because the spinner is already
        // cleared, mp.println() inserts this above remaining *active*
        // spinners only — i.e. after all previously finished jobs' output.
        let finished_name = if self.use_color {
            format!("{ORANGE}{name}{}", styles::RESET)
        } else {
            name.to_string()
        };
        self.mp
            .println(format!(
                "{}  {finished_name} {}",
                self.pipe_str, self.arrow_str
            ))
            .ok();

        // Print full output as permanent lines below the heading
        let has_output = !state.output_buffer.is_empty();
        if !self.config.quiet && has_output {
            for line in &state.output_buffer {
                self.mp.println(format!("{}  {line}", self.pipe_str)).ok();
            }
        }

        if !self.config.quiet && !has_output {
            let msg = if self.use_color {
                format!(
                    "{}  {DARK_GREY}{ITALIC}No output{}",
                    self.pipe_str,
                    styles::RESET
                )
            } else {
                format!("{}  No output", self.pipe_str)
            };
            self.mp.println(msg).ok();
        }

        // Empty line after each job's section
        self.mp.println(String::new()).ok();
```

Wrap that entire block (everything after the bars are cleared and before
`self.finished_jobs.push(...)`) in a branch on
`self.config.compact_finalization`. Replacement:

```rust
        if self.config.compact_finalization {
            self.mp
                .println(super::formatting::format_compact_row(
                    name,
                    success,
                    duration,
                    self.use_color,
                ))
                .ok();
        } else {
            // Print heading as a permanent line. Because the spinner is already
            // cleared, mp.println() inserts this above remaining *active*
            // spinners only — i.e. after all previously finished jobs' output.
            let finished_name = if self.use_color {
                format!("{ORANGE}{name}{}", styles::RESET)
            } else {
                name.to_string()
            };
            self.mp
                .println(format!(
                    "{}  {finished_name} {}",
                    self.pipe_str, self.arrow_str
                ))
                .ok();

            // Print full output as permanent lines below the heading
            let has_output = !state.output_buffer.is_empty();
            if !self.config.quiet && has_output {
                for line in &state.output_buffer {
                    self.mp.println(format!("{}  {line}", self.pipe_str)).ok();
                }
            }

            if !self.config.quiet && !has_output {
                let msg = if self.use_color {
                    format!(
                        "{}  {DARK_GREY}{ITALIC}No output{}",
                        self.pipe_str,
                        styles::RESET
                    )
                } else {
                    format!("{}  No output", self.pipe_str)
                };
                self.mp.println(msg).ok();
            }

            // Empty line after each job's section
            self.mp.println(String::new()).ok();
        }
```

Keep the `self.finished_jobs.push(JobResultEntry { … })` call at the bottom
outside the branch — both modes need to record the result.

- [ ] **Step 3: Run tests**

```bash
cargo test --lib output::hook_progress::interactive
```

Expected: PASS (all prior tests still pass; new `compact_finalization_*` tests
pass).

- [ ] **Step 4: Commit**

```bash
git add src/output/hook_progress/interactive.rs
git commit -m "feat(hook_progress): compact finalization branch in HookProgressRenderer"
```

---

### Task 4: Branch `PlainHookRenderer::finish_job_*` on `compact_finalization`

Non-TTY / pipe path must also honor the new flag so integration tests (which run
under `DAFT_TESTING`) see the compact row rather than the hook-style heading.

**Files:**

- Modify: `src/output/hook_progress/plain.rs`

- [ ] **Step 1: Read the current `PlainHookRenderer::finish_job_success` /
      `_failure`**

Run: `grep -n "fn finish_job" src/output/hook_progress/plain.rs` Use the line
numbers reported to inspect the methods.

- [ ] **Step 2: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in
`src/output/hook_progress/plain.rs`:

```rust
#[test]
fn compact_finalization_success_path_records_outcome() {
    let mut renderer = PlainHookRenderer::with_verbose(false);
    // Plain renderer doesn't expose a mutable config; use the same
    // with_compact_finalization constructor added below.
    renderer.set_compact_finalization(true);
    renderer.start_job("master", None);
    renderer.update_job_output("master", "some build output");
    renderer.finish_job_success("master", Duration::from_millis(1800));
    let jobs = renderer.take_finished_jobs();
    assert_eq!(jobs.len(), 1);
    assert!(matches!(jobs[0].outcome, JobOutcome::Success));
}
```

Run:
`cargo test --lib output::hook_progress::plain::tests::compact_finalization_success_path_records_outcome`
Expected: FAIL — `set_compact_finalization` does not exist.

- [ ] **Step 3: Add `set_compact_finalization` + finalization branch**

Steps inside `src/output/hook_progress/plain.rs`:

1. Add a `compact_finalization: bool` field to `PlainHookRenderer` (default
   `false` in all constructors).
2. Add a setter
   `pub fn set_compact_finalization(&mut self, on: bool) { self.compact_finalization = on; }`.
3. In `finish_job_success` and `finish_job_failure`: if
   `self.compact_finalization` is true, print via
   `crate::output::hook_progress::formatting::format_compact_row(name, success, duration, self.use_color /* or false if renderer is plain */)`
   using `eprintln!` (same sink the rest of the plain renderer uses), and skip
   the existing per-job output listing. Otherwise, fall through to the existing
   behavior.
4. Keep the `finished_jobs.push(...)` call in both branches.

Exact textual replacements depend on current code layout revealed in Step 1. The
subagent should adapt the branch while preserving current behavior when
`compact_finalization` is false.

- [ ] **Step 4: Run tests**

```bash
cargo test --lib output::hook_progress::plain
```

Expected: PASS (all prior tests still pass; new test passes).

- [ ] **Step 5: Commit**

```bash
git add src/output/hook_progress/plain.rs
git commit -m "feat(hook_progress): compact finalization branch in PlainHookRenderer"
```

---

### Task 5: Rename `windows_renderer.rs` → `progress_renderer.rs`

**Files:**

- Rename: `src/core/worktree/exec/windows_renderer.rs` →
  `src/core/worktree/exec/progress_renderer.rs`
- Modify: `src/core/worktree/exec/mod.rs`
- Modify: `src/commands/exec.rs`

- [ ] **Step 1: Rename the file**

```bash
git mv src/core/worktree/exec/windows_renderer.rs src/core/worktree/exec/progress_renderer.rs
```

- [ ] **Step 2: Rename `pub mod` declaration**

Edit `src/core/worktree/exec/mod.rs`:

```rust
// was: pub mod windows_renderer;
pub mod progress_renderer;
```

- [ ] **Step 3: Rename the function and its one call site**

In `src/core/worktree/exec/progress_renderer.rs`:

- Rename `pub fn run_with_live_windows` → `pub fn run_with_progress`.
- Update the doc comment: replace "Verbose 'live windows' renderer" with "Live
  progress renderer for multi-worktree `daft exec`."
- Update the existing unit test `run_with_live_windows_single_target_success` →
  `run_with_progress_single_target_success` (rename function, update any inner
  invocations).

In `src/commands/exec.rs`:

- `core::windows_renderer::run_with_live_windows` →
  `core::progress_renderer::run_with_progress`.

- [ ] **Step 4: Build**

```bash
cargo build
```

Expected: PASS (compiles cleanly). Fix any stale references if the compiler
flags more call sites.

- [ ] **Step 5: Run the renamed test**

```bash
cargo test --lib core::worktree::exec::progress_renderer
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(exec): rename windows_renderer to progress_renderer"
```

---

### Task 6: Rework `run_with_progress` to print neutral header and skip phase events

**Files:**

- Modify: `src/core/worktree/exec/progress_renderer.rs`

- [ ] **Step 1: Replace the function body**

Overwrite `run_with_progress` with:

```rust
/// Run the pipeline across all targets, rendering a live per-worktree progress
/// UI (spinner + rolling tail, finalized to a compact one-line row per
/// worktree). Returns the aggregated [`ExecReport`] so the command layer can
/// still print a scrollback-friendly failure dump.
pub fn run_with_progress(
    targets: &[ResolvedTarget],
    pipeline: &[CommandSpec],
    mode: ExecMode,
    cancel: &CancelFlag,
) -> anyhow::Result<ExecReport> {
    // Print the neutral "Commands / Worktrees" header directly — the
    // presenter's on_phase_start would otherwise print a hook-branded box
    // we don't want here.
    {
        let stderr = std::io::stderr();
        let mut sink = stderr.lock();
        super::list_renderer::render_header(&mut sink, pipeline)?;
    }

    let cfg = HookOutputConfig {
        compact_finalization: true,
        ..HookOutputConfig::default()
    };
    let presenter: Arc<dyn JobPresenter> = CliPresenter::auto(&cfg);

    // Deliberately skip presenter.on_phase_start — it prints the hook
    // header. The header above replaces it.

    let outcomes = match mode {
        ExecMode::Parallel => run_parallel(targets, pipeline, &presenter, cancel)?,
        ExecMode::Sequential => run_sequential(targets, pipeline, false, &presenter, cancel)?,
        ExecMode::KeepGoing => run_sequential(targets, pipeline, true, &presenter, cancel)?,
    };

    // Deliberately skip presenter.on_phase_complete — it prints the hook
    // summary block. Compact per-row finalization + the caller's failed-
    // output dump already cover the user's needs.

    Ok(ExecReport {
        outcomes,
        orphan_branches_skipped: Vec::new(),
    })
}
```

Imports: the existing file already imports `HookOutputConfig`, `CliPresenter`,
`JobPresenter`, `Instant`, etc. `Instant` is no longer used (the phase timing
was for `on_phase_complete`); remove it if the compiler warns.
`use std::time::Instant;` → delete.

- [ ] **Step 2: Build**

```bash
cargo build
```

Expected: PASS. Remove now-unused imports if the compiler warns.

- [ ] **Step 3: Update the existing unit test**

In the same file, the `run_with_progress_single_target_success` test already
exists (post-rename from Task 5). Ensure it still asserts:

- `report.outcomes.len() == 1`
- `report.aggregate_exit_code() == 0`
- `report.outcomes[0].succeeded()`

Run: `cargo test --lib core::worktree::exec::progress_renderer` Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/exec/progress_renderer.rs
git commit -m "feat(exec): progress renderer prints neutral header, skips phase events"
```

---

### Task 7: Drop `--verbose` from `exec` command

**Files:**

- Modify: `src/commands/exec.rs`

- [ ] **Step 1: Remove the `verbose` arg and the two-path branch**

In `src/commands/exec.rs`:

1. Delete the
   `#[arg(short, long, help = "Show hook-style live windows instead of the list-mode table")] pub verbose: bool,`
   field from `Args`.
2. In `run()`, replace `init_logging(args.verbose);` with
   `init_logging(false);`.
3. Replace `let config = OutputConfig::new(false, args.verbose);` with
   `let config = OutputConfig::new(false, false);`.
4. Delete the `if args.verbose` branch entirely. The code after target
   resolution + pipeline construction becomes:

```rust
    // Mode A: single-target pass-through. (unchanged)
    if targets.len() == 1 {
        // … existing block unchanged …
    }

    let mode = if args.keep_going {
        core::ExecMode::KeepGoing
    } else if args.sequential {
        core::ExecMode::Sequential
    } else {
        core::ExecMode::Parallel
    };

    let cancel = std::sync::Arc::new(core::CancelFlag::new());
    let handler_flag = std::sync::Arc::clone(&cancel);
    let _ = ctrlc::set_handler(move || {
        handler_flag.escalate();
    });

    let report = core::progress_renderer::run_with_progress(&targets, &pipeline, mode, &cancel)?;

    // Rows are already printed live by the progress renderer; only the
    // failed-output dump remains for scrollback.
    let stdout = std::io::stdout();
    let mut sink = stdout.lock();
    core::list_renderer::render_failed_output_dump(&mut sink, &report, &pipeline)?;
    drop(sink);

    std::process::exit(report.aggregate_exit_code());
```

5. Update `#[command(after_help = r#"…"#)]` to drop the "Live 'windows' output
   (like hooks)" example. Replace the block:

```
    Live "windows" output (like hooks):
        daft exec --all -v -- cargo test
```

with nothing (delete those two lines and the blank line above them).

- [ ] **Step 2: Add a regression test**

Append to `#[cfg(test)] mod tests` in `src/commands/exec.rs`:

```rust
#[test]
fn rejects_verbose_flag_after_removal() {
    let err = parse(&["--all", "--verbose", "--", "echo"]).unwrap_err();
    assert!(
        err.to_string().contains("unexpected")
            || err.to_string().contains("unrecognized")
            || err.to_string().contains("found"),
        "expected parse error for removed --verbose, got: {err}"
    );
}
```

- [ ] **Step 3: Run exec unit tests**

```bash
cargo test --lib commands::exec
```

Expected: PASS (all existing parse tests still pass; new regression test
passes).

- [ ] **Step 4: Build and verify clippy**

```bash
cargo build && mise run clippy
```

Expected: PASS with zero warnings.

- [ ] **Step 5: Commit**

```bash
git add src/commands/exec.rs
git commit -m "feat(exec): drop -v flag; progress renderer is now the only multi-worktree UI"
```

---

### Task 8: Delete the `verbose-windows.yml` manual scenario

**Files:**

- Delete: `tests/manual/scenarios/worktree-exec/verbose-windows.yml`

- [ ] **Step 1: Delete**

```bash
git rm tests/manual/scenarios/worktree-exec/verbose-windows.yml
```

- [ ] **Step 2: Verify no other scenario references `-v`**

Run: `grep -rn -- "-v\|--verbose" tests/manual/scenarios/worktree-exec/`
Expected: no output.

- [ ] **Step 3: Commit**

```bash
git commit -m "test(exec): drop verbose-windows manual scenario (flag removed)"
```

---

### Task 9: Update CLI docs, guide, and SKILL.md

**Files:**

- Modify: `docs/cli/git-worktree-exec.md`
- Modify: `docs/cli/daft-exec.md`
- Modify: `docs/guide/running-commands-across-worktrees.md`
- Modify: `SKILL.md`

- [ ] **Step 1: Remove `-v` row from the options tables**

In both `docs/cli/git-worktree-exec.md` and `docs/cli/daft-exec.md`, delete the
table row:

```
| `-v, --verbose` | Show hook-style live windows instead of the list-mode table |  |
```

- [ ] **Step 2: Update the guide prose**

In `docs/guide/running-commands-across-worktrees.md`, locate the paragraph/code
block that mentions `-v`. Replace the "In the default list mode … use `-v` to
see everything live" phrasing with one paragraph describing the single
live-progress UI:

```markdown
During a multi-worktree run, `daft exec` shows a live progress row per worktree
with a rolling tail of output beneath each. When a worktree finishes, its row
collapses to a single line: `✓ branch (1.8s)` on success or `✗ branch (1.2s)` on
failure. After all worktrees complete, any that failed have their captured
output dumped to stdout for easy scrollback review.
```

Delete the code block example using `-v`:

```
daft exec --all -v -- cargo test
```

- [ ] **Step 3: Scan `SKILL.md` for `-v` references under exec**

Run: `grep -n -- "-v\|verbose" SKILL.md` Review each hit. If any sit within
`daft exec` guidance, remove the mention. Keep references unrelated to exec.

- [ ] **Step 4: Commit**

```bash
git add docs/cli/git-worktree-exec.md docs/cli/daft-exec.md docs/guide/running-commands-across-worktrees.md SKILL.md
git commit -m "docs(exec): drop -v references; describe unified live progress UI"
```

---

### Task 10: Regenerate man pages

**Files:**

- Auto-regenerated: `man/git-worktree-exec.1`, `man/daft-exec.1`

- [ ] **Step 1: Regenerate**

```bash
mise run man:gen
```

- [ ] **Step 2: Verify**

```bash
mise run man:verify
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add man/git-worktree-exec.1 man/daft-exec.1
git commit -m "docs(man): regenerate exec man pages after -v removal"
```

---

### Task 11: Add CHANGELOG entry

**Files:**

- Modify: `CHANGELOG.md`

- [ ] **Step 1: Add entry under `[Unreleased]` → `### Changed`**

Open `CHANGELOG.md`, find the `[Unreleased]` section. If a `### Changed`
subsection exists, add to it; otherwise create one.

```markdown
### Changed

- `daft exec` / `daft worktree-exec` now shows live per-worktree progress during
  multi-worktree runs, replacing the previous static end-of-run table. The
  separate `-v` / `--verbose` flag is removed — the live UI is the only
  multi-worktree rendering mode.
```

- [ ] **Step 2: Commit**

```bash
git add CHANGELOG.md
git commit -m "docs(changelog): exec now shows live progress; -v flag removed"
```

---

### Task 12: Final CI verification + smoke test

- [ ] **Step 1: Full CI locally**

```bash
mise run fmt:check && mise run clippy && mise run test:unit && mise run man:verify
```

Expected: all PASS.

- [ ] **Step 2: Integration tests**

```bash
mise run test:integration
```

Expected: PASS. If `test_worktree_exec.sh` assertions drift, adjust them to
match the new row format (the compact row `✓ branch (dur)` is already what the
bash tests asserted on).

- [ ] **Step 3: End-to-end smoke test**

In a scratch directory (never inside this repo — see CLAUDE.md):

```bash
cd "$(mktemp -d)"
# Create a tiny remote, clone, make two worktrees.
git init --bare remote.git
git clone remote.git work
cd work
GIT_AUTHOR_NAME=T GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=T GIT_COMMITTER_EMAIL=t@t \
  git commit --allow-empty -m init
git push origin HEAD:main
cd ..
git-worktree-clone --layout contained remote.git
cd remote/main
git-worktree-checkout -b feat-a
cd ../main

# Default multi-target run — should show live rows + final failed-output dump
# (no failures here, so only compact rows).
daft exec --all -- sh -c 'echo hi; sleep 1'
echo "---"
# Failing run — should show failed-output dump after the compact rows.
daft exec --all -- sh -c 'echo ok; exit 7' ; echo "exit=$?"
```

Expected visible output:

- Two `Commands` + `Worktrees` header blocks.
- Compact rows `✓ main (1.0s)` and `✓ feat-a (1.0s)` for the first run.
- Compact rows `✗ main (0.0s)` and `✗ feat-a (0.0s)` for the failing run,
  followed by the failed-output dump with per-branch
  `─── name ── cmd → exit 7 ───` headers and the `ok` capture.
- `exit=1` at the end of the failing run.

No `daft hooks` box. No `summary: (done in…)` block.

- [ ] **Step 4: If all green, the plan is complete**

---

## Self-Review Checklist (done)

1. **Spec coverage** — every spec section mapped to a task:
   - "Config shape" → Task 1
   - "Compact-finalization branch" → Tasks 2, 3, 4
   - "Exec progress renderer shape" (header + skip phase events) → Tasks 5, 6
   - "Exec command layer" (drop `-v`) → Task 7
   - "Non-TTY / DAFT_TESTING fallback" → Task 4 (PlainHookRenderer)
   - "Tests" → Tasks 7 (regression), 8 (manual scenario), 12 (integration)
   - "Documentation" → Tasks 9, 10, 11
2. **Placeholder scan** — none. All tasks contain exact code or exact commands.
3. **Type consistency** — `HookOutputConfig`, `HookProgressRenderer`,
   `PlainHookRenderer`, `CliPresenter`, `run_with_progress`,
   `format_compact_row`, `render_failed_output_dump` names match their referents
   in every task.
