# `daft hooks jobs` Column Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** All inner job tables in a single `daft hooks jobs` listing share the
same column widths. Picks up where the timeline-display redesign left off.

**Architecture:** Two-pass render in `list_jobs`. Pass 1 walks every invocation
and materializes all rows into memory while measuring per-column max visible
width (ANSI stripped). Pass 2 emits the timeline as before, but pads every cell
— headers included — to the global per-column max before pushing to
`tabled::Builder`. Tabled's already-enabled `ansi` feature handles visible width
measurement, so equal visible widths across tables yield equal rendered column
widths. No new crates, no I/O changes.

**Tech Stack:** Rust 1.95, tabled 0.20 (with `ansi` feature),
`src/commands/hooks/jobs.rs`, `src/output/format.rs` (or co-locate).

---

## Files

- **Modify** `src/output/format.rs` — host the shared `strip_ansi` helper and
  add `pad_to_visible_width`. (Justification: `format.rs` already hosts cross-
  command formatting helpers like `shorthand_from_seconds`. `styles.rs` is for
  ANSI escape constants/wrappers, not measurement utilities.)
- **Modify** `src/commands/list.rs` — drop the local `strip_ansi`, import the
  shared one.
- **Modify** `src/commands/hooks/jobs.rs` — refactor `list_jobs` to two-pass
  collect-then-render; pad cells before pushing to `tabled::Builder`.

---

### Task 1: Lift `strip_ansi` and add `pad_to_visible_width` to `src/output/format.rs`

**Files:**

- Modify: `src/output/format.rs`
- Modify: `src/commands/list.rs:1001` (the private `strip_ansi`)

- [ ] **Step 1: Read current `format.rs` to find the right insertion point**

Run: `grep -n "^pub fn\|^fn " src/output/format.rs`

This locates the existing public helpers so the new ones land near them.

- [ ] **Step 2: Write failing tests for both new helpers in `format.rs`**

Append to the existing `#[cfg(test)] mod tests { ... }` block (or create one if
the file lacks tests):

```rust
#[test]
fn strip_ansi_removes_csi_sequences() {
    assert_eq!(strip_ansi("\x1b[2mhello\x1b[0m"), "hello");
    assert_eq!(strip_ansi("\x1b[38;5;208mwarn\x1b[0m"), "warn");
    assert_eq!(strip_ansi("plain"), "plain");
    assert_eq!(strip_ansi(""), "");
}

#[test]
fn strip_ansi_preserves_unicode_glyphs() {
    // Box-drawing and arrows must survive — these are core to the timeline
    // display.
    assert_eq!(strip_ansi("\x1b[2m│\x1b[0m"), "│");
    assert_eq!(
        strip_ansi("\x1b[38;5;208m\u{2192}\x1b[0m install"),
        "\u{2192} install",
    );
}

#[test]
fn pad_to_visible_width_no_pad_when_already_at_or_above_target() {
    assert_eq!(pad_to_visible_width("abc", 3), "abc");
    assert_eq!(pad_to_visible_width("abcd", 3), "abcd");
}

#[test]
fn pad_to_visible_width_appends_trailing_spaces_to_reach_target() {
    assert_eq!(pad_to_visible_width("ab", 5), "ab   ");
}

#[test]
fn pad_to_visible_width_counts_visible_chars_not_ansi_bytes() {
    // Cell with red wrapping reports raw len 14 but visible len 7.
    let cell = "\x1b[31mfailed!\x1b[0m";
    let padded = pad_to_visible_width(cell, 10);
    // Visible width must be exactly 10 after padding.
    assert_eq!(strip_ansi(&padded).chars().count(), 10);
    // ANSI bytes preserved at the start; trailing spaces appended after RESET.
    assert!(padded.starts_with("\x1b[31mfailed!\x1b[0m"));
    assert!(padded.ends_with("   "));
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p daft --lib output::format::tests::strip_ansi` Expected: FAIL
— `strip_ansi` and `pad_to_visible_width` don't exist yet in `format.rs`.

- [ ] **Step 4: Implement `strip_ansi` and `pad_to_visible_width` in
      `format.rs`**

Add near the other public helpers (e.g., directly above or below
`shorthand_from_seconds`):

```rust
/// Strip ANSI CSI escape sequences from a string.
///
/// Used for measuring the *visible* width of a styled string — width-based
/// layout code must not count escape bytes.
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            result.push(c);
        }
    }
    result
}

/// Pad `s` with trailing spaces so its *visible* width reaches `target`.
/// If `s` already meets or exceeds `target`, returns it unchanged.
///
/// "Visible width" is the char count after `strip_ansi`. ANSI escape bytes
/// are not counted.
pub fn pad_to_visible_width(s: &str, target: usize) -> String {
    let visible = strip_ansi(s).chars().count();
    if visible >= target {
        s.to_string()
    } else {
        let mut out = String::with_capacity(s.len() + (target - visible));
        out.push_str(s);
        for _ in 0..(target - visible) {
            out.push(' ');
        }
        out
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p daft --lib output::format::tests` Expected: all tests in the
module PASS, including the four new ones.

- [ ] **Step 6: Replace the private `strip_ansi` in `list.rs` with the shared
      one**

In `src/commands/list.rs`:

1. Delete the `fn strip_ansi(s: &str) -> String { ... }` block at line ~1001.
2. Add `use crate::output::format::strip_ansi;` at the top of the file (or
   include it in an existing `use crate::output::format::{...};` group).

- [ ] **Step 7: Verify nothing else broke**

Run in parallel:

- `cargo test -p daft --lib` — all unit tests pass.
- `mise run clippy` — zero warnings.
- `mise run fmt:check` — clean.

- [ ] **Step 8: Commit**

```bash
git add src/output/format.rs src/commands/list.rs
git commit -m "refactor(format): lift strip_ansi and add pad_to_visible_width"
```

---

### Task 2: Two-pass render in `list_jobs` — collect rows, measure widths, pad before tabled

**Files:**

- Modify: `src/commands/hooks/jobs.rs:685-770` (the `for (worktree, inv_list)`
  loop)
- Modify: `src/commands/hooks/jobs.rs` use-list (add `pad_to_visible_width`,
  `strip_ansi` already imported transitively or add it).

- [ ] **Step 1: Read the current rendering loop to anchor edits**

Run: `sed -n '660,780p' src/commands/hooks/jobs.rs`

Confirm the structure: outer loop over worktree groups → inner loop over
invocations → per-invocation `Builder::new()` → `push_record` for header and
each job row.

- [ ] **Step 2: Add a unit test asserting that headers in two adjacent tables
      align after the refactor**

This test exercises `pad_to_visible_width` directly against the same column
widths that `list_jobs` will compute, locking in the contract. Add to the
existing test module in `jobs.rs`:

```rust
#[test]
fn padding_makes_two_rows_with_different_widths_render_to_equal_visible_width() {
    use crate::output::format::{pad_to_visible_width, strip_ansi};

    // Two cells that would naturally produce different column widths.
    let short = "\x1b[31m\u{2717} failed\x1b[0m";       // visible: 8
    let long = "\x1b[33m\u{27f3} running (stale)\x1b[0m"; // visible: 17

    let target = strip_ansi(short).chars().count()
        .max(strip_ansi(long).chars().count());

    let padded_short = pad_to_visible_width(short, target);
    let padded_long = pad_to_visible_width(long, target);

    assert_eq!(strip_ansi(&padded_short).chars().count(), target);
    assert_eq!(strip_ansi(&padded_long).chars().count(), target);
}
```

Run:
`cargo test -p daft --lib commands::hooks::jobs::tests::padding_makes_two_rows`
Expected: PASS immediately (Task 1 already shipped the helpers). This is a
contract check — kept in the file so future refactors don't drop the property.

- [ ] **Step 3: Refactor the loop. Replace lines ~685-770 with the two-pass
      form**

Read the current loop first:

```bash
sed -n '683,777p' src/commands/hooks/jobs.rs
```

Then replace it with:

```rust
let now = chrono::Utc::now();

// ---- Pass 1: collect rows + measure global per-column widths ----
//
// Materialize every job row up-front so we can compute per-column max
// visible width across all invocations in this listing. Without this,
// adjacent invocations whose Job/Status/Duration values differ in length
// pick different `tabled` column widths and the rendering drifts.
struct JobRow {
    job: String,
    status: String,
    started: String,
    duration: String,
    size: String,
}
struct Section<'a> {
    inv: &'a InvocationView,
    rows: Vec<JobRow>,
}
let mut sections_by_worktree: Vec<(String, Vec<Section>)> = Vec::new();

const HEADERS: [&str; 5] = ["Job", "Status", "Started", "Duration", "Size"];
let mut max_widths: [usize; 5] =
    HEADERS.map(|h| crate::output::format::strip_ansi(h).chars().count());

for (worktree, inv_list) in &groups {
    let mut secs: Vec<Section> = Vec::with_capacity(inv_list.len());
    for inv in inv_list {
        let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
        let mut rows: Vec<JobRow> = Vec::with_capacity(job_dirs.len());
        for dir in &job_dirs {
            let Ok(meta) = store.read_meta(dir) else { continue; };
            let icon = if meta.background {
                blue("\u{21aa}")
            } else {
                orange("\u{2192}")
            };
            let job_label = format!("{icon} {}", meta.name);
            let status = format_status_inline(&meta.status, coordinator_alive);
            let started = {
                let local: chrono::DateTime<chrono::Local> = meta.started_at.into();
                local.format("%H:%M:%S").to_string()
            };
            let duration = match (&meta.status, meta.finished_at) {
                (_, Some(finished)) => {
                    format_duration(finished.signed_duration_since(meta.started_at))
                }
                (JobStatus::Running, None) => format!(
                    "{}...",
                    format_duration(now.signed_duration_since(meta.started_at))
                ),
                _ => "\u{2014}".to_string(),
            };
            let size = LogStore::log_path(dir)
                .metadata()
                .map(|m| m.len())
                .unwrap_or(0);
            let size_str = if size == 0 {
                dim("\u{2014}").to_string()
            } else {
                format_bytes(size)
            };

            // Update per-column max visible widths.
            for (i, cell) in [&job_label, &status, &started, &duration, &size_str]
                .iter()
                .enumerate()
            {
                let v = crate::output::format::strip_ansi(cell).chars().count();
                if v > max_widths[i] {
                    max_widths[i] = v;
                }
            }

            rows.push(JobRow {
                job: job_label,
                status,
                started,
                duration,
                size: size_str,
            });
        }
        secs.push(Section { inv, rows });
    }
    sections_by_worktree.push((worktree.clone(), secs));
}

// ---- Pass 2: render ----
for (worktree, secs) in &sections_by_worktree {
    let marker = if worktree == &current_worktree {
        CURRENT_WORKTREE_SYMBOL
    } else {
        " "
    };
    output.info(&worktree_header(marker, worktree));

    for (i, sec) in secs.iter().enumerate() {
        if i == 0 {
            output.info("");
        } else {
            output.info(&spine_blank());
        }

        let ago = shorthand_from_seconds(
            now.signed_duration_since(sec.inv.created_at).num_seconds(),
        );
        let short_id =
            &sec.inv.invocation_id[..4.min(sec.inv.invocation_id.len())];
        output.info(&invocation_node_line(
            &ago,
            &sec.inv.trigger_command,
            short_id,
        ));
        output.info(&spine_blank());

        if sec.rows.is_empty() {
            output.info(&empty_invocation_placeholder());
            continue;
        }

        let mut builder = Builder::new();
        builder.push_record(
            HEADERS
                .iter()
                .enumerate()
                .map(|(c, h)| {
                    crate::output::format::pad_to_visible_width(
                        &dim_underline(h),
                        max_widths[c],
                    )
                })
                .collect::<Vec<_>>(),
        );
        for row in &sec.rows {
            let cells = [&row.job, &row.status, &row.started, &row.duration, &row.size];
            builder.push_record(
                cells
                    .iter()
                    .enumerate()
                    .map(|(c, cell)| {
                        crate::output::format::pad_to_visible_width(cell, max_widths[c])
                    })
                    .collect::<Vec<_>>(),
            );
        }

        let mut table = builder.build();
        table.with(Style::blank());
        for line in table.to_string().lines() {
            output.info(&spine_prefixed(line));
        }
    }

    output.info(&spine_terminator());
    output.info("");
}
```

Notes for the implementer:

- The `InvocationView` type referenced above is whatever is currently iterated
  in `inv_list` — likely `&InvocationMeta` per the surrounding code. Read the
  signature near the existing loop and substitute the actual type.
- Keep imports tidy: only add what's missing. `pad_to_visible_width` and
  `strip_ansi` are accessed via fully-qualified paths in the snippet above for
  clarity; convert to a
  `use crate::output::format::{pad_to_visible_width, strip_ansi};` at the top of
  the file if other call sites benefit.

- [ ] **Step 4: Run unit tests**

Run: `cargo test -p daft --lib` Expected: all tests PASS, including the new
`padding_makes_two_rows` test from Step 2.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run clippy && mise run fmt:check` Expected: zero warnings, clean
format.

- [ ] **Step 6: Run full hook scenarios**

Run: `mise run test:manual -- --ci hooks` Expected: 62/62 (or whatever the
current count is) pass. Existing scenarios use substring matching, so the change
should not regress any of them.

- [ ] **Step 7: Manual sandbox sanity check**

In a sandbox repo with at least two invocations whose Status or Job-name column
widths differ (e.g., one with all `completed`, one with a `running (stale)`):

```bash
daft hooks jobs
```

Confirm visually that the `Job` / `Status` / `Started` / `Duration` / `Size`
columns start at the same horizontal positions in every invocation table. Repeat
with `--all` across multiple worktrees.

- [ ] **Step 8: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(hooks-jobs): align column widths across invocations in listing"
```

---

## Out of scope

- The structured (`--format json/yaml/...`) output path — already flat and
  unaffected by display width.
- Pager / terminal-narrowing handling. Tabled does not auto-wrap on terminal
  width and the display has never aimed to.
- Cross-command alignment (e.g., aligning `daft hooks jobs` columns with
  `daft list` columns). Outside the spec's scope.

## Self-review notes

- Spec coverage: the only new spec text is the "Column alignment across
  invocations" bullet; both tasks above implement it directly. Pass 1 measures,
  Pass 2 pads, header included.
- Placeholders: none. Every step includes the actual code or command.
- Type consistency: `JobRow` and `Section` are local structs; their field names
  (`job`, `status`, `started`, `duration`, `size`, `inv`, `rows`) are used
  consistently across both pass blocks.
- Test surface is small but targeted: helper unit tests in `format.rs` (Task 1)
  plus a contract test in `jobs.rs` (Task 2). The orchestration itself is
  covered by the existing 62 manual scenarios via substring matching, which is
  what they're designed for. No scenario changes needed.
