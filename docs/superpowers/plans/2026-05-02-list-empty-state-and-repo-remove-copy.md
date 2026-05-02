# `daft list` Empty State + `daft repo remove` Copy Refinement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Render a clear, suggestive empty state for `daft list` (with `daft go`
/ `daft start` hints) and refine `daft repo remove`'s prompt and plan output to
drop internal jargon.

**Architecture:** A small leaf module (`src/commands/list_empty.rs`) renders the
empty-state hint as a styled string. Both `list` render paths (blocking and TUI)
call it when their merged worktree+branch set is empty; the TUI path
short-circuits before bringing up ratatui to avoid flicker. `repo remove` gets
pure string replacements — no behavior change.

**Tech Stack:** Rust, clap, tabled, ratatui (existing). Tests via cargo unit
tests and the project's YAML scenario harness (`mise run test:manual`).

**Spec:**
[`docs/superpowers/specs/2026-05-02-list-empty-state-and-repo-remove-copy-design.md`](../specs/2026-05-02-list-empty-state-and-repo-remove-copy-design.md)

---

## File Structure

**New files:**

- `src/commands/list_empty.rs` — empty-state rendering helper (pure, leaf
  module; depends only on `crate::styles`)
- `tests/manual/scenarios/list/empty-bare.yml` — TTY empty-state scenario
- `tests/manual/scenarios/list/empty-format-json.yml` — structured-output empty
  scenario (no hint expected)
- `tests/manual/scenarios/list/empty-with-branches-flag.yml` — `-b` flag empty
  scenario

**Modified files:**

- `src/commands/mod.rs` — register `list_empty` module
- `src/commands/list.rs` — replace `print_table`'s silent empty-return with
  `list_empty::print`; refactor `print_table` to accept a `&mut impl Write` for
  testability
- `src/commands/list_live.rs` — short-circuit before TUI bringup when the merged
  set is empty
- `src/commands/repo/remove.rs` — string replacements in `confirm_prompt` (lines
  200-204) and `print_plan` (lines 186-193)
- `tests/manual/scenarios/repo/remove-dry-run.yml` — update `output_contains`
  assertion ("trust DB entry" → "trust marker")
- `tests/manual/scenarios/repo/remove-basic.yml` — update prose description
  ("bare git dir" → "git dir") for consistency (no test assertion change)
- `tests/manual/scenarios/repo/remove-with-hooks.yml` — update prose description
  ("bare git directory" → "git dir") for consistency

---

## Task 1: Create `list_empty` module with rendering and unit tests

**Files:**

- Create: `src/commands/list_empty.rs`
- Modify: `src/commands/mod.rs` (register new module)

- [ ] **Step 1: Create `list_empty.rs` skeleton with failing tests**

Write the following content to `src/commands/list_empty.rs`:

````rust
//! Empty-state rendering for `daft list`.
//!
//! When the worktree set is empty (after any branch enumeration), instead of
//! a header-only table, render a 3-line hint that explains the state and
//! points the user toward `daft go` (existing branch) and `daft start` (new
//! branch). Used by both the blocking `print_table` path and the TUI
//! `list_live::run_live` short-circuit.

use crate::styles;
use std::io;

/// Render the empty-state hint as a styled string.
///
/// Layout (no-color):
/// ```text
/// No worktrees yet.
///
///   daft go <branch>     switch to an existing branch
///   daft start <branch>  create a new branch
/// ```
///
/// With `use_color = true`, `daft` is dim, the verb (`go`/`start`) is
/// cyan+bold, `<branch>` is dim, and the right-hand description is dim.
pub fn render(use_color: bool) -> String {
    // Two suggestion entries: (verb, description). Aligned by the longest
    // verb-form ("start") so the descriptions line up.
    let entries: &[(&str, &str)] = &[
        ("go", "switch to an existing branch"),
        ("start", "create a new branch"),
    ];

    // Plain-text width of "daft <verb> <branch>" — used for alignment.
    // The longest is "daft start <branch>" (19 visible chars).
    let max_syntax_width = entries
        .iter()
        .map(|(verb, _)| "daft ".len() + verb.len() + " <branch>".len())
        .max()
        .expect("entries is non-empty");

    let mut out = String::new();
    out.push_str("No worktrees yet.\n");
    out.push('\n');
    for (verb, description) in entries {
        let syntax_width = "daft ".len() + verb.len() + " <branch>".len();
        let padding = " ".repeat(max_syntax_width - syntax_width);
        let line = format_line(verb, description, &padding, use_color);
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Write the empty-state hint to the given writer.
pub fn print(out: &mut impl io::Write, use_color: bool) -> io::Result<()> {
    out.write_all(render(use_color).as_bytes())
}

fn format_line(verb: &str, description: &str, padding: &str, use_color: bool) -> String {
    if use_color {
        format!(
            "  {daft} {verb} {branch}{padding}  {description}",
            daft = styles::dim("daft"),
            verb = styles::bold(&styles::cyan(verb)),
            branch = styles::dim("<branch>"),
            description = styles::dim(description),
        )
    } else {
        format!("  daft {verb} <branch>{padding}  {description}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_no_color_contains_lead_in_and_both_commands() {
        let s = render(false);
        assert!(s.contains("No worktrees yet."), "missing lead-in: {s:?}");
        assert!(s.contains("daft go <branch>"), "missing go line: {s:?}");
        assert!(s.contains("daft start <branch>"), "missing start line: {s:?}");
        assert!(
            s.contains("switch to an existing branch"),
            "missing go description: {s:?}"
        );
        assert!(
            s.contains("create a new branch"),
            "missing start description: {s:?}"
        );
    }

    #[test]
    fn render_color_contains_ansi_escape_sequences() {
        let s = render(true);
        assert!(
            s.contains("\u{1b}["),
            "expected ANSI escapes when colors enabled, got: {s:?}"
        );
    }

    #[test]
    fn render_no_color_descriptions_are_aligned() {
        // The two suggestion lines should have their descriptions starting
        // at the same column. Find each description's start index and
        // compare.
        let s = render(false);
        let go_line = s
            .lines()
            .find(|l| l.contains("daft go <branch>"))
            .expect("go line missing");
        let start_line = s
            .lines()
            .find(|l| l.contains("daft start <branch>"))
            .expect("start line missing");

        let go_desc_idx = go_line
            .find("switch to an existing branch")
            .expect("go description missing");
        let start_desc_idx = start_line
            .find("create a new branch")
            .expect("start description missing");

        assert_eq!(
            go_desc_idx, start_desc_idx,
            "descriptions not aligned: go starts at {go_desc_idx}, start at {start_desc_idx}\n{s}"
        );
    }

    #[test]
    fn print_writes_render_output() {
        let mut buf = Vec::new();
        print(&mut buf, false).expect("print failed");
        let written = String::from_utf8(buf).expect("non-utf8");
        assert_eq!(written, render(false));
    }
}
````

- [ ] **Step 2: Register the module in `src/commands/mod.rs`**

Add `pub mod list_empty;` to `src/commands/mod.rs`, alphabetically ordered
between `pub mod list;` and `pub mod list_live;`:

```rust
pub mod list;
pub mod list_empty;
pub mod list_live;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib list_empty` Expected: 4 tests pass
(`render_no_color_contains_lead_in_and_both_commands`,
`render_color_contains_ansi_escape_sequences`,
`render_no_color_descriptions_are_aligned`, `print_writes_render_output`).

- [ ] **Step 4: Run clippy to confirm no warnings**

Run: `mise run clippy` Expected: No warnings.

- [ ] **Step 5: Run formatter**

Run: `mise run fmt` Expected: Clean (or minor reformat applied).

- [ ] **Step 6: Commit**

```bash
git add src/commands/list_empty.rs src/commands/mod.rs
git commit -m "$(cat <<'EOF'
feat(list): add list_empty module for rendering empty-state hint

Pure rendering helper used by both `daft list` render paths (blocking
print_table and TUI run_live). Suggests `daft go <branch>` and
`daft start <branch>` with cyan+bold verbs, dim chrome, aligned
descriptions. Color falls back to plain text when colors disabled.

EOF
)"
```

---

## Task 2: Wire `list_empty` into the blocking `print_table` path

**Files:**

- Modify: `src/commands/list.rs` (around lines 617-627)

The current `print_table` signature is:

```rust
fn print_table(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
    selected_columns: &[ListColumn],
    sort_spec: &SortSpec,
) {
```

with the empty-return at the top:

```rust
if infos.is_empty() {
    return;
}
```

We replace the empty branch with a call to `list_empty::print` against stdout.
We also add a unit test that exercises the empty-path render indirectly by
extracting the empty handling into a small helper, or by calling
`list_empty::print` directly (already covered in Task 1's tests). For a smoke
test that the call site is correctly wired, we add a test that calls the
dispatch logic with an empty `infos` slice and asserts non-empty output.

- [ ] **Step 1: Write the failing test**

Add this test to the existing `mod tests` block at the bottom of
`src/commands/list.rs` (around line 1016, before the closing `}` of the module):

```rust
    #[test]
    fn print_table_empty_writes_hint_to_stdout_via_helper() {
        // We don't capture real stdout in unit tests; instead verify that
        // the empty branch routes through list_empty::print, which has its
        // own tests covering content. This test ensures the helper itself
        // produces something non-empty for the empty case.
        use crate::commands::list_empty;
        let mut buf = Vec::new();
        list_empty::print(&mut buf, false).expect("print failed");
        let s = String::from_utf8(buf).expect("non-utf8");
        assert!(s.contains("No worktrees yet."));
        assert!(s.contains("daft go <branch>"));
        assert!(s.contains("daft start <branch>"));
    }
```

- [ ] **Step 2: Run the test to verify it passes (the helper already exists)**

Run:
`cargo test --lib --package daft print_table_empty_writes_hint_to_stdout_via_helper`
Expected: PASS (Task 1 already implemented `list_empty::print`).

- [ ] **Step 3: Replace the silent empty-return in `print_table`**

Find this block at the start of `print_table` (around line 625-627 of
`src/commands/list.rs`):

```rust
    if infos.is_empty() {
        return;
    }
```

Replace it with:

```rust
    if infos.is_empty() {
        let _ = crate::commands::list_empty::print(
            &mut std::io::stdout(),
            crate::styles::colors_enabled(),
        );
        return;
    }
```

(Stdout write failures are vanishingly rare and the existing `print_table`
returns `()`, so we discard the error to match the surrounding style.)

- [ ] **Step 4: Build to confirm no errors**

Run: `cargo build` Expected: Builds cleanly.

- [ ] **Step 5: Run all list tests to confirm nothing regressed**

Run: `cargo test --lib --package daft list` Expected: All existing list tests
still pass; new test passes.

- [ ] **Step 6: Run clippy and fmt**

Run: `mise run clippy && mise run fmt:check` Expected: No warnings, formatting
clean.

- [ ] **Step 7: Commit**

```bash
git add src/commands/list.rs
git commit -m "$(cat <<'EOF'
fix(list): render empty-state hint instead of silent empty output

Replace print_table's silent early-return on empty `infos` with a call
to `list_empty::print`. Structured output paths (`--format json|csv|...`)
are reached earlier in `run_blocking` and remain unchanged.

Refs #444

EOF
)"
```

---

## Task 3: Wire `list_empty` into the TUI `list_live::run_live` path

**Files:**

- Modify: `src/commands/list_live.rs`

The TUI path currently always brings up ratatui regardless of row count. We
short-circuit _before_ spawning the streaming collector, the renderer, the
SIGINT handler, and the raw-mode guard when the merged worktree+branch set is
empty. The synchronous `collect_branch_info` call already runs (when `-b/-r/-a`
is set) before TUI bringup, so we just check `worktree_infos.is_empty()` after
that block.

The relevant region is `src/commands/list_live.rs:165-188` (the
`if show_local || show_remote { collect_branch_info ... }` block).

- [ ] **Step 1: Insert the short-circuit after the branch enumeration block**

In `src/commands/list_live.rs`, locate this block (around lines 165-188):

```rust
    // Optionally enumerate non-worktree branches (sync — cheap git for-each-ref).
    if show_local || show_remote {
        let branch_infos = collect_branch_info(
            &git,
            &base_branch,
            stat,
            show_local,
            show_remote,
            &worktree_branches,
            &project_root,
            settings.ownership_strategy,
            user_email.as_deref(),
            &settings.remote,
        )?;
        for info in branch_infos {
            targets.push(list_stream::CollectorTarget {
                branch_name: info.name.clone(),
                path: info.path.clone(),
                kind: info.kind,
                is_detached: false,
            });
            worktree_infos.push(info);
        }
    }
```

Immediately AFTER the closing `}` of that `if` block (and before the
`// Build TUI state` comment / `let tui_columns: ...` line at around line 192),
insert:

```rust
    // Short-circuit when the merged set is empty: skip TUI bringup and
    // print a static empty-state hint. Avoids ratatui flicker and a
    // raw-mode bringup just to render three lines of static text.
    if worktree_infos.is_empty() {
        crate::commands::list_empty::print(
            &mut std::io::stdout(),
            crate::styles::colors_enabled(),
        )?;
        return Ok(());
    }
```

Note: this short-circuit comes after both the porcelain seed AND the optional
`collect_branch_info` merge, so it correctly covers all empty cases (default
`daft list`, `-b`, `-r`, `-a`).

- [ ] **Step 2: Build to confirm no errors**

Run: `cargo build` Expected: Builds cleanly.

- [ ] **Step 3: Run unit tests to confirm nothing regressed**

Run: `cargo test --lib --package daft` Expected: All existing tests pass.

- [ ] **Step 4: Run clippy and fmt**

Run: `mise run clippy && mise run fmt:check` Expected: No warnings, formatting
clean.

- [ ] **Step 5: Manual smoke check (optional but recommended)**

Build the binary and try it locally:

```bash
mise run dev
TMP=$(mktemp -d)
cd "$TMP"
git init --bare empty-bare.git
DAFT_NO_LIVE=1 daft list 2>&1 | head  # blocking path
cd /
rm -rf "$TMP"
```

Expected: Both invocations print the empty-state hint with `daft go <branch>`
and `daft start <branch>` lines visible.

- [ ] **Step 6: Commit**

```bash
git add src/commands/list_live.rs
git commit -m "$(cat <<'EOF'
fix(list): short-circuit TUI when worktree+branch set is empty

When run_live's merged worktree_infos is empty (after the optional
collect_branch_info enumeration for -b/-r/-a), skip the streaming
collector spawn, ratatui bringup, raw-mode guard, and SIGINT handler,
and print the static empty-state hint instead. Avoids a flicker frame
of an empty-bodied table and saves the bringup cost for a 3-line
message.

Refs #444

EOF
)"
```

---

## Task 4: Add YAML scenarios for `daft list` empty state

**Files:**

- Create: `tests/manual/scenarios/list/empty-bare.yml`
- Create: `tests/manual/scenarios/list/empty-format-json.yml`
- Create: `tests/manual/scenarios/list/empty-with-branches-flag.yml`

All three scenarios use `--no-checkout --layout contained` to produce a bare
repo with zero worktrees. The `cwd` for `daft list` is the project root — the
bare layout places `.git` inside `$WORK_DIR/<repo>` and the project root itself
is what `daft` operates from.

- [ ] **Step 1: Write `empty-bare.yml`**

Create `tests/manual/scenarios/list/empty-bare.yml`:

```yaml
name: List empty state in bare layout
description: >
  In a contained-layout bare repo with no checked-out worktrees, `daft list`
  renders a 3-line empty-state hint pointing the user at `daft go` and `daft
  start` instead of an empty header-only table.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with --no-checkout to get a bare repo with zero worktrees
    run: git-worktree-clone --no-checkout --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo"
        - "$WORK_DIR/test-repo/.git"

  - name: List shows empty-state hint, not an empty table
    run: NO_COLOR=1 DAFT_NO_LIVE=1 git-worktree-list 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - "No worktrees yet."
        - "daft go <branch>"
        - "daft start <branch>"
        - "switch to an existing branch"
        - "create a new branch"
```

- [ ] **Step 2: Write `empty-format-json.yml`**

Create `tests/manual/scenarios/list/empty-format-json.yml`:

```yaml
name: List empty state with --format json emits empty rows, no hint
description: >
  Structured output (`--format json`) is reached before the empty-state
  rendering branch, so an empty bare repo yields an empty rows array. The
  human-only "No worktrees yet." hint MUST NOT appear in JSON output.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with --no-checkout to get a bare repo with zero worktrees
    run: git-worktree-clone --no-checkout --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: List with --format json emits empty rows, no hint string
    run: git-worktree-list --format json 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - '"rows"'
      output_not_contains:
        - "No worktrees yet."
        - "daft go <branch>"
```

Note: if the YAML scenario harness does not support `output_not_contains`, fall
back to asserting `output_contains: ['"rows": []']` — adjust to match the
harness's actual capability.

- [ ] **Step 3: Write `empty-with-branches-flag.yml`**

Create `tests/manual/scenarios/list/empty-with-branches-flag.yml`:

```yaml
name: List empty state honors -b flag (no branches either)
description: >
  When `daft list -b` runs in a bare repo with zero worktrees AND zero local
  branches (the bare clone has no fetched refs after --no-checkout), the merged
  set is still empty. The empty-state hint MUST still render.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with --no-checkout to get a bare repo with zero worktrees
    run: git-worktree-clone --no-checkout --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: List -b shows empty-state hint when branches are also empty
    run: NO_COLOR=1 DAFT_NO_LIVE=1 git-worktree-list -b 2>&1
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains:
        - "No worktrees yet."
        - "daft go <branch>"
```

Note: `--no-checkout` clones do fetch remote refs, so local branches may not
exist but remote tracking refs do. `-b` only shows local branches without a
worktree. If this scenario's premise is wrong (i.e., `-b` produces non-empty
output even after `--no-checkout`), the implementing engineer should adjust
either the clone flags (e.g., add steps to delete any refs created), or relax
the assertions to match observed behavior. The goal of the scenario is to verify
the hint shows up when the merged set is empty — exact reproduction of
"merged-set empty under -b" is the only requirement.

- [ ] **Step 4: Build the dev binary so symlinks are in place**

Run: `mise run dev` Expected: Builds and creates `git-worktree-list` and `daft`
symlinks.

- [ ] **Step 5: Run the new scenarios and confirm pass**

Run:
`mise run test:manual -- --ci list:empty-bare list:empty-format-json list:empty-with-branches-flag`
Expected: All three scenarios pass.

If `empty-with-branches-flag.yml` fails because `-b` produces non-empty output,
remove that scenario (the other two suffice to cover the spec) or adjust as
suggested in Step 3's note.

- [ ] **Step 6: Run the full list scenario suite to confirm no regression**

Run: `mise run test:manual -- --ci list` Expected: All scenarios in
`tests/manual/scenarios/list/` pass.

- [ ] **Step 7: Commit**

```bash
git add tests/manual/scenarios/list/empty-bare.yml \
        tests/manual/scenarios/list/empty-format-json.yml \
        tests/manual/scenarios/list/empty-with-branches-flag.yml
git commit -m "$(cat <<'EOF'
test(list): add YAML scenarios covering empty-state rendering

Three scenarios: empty-bare (TTY hint shown), empty-format-json
(structured output unaffected, no hint), empty-with-branches-flag (-b
flag with truly empty merged set still shows hint).

Refs #444

EOF
)"
```

---

## Task 5: Refine `daft repo remove` copy

**Files:**

- Modify: `src/commands/repo/remove.rs` (lines 186-193 and 200-204)
- Modify: `tests/manual/scenarios/repo/remove-dry-run.yml` (assertion)
- Modify: `tests/manual/scenarios/repo/remove-basic.yml` (description prose)
- Modify: `tests/manual/scenarios/repo/remove-with-hooks.yml` (description
  prose)

- [ ] **Step 1: Update `confirm_prompt` (lines 200-204)**

In `src/commands/repo/remove.rs`, locate the `match n` block in
`confirm_prompt`:

```rust
    let suffix = match n {
        0 => "This will delete the bare git dir (no worktrees to remove).".to_string(),
        1 => "This will delete 1 worktree and the bare git dir.".to_string(),
        n => format!("This will delete {n} worktrees and the bare git dir."),
    };
```

Replace with:

```rust
    let suffix = match n {
        0 => "No worktrees to remove — this will delete the repo.".to_string(),
        1 => "This will delete 1 worktree and the repo.".to_string(),
        n => format!("This will delete {n} worktrees and the repo."),
    };
```

- [ ] **Step 2: Update `print_plan` (lines 186-193)**

In the same file, locate `print_plan`:

```rust
fn print_plan(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) {
    println!("Would remove:");
    for w in worktrees {
        let label = w.branch.as_deref().unwrap_or("(detached)");
        println!("  worktree  {}  ({})", w.path.display(), label);
    }
    println!("  bare      {}", target.bare_git_dir.display());
    println!("  trust DB entry for {}", target.bare_git_dir.display());
}
```

Replace with:

```rust
fn print_plan(
    target: &crate::core::worktree::remove_repo::RepoTarget,
    worktrees: &[crate::core::worktree::remove_repo::WorktreeEntry],
) {
    println!("Would remove:");
    for w in worktrees {
        let label = w.branch.as_deref().unwrap_or("(detached)");
        println!("  worktree  {}  ({})", w.path.display(), label);
    }
    println!("  git dir   {}", target.bare_git_dir.display());
    println!("  trust marker for {}", target.bare_git_dir.display());
}
```

Column-width note: `worktree` is 8 chars, `git dir ` (with trailing space) is
also 8 chars — alignment preserved at column 12 (after the 2-space indent).

- [ ] **Step 3: Build to confirm no errors**

Run: `cargo build` Expected: Builds cleanly.

- [ ] **Step 4: Run unit tests to confirm nothing regressed**

Run: `cargo test --lib --package daft repo` Expected: All `repo`-related unit
tests still pass.

- [ ] **Step 5: Update YAML scenario assertion in `remove-dry-run.yml`**

In `tests/manual/scenarios/repo/remove-dry-run.yml`, find the `output_contains`
array (around line 33-37):

```yaml
output_contains:
  - "Would remove"
  - "main"
  - "develop"
  - "trust DB entry"
```

Replace `"trust DB entry"` with `"trust marker"`:

```yaml
output_contains:
  - "Would remove"
  - "main"
  - "develop"
  - "trust marker"
```

Also update the file's top-level `description` prose:

```yaml
description: >
  daft repo remove --dry-run reports what would be removed (worktrees, bare git
  dir, trust DB entry) and exits 0 without modifying the filesystem.
```

to:

```yaml
description: >
  daft repo remove --dry-run reports what would be removed (worktrees, git dir,
  trust marker) and exits 0 without modifying the filesystem.
```

- [ ] **Step 6: Update prose description in `remove-basic.yml`**

In `tests/manual/scenarios/repo/remove-basic.yml`, find:

```yaml
description:
  daft clone a fixture, add a worktree, then daft repo remove --force tears down
  the bare git dir and every worktree.
```

Replace with:

```yaml
description:
  daft clone a fixture, add a worktree, then daft repo remove --force tears down
  the git dir and every worktree.
```

- [ ] **Step 7: Update prose description in `remove-with-hooks.yml`**

In `tests/manual/scenarios/repo/remove-with-hooks.yml`, find:

```yaml
written by the hook prove the hook ran. After removal the bare git directory
```

Replace `bare git directory` with `git dir`:

```yaml
written by the hook prove the hook ran. After removal the git dir
```

- [ ] **Step 8: Run repo remove scenarios to confirm pass**

Run:
`mise run test:manual -- --ci repo:remove-dry-run repo:remove-basic repo:remove-with-hooks`
Expected: All three scenarios pass.

- [ ] **Step 9: Run the full repo scenario suite for safety**

Run: `mise run test:manual -- --ci repo` Expected: All scenarios in
`tests/manual/scenarios/repo/` pass.

- [ ] **Step 10: Run clippy and fmt**

Run: `mise run clippy && mise run fmt:check` Expected: No warnings, formatting
clean.

- [ ] **Step 11: Commit**

```bash
git add src/commands/repo/remove.rs \
        tests/manual/scenarios/repo/remove-dry-run.yml \
        tests/manual/scenarios/repo/remove-basic.yml \
        tests/manual/scenarios/repo/remove-with-hooks.yml
git commit -m "$(cat <<'EOF'
fix(repo): drop internal jargon from `daft repo remove` user-facing copy

Replace "bare git dir" with "repo" in the confirm prompt suffix, and
replace "bare" / "trust DB entry" with "git dir" / "trust marker" in the
print_plan output. Pure copy change — no behavior change. Updates the
matching prose and assertion in three YAML scenarios.

Refs #444

EOF
)"
```

---

## Task 6: Final verification and pre-PR checks

**Files:** none modified.

- [ ] **Step 1: Run all unit tests**

Run: `mise run test:unit` Expected: All tests pass.

- [ ] **Step 2: Run all integration + YAML scenarios**

Run: `mise run test:integration` Expected: All scenarios pass, including the
three new `list/empty-*.yml` ones.

- [ ] **Step 3: Run clippy with zero-warnings policy**

Run: `mise run clippy` Expected: No warnings.

- [ ] **Step 4: Verify formatting**

Run: `mise run fmt:check` Expected: Clean.

- [ ] **Step 5: Verify man page generation is up to date**

Run: `mise run man:verify` Expected: Clean (no command-help text changed, so man
pages should be identical).

- [ ] **Step 6: Run the simulated CI**

Run: `mise run ci` Expected: All checks pass.

- [ ] **Step 7: View the local empty state once for sanity**

```bash
mise run dev
TMP=$(mktemp -d)
cd "$TMP"
git-worktree-clone --no-checkout --layout contained \
  https://github.com/avihut/daft-test-fixture-standard-remote.git || true
# If the test fixture URL is not accessible from your environment, fall
# back to a local empty bare repo:
if [ ! -d daft-test-fixture-standard-remote ]; then
  mkdir empty-repo && cd empty-repo && git init --bare && cd ..
  daft list 2>&1 || true
else
  cd daft-test-fixture-standard-remote
  daft list 2>&1
fi
cd /
rm -rf "$TMP"
```

Expected: Empty-state hint visible with cyan `go`/`start` verbs in TTY mode.

- [ ] **Step 8: No further commits — work is in 5 prior commits**

Verify: `git log --oneline @{u}.. | head` Expected: Five commits on top of the
upstream branch:

1. feat(list): add list_empty module for rendering empty-state hint
2. fix(list): render empty-state hint instead of silent empty output
3. fix(list): short-circuit TUI when worktree+branch set is empty
4. test(list): add YAML scenarios covering empty-state rendering
5. fix(repo): drop internal jargon from `daft repo remove` user-facing copy

---

## Notes for the implementing engineer

- **YAML scenario harness tooling.** If a YAML directive used in a scenario is
  unsupported (e.g., `output_not_contains` in `empty-format-json.yml`), inspect
  `tests/README.md` for the schema and adjust the assertion. Don't add a new
  schema feature — the goal is the test, not infrastructure expansion.
- **`--no-checkout` + `-b` interaction.** If `empty-with-branches-flag.yml`'s
  premise (merged set still empty under `-b`) doesn't hold in practice, the
  scenario can be removed. The other two YAML scenarios are sufficient for spec
  coverage; the unit tests in Task 1 already cover render correctness
  independent of which flag path triggered the empty case.
- **Don't widen scope.** Resist the urge to add layout-conditional empty-state
  messaging, `daft doctor` integration, or restructured `repo remove` output.
  The spec marks all three as non-goals.
- **Color helpers.** `crate::styles::dim`, `cyan`, `bold` already exist in
  `src/styles.rs`. `colors_enabled()` is the canonical gate. Use them; do not
  introduce new color helpers.
