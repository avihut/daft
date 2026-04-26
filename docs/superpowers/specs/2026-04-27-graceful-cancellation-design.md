# Graceful Ctrl-C Cancellation for `daft list` Live UX

**Status:** Approved **Scope:** `daft list` only (prune/sync deferred)
**Author:** Avihu Turzion (drafted via Claude) **Date:** 2026-04-27

## Problem

When the user hits Ctrl-C during `daft list`'s live cell population, the inline
viewport leaves an ungraceful final frame:

1. Cells that hadn't received their patch keep showing the breathing `▬▬▬▬`
   skeleton bar — frozen mid-animation, indistinguishable from "still loading."
2. The shell's `^C` echo lands inside the table content (next to a skeleton bar)
   instead of on its own line. Captured terminal output ended with `▬▬▬▬^C%` —
   the trailing `%` is zsh's "previous output didn't end with a newline" marker.
3. There is no visible signal that the operation was cancelled — the user has to
   infer it from the missing data and the skeleton bars.

Captured symptom (from sandbox at
`/Users/avihu/Projects/daft/.git/.daft-sandbox`):

```
     daft-345/fix/shift-tab-exits-manage-shared-editor  ▬▬▬▬               +233 -14          1d   Avihu Turzion  47m fix(exec): resolve user shell aliases and functio...
  ✦  master                                             ▬▬▬▬^C%
```

## Goal

After Ctrl-C in `daft list`:

- Cells that haven't received their patch render a clear "didn't load" marker
  that is visually distinct from both the loading shimmer and a legitimately
  empty cell.
- The terminal exits cleanly: shell prompt returns on its own line, no half-row
  overlap.
- Verbose mode shows a `cancelled` indicator in the footer.

Out of scope:

- `daft prune` / `daft sync` cancellation polish (different cancellation
  semantics, per-row Status column already carries the signal).
- Killing in-flight git child processes (workers exit cooperatively between
  cluster calls; current `join_thread.join()` waits for the slowest in-flight
  child to settle).
- Reworking the existing `set_cursor_position` calculation in the final-draw
  macro (only adding a trailing newline; not modifying the cursor math).

## Architecture

A new `cancelled: bool` flag on `LiveTable` is the single source of truth for
"user aborted." It's set by a new `mark_cancelled()` method, which also sets
`collection_complete = true`. This means the existing `is_cell_loading`
predicate (`!collection_complete && !received`) naturally returns `false`
post-cancel — no two-flag bookkeeping in the loading-state predicate.

The render path consults a sibling predicate
`is_cell_unloaded(row, field) → bool` that returns `cancelled && !received`.
When this predicate is true and the cell value is empty, the renderer emits a
"didn't load" cell instead of the loading shimmer.

The Ctrl-C arm in `TuiRenderer::run` calls `state.live.mark_cancelled()` before
the existing `final_draw_and_return!()` macro so the macro's last
`terminal.draw(...)` paints with the new state. After viewport teardown, the
cancelled path writes a single `\n` to stderr to guarantee the shell prompt
lands on its own row.

`prune`/`sync` are unaffected: they share `LiveTable` and `TuiRenderer`, but
their `cancelled` flag stays `false` — they don't go through the same list-only
Ctrl-C arm, and even if they did, marking unfilled cells as "didn't load" would
be additive, not regressive.

## Components

### `src/output/tui/live_table.rs`

Additions only. No removals, no signature changes to existing methods.

- New field: `pub cancelled: bool` on `LiveTable`. Initialized to `false` in
  `LiveTable::new`.
- New method: `pub fn mark_cancelled(&mut self)`. Sets:
  - `self.cancelled = true`
  - `self.collection_complete = true`
  - `self.pending_resort = true`
- New method:
  `pub fn is_cell_unloaded(&self, row_idx: usize, field: FieldSet) -> bool`.
  Returns `self.cancelled && !self.received_patches[row_idx].contains(field)`.
- Existing `is_cell_loading` is unchanged in source. Its return value naturally
  flips to `false` post-cancel because `collection_complete` is set by
  `mark_cancelled`.

### `src/output/tui/render.rs`

Three additions plus signature change to `render_cell`.

- New helper: `fn not_loaded_cell() -> Cell<'static>`. Returns
  `Cell::from(Span::styled("\u{2014}", Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)))`.
  Single em-dash, no width parameter, default left-aligned in cell.
- `render_cell` signature gains a second closure parameter
  `is_cell_unloaded: impl Fn(FieldSet) -> bool`. Position: directly after the
  existing `is_cell_loading` parameter.
- For each loadable column branch (`Size`, `Base`, `Changes`, `Remote`, `Age`,
  `Owner`, `Hash`, `LastCommit`), check `is_cell_unloaded(field)` BEFORE the
  existing `is_cell_loading(field)` check. The "is this empty?" gate (e.g.,
  `vals.size.is_empty()`, `wt.info.ahead.is_none() && wt.info.behind.is_none()`,
  etc.) is unchanged. Pattern:

  ```rust
  Column::Size => {
      if vals.size.is_empty() {
          if is_cell_unloaded(FieldSet::SIZE) {
              not_loaded_cell()
          } else if is_cell_loading(FieldSet::SIZE) {
              loading_shimmer_cell(width, tick)
          } else {
              Cell::from("")
          }
      } else {
          Cell::from(vals.size.clone())
      }
  }
  ```

- Both call sites in `render_table` (currently around lines 344 and 363) pass
  `|fs| state.live.is_cell_unloaded(row_idx, fs)` as the new closure alongside
  the existing loading closure.
- `render_footer`: when `state.live.cancelled` is true, append
  ` \u{00B7} cancelled` to the footer string (after the existing
  `inflight: N · elapsed: T.Xs` content). Same dim styling.

### `src/output/tui/driver.rs`

Two surgical changes to the Ctrl-C arm + teardown.

- In the Ctrl-C branch (currently around lines 252–261), call
  `self.state.live.mark_cancelled()` before the existing
  `self.state.done = true; final_draw_and_return!()` lines.
- After `drop(terminal)` in `final_draw_and_return!()`, write `b"\n"` to stderr
  when `self.state.live.cancelled` is true. Single line:
  `if self.state.live.cancelled { let _ = std::io::stderr().write_all(b"\n"); }`.
  Best-effort, ignored failure.

## Data flow

```
User Ctrl-C
  ↓
crossterm Event::Key (driver.rs poll, ~line 251)
  ↓
cancel_signal.store(true)              ← collector workers exit between clusters
state.live.mark_cancelled()            ← cancelled = true, collection_complete = true
state.done = true
  ↓
final_draw_and_return!()
  └─ terminal.draw(|frame| {
       render_header / render_table / render_footer
         └─ render_cell(...) for each loadable column:
              if value missing && is_cell_unloaded(field) → not_loaded_cell()  // NEW
              else if value missing && is_cell_loading(field) → shimmer (now never true after cancel)
              else → render value
         render_footer appends " · cancelled" when state.live.cancelled
     })
  └─ drop(terminal)                    ← inline viewport torn down
  └─ stderr.write_all(b"\n") if cancelled
  └─ return Ok(state)
  ↓
list_live::run
  └─ join_thread.join()                ← waits for workers (out of scope: not changed)
  ↓
process exits cleanly → shell prompt on fresh line
```

## Testing

### Unit tests — `live_table.rs`

- `mark_cancelled_sets_cancelled_and_collection_complete` — both flags flip,
  `pending_resort` is set.
- `is_cell_unloaded_true_when_cancelled_and_not_received` — basic positive.
- `is_cell_unloaded_false_when_received_even_after_cancel` — received cells are
  not marked unloaded.
- `is_cell_unloaded_false_before_cancel` — predicate is gated on cancellation.
- `is_cell_loading_returns_false_after_mark_cancelled` — confirms the
  `collection_complete` side effect (regression guard for the "two predicates"
  simplification).

### Unit tests — `render.rs`

- `not_loaded_cell_renders_dim_em_dash` — `TestBackend`, single column, assert
  buffer cell contains `"—"` and the styled span carries `Color::DarkGray` +
  `Modifier::DIM`.
- `render_cell_uses_not_loaded_when_cancelled_and_unfilled` — for each of `Size`
  / `Base` / `Changes` / `Remote` / `Age` / `Owner` / `Hash` / `LastCommit`:
  with `is_cell_unloaded` returning true and the value-empty gate satisfied,
  assert `"—"` is rendered.
- `render_cell_uses_value_when_received_even_if_cancelled` — guard against
  rendering `"—"` over a real value (e.g., `Size = "1.2 MB"` should still render
  as the value when `is_cell_unloaded` returns false because `received` is
  true).
- `render_footer_appends_cancelled_when_live_cancelled` — verbose mode,
  `live.cancelled = true` → footer string contains ` · cancelled` after the
  elapsed-time segment.
- `render_footer_no_cancelled_suffix_when_not_cancelled` — guard against false
  positive when `live.cancelled` is false.

### Unit test — `driver.rs`

- `ctrl_c_sets_cancelled_on_state` — synthesize `Event::Key(Ctrl-C)`, drive one
  loop turn, assert `state.live.cancelled` is true, `state.done` is true,
  `cancel_signal` is true. Existing `cancel_signal_can_be_set_externally` is the
  template.

### Manual smoke

In a sandbox repo with multiple worktrees:

```
daft list --columns=-path,+size --sort activity --stat lines
```

Hit Ctrl-C while the `▬` skeletons are still pulsing. Confirm:

- All unfilled cells show dim `—` rather than frozen `▬▬▬▬`.
- Shell prompt returns on a fresh line — no `^C%` adjacent to any table row.
- Verbose mode (`-v` flag, if applicable) shows `· cancelled` appended to the
  footer.
- Already-loaded cells still render their real values.

## Trade-offs and rationale

**Why a single dim `—` rather than a row of `—`s sized to column width?** The
single character is the universal table convention for "no value" (Wikipedia
tables, financial reports, CSVs rendered as ASCII). A row of em-dashes would
visually mimic the skeleton bar's footprint, weakening the "this is different"
signal. The single em-dash is small enough to read as "absence" and large enough
to be obvious.

**Why mark `collection_complete = true` inside `mark_cancelled`?** It avoids
splitting "is this cell loading?" across two flags. The semantic reading of
`collection_complete` becomes "the streaming work is over, whether by completion
or by cancellation." Render-path predicates stay simple. The advisor flagged
this simplification explicitly.

**Why a single `\n` after teardown rather than reworking cursor placement?** The
existing cursor calculation in `final_draw_and_return!()` works correctly for
normal completion. The Ctrl-C symptom (cursor inside table content) appears to
stem from the inline viewport's last-row interaction with terminal height
clamping — fixing the calculation risks breaking the normal path. A trailing
newline is a robust, low-risk guarantee that the shell prompt lands on a fresh
line regardless of where the cursor ended up.

**Why not kill in-flight git children?** Workers respond to `cancel_signal`
between cluster calls. The window where a child is still mid-execution is small.
Killing children mid-stream risks leaving the worktree in an inconsistent state
and adds platform-specific process-group complexity. Defer until evidence of
user-visible delay warrants it.
