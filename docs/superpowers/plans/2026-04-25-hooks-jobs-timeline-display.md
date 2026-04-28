# Hooks Jobs Timeline Display Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Re-render the `daft hooks jobs` listing as a vertical-spine timeline.
Each worktree owns a continuous `│` spine; each invocation appears as a `●`
bullet node pinned to that spine; jobs hang from each node along the spine.
Replaces the current flat header + table layout. The spec is
`docs/superpowers/specs/2026-04-10-hooks-jobs-redesign.md` (Display Format +
Rendering Details sections, already updated).

**Architecture:** Pure-function composition helpers each produce one styled line
of the listing. `list_jobs` becomes a thin orchestrator that calls helpers in
sequence and emits each line via `output.info`. The inner job table is still
rendered with `tabled::Builder` + `Style::blank()`, but its rendered string is
split on `\n` and each line is prefixed with the spine column (`"  │     "`)
before being emitted. No new files; all helpers stay private to
`src/commands/hooks/jobs.rs`.

**Tech Stack:** Rust, `tabled` for the inner table, existing ANSI helpers from
`src/styles.rs` (`dim`, `bold`, `cyan`, `BOLD`, `CYAN`, `RESET`,
`CURRENT_WORKTREE_SYMBOL`).

---

## File Structure

| File                                       | Responsibility                         | Change                                                     |
| ------------------------------------------ | -------------------------------------- | ---------------------------------------------------------- |
| `src/commands/hooks/jobs.rs`               | Owns `list_jobs` and emits the listing | Add 5 helpers + refactor the rendering body of `list_jobs` |
| `src/commands/hooks/jobs.rs` (`mod tests`) | Unit tests                             | Add 5 tests for the helpers                                |

No new files. No public API change. JSON / structured output path
(`args.emit.is_structured()`) is untouched — only the human-readable rendering
changes.

---

## Decisions

- **Helpers are pure**: each takes `&str` inputs and returns a styled `String`.
  Tests compose expected output using the same `dim()` primitive — no ANSI
  stripping needed.
- **Spine glyphs are dim**: `dim("│")` and `dim("●")`. The spine is a grouping
  cue, not primary content.
- **Inner table stays `tabled`**: only the framing changes. We split the
  rendered table on `\n` and prefix each line (including the header row and any
  blank lines `tabled` emits) with `"  " + dim("│") + "     "` (5-space gutter
  inside the spine column).
- **Worktree separator**: blank line (no spine) between worktrees. Worktrees are
  parallel timelines, not one continuous stream.
- **Inter-invocation separator**: a single `dim("│")` line between adjacent
  invocations within a worktree. Spine is continuous within a worktree, breaks
  at the worktree boundary.
- **First node within a worktree**: preceded by a blank line (no spine) between
  the worktree header and the first node, mirroring the mockup. Subsequent nodes
  are preceded by a `dim("│")` line.
- **Termination**: the spine ends at the last job's row of the last invocation.
  No trailing terminator glyph.
- **`shorthand_from_seconds` returns `"7h"` (no "ago" suffix)**. The new node
  helper appends `" ago"` so the rendered text reads `"7h ago"`, matching the
  spec mockup.
- **First-column padding override is dropped**: existing code calls
  `table.modify(Columns::first(), Padding::new(2, 1, 0, 0))` to inset the Job
  column. The new spine gutter (5 spaces) replaces that inset, so this `modify`
  line is removed.

## Out of scope

- The wider redesign tracked in
  `docs/superpowers/specs/2026-04-10-hooks-jobs-redesign.md` beyond Display
  Format + Rendering Details (job addressing, completions, `--inv`).
- The `show` single-job view (`render_single_job_log`) and the `--inv`
  invocation-logs view (`render_invocation_logs`). They have their own framing.
- JSON / structured output, completions, docs/cli refresh — none of these encode
  the listing format.

---

### Task 1: Add timeline composition helpers + tests

**Files:**

- Modify: `src/commands/hooks/jobs.rs` — insert helpers after `format_duration`
  (before `KNOWN_HOOK_TYPES` at line 306).
- Test: `src/commands/hooks/jobs.rs` — append 5 tests inside the existing
  `mod tests` block at line 1505.

- [ ] **Step 1: Write the failing tests**

Append the following to the `mod tests` block in `src/commands/hooks/jobs.rs`
(just before the closing `}` of the test module). The `BOLD`, `CYAN`, `RESET`,
and `dim` symbols are already in scope via `use super::*;`.

```rust
#[test]
fn worktree_header_renders_marker_then_space_then_name() {
    assert_eq!(
        worktree_header(">", "feature/tax-calc"),
        format!("{BOLD}{CYAN}> feature/tax-calc{RESET}"),
    );
    // Non-current worktrees pass " " as the marker → two leading spaces.
    assert_eq!(
        worktree_header(" ", "main"),
        format!("{BOLD}{CYAN}  main{RESET}"),
    );
}

#[test]
fn invocation_node_line_appends_ago_and_dims_node_and_bracket() {
    assert_eq!(
        invocation_node_line("2h", "worktree-post-create", "c9d4"),
        format!(
            "  {}  {} · worktree-post-create {}",
            dim("●"),
            dim("2h ago"),
            dim("[c9d4]"),
        ),
    );
}

#[test]
fn spine_blank_is_two_spaces_then_dim_pipe() {
    assert_eq!(spine_blank(), format!("  {}", dim("│")));
}

#[test]
fn spine_prefixed_inserts_pipe_and_five_space_gutter() {
    assert_eq!(
        spine_prefixed("Job   Status   Started"),
        format!("  {}     Job   Status   Started", dim("│")),
    );
}

#[test]
fn empty_invocation_placeholder_is_dimmed_under_spine() {
    assert_eq!(
        empty_invocation_placeholder(),
        format!("  {}     {}", dim("│"), dim("(no jobs declared)")),
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p daft --lib commands::hooks::jobs::tests::worktree_header_renders_marker_then_space_then_name
```

Expected: FAIL — `cannot find function 'worktree_header'`.

- [ ] **Step 3: Add the helpers**

Insert into `src/commands/hooks/jobs.rs` immediately after `format_duration`
(around line 70, before `KNOWN_HOOK_TYPES`):

```rust
/// One-line worktree header. The marker is `CURRENT_WORKTREE_SYMBOL` (`">"`)
/// for the current worktree; non-current worktrees pass a single space so
/// the worktree-name column lines up across both.
fn worktree_header(marker: &str, name: &str) -> String {
    format!("{BOLD}{CYAN}{marker} {name}{RESET}")
}

/// One-line invocation header pinned to the spine as a bullet node.
/// `time_ago` is the bare relative duration (e.g. `"2h"`); the helper
/// appends `" ago"` so the rendered text reads `"2h ago"`.
fn invocation_node_line(time_ago: &str, trigger: &str, short_id: &str) -> String {
    format!(
        "  {}  {} · {trigger} {}",
        dim("●"),
        dim(&format!("{time_ago} ago")),
        dim(&format!("[{short_id}]")),
    )
}

/// Spine-only line: emits the `│` glyph in dim with no inner content.
/// Used between adjacent invocation nodes within a worktree, and between
/// an invocation node and the table that hangs from it.
fn spine_blank() -> String {
    format!("  {}", dim("│"))
}

/// Prefix one inner-table content line with the spine + a 5-space gutter.
fn spine_prefixed(content: &str) -> String {
    format!("  {}     {content}", dim("│"))
}

/// Placeholder rendered when an invocation has no jobs.
fn empty_invocation_placeholder() -> String {
    format!("  {}     {}", dim("│"), dim("(no jobs declared)"))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p daft --lib commands::hooks::jobs::tests
```

Expected: PASS — all 5 new tests + every existing test in the module.

- [ ] **Step 5: Run clippy + fmt**

```bash
mise run clippy && mise run fmt:check
```

Expected: zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(hooks): add timeline composition helpers for jobs listing"
```

---

### Task 2: Refactor `list_jobs` rendering body to emit timeline format

**Files:**

- Modify: `src/commands/hooks/jobs.rs` — replace the rendering body inside
  `list_jobs` (the worktree loop currently spanning lines 631–734).

- [ ] **Step 1: Replace the rendering body**

In `src/commands/hooks/jobs.rs`, find the block beginning with
`// Group invocations by worktree.` (currently around line 631) and ending at
the closing `}` of the outer `for (worktree, inv_list) in &groups` loop
(currently around line 734, just before
`if args.all { ... print_log_clean_footer ... }`).

Replace that block with:

```rust
    // Group invocations by worktree.
    let mut groups: std::collections::BTreeMap<String, Vec<&InvocationMeta>> =
        std::collections::BTreeMap::new();
    for inv in &invocations {
        groups.entry(inv.worktree.clone()).or_default().push(inv);
    }

    let now = chrono::Utc::now();
    let mut first_group = true;

    for (worktree, inv_list) in &groups {
        if !first_group {
            output.info("");
        }
        let marker = if worktree == &current_worktree {
            CURRENT_WORKTREE_SYMBOL
        } else {
            " "
        };
        output.info(&worktree_header(marker, worktree));
        first_group = false;

        for (i, inv) in inv_list.iter().enumerate() {
            // Separator before this node: blank line (no spine) between
            // worktree header and the first node; dim spine line between
            // adjacent nodes within the same worktree.
            if i == 0 {
                output.info("");
            } else {
                output.info(&spine_blank());
            }

            let ago =
                shorthand_from_seconds(now.signed_duration_since(inv.created_at).num_seconds());
            let short_id = &inv.invocation_id[..4.min(inv.invocation_id.len())];

            output.info(&invocation_node_line(&ago, &inv.trigger_command, short_id));
            // Spine breathes between the node and the table.
            output.info(&spine_blank());

            let job_dirs = store.list_jobs_in_invocation(&inv.invocation_id)?;
            if job_dirs.is_empty() {
                output.info(&empty_invocation_placeholder());
                continue;
            }

            let mut builder = Builder::new();
            builder.push_record(vec![
                dim_underline("Job"),
                dim_underline("Status"),
                dim_underline("Started"),
                dim_underline("Duration"),
                dim_underline("Size"),
            ]);

            for dir in &job_dirs {
                if let Ok(meta) = store.read_meta(dir) {
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
                    builder.push_record(vec![job_label, status, started, duration, size_str]);
                }
            }

            let mut table = builder.build();
            table.with(Style::blank());
            // Note: the previous first-column left-padding override is dropped
            // because the spine gutter (5 spaces) supplies the inset.

            for line in table.to_string().lines() {
                output.info(&spine_prefixed(line));
            }
        }
    }
```

- [ ] **Step 2: Strip now-unused imports**

After the refactor, `Columns` and `Padding` may no longer be used inside
`list_jobs`. Check the `use tabled::...` block at the top of the file and remove
any tabled imports that are no longer referenced anywhere in the module. Run
`mise run clippy` to surface unused-import warnings.

- [ ] **Step 3: Run unit tests**

```bash
mise run test:unit
```

Expected: PASS — no unit tests assert on `list_jobs` output directly, so the
refactor only needs the test count to remain green.

- [ ] **Step 4: Run clippy + fmt**

```bash
mise run clippy && mise run fmt:check
```

Expected: zero warnings.

- [ ] **Step 5: Manual visual smoke**

Build a debug binary and run against a sandbox repo with at least two
invocations:

```bash
cargo build -p daft
# In a sandbox with hook history:
./target/debug/daft hooks jobs
./target/debug/daft hooks jobs --all
```

Expected output shape (compare against
`docs/superpowers/specs/2026-04-10-hooks-jobs-redesign.md` Display Format
section):

- Worktree header (`> name` or `  name`) followed by a blank line.
- For each invocation: `●` node line, `│` separator, table rows each prefixed
  with `│     `.
- Between two invocations: `│` separator line.
- Between two worktrees: blank line (no spine).
- After the last invocation's last table row: nothing — spine terminates
  naturally.

- [ ] **Step 6: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "feat(hooks): render jobs listing as vertical-spine timeline"
```

---

### Task 3: Verify integration scenarios + adapt any breakage

**Files:**

- Verify: `tests/manual/scenarios/hooks/*.yml`
- Modify (only if a scenario fails): the affected `output_contains` /
  `output_not_contains` lines.

- [ ] **Step 1: Run the hooks scenario suite**

```bash
mise run test:manual -- --ci hooks
```

Expected: PASS. The existing `output_contains` asserts only check substrings
(job names, hook type names, status words, `(no jobs declared)`); none of them
encode the spine glyph or the `--`/`·` separator. They should keep passing under
the new format.

- [ ] **Step 2: If anything failed, adapt the assert**

If a scenario fails because an assert relied on flat-table layout (a literal
`--` separator, exact column spacing, or any other format detail that changed):

1. Identify the exact `output_contains` / `output_not_contains` line that broke.
2. Replace the assertion with one that targets the same semantic property (e.g.
   "the trigger command appears", "the job name appears") under the new format.
3. Re-run `mise run test:manual -- --ci hooks` to confirm green.
4. Commit:

```bash
git add tests/manual/scenarios/hooks/<file>.yml
git commit -m "test(hooks): adapt listing scenarios to timeline format"
```

If everything passes on the first run, skip the commit — there is nothing to
change.

---

## Verification (run before declaring complete)

1. `mise run test:unit` — green.
2. `mise run clippy` — zero warnings.
3. `mise run fmt:check` — clean.
4. `mise run test:manual -- --ci hooks` — green.
5. Manual visual: `daft hooks jobs` and `daft hooks jobs --all` in a sandbox
   match the spec mockups in
   `docs/superpowers/specs/2026-04-10-hooks-jobs-redesign.md`.

---

## LOC Estimate

| Bucket                           | Lines    |
| -------------------------------- | -------- |
| Helpers (5 functions)            | ~35      |
| Helper tests                     | ~50      |
| `list_jobs` refactor (net delta) | ~+15     |
| Total                            | ~100 LOC |
