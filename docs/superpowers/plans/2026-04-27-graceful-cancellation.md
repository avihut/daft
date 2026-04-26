# Graceful Ctrl-C Cancellation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `daft list` Ctrl-C exit cleanly and mark cells that didn't finish
loading with a distinct "didn't load" indicator instead of a frozen skeleton
bar.

**Architecture:** Add a `cancelled: bool` flag on `LiveTable` (set via
`mark_cancelled()` which also flips `collection_complete = true`); render path
consults a sibling `is_cell_unloaded(row, field)` predicate to swap the loading
shimmer for a dim em-dash; driver Ctrl-C arm calls `mark_cancelled()` before the
final draw and writes a trailing `\n` to stderr after viewport teardown so the
shell prompt lands on its own line.

**Tech Stack:** Rust 2021, ratatui 0.30, crossterm 0.29, std::io::Write,
std::sync::atomic.

**Spec:** `docs/superpowers/specs/2026-04-27-graceful-cancellation-design.md`
(commit `4ca43230`).

---

## File Structure

| File                           | Changes                                                                                                                                                                                |
| ------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `src/output/tui/live_table.rs` | Add `cancelled: bool` field, `mark_cancelled()` method, `is_cell_unloaded()` method, tests.                                                                                            |
| `src/output/tui/render.rs`     | Add `not_loaded_cell()` helper, second closure parameter on `render_cell`, per-column branch updates, both `render_table` call sites updated, `render_footer` cancelled suffix, tests. |
| `src/output/tui/driver.rs`     | Add `use std::io::Write;`, call `state.live.mark_cancelled()` in Ctrl-C arm, write `\n` to stderr after `drop(terminal)` when cancelled, test.                                         |

Sequencing rationale: `live_table.rs` first (data model). Then `render.rs`
(consumer of the data model). Then `driver.rs` (producer of the cancellation
event). Each task is self-contained and leaves the build green.

---

## Task 1: Add `cancelled` field + `mark_cancelled()` to `LiveTable`

**Files:**

- Modify: `src/output/tui/live_table.rs`

- [ ] **Step 1: Write the failing test**

Append to the `mod tests` block at the end of `src/output/tui/live_table.rs`
(after the existing tests, before the closing `}`):

```rust
    #[test]
    fn mark_cancelled_sets_cancelled_and_collection_complete() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.cancelled);
        assert!(!t.collection_complete);
        t.mark_cancelled();
        assert!(t.cancelled);
        assert!(t.collection_complete);
        assert!(t.pending_resort);
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib --quiet output::tui::live_table::tests::mark_cancelled_sets_cancelled_and_collection_complete 2>&1 | tail -20
```

Expected: compile error — `LiveTable` has no field named `cancelled` and no
method named `mark_cancelled`.

- [ ] **Step 3: Add the `cancelled` field to `LiveTable`**

In `src/output/tui/live_table.rs`, modify the `LiveTable` struct definition:

```rust
pub struct LiveTable {
    pub rows: Vec<WorktreeRow>,
    pub cfg: LiveTableConfig,
    pub pending_resort: bool,
    pub collection_complete: bool,
    /// Set when the user cancels (Ctrl-C). Cells that haven't received their
    /// patch should render a "data didn't load" marker rather than the
    /// loading shimmer. `mark_cancelled` also sets `collection_complete = true`
    /// so `is_cell_loading` naturally returns false post-cancel.
    pub cancelled: bool,
    pub source_log: PatchSourceLog,
    /// Per-row bitmask of "patches received".
    pub received_patches: Vec<FieldSet>,
    /// Index of the first row in the unowned section, or `None` if no
    /// partition. Recomputed when `partition_by_owner` is true.
    pub unowned_start_index: Option<usize>,
}
```

In `LiveTable::new`, initialize the field. Modify the existing
`let mut t = Self { ... };` block to add `cancelled: false,`:

```rust
        let mut t = Self {
            rows,
            cfg,
            pending_resort: true,
            collection_complete: false,
            cancelled: false,
            source_log: PatchSourceLog::default(),
            received_patches,
            unowned_start_index: None,
        };
```

- [ ] **Step 4: Add the `mark_cancelled` method**

In `src/output/tui/live_table.rs`, add this method inside the
`impl LiveTable { ... }` block (place it after the existing `apply_event` method
for visibility, but anywhere inside the impl is fine):

```rust
    /// Mark the live table as cancelled by user (Ctrl-C). Sets
    /// `collection_complete = true` so the loading shimmer stops and
    /// `pending_resort = true` so the next tick re-runs the sort/partition.
    /// Cells that haven't received their patch will render via
    /// `is_cell_unloaded` rather than the loading shimmer.
    pub fn mark_cancelled(&mut self) {
        self.cancelled = true;
        self.collection_complete = true;
        self.pending_resort = true;
    }
```

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test --lib --quiet output::tui::live_table::tests::mark_cancelled_sets_cancelled_and_collection_complete 2>&1 | tail -10
```

Expected: PASS (1 passed; 0 failed).

- [ ] **Step 6: Run full test suite to confirm no regressions**

```bash
mise run test:unit 2>&1 | tail -15
```

Expected: all tests pass.

- [ ] **Step 7: Lint and format**

```bash
mise run fmt && mise run clippy 2>&1 | tail -10
```

Expected: clippy emits no warnings; fmt completes without diffs needing fixup.

- [ ] **Step 8: Commit**

```bash
git add src/output/tui/live_table.rs
git commit -m "$(cat <<'EOF'
feat(tui): add cancelled flag and mark_cancelled to LiveTable (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Add `is_cell_unloaded()` predicate to `LiveTable`

**Files:**

- Modify: `src/output/tui/live_table.rs`

- [ ] **Step 1: Write the failing tests**

Append these tests inside the `mod tests` block in
`src/output/tui/live_table.rs`:

```rust
    #[test]
    fn is_cell_unloaded_false_before_cancel() {
        let t = LiveTable::new(vec![info("a")], cfg());
        assert!(!t.is_cell_unloaded(0, FieldSet::SIZE));
    }

    #[test]
    fn is_cell_unloaded_true_when_cancelled_and_not_received() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.mark_cancelled();
        assert!(t.is_cell_unloaded(0, FieldSet::SIZE));
    }

    #[test]
    fn is_cell_unloaded_false_when_received_even_after_cancel() {
        let mut t = LiveTable::new(vec![info("a")], cfg());
        t.apply_event(&DagEvent::WorktreeInfoUpdated {
            branch_name: "a".into(),
            patch: WorktreeInfoPatch::Size(Some(123)),
            source: PatchSource::Collector,
        });
        t.mark_cancelled();
        assert!(!t.is_cell_unloaded(0, FieldSet::SIZE));
    }

    #[test]
    fn is_cell_loading_returns_false_after_mark_cancelled() {
        // Regression guard: mark_cancelled sets collection_complete = true,
        // which makes is_cell_loading naturally return false. We rely on this
        // so the render path doesn't need a second "and not cancelled" check
        // in the loading branch.
        let mut t = LiveTable::new(vec![info("a")], cfg());
        assert!(t.is_cell_loading(0, FieldSet::SIZE));
        t.mark_cancelled();
        assert!(!t.is_cell_loading(0, FieldSet::SIZE));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib --quiet output::tui::live_table::tests::is_cell_unloaded 2>&1 | tail -10
```

Expected: compile error — `is_cell_unloaded` does not exist.

- [ ] **Step 3: Add the `is_cell_unloaded` method**

In `src/output/tui/live_table.rs`, add this method to the
`impl LiveTable { ... }` block, right next to (immediately after) the existing
`is_cell_loading` method:

```rust
    /// True when the cell for `field` on `row_idx` should render the
    /// "data didn't load" marker because the user cancelled before the
    /// patch arrived. Mutually exclusive with `is_cell_loading` after
    /// `mark_cancelled()` runs (which sets `collection_complete = true`).
    pub fn is_cell_unloaded(&self, row_idx: usize, field: FieldSet) -> bool {
        self.cancelled && !self.received_patches[row_idx].contains(field)
    }
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --lib --quiet output::tui::live_table::tests::is_cell 2>&1 | tail -15
```

Expected: 5 passed (4 new + 1 pre-existing `is_cell_loading_*` and
`patch_applied_marks_received_for_loading_glyph`). Match by `is_cell` prefix.

- [ ] **Step 5: Run full test suite to confirm no regressions**

```bash
mise run test:unit 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Lint**

```bash
mise run clippy 2>&1 | tail -10
```

Expected: zero warnings.

- [ ] **Step 7: Commit**

```bash
git add src/output/tui/live_table.rs
git commit -m "$(cat <<'EOF'
feat(tui): add is_cell_unloaded predicate to LiveTable (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Add `not_loaded_cell()` helper in `render.rs`

**Files:**

- Modify: `src/output/tui/render.rs`

- [ ] **Step 1: Write the failing test**

Append this test inside the `mod tests` block at the bottom of
`src/output/tui/render.rs` (after
`render_footer_shows_inflight_and_elapsed_when_verbose`, before the closing
`}`):

```rust
    #[test]
    fn not_loaded_cell_renders_dim_em_dash() {
        // The "didn't load" cell should be a single em-dash (U+2014) styled
        // dim + DarkGray, distinct from the breathing skeleton bar (which
        // fills the column with U+25AC).
        let backend = TestBackend::new(5, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = not_loaded_cell();
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(5)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "\u{2014}");
        assert_eq!(buffer[(0, 0)].fg, ratatui::style::Color::DarkGray);
        assert!(
            buffer[(0, 0)]
                .modifier
                .contains(ratatui::style::Modifier::DIM),
            "not_loaded_cell should be DIM"
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib --quiet output::tui::render::tests::not_loaded_cell_renders_dim_em_dash 2>&1 | tail -10
```

Expected: compile error — `not_loaded_cell` is not defined.

- [ ] **Step 3: Add the `not_loaded_cell` helper**

In `src/output/tui/render.rs`, add this function immediately after the
`loading_shimmer_cell` definition (currently around line 633–643). Place it
right after `skeleton_pulse_color` so the loading-glyph helpers stay grouped:

```rust
/// Render a "data didn't load" placeholder for a cell whose patch was not
/// received before the user cancelled (Ctrl-C). Single em-dash (U+2014),
/// dim + DarkGray. Distinct from the loading shimmer (which is a full-width
/// bar of U+25AC) and from a legitimately-empty cell (a blank).
fn not_loaded_cell() -> Cell<'static> {
    Cell::from(Span::styled(
        "\u{2014}",
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM),
    ))
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib --quiet output::tui::render::tests::not_loaded_cell_renders_dim_em_dash 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Run full test suite**

```bash
mise run test:unit 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Lint**

```bash
mise run clippy 2>&1 | tail -10
```

Expected: clippy may emit `dead_code` warning for `not_loaded_cell` since no
caller exists yet — acceptable, will be wired in Task 4. If it fires, add
`#[allow(dead_code)]` directly above the function and remove the attribute in
Task 4. Confirm with:

```bash
mise run clippy 2>&1 | grep -E "warning|error" | head -5
```

If only `dead_code: function .* is never used` appears for `not_loaded_cell`,
add the attribute. Otherwise proceed.

- [ ] **Step 7: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "$(cat <<'EOF'
feat(tui): add not_loaded_cell helper for cancelled-state placeholder (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Wire `is_cell_unloaded` through `render_cell` and both `render_table` call sites

This task ships together because changing `render_cell`'s signature breaks both
`render_table` call sites; partial commits don't compile.

**Files:**

- Modify: `src/output/tui/render.rs`

- [ ] **Step 1: Write the failing test for `render_cell`**

Append this test inside the `mod tests` block at the bottom of
`src/output/tui/render.rs`:

```rust
    #[test]
    fn render_cell_uses_not_loaded_when_cancelled_and_unfilled() {
        // For each loadable column, with is_cell_unloaded returning true,
        // the cell should render the dim em-dash, not the shimmer bar and
        // not an empty cell.
        use crate::core::worktree::info_field::FieldSet;
        use crate::output::format::compute_column_values;
        use crate::output::tui::columns::ColumnContext;
        use crate::output::tui::state::WorktreeRow;

        let info = WorktreeInfo::empty("a");
        let wt = WorktreeRow::idle(info.clone());
        let ctx = ColumnContext {
            project_root: &PathBuf::from("/tmp"),
            cwd: &PathBuf::from("/tmp"),
            now: 0,
            stat: Stat::Lines,
        };
        let vals = compute_column_values(&info, &ctx);

        let columns = [
            (Column::Size, FieldSet::SIZE),
            (Column::Base, FieldSet::BASE_AHEAD_BEHIND),
            (Column::Changes, FieldSet::CHANGES),
            (Column::Remote, FieldSet::REMOTE_AHEAD_BEHIND),
            (Column::Age, FieldSet::BRANCH_AGE),
            (Column::Owner, FieldSet::OWNER),
            (Column::Hash, FieldSet::LAST_COMMIT),
            (Column::LastCommit, FieldSet::LAST_COMMIT),
        ];

        for (col, _field) in columns {
            let backend = TestBackend::new(10, 1);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let cell = render_cell(
                        &col,
                        &wt,
                        &vals,
                        0,
                        Stat::Lines,
                        10,
                        |_fs| false,         // not loading (cancelled implies collection_complete)
                        |_fs| true,          // is_cell_unloaded → true
                    );
                    let table =
                        Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(10)]);
                    frame.render_widget(table, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            assert_eq!(
                buffer[(0, 0)].symbol(),
                "\u{2014}",
                "column {col:?} should render em-dash when cancelled and unfilled"
            );
        }
    }

    #[test]
    fn render_cell_uses_value_when_received_even_if_cancelled() {
        // If the cell value is non-empty (received), is_cell_unloaded should
        // be false and the value should render. Guards against rendering
        // "—" over real data.
        use crate::output::format::compute_column_values;
        use crate::output::tui::columns::ColumnContext;
        use crate::output::tui::state::WorktreeRow;

        let mut info = WorktreeInfo::empty("a");
        info.size_bytes = Some(1024);
        let wt = WorktreeRow::idle(info.clone());
        let ctx = ColumnContext {
            project_root: &PathBuf::from("/tmp"),
            cwd: &PathBuf::from("/tmp"),
            now: 0,
            stat: Stat::Lines,
        };
        let vals = compute_column_values(&info, &ctx);

        let backend = TestBackend::new(10, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let cell = render_cell(
                    &Column::Size,
                    &wt,
                    &vals,
                    0,
                    Stat::Lines,
                    10,
                    |_fs| false,         // not loading
                    |_fs| false,         // not unloaded — received
                );
                let table = Table::new(vec![Row::new(vec![cell])], &[Constraint::Length(10)]);
                frame.render_widget(table, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..10).map(|x| buffer[(x, 0)].symbol().to_string()).collect();
        assert!(
            !row.contains("\u{2014}"),
            "received cell should render value, not em-dash; got {row:?}"
        );
        assert!(
            row.trim_end().chars().any(|c| c.is_ascii_digit() || c == 'B' || c == 'K'),
            "received Size cell should render numeric/unit value; got {row:?}"
        );
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib --quiet output::tui::render::tests::render_cell_uses 2>&1 | tail -15
```

Expected: compile error — `render_cell` takes 7 args, not 8.

- [ ] **Step 3: Update `render_cell` signature**

In `src/output/tui/render.rs`, modify the `render_cell` function signature
(currently around lines 665–673):

```rust
/// Render a single cell for the given column and worktree row.
///
/// `width` is the column's assigned width — used to size shimmer bars when
/// the cell is in a loading state.
/// `is_cell_unloaded` returns true when the user cancelled before the cell's
/// patch arrived; takes precedence over `is_cell_loading`.
fn render_cell(
    col: &Column,
    wt: &super::state::WorktreeRow,
    vals: &ColumnValues,
    tick: usize,
    stat: Stat,
    width: u16,
    is_cell_loading: impl Fn(FieldSet) -> bool,
    is_cell_unloaded: impl Fn(FieldSet) -> bool,
) -> Cell<'static> {
```

- [ ] **Step 4: Update each loadable column branch in `render_cell`**

Replace the bodies of the loadable column match arms inside `render_cell`. Each
follows the same pattern: `is_cell_unloaded` check first, then existing
`is_cell_loading` check, then value render.

Replace `Column::Size` arm with:

```rust
        Column::Size => {
            if vals.size.is_empty() {
                if is_cell_unloaded(FieldSet::SIZE) {
                    not_loaded_cell()
                } else if is_cell_loading(FieldSet::SIZE) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.size.clone())
                }
            } else {
                Cell::from(vals.size.clone())
            }
        }
```

Replace `Column::Base` arm with:

```rust
        Column::Base => {
            let unfilled = wt.info.ahead.is_none() && wt.info.behind.is_none();
            if unfilled && is_cell_unloaded(FieldSet::BASE_AHEAD_BEHIND) {
                not_loaded_cell()
            } else if unfilled && is_cell_loading(FieldSet::BASE_AHEAD_BEHIND) {
                loading_shimmer_cell(width, tick)
            } else {
                render_base_cell(&wt.info, stat)
            }
        }
```

Replace `Column::Changes` arm with:

```rust
        Column::Changes => {
            let unfilled = wt.info.staged + wt.info.unstaged + wt.info.untracked == 0;
            if unfilled && is_cell_unloaded(FieldSet::CHANGES) {
                not_loaded_cell()
            } else if unfilled && is_cell_loading(FieldSet::CHANGES) {
                loading_shimmer_cell(width, tick)
            } else {
                render_changes_cell(&wt.info, stat)
            }
        }
```

Replace `Column::Remote` arm with:

```rust
        Column::Remote => {
            let unfilled = wt.info.remote_ahead.is_none() && wt.info.remote_behind.is_none();
            if unfilled && is_cell_unloaded(FieldSet::REMOTE_AHEAD_BEHIND) {
                not_loaded_cell()
            } else if unfilled && is_cell_loading(FieldSet::REMOTE_AHEAD_BEHIND) {
                loading_shimmer_cell(width, tick)
            } else {
                render_remote_cell(&wt.info, stat)
            }
        }
```

Replace `Column::Age` arm with:

```rust
        Column::Age => {
            if vals.branch_age.is_empty() {
                if is_cell_unloaded(FieldSet::BRANCH_AGE) {
                    not_loaded_cell()
                } else if is_cell_loading(FieldSet::BRANCH_AGE) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.branch_age.clone())
                }
            } else {
                let cell = Cell::from(vals.branch_age.clone());
                if vals.is_old_branch {
                    cell.style(Style::default().add_modifier(Modifier::DIM))
                } else {
                    cell
                }
            }
        }
```

Replace `Column::Owner` arm with:

```rust
        Column::Owner => {
            if vals.owner.is_empty() {
                if is_cell_unloaded(FieldSet::OWNER) {
                    not_loaded_cell()
                } else if is_cell_loading(FieldSet::OWNER) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.owner.clone())
                }
            } else {
                Cell::from(vals.owner.clone())
            }
        }
```

Replace `Column::Hash` arm with:

```rust
        Column::Hash => {
            if vals.hash.is_empty() {
                if is_cell_unloaded(FieldSet::LAST_COMMIT) {
                    not_loaded_cell()
                } else if is_cell_loading(FieldSet::LAST_COMMIT) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from(vals.hash.clone())
                }
            } else {
                Cell::from(vals.hash.clone())
            }
        }
```

Replace `Column::LastCommit` arm with:

```rust
        Column::LastCommit => {
            if vals.last_commit_age.is_empty() && vals.last_commit_subject.is_empty() {
                if is_cell_unloaded(FieldSet::LAST_COMMIT) {
                    not_loaded_cell()
                } else if is_cell_loading(FieldSet::LAST_COMMIT) {
                    loading_shimmer_cell(width, tick)
                } else {
                    Cell::from("")
                }
            } else if vals.last_commit_age.is_empty() {
                Cell::from(vals.last_commit_subject.clone())
            } else if vals.last_commit_subject.is_empty() {
                let cell = Cell::from(vals.last_commit_age.clone());
                if vals.is_old_commit {
                    cell.style(Style::default().add_modifier(Modifier::DIM))
                } else {
                    cell
                }
            } else {
                let age_style = if vals.is_old_commit {
                    Style::default().add_modifier(Modifier::DIM)
                } else {
                    Style::default()
                };
                Cell::from(Line::from(vec![
                    Span::styled(vals.last_commit_age.clone(), age_style),
                    Span::raw(format!(" {}", vals.last_commit_subject)),
                ]))
            }
        }
```

- [ ] **Step 5: Update both `render_table` call sites**

In `src/output/tui/render.rs`, the function `render_table` calls `render_cell`
in two places (currently around lines 344 and 363). Update both to pass the new
closure.

First call site (the pruned-row branch, currently around line 344):

```rust
                    if matches!(col, Column::Status | Column::Annotation) {
                        render_cell(
                            col,
                            wt,
                            vals,
                            state.tick,
                            state.live.cfg.stat,
                            assigned_widths[i],
                            |fs| state.live.is_cell_loading(row_idx, fs),
                            |fs| state.live.is_cell_unloaded(row_idx, fs),
                        )
                    } else {
                        Cell::from("")
                    }
```

Second call site (the normal-row branch, currently around line 363):

```rust
                .map(|(i, col)| {
                    render_cell(
                        col,
                        wt,
                        vals,
                        state.tick,
                        state.live.cfg.stat,
                        assigned_widths[i],
                        |fs| state.live.is_cell_loading(row_idx, fs),
                        |fs| state.live.is_cell_unloaded(row_idx, fs),
                    )
                })
```

- [ ] **Step 6: If Task 3 added `#[allow(dead_code)]` to `not_loaded_cell`,
      remove it now**

The function now has callers (every loadable column branch). Search:

```bash
grep -n "allow(dead_code)" src/output/tui/render.rs
```

If a `#[allow(dead_code)]` line precedes `fn not_loaded_cell`, delete that line.

- [ ] **Step 7: Run new tests to verify they pass**

```bash
cargo test --lib --quiet output::tui::render::tests::render_cell_uses 2>&1 | tail -15
```

Expected: 2 passed (`render_cell_uses_not_loaded_when_cancelled_and_unfilled` +
`render_cell_uses_value_when_received_even_if_cancelled`).

- [ ] **Step 8: Run full test suite to confirm no regressions**

```bash
mise run test:unit 2>&1 | tail -10
```

Expected: all tests pass. Watch for any `output::tui::render::tests::*` test
that was using a state with `cancelled = false` — it should still pass because
`is_cell_unloaded` returns false in that state.

- [ ] **Step 9: Lint and format**

```bash
mise run fmt && mise run clippy 2>&1 | tail -10
```

Expected: zero clippy warnings.

- [ ] **Step 10: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "$(cat <<'EOF'
feat(tui): wire is_cell_unloaded through render_cell for cancelled cells (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Append "cancelled" suffix to the verbose footer

**Files:**

- Modify: `src/output/tui/render.rs`

- [ ] **Step 1: Write the failing tests**

Append these tests inside the `mod tests` block of `src/output/tui/render.rs`:

```rust
    #[test]
    fn render_footer_appends_cancelled_when_live_cancelled() {
        let mut state = make_test_state(1);
        state.render_start_elapsed = std::time::Duration::from_millis(1234);
        state.live.mark_cancelled();
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_footer(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(row.contains("cancelled"), "row was: {row:?}");
        assert!(row.contains("inflight:"), "row was: {row:?}");
    }

    #[test]
    fn render_footer_no_cancelled_suffix_when_not_cancelled() {
        let mut state = make_test_state(1);
        state.render_start_elapsed = std::time::Duration::from_millis(1234);
        // NOT calling mark_cancelled.
        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_footer(&state, frame, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect();
        assert!(!row.contains("cancelled"), "row was: {row:?}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test --lib --quiet output::tui::render::tests::render_footer_ 2>&1 | tail -15
```

Expected: `render_footer_appends_cancelled_when_live_cancelled` fails (no
"cancelled" in output). `render_footer_no_cancelled_suffix_when_not_cancelled`
passes (already true).

- [ ] **Step 3: Update `render_footer` to append the suffix**

In `src/output/tui/render.rs`, modify `render_footer` (currently around lines
91–108). Replace the body with:

```rust
pub fn render_footer(state: &TuiState, frame: &mut Frame, area: Rect) {
    if !state.show_hook_sub_rows {
        return;
    }
    let inflight: usize = state
        .live
        .received_patches
        .iter()
        .filter(|fs| !fs.contains(crate::core::worktree::info_field::FieldSet::ALL))
        .count();
    let elapsed_secs = state.render_start_elapsed.as_secs_f32();
    let mut text = format!(" inflight: {inflight} \u{00B7} elapsed: {elapsed_secs:.1}s");
    if state.live.cancelled {
        text.push_str(" \u{00B7} cancelled");
    }
    let line = Line::from(Span::styled(
        text,
        Style::default().add_modifier(Modifier::DIM),
    ));
    frame.render_widget(Paragraph::new(line), area);
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test --lib --quiet output::tui::render::tests::render_footer_ 2>&1 | tail -10
```

Expected: 4 passed (2 new + 2 existing — `render_footer_no_op_when_not_verbose`,
`render_footer_shows_inflight_and_elapsed_when_verbose`).

- [ ] **Step 5: Run full test suite**

```bash
mise run test:unit 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 6: Lint and format**

```bash
mise run fmt && mise run clippy 2>&1 | tail -10
```

Expected: zero warnings.

- [ ] **Step 7: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "$(cat <<'EOF'
feat(tui): append cancelled suffix to verbose footer (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Driver Ctrl-C arm calls `mark_cancelled`

**Files:**

- Modify: `src/output/tui/driver.rs`

- [ ] **Step 1: Write the failing test**

Append this test inside the `mod tests` block at the bottom of
`src/output/tui/driver.rs`. It mirrors the existing
`cancel_signal_can_be_set_externally` test (which sets the signal and reads it
back). The new test additionally asserts that calling `mark_cancelled` on the
state's live table gives us the expected post-cancel state.

```rust
    #[test]
    fn mark_cancelled_via_state_flips_live_cancelled() {
        // Direct unit test for the post-Ctrl-C state mutation that the
        // driver's Ctrl-C arm performs. We can't easily synthesize a
        // crossterm Event in a unit test, so we exercise the same
        // mutation path the arm performs.
        let phases = Vec::<crate::core::worktree::sync_dag::OperationPhase>::new();
        let infos = vec![crate::core::worktree::list::WorktreeInfo::empty("a")];
        let mut state = TuiState::new(
            phases,
            infos,
            std::path::PathBuf::from("/tmp"),
            std::path::PathBuf::from("/tmp"),
            crate::core::worktree::list::Stat::Summary,
            0,
            None,
            false,
            None,
            None,
            true,
            false,
        );
        assert!(!state.live.cancelled);
        assert!(!state.live.collection_complete);

        state.live.mark_cancelled();
        state.done = true;

        assert!(state.live.cancelled);
        assert!(state.live.collection_complete);
        assert!(state.done);
    }
```

- [ ] **Step 2: Run test to verify it passes (the methods already exist from
      Tasks 1–2)**

```bash
cargo test --lib --quiet output::tui::driver::tests::mark_cancelled_via_state_flips_live_cancelled 2>&1 | tail -10
```

Expected: PASS. (This is a regression-style guard test for the post-cancel state
combination — it should already work because Tasks 1–2 wired `mark_cancelled`.
Confirms the driver-side wiring path is exercising the right API.)

- [ ] **Step 3: Wire `mark_cancelled` into the Ctrl-C arm**

In `src/output/tui/driver.rs`, the Ctrl-C handling block sits inside the main
`run` loop, currently around lines 251–263. Modify it to add the
`mark_cancelled` call before `state.done = true`:

```rust
            // Poll for keyboard events (Ctrl-C). Non-blocking. If a Ctrl-C
            // is observed, flip the optional cancel signal so the producer
            // exits cooperatively, mark state done and live as cancelled,
            // and emit a final draw.
            if event::poll(Duration::from_millis(0)).unwrap_or(false) {
                if let Ok(Event::Key(key)) = event::read() {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if let Some(sig) = &self.cancel_signal {
                            sig.store(true, Ordering::Relaxed);
                        }
                        self.state.live.mark_cancelled();
                        self.state.done = true;
                        final_draw_and_return!();
                    }
                }
            }
```

- [ ] **Step 4: Run full test suite to confirm no regressions**

```bash
mise run test:unit 2>&1 | tail -10
```

Expected: all tests pass, including the new
`mark_cancelled_via_state_flips_live_cancelled`.

- [ ] **Step 5: Lint and format**

```bash
mise run fmt && mise run clippy 2>&1 | tail -10
```

Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/driver.rs
git commit -m "$(cat <<'EOF'
feat(tui): mark live table cancelled in driver Ctrl-C handler (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Write trailing newline to stderr after cancelled teardown

This task adds a single `\n` write to stderr after `drop(terminal)` so the shell
prompt lands on its own line. Best-effort, ignored failure. Only runs when
`state.live.cancelled` is true to avoid adding a blank line on normal
completion.

**Files:**

- Modify: `src/output/tui/driver.rs`

- [ ] **Step 1: Add `std::io::Write` import**

In `src/output/tui/driver.rs`, the existing imports near the top of the file
(currently lines 1–13) do not include `std::io::Write`. Add it. Insert this line
after the existing `std::sync::Arc;` import (alphabetical-ish ordering,
alongside other `std::` imports):

```rust
use std::io::Write;
```

- [ ] **Step 2: Update the `final_draw_and_return!` macro**

In `src/output/tui/driver.rs`, modify the `final_draw_and_return!` macro
(currently around lines 178–205). Add a single conditional write to stderr after
`drop(terminal)`. Replace the macro body with:

```rust
        macro_rules! final_draw_and_return {
            () => {{
                let total_rows = self.total_rendered_rows();
                terminal.draw(|frame| {
                    let area = frame.area();
                    let chunks = Layout::vertical([
                        Constraint::Length(header_height),
                        Constraint::Fill(1),
                        Constraint::Length(footer_height),
                    ])
                    .split(area);
                    render::render_header(&self.state, frame, chunks[0]);
                    render::render_table(&self.state, frame, chunks[1]);
                    render::render_footer(&self.state, frame, chunks[2]);

                    // sort summary rows (when present) + table header (1 row)
                    // + data rows (including hook sub-rows / size summary).
                    let content_bottom =
                        area.y + header_height + sort_rows + 1 + total_rows + footer_height;
                    frame.set_cursor_position(Position {
                        x: 0,
                        y: content_bottom,
                    });
                })?;
                drop(terminal);
                if self.state.live.cancelled {
                    // After a cancelled run the cursor can land inside the
                    // last table row (terminal-height clamping of the inline
                    // viewport). Emit a newline so the shell prompt starts on
                    // a fresh line. Best-effort.
                    let _ = std::io::stderr().write_all(b"\n");
                }
                return Ok(self.state);
            }};
        }
```

- [ ] **Step 3: Build to confirm compilation**

```bash
cargo build --quiet 2>&1 | tail -10
```

Expected: clean build, no warnings.

- [ ] **Step 4: Run full test suite**

```bash
mise run test:unit 2>&1 | tail -10
```

Expected: all tests pass.

- [ ] **Step 5: Lint and format**

```bash
mise run fmt && mise run clippy 2>&1 | tail -10
```

Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/driver.rs
git commit -m "$(cat <<'EOF'
feat(tui): write trailing newline after cancelled viewport teardown (#402)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Manual smoke verification

This is a non-TDD verification task. Test in a sandbox repo with multiple
worktrees so the live UX has cells in flight when Ctrl-C lands.

**Files:** none modified.

- [ ] **Step 1: Open the existing daft sandbox in a fresh terminal**

Use the project's sandbox at `/Users/avihu/Projects/daft/.git/.daft-sandbox`
(created by `daft sandbox init`). If a sandbox is not already initialized for
this branch, create one:

```bash
cd /Users/avihu/Projects/daft/daft-402/feat/live-list-population
mise run dev
# In a new shell:
DAFT_CONFIG_DIR=/Users/avihu/Projects/daft/.git/.daft-sandbox daft list --columns=-path,+size --sort activity --stat lines
```

Hit Ctrl-C while the `▬` skeleton bars are still pulsing (within the first
second).

Expected:

- All cells whose patches hadn't arrived render as a dim em-dash `—`, NOT as
  frozen `▬▬▬▬`.
- Already-loaded cells (Branch, Annotation, Status, plus any cell that finished
  before cancel) show their real values.
- The shell prompt returns on its own line — no `^C%` overlap with the master
  row's content.

- [ ] **Step 2: Verify the cancelled-suffix behavior in verbose mode**

Run with verbose flag:

```bash
DAFT_CONFIG_DIR=/Users/avihu/Projects/daft/.git/.daft-sandbox daft list -v --columns=-path,+size --sort activity --stat lines
```

Hit Ctrl-C. Expected: footer shows ` · cancelled` appended after
`inflight: N · elapsed: T.Xs`.

- [ ] **Step 3: Verify normal completion is unchanged**

Let `daft list` run to completion (do not hit Ctrl-C). Expected:

- All cells render their real values; no `—` markers anywhere.
- Footer (in verbose mode) does NOT contain "cancelled".
- Shell prompt returns on its own line as before.

- [ ] **Step 4: Run the full integration suite as a regression guard**

```bash
mise run test:integration 2>&1 | tail -20
```

Expected: all integration tests pass. (Long-running; ~20 min.)

- [ ] **Step 5: If everything passes, no commit needed for this task**

This is verification only.

---

## Spec Coverage Self-Review

| Spec section / requirement                                                              | Implemented in           |
| --------------------------------------------------------------------------------------- | ------------------------ |
| `LiveTable.cancelled` field, default false                                              | Task 1                   |
| `mark_cancelled()` sets `cancelled`, `collection_complete`, `pending_resort`            | Task 1                   |
| `is_cell_unloaded(row, field)` predicate                                                | Task 2                   |
| `is_cell_loading` returns false post-cancel via `collection_complete`                   | Task 2 (regression test) |
| `not_loaded_cell()` helper renders dim em-dash                                          | Task 3                   |
| `render_cell` accepts `is_cell_unloaded` closure parameter                              | Task 4                   |
| Each loadable column branch checks `is_cell_unloaded` before `is_cell_loading`          | Task 4 (8 column arms)   |
| Both `render_table` call sites pass the new closure                                     | Task 4                   |
| `render_footer` appends ` · cancelled` when `live.cancelled`                            | Task 5                   |
| Driver Ctrl-C arm calls `state.live.mark_cancelled()`                                   | Task 6                   |
| Driver writes `\n` to stderr after `drop(terminal)` when cancelled                      | Task 7                   |
| Manual smoke test (clean prompt, em-dashes, footer suffix, normal completion unchanged) | Task 8                   |

No gaps. Out-of-scope items from the spec (prune/sync polish, killing in-flight
git children, cursor-positioning rework) are deliberately not present in any
task.
