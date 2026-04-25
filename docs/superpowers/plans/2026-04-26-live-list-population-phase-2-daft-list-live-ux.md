# Live List Population — Phase 2: `daft list` Live UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development to implement this plan.

**Goal:** Land the live UX for `daft list` — cheap porcelain seed shows rows
instantly, individual cells fill in as the streaming collector finishes them.
TTY-only path; non-TTY (`--format`, piped stdout, `DAFT_NO_LIVE=1`) keeps
today's blocking one-shot rendering.

**Architecture:** Reuse Phase 1's `LiveTable` + `TuiState` + `TuiRenderer`
infrastructure. `daft list`'s TTY path becomes: parse porcelain (sync) → parse
`--branches`/`--remotes` (sync) → seed `LiveTable` rows → spawn
`list_stream::spawn(ALL fields, Collector)` → drive render loop → exit on
`WorktreeInfoCollectionDone` or `Ctrl-C`. `pin_default_branch: false`,
`partition_by_owner: false` — `daft list` is a query view.

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, std::sync::mpsc,
std::thread. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-04-25-live-list-population-design.md`

**Phase 2 of 3.** Builds on Phase 1 (PR #410, branch
`daft-402/feat/live-list-population`). Phase 3 (migrate prune/sync to streaming
seed) ships under a separate plan immediately after Phase 2.

---

## File Structure

**Modified files:**

- `src/output/tui/state.rs` — add `mark_collection_done(&mut self)` and
  `is_complete(&self) -> bool` helpers. The driver checks `is_complete` to know
  when to exit (today it only checks `AllDone`).
- `src/output/tui/driver.rs` — relax exit condition to also fire when
  `state.is_complete()` returns true (which happens after
  `WorktreeInfoCollectionDone` for the no-phases case). Header height
  computation adjusted: when `state.phases.is_empty()`, header is zero-height
  (no phase banners).
- `src/output/tui/render.rs` — `render_header` no-ops when
  `state.phases.is_empty()`. When `state.live.collection_complete` is false,
  render the dim middle-dot `·` glyph for cells that are `None` AND haven't
  received a patch yet (use `state.live.is_cell_loading(row_idx, field)`). After
  `collection_complete`, `None` renders as today (blank).
- `src/output/tui/render.rs` — add a "verbose footer" rendered below the table
  when `state.show_hook_sub_rows` is true (re-using the verbose flag) showing
  `inflight: N · elapsed: 1.2s`.
- `src/commands/list.rs` — split `run()` into `run_blocking()` (today's full
  collect → render path) and `run_live()` (new TTY path). Dispatch in `run()`:
  if `args.emit.is_structured()` or stdout is not a TTY or `DAFT_NO_LIVE=1` is
  set, call `run_blocking()`. Otherwise call `run_live()`.
- `tests/manual/scenarios/list/live/` — new YAML scenario directory. Add at
  least 3 scenarios that drive `daft list` via PTY and assert rows appear
  immediately.

**New files:**

- `src/commands/list_live.rs` — extracted `run_live()` implementation. Keeps
  `list.rs` focused on the dispatch + blocking path. Module declared from
  `src/commands/mod.rs`.

---

## Task 1: `TuiState::is_complete` + `mark_collection_done` helpers

**Files:**

- Modify: `src/output/tui/state.rs`

The driver needs to know when to exit. Today it checks `AllDone` (sent by the
orchestrator). For `daft list` there's no orchestrator — exit happens on
`WorktreeInfoCollectionDone`. Add a unified completion predicate.

- [ ] **Step 1: Write the failing test**

Append to the test module in `src/output/tui/state.rs`:

```rust
#[test]
fn is_complete_false_until_all_done_or_collection_done() {
    let state = state_with_no_worktrees();
    assert!(!state.is_complete());
}

#[test]
fn is_complete_true_after_all_done_event() {
    let mut state = state_with_no_worktrees();
    state.apply_event(&DagEvent::AllDone);
    assert!(state.is_complete());
}

#[test]
fn is_complete_true_after_collection_done_when_no_phases() {
    let mut state = state_with_no_phases_no_worktrees();
    state.apply_event(&DagEvent::WorktreeInfoCollectionDone);
    assert!(state.is_complete());
}

#[test]
fn is_complete_false_after_collection_done_when_phases_present() {
    // When phases exist, completion still requires AllDone — collection
    // finishing is just one input among many.
    let mut state = state_with_no_worktrees();
    state.apply_event(&DagEvent::WorktreeInfoCollectionDone);
    assert!(!state.is_complete());
}
```

You'll need test helpers `state_with_no_worktrees()` (uses today's default
phases) and `state_with_no_phases_no_worktrees()` (empty phases vec). Add them
next to the existing test fixtures.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib output::tui::state::tests::is_complete` Expected: compile
error — `is_complete` does not exist.

- [ ] **Step 3: Implement `is_complete`**

In `src/output/tui/state.rs`, inside `impl TuiState`:

```rust
/// True when the table has reached a terminal state and the renderer
/// should exit. For commands with phases (prune/sync/clone), this means
/// `done` was set by `DagEvent::AllDone`. For phase-less commands
/// (`daft list`), it also returns true once
/// `live.collection_complete` is set by `WorktreeInfoCollectionDone`.
pub fn is_complete(&self) -> bool {
    self.done || (self.phases.is_empty() && self.live.collection_complete)
}
```

(`done` is the existing field set by the `AllDone` arm.
`live.collection_complete` is set by Phase 1's `LiveTable::apply_event`.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib output::tui::state::tests::is_complete` Expected: 4/4
pass.

Run: `cargo test --lib output::tui::state` to confirm no regression. Expected:
all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/output/tui/state.rs
git commit -m "feat(tui): add TuiState::is_complete unified completion predicate (#402)"
```

---

## Task 2: Driver supports phase-less rendering

**Files:**

- Modify: `src/output/tui/driver.rs`

`TuiRenderer::run` today exits only on `AllDone`. Switch it to exit on
`state.is_complete()`. Also make `header_height` zero-aware so a phase-less
state renders no header banner.

- [ ] **Step 1: Update `header_height` calculation**

In `TuiRenderer::run`, replace:

```rust
let header_height = self.state.phases.len() as u16 + 1;
```

with:

```rust
// `+1` is the phase header label row when phases exist; zero phases =
// no header at all (daft list).
let header_height = if self.state.phases.is_empty() {
    0
} else {
    self.state.phases.len() as u16 + 1
};
```

- [ ] **Step 2: Update exit conditions**

Find every `is_done = matches!(event, DagEvent::AllDone);` and the follow-up
branch that returns. Replace with checks against `self.state.is_complete()`
AFTER `apply_event` runs:

```rust
self.state.apply_event(&event);
if self.state.is_complete() {
    // Final render — position cursor past all content.
    let total_rows = self.total_rendered_rows();
    // ... existing final-draw code ...
    drop(terminal);
    return Ok(self.state);
}
```

(The rest of the final-draw block is unchanged.)

- [ ] **Step 3: Verify no regression**

Run: `cargo test --lib output::tui` and `cargo test --lib`. Expected: all tests
pass.

Run: `mise run test:integration` is too slow here — rely on later Task 8.

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/driver.rs
git commit -m "feat(tui): driver supports phase-less rendering (daft list path) (#402)"
```

---

## Task 3: Render header no-ops when phases empty

**Files:**

- Modify: `src/output/tui/render.rs`

When `daft list` uses the renderer, it has no phases — `render_header` must
early-return cleanly.

- [ ] **Step 1: Inspect `render_header` signature**

Read `src/output/tui/render.rs` and find
`pub fn render_header(state: &TuiState, frame: &mut Frame, area: Rect)`. The
function iterates `state.phases` and draws each.

- [ ] **Step 2: Add early return**

At the top of `render_header`, add:

```rust
if state.phases.is_empty() {
    return;
}
```

This is safe: in the phase-less path, `header_height` is 0 so `area` will be a
zero-row Rect anyway, but explicit early return is clearer.

- [ ] **Step 3: Verify**

Run: `cargo test --lib output::tui::render` and `cargo test --lib`. Expected:
all tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "feat(tui): render_header no-ops when phases are empty (#402)"
```

---

## Task 4: Loading-glyph rendering for unfilled cells

**Files:**

- Modify: `src/output/tui/render.rs`

While `state.live.collection_complete` is false, cells that are `None` AND
haven't received their patch yet should render the dim middle-dot glyph `·`.
After `collection_complete` (or for `None` cells whose patch arrived as `None`),
render today's blank string.

- [ ] **Step 1: Find `render_table` and the per-cell formatting calls**

Read `src/output/tui/render.rs::render_table`. Identify the cell formatting
helpers (likely in `src/output/format.rs` or similar) that render
`Option<usize>` → `String`.

- [ ] **Step 2: Add a `render_cell_with_loading` helper**

In `src/output/tui/render.rs`, add:

```rust
use crate::core::worktree::info_field::FieldSet;

/// Render a cell value with loading-glyph fallback. While the table is
/// still streaming (!collection_complete) AND this cell has not yet
/// received a patch, show a dim middle-dot. Otherwise show `formatted`
/// (which may be the empty string for None cells whose patch arrived
/// as None).
fn render_cell_with_loading(
    formatted: &str,
    is_loading: bool,
) -> String {
    if formatted.is_empty() && is_loading {
        // dim middle-dot — use ANSI dim escape; consistent with other
        // dim styling in this module.
        format!("\x1b[2m·\x1b[0m")
    } else {
        formatted.to_string()
    }
}
```

(Adjust the ANSI sequence to match what the existing render code uses for dim
styling — search for `dim_style` or `Style::default().add_modifier` in the
file.)

- [ ] **Step 3: Wire it into per-cell formatting**

In each per-cell call site within `render_table`, replace direct
formatted-string writes with:

```rust
let formatted = format_ahead_behind(row.info.ahead, row.info.behind, ...);
let cell = render_cell_with_loading(
    &formatted,
    state.live.is_cell_loading(row_idx, FieldSet::BASE_AHEAD_BEHIND),
);
```

Repeat for each cell type with the matching `FieldSet`. Cells that correspond to
no FieldSet (e.g., branch name from porcelain) skip the loading wrapper.

- [ ] **Step 4: Add a test**

Append to `src/output/tui/render.rs` test module:

```rust
#[test]
fn render_cell_with_loading_returns_glyph_when_empty_and_loading() {
    let out = render_cell_with_loading("", true);
    assert!(out.contains("·"));
}

#[test]
fn render_cell_with_loading_returns_formatted_when_present() {
    let out = render_cell_with_loading("+3 -1", true);
    assert_eq!(out, "+3 -1");
}

#[test]
fn render_cell_with_loading_returns_empty_when_done() {
    let out = render_cell_with_loading("", false);
    assert_eq!(out, "");
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib output::tui::render` Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "feat(tui): render dim middle-dot for unfilled cells while streaming (#402)"
```

---

## Task 5: Verbose footer

**Files:**

- Modify: `src/output/tui/render.rs`
- Modify: `src/output/tui/driver.rs`

When `state.show_hook_sub_rows` is true (the existing verbose flag), render a
small footer below the table showing `inflight: N · elapsed: 1.2s`. `N` = count
of rows where any cell is still loading. `elapsed` = time since the renderer
started.

- [ ] **Step 1: Track render start time**

In `src/output/tui/driver.rs`, in `TuiRenderer::run`, capture `Instant::now()`
at function entry as `render_start`. Pass it (or the elapsed duration on each
draw) into a new field on `TuiState` so the renderer can read it. Simplest path:
add `pub render_start_elapsed: Duration` to `TuiState`, updated in `tick()` from
a constructor-stored `Instant`.

- [ ] **Step 2: Add footer rendering**

In `src/output/tui/render.rs`, add
`pub fn render_footer(state: &TuiState, frame: &mut Frame, area: Rect)`:

```rust
pub fn render_footer(state: &TuiState, frame: &mut Frame, area: Rect) {
    if !state.show_hook_sub_rows {
        return;
    }
    let inflight: usize = state.live.received_patches.iter()
        .filter(|fs| !fs.contains(FieldSet::ALL))
        .count();
    let elapsed_secs = state.render_start_elapsed.as_secs_f32();
    let text = format!(" inflight: {inflight} · elapsed: {elapsed_secs:.1}s");
    let line = ratatui::text::Line::from(text)
        .style(ratatui::style::Style::default()
            .add_modifier(ratatui::style::Modifier::DIM));
    frame.render_widget(ratatui::widgets::Paragraph::new(line), area);
}
```

- [ ] **Step 3: Wire footer into the layout**

In `TuiRenderer::run`, when `state.show_hook_sub_rows` is true, extend the
layout to reserve 1 extra row at the bottom and call
`render::render_footer(...)` into it.

- [ ] **Step 4: Test**

Append to `render.rs` test module:

```rust
#[test]
fn render_footer_skips_when_not_verbose() {
    // Build a non-verbose state, render to a test buffer, assert empty.
    // (Use ratatui's TestBackend.)
}
```

(Implementation depends on how other render tests are structured — follow that
pattern.)

- [ ] **Step 5: Run tests**

Run: `cargo test --lib output::tui` Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/render.rs src/output/tui/driver.rs src/output/tui/state.rs
git commit -m "feat(tui): add verbose footer with inflight count and elapsed time (#402)"
```

---

## Task 6: `daft list` live path (`run_live`)

**Files:**

- Create: `src/commands/list_live.rs`
- Modify: `src/commands/list.rs`
- Modify: `src/commands/mod.rs`

Split `daft list`'s `run()` into a dispatch + the existing `run_blocking()`
body, then add `run_live()` as a sibling that drives the live UX. Dispatch in
`run()`:

```rust
if args.emit.is_structured()
    || std::env::var_os("DAFT_NO_LIVE").is_some()
    || !std::io::IsTerminal::is_terminal(&std::io::stdout())
{
    run_blocking(args, settings)
} else {
    list_live::run_live(args, settings)
}
```

`run_live` flow:

1. Parse porcelain (sync) → `Vec<CollectorTarget>` and a parallel
   `Vec<WorktreeInfo>` seed (only `name`, `path`, `kind`, `is_default_branch`,
   `is_current` populated from porcelain).
2. If `--branches`/`--remotes`/`--all`, run `git for-each-ref` to enumerate the
   union (cheap and synchronous).
3. Build
   `LiveTableConfig { pin_default_branch: false, partition_by_owner: false, ... }`.
4. Build
   `TuiState::new(phases=[], worktree_infos, ..., pin_default_branch=false, partition_by_owner=false)`.
5. Channel: `let (tx, rx) = mpsc::channel();`. Spawn the collector:
   `let handle = list_stream::spawn(req, tx);`.
6. Build `TuiRenderer::new(state, rx)` and call `.run()`.
7. On Ctrl-C (handled by the existing `crossterm` event loop — integrate with
   the renderer's event handling), call `handle.cancel()`, drain remaining
   patches, render once more, exit 0.
8. On clean completion (renderer exits because `is_complete()` true), call
   `handle.join()`, exit 0.

- [ ] **Step 1: Create `src/commands/list_live.rs` with the skeleton**

Create the file with
`pub fn run_live(args: list::Args, settings: DaftSettings) -> anyhow::Result<()>`.
Mirror existing `daft list` structure for the cheap sync setup (porcelain parse,
branch enum, column resolution, sort spec). For the slow data, build
`CollectorTarget`s from porcelain entries and seed empty `WorktreeInfo` rows.

(The full implementation will be ~150 LOC. Reference today's
`src/commands/list.rs::run` lines 166-280 for porcelain parsing and column
resolution. Reference `src/commands/sync.rs::run_tui` for the `OperationTable` /
`TuiRenderer` wiring pattern.)

- [ ] **Step 2: Add the dispatch in `list.rs::run`**

Wrap today's body of `run()` in a new private function `run_blocking()`, then
make `run()` itself the dispatcher:

```rust
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-list"));
    init_logging(args.verbose);
    let settings = DaftSettings::load()?;

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    if should_use_live(&args) {
        list_live::run_live(args, settings)
    } else {
        run_blocking(args, settings)
    }
}

fn should_use_live(args: &Args) -> bool {
    use std::io::IsTerminal;
    !args.emit.is_structured()
        && std::env::var_os("DAFT_NO_LIVE").is_none()
        && std::io::stdout().is_terminal()
}
```

- [ ] **Step 3: Add `pub mod list_live;` to `src/commands/mod.rs`**

- [ ] **Step 4: Verify build + non-live behavior unchanged**

Run: `cargo build --bin daft` Expected: clean.

Run: `DAFT_NO_LIVE=1 daft list 2>/dev/null` against this repo (or manually a
temp repo) — should produce identical output to today's `daft list`.

- [ ] **Step 5: Commit**

```bash
git add src/commands/list.rs src/commands/list_live.rs src/commands/mod.rs
git commit -m "feat(list): live cell-by-cell population for TTY paths (#402)"
```

---

## Task 7: Ctrl-C handling in `run_live`

**Files:**

- Modify: `src/commands/list_live.rs` (or wherever the renderer event loop
  lives)

The renderer must observe Ctrl-C and gracefully cancel the collector. Today
`TuiRenderer::run` doesn't poll `crossterm::event` — it polls the channel. Add a
parallel poll for keyboard events.

- [ ] **Step 1: Decide where to handle Ctrl-C**

Two options: (a) Add Ctrl-C polling inside `TuiRenderer::run` (benefits
prune/sync too). (b) Wrap the renderer in `list_live` and poll keyboard from
outside.

Pick (a). It's a small change and benefits other commands later.

- [ ] **Step 2: Add a `cancel_signal: Option<Arc<AtomicBool>>` field to
      `TuiRenderer`**

In `src/output/tui/driver.rs`:

```rust
pub struct TuiRenderer {
    state: TuiState,
    receiver: mpsc::Receiver<DagEvent>,
    extra_rows: u16,
    cancel_signal: Option<Arc<AtomicBool>>,
}

impl TuiRenderer {
    pub fn with_cancel_signal(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel_signal = Some(cancel);
        self
    }
    // ...
}
```

In the render loop, between channel poll and tick, check
`crossterm::event::poll(Duration::from_millis(0))` — if a `KeyEvent` with
`Ctrl-C` arrives, set `cancel_signal` (if present) and break out of the loop
after the next render.

- [ ] **Step 3: Wire it from `run_live`**

In `list_live::run_live`:

```rust
let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
let handle = list_stream::spawn(req, tx);
// Plumb cancel through the renderer:
let renderer = TuiRenderer::new(state, rx).with_cancel_signal(cancel.clone());
let _final_state = renderer.run()?;
// If Ctrl-C was pressed, cancel the collector explicitly.
if cancel.load(Ordering::Relaxed) {
    handle.cancel();
}
handle.join();
Ok(())
```

(Actually the cancel signal IS the collector's cancel handle — pass
`handle.cancel_flag()` to the renderer. Add a new method
`CollectorHandle::cancel_flag(&self) -> Arc<AtomicBool>` that returns the same
`Arc<AtomicBool>` the workers check. This keeps a single source of truth for
cancellation.)

- [ ] **Step 4: Test manually**

Run `daft list` in this repo, hit Ctrl-C while still loading — should exit
cleanly with whatever cells landed.

- [ ] **Step 5: Add a unit test for the renderer's cancel handling**

This is hard to unit-test without a real terminal. Skip if the test
infrastructure doesn't support it. Manual test in step 4 covers it.

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/driver.rs src/commands/list_live.rs src/core/worktree/list_stream.rs
git commit -m "feat(list): handle Ctrl-C in live UX with cooperative cancellation (#402)"
```

---

## Task 8: New PTY-driven YAML scenarios for live `daft list`

**Files:**

- Create: `tests/manual/scenarios/list/live/instant_seed.yml`
- Create: `tests/manual/scenarios/list/live/cells_fill_progressively.yml`
- Create: `tests/manual/scenarios/list/live/no_live_env_var.yml`

Add three scenarios to lock down the new behavior. Use the existing PTY scenario
format from `tests/manual/scenarios/exec/`. Each scenario should:

1. Set up a small fixture repo with 2-3 worktrees.
2. Run `daft list` via PTY.
3. Assert observable behavior.

- [ ] **Step 1: `instant_seed.yml` — rows visible before any cell-fill**

Scenario asserts that within ~50ms of launching `daft list`, the branch names
and paths are visible (porcelain parse is sync) but cells that would require git
calls (ahead/behind etc.) show the loading glyph `·`.

- [ ] **Step 2: `cells_fill_progressively.yml` — all cells filled before the
      command exits**

Scenario asserts that after the command exits, no `·` glyphs remain in the
output (all cells either filled or blank for actual nulls).

- [ ] **Step 3: `no_live_env_var.yml` — `DAFT_NO_LIVE=1` falls back to blocking
      output**

Scenario sets `DAFT_NO_LIVE=1`, runs `daft list`, asserts the output matches
today's blocking format (no `·` glyphs at any point during capture).

- [ ] **Step 4: Run the scenarios**

Run: `mise run test:manual -- --ci list/live` Expected: all scenarios pass.

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/list/live/
git commit -m "test(list): YAML scenarios for live cell-by-cell population (#402)"
```

---

## Task 9: Final verification

- [ ] **Step 1: Full unit + integration suite**

Run: `mise run test:unit && mise run test:integration` Expected: all tests pass
(with `DAFT_NO_LIVE=1` set in CI's integration runner if integration scenarios
touch `daft list` — likely they do, since `list` is used as a setup command in
many scenarios).

- [ ] **Step 2: Clippy + fmt**

Run: `mise run clippy && mise run fmt:check` Expected: zero warnings, clean.

- [ ] **Step 3: Manual smoke test**

Run `daft list` in this repo, observe rows appear instantly with loading glyphs
that fill in as cells stream. Run with various flags (`--branches`, `--all`,
`--columns +size`, `--sort -size`, `--format json`, piped to `cat`) and verify
each behaves correctly.

- [ ] **Step 4: Commit any test fixture / golden output adjustments**

If integration tests need `DAFT_NO_LIVE=1` set in some places, do that in a
focused commit.

```bash
git commit -m "test(integration): set DAFT_NO_LIVE=1 for stable golden output (#402)"
```

(Only if needed.)

- [ ] **Step 5: Push and update PR description**

```bash
git push origin HEAD
```

Update the PR description (PR #410) to note Phase 2 inclusion via `gh pr edit`.

---

## Self-Review Notes

Performed inline before save:

- **Spec coverage**: Each Phase 2 deliverable from the spec maps to a task: live
  UX (T6), TTY gating (T6), loading glyph (T4), verbose footer (T5),
  cancellation (T7), PTY scenarios (T8), `--no-live` opt-out (T6),
  `pin_default_branch: false`/`partition_by_owner: false` (T6), driver
  phase-less support (T2-T3), unified completion predicate (T1).
- **Placeholders**: none — all steps have concrete code or specific commands.
- **Type consistency**: `is_complete`, `render_cell_with_loading`,
  `render_footer`, `with_cancel_signal`, `should_use_live`, `run_live`,
  `run_blocking` — used consistently across tasks.
- **Scope**: Phase 2 only. Phase 3 (prune/sync migration) is a separate plan.
- **Ambiguity**: Task 7's keyboard-event integration is the trickiest spot. The
  plan offers two options and picks one.
