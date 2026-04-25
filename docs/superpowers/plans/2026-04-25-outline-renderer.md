# Outline Renderer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the vertical-spine timeline rendering from
`src/commands/hooks/jobs.rs` into a reusable, domain-agnostic component in
`src/output/outline.rs`. Callers describe a
`Outline { sections → nodes → body }` structure; the renderer owns all spine
glyphs, gutters, and terminators.

**Architecture:**

A small data type plus a `render` function:

```rust
// In src/output/outline.rs
pub struct Outline { pub sections: Vec<Section> }
pub struct Section { pub header: String, pub nodes: Vec<Node> }
pub struct Node { pub label: String, pub body: Body }
pub enum Body { Lines(Vec<String>), Empty(String) }

pub fn render(outline: &Outline, emit: impl FnMut(&str));
```

The renderer walks the structure once and calls `emit(line)` per output line.
Spine glyphs (`│`, `●`, `╰─╴`) and gutter (3 spaces) are rendered with `dim()`
styling and live entirely inside `outline.rs`.

The `list_jobs` orchestration in `src/commands/hooks/jobs.rs` shrinks to: build
the `Outline`, then `outline::render(&outline, |line| output.info(line))`. The
per-element spine helpers (`spine_blank`, `spine_prefixed`,
`empty_invocation_placeholder`, `spine_terminator`) are deleted; their behavior
moves into the renderer. `invocation_node_line` is reduced to a label-only
formatter (the bullet `●` is now the renderer's job).

**Tech Stack:** Rust 1.95, no new crates. Uses existing `crate::styles::dim`.

---

## Files

- **Create** `src/output/outline.rs` — `Outline`/`Section`/`Node`/`Body` data
  types + `render` + unit tests.
- **Modify** `src/output/mod.rs` — register `pub mod outline;`.
- **Modify** `src/commands/hooks/jobs.rs` —
  - rebuild `list_jobs` rendering: construct an `Outline`, call
    `outline::render`.
  - delete `spine_blank`, `spine_prefixed`, `empty_invocation_placeholder`,
    `spine_terminator` and their unit tests.
  - reduce `invocation_node_line` to a label-only formatter (drop the bullet
    from its output) and rename to reflect its narrowed role.
  - keep `worktree_header` (used as `Section::header`).

---

### Task 1: Build the `Outline` component in `src/output/outline.rs`

**Files:**

- Create: `src/output/outline.rs`
- Modify: `src/output/mod.rs`

- [ ] **Step 1: Add the module declaration**

In `src/output/mod.rs`, add `pub mod outline;` near the other `pub mod`
declarations (alphabetically — between `format` and `hook_progress`).

- [ ] **Step 2: Write failing unit tests for the renderer**

Create `src/output/outline.rs` with this scaffolding plus tests. Leave the
public types and `render` as `unimplemented!()` so the tests compile and fail.

```rust
//! Vertical-spine timeline outline renderer.
//!
//! Use this when you need to render a hierarchical structure as a
//! box-drawing timeline: a column-0 spine (`│`) per section, bullet (`●`)
//! per node, and a tapering corner (`╰─╴`) closing each section. The
//! renderer is domain-agnostic — callers supply pre-formatted strings for
//! headers, labels, and body lines.

use crate::styles::dim;

/// A vertical-spine outline.
pub struct Outline {
    pub sections: Vec<Section>,
}

/// A top-level section in the outline. Each section owns its own spine.
pub struct Section {
    /// Header line printed above the section. Caller-styled.
    pub header: String,
    pub nodes: Vec<Node>,
}

/// A node hanging off the section's spine.
pub struct Node {
    /// Label printed next to the bullet. Caller-styled.
    pub label: String,
    pub body: Body,
}

/// The body of a node.
pub enum Body {
    /// Pre-rendered body lines. Each line is emitted with the spine glyph
    /// + a 3-space gutter prepended.
    Lines(Vec<String>),
    /// Single-line placeholder shown when the node has no body content.
    /// Emitted with the spine glyph + a 4-space gutter prepended so the
    /// placeholder aligns under typical tabled body content (which carries
    /// 1 char of its own left padding under `Style::blank()`).
    Empty(String),
}

/// Render the outline, emitting one line at a time via `emit`.
pub fn render(_outline: &Outline, _emit: impl FnMut(&str)) {
    unimplemented!("see Step 4")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(outline: &Outline) -> Vec<String> {
        let mut lines = Vec::new();
        render(outline, |line| lines.push(line.to_string()));
        lines
    }

    #[test]
    fn render_empty_outline_emits_nothing() {
        let outline = Outline { sections: vec![] };
        assert!(collect(&outline).is_empty());
    }

    #[test]
    fn render_single_section_with_lines_body() {
        let outline = Outline {
            sections: vec![Section {
                header: "header".into(),
                nodes: vec![Node {
                    label: "node-a".into(),
                    body: Body::Lines(vec!["row1".into(), "row2".into()]),
                }],
            }],
        };
        let lines = collect(&outline);
        // Layout:
        //   "header"
        //   "" (blank, no spine)
        //   "● node-a" (bullet dim, label as-is)
        //   "│" (spine breath)
        //   "│   row1" (3-space gutter)
        //   "│   row2"
        //   "╰─╴" (terminator)
        //   "" (trailing blank)
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0], "header");
        assert_eq!(lines[1], "");
        assert!(lines[2].contains("\u{25cf}"), "node line missing bullet: {:?}", lines[2]);
        assert!(lines[2].contains("node-a"));
        assert!(lines[3].contains("\u{2502}"), "spine breath missing │");
        assert!(lines[4].contains("\u{2502}") && lines[4].contains("row1"));
        assert!(lines[4].ends_with("row1"));
        assert!(lines[5].ends_with("row2"));
        assert!(lines[6].contains("\u{2570}\u{2500}\u{2574}"), "terminator missing ╰─╴: {:?}", lines[6]);
        assert_eq!(lines[7], "");
    }

    #[test]
    fn render_node_with_empty_body_uses_4_space_gutter() {
        let outline = Outline {
            sections: vec![Section {
                header: "h".into(),
                nodes: vec![Node {
                    label: "n".into(),
                    body: Body::Placeholder("(no body)".into()),
                }],
            }],
        };
        let lines = collect(&outline);
        // Find the placeholder line — it's after header, blank, node, spine.
        let placeholder = &lines[4];
        // Strip ANSI to count visible characters between │ and the text.
        let visible = crate::output::format::strip_ansi(placeholder);
        assert!(visible.starts_with("\u{2502}    (no body)"),
                "expected `│    (no body)`, got {visible:?}");
    }

    #[test]
    fn render_multiple_nodes_inserts_spine_breath_between() {
        let outline = Outline {
            sections: vec![Section {
                header: "h".into(),
                nodes: vec![
                    Node {
                        label: "n1".into(),
                        body: Body::Lines(vec!["b1".into()]),
                    },
                    Node {
                        label: "n2".into(),
                        body: Body::Lines(vec!["b2".into()]),
                    },
                ],
            }],
        };
        let lines = collect(&outline);
        // Sequence:
        //   header, "", "● n1", "│", "│   b1", "│", "● n2", "│", "│   b2", "╰─╴", ""
        assert_eq!(lines.len(), 11);
        // Between the two nodes, the separator is a spine line "│" — NOT
        // a blank "" (which is reserved for the section's first node).
        assert!(lines[5].contains("\u{2502}") && !lines[5].contains("b1") && !lines[5].contains("n2"),
                "expected spine separator between nodes, got {:?}", lines[5]);
        assert!(lines[5].len() < 10, "expected just `│` (with ANSI), got {:?}", lines[5]);
    }

    #[test]
    fn render_multiple_sections_inserts_blank_line_between() {
        let outline = Outline {
            sections: vec![
                Section {
                    header: "h1".into(),
                    nodes: vec![Node {
                        label: "n1".into(),
                        body: Body::Lines(vec!["b1".into()]),
                    }],
                },
                Section {
                    header: "h2".into(),
                    nodes: vec![Node {
                        label: "n2".into(),
                        body: Body::Lines(vec!["b2".into()]),
                    }],
                },
            ],
        };
        let lines = collect(&outline);
        // Each section ends with terminator + "". Find the index of the
        // first terminator and assert the next line is "" and the line
        // after that is the second header.
        let term_idx = lines.iter().position(|l| l.contains("\u{2570}")).unwrap();
        assert_eq!(lines[term_idx + 1], "");
        assert_eq!(lines[term_idx + 2], "h2");
    }

    #[test]
    fn render_section_with_no_nodes_still_emits_header_and_terminator() {
        let outline = Outline {
            sections: vec![Section {
                header: "lone".into(),
                nodes: vec![],
            }],
        };
        let lines = collect(&outline);
        assert_eq!(lines[0], "lone");
        assert!(lines.iter().any(|l| l.contains("\u{2570}\u{2500}\u{2574}")));
        assert_eq!(lines.last().unwrap(), "");
    }

    #[test]
    fn render_emits_dim_styling_on_spine_glyphs() {
        let outline = Outline {
            sections: vec![Section {
                header: "h".into(),
                nodes: vec![Node {
                    label: "n".into(),
                    body: Body::Lines(vec!["body".into()]),
                }],
            }],
        };
        let lines = collect(&outline);
        // `dim()` wraps text with `\x1b[2m...\x1b[0m`. Any line containing
        // a spine glyph (│, ●, ╰─╴) must carry that wrapping.
        for line in &lines {
            if line.contains('\u{2502}') || line.contains('\u{25cf}') || line.contains('\u{2570}') {
                assert!(line.contains("\x1b[2m"), "expected dim escape on spine line: {line:?}");
                assert!(line.contains("\x1b[0m"), "expected reset escape on spine line: {line:?}");
            }
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p daft --lib output::outline::tests` Expected: all FAIL (panic
from `unimplemented!()` in `render`).

- [ ] **Step 4: Implement `render`**

Replace the `unimplemented!()` body with:

```rust
pub fn render(outline: &Outline, mut emit: impl FnMut(&str)) {
    const SPINE: &str = "\u{2502}";       // │
    const BULLET: &str = "\u{25cf}";      // ●
    const TERMINATOR: &str = "\u{2570}\u{2500}\u{2574}"; // ╰─╴
    const LINES_GUTTER: &str = "   ";     // 3 spaces — body lines carry their own padding
    const EMPTY_GUTTER: &str = "    ";    // 4 spaces — placeholder text aligns with table content

    for section in &outline.sections {
        emit(&section.header);

        for (i, node) in section.nodes.iter().enumerate() {
            // Separator before this node: blank line above the section's
            // first node, dim spine line between adjacent nodes.
            if i == 0 {
                emit("");
            } else {
                emit(&dim(SPINE));
            }

            // Node bullet + caller-styled label.
            emit(&format!("{} {}", dim(BULLET), node.label));

            // Spine breath between label and body.
            emit(&dim(SPINE));

            match &node.body {
                Body::Lines(lines) => {
                    let spine = dim(SPINE);
                    for line in lines {
                        emit(&format!("{spine}{LINES_GUTTER}{line}"));
                    }
                }
                Body::Placeholder(text) => {
                    emit(&format!("{}{EMPTY_GUTTER}{text}", dim(SPINE)));
                }
            }
        }

        emit(&dim(TERMINATOR));
        emit("");
    }
}
```

Notes:

- `dim()` is `crate::styles::dim`. The `use crate::styles::dim;` at the top of
  the file (added in Step 2) covers it.
- `LINES_GUTTER` is 3 spaces because pre-rendered body lines (e.g., from
  `tabled::Builder` with `Style::blank()`) carry 1 char of their own left
  padding. `EMPTY_GUTTER` is 4 spaces so the bare placeholder string lines up
  with body content visually.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p daft --lib output::outline::tests` Expected: all 7 tests
PASS.

- [ ] **Step 6: Run the rest of the suite + clippy + fmt**

Run in parallel:

- `cargo test -p daft --lib`
- `mise run clippy`
- `mise run fmt:check` (ignore complaints about markdown plan files; only block
  on Rust source).

All clean.

- [ ] **Step 7: Commit**

```bash
git add src/output/outline.rs src/output/mod.rs
git commit -m "feat(output): add outline renderer for vertical-spine timelines"
```

---

### Task 2: Migrate `list_jobs` to use the outline renderer

**Files:**

- Modify: `src/commands/hooks/jobs.rs`

- [ ] **Step 1: Read the current `list_jobs` rendering loop**

Run:
`grep -n "fn list_jobs\|spine_blank\|spine_prefixed\|empty_invocation_placeholder\|spine_terminator\|invocation_node_line\|worktree_header" src/commands/hooks/jobs.rs`

Confirm the helpers' current locations and the rendering loop bounds.

- [ ] **Step 2: Add imports**

At the top of `src/commands/hooks/jobs.rs`, in the existing
`use crate::output::...` group, add:

```rust
use crate::output::outline::{self, Body, Node, Outline, Section};
```

- [ ] **Step 3: Rewrite the rendering loop in `list_jobs`**

Locate the `// ---- Pass 2: render ----` block (added by the column-alignment
work) and replace it (everything from that comment down to and including the
`for (worktree, secs) in &sections_by_worktree { ... }` loop, but NOT the
`--all` cleanup-footer block that follows) with this:

```rust
// ---- Pass 2: build outline + render ----
//
// The spine timeline (column-0 │, ● per node, ╰─╴ terminator, gutter
// widths) lives in `crate::output::outline`. Here we just describe the
// structure: one section per worktree, one node per invocation, body
// either pre-rendered table lines or a placeholder string.
let outline = Outline {
    sections: sections_by_worktree
        .into_iter()
        .map(|(worktree, secs)| {
            let marker = if worktree == current_worktree {
                CURRENT_WORKTREE_SYMBOL
            } else {
                " "
            };
            Section {
                header: worktree_header(marker, &worktree),
                nodes: secs
                    .into_iter()
                    .map(|sec| {
                        let ago = shorthand_from_seconds(
                            now.signed_duration_since(sec.inv.created_at).num_seconds(),
                        );
                        let short_id = &sec.inv.invocation_id
                            [..4.min(sec.inv.invocation_id.len())];
                        let label = invocation_node_label(
                            &ago,
                            &sec.inv.trigger_command,
                            short_id,
                        );

                        let body = if sec.rows.is_empty() {
                            Body::Placeholder(dim("(no jobs declared)"))
                        } else {
                            let mut builder = Builder::new();
                            builder.push_record(
                                HEADERS
                                    .iter()
                                    .enumerate()
                                    .map(|(c, h)| {
                                        pad_to_visible_width(
                                            &dim_underline(h),
                                            max_widths[c],
                                        )
                                    })
                                    .collect::<Vec<_>>(),
                            );
                            for row in &sec.rows {
                                let cells = [
                                    &row.job,
                                    &row.status,
                                    &row.started,
                                    &row.duration,
                                    &row.size,
                                ];
                                builder.push_record(
                                    cells
                                        .iter()
                                        .enumerate()
                                        .map(|(c, cell)| {
                                            pad_to_visible_width(cell, max_widths[c])
                                        })
                                        .collect::<Vec<_>>(),
                                );
                            }
                            let mut table = builder.build();
                            table.with(Style::blank());
                            Body::Lines(
                                table
                                    .to_string()
                                    .lines()
                                    .map(String::from)
                                    .collect(),
                            )
                        };

                        Node { label, body }
                    })
                    .collect(),
            }
        })
        .collect(),
};

outline::render(&outline, |line| output.info(line));
```

Adaptations the implementer may need:

- The variable name `current_worktree` may be `&current_worktree` depending on
  whether it's a `String` or `&str` in scope — match the original code.
- `pad_to_visible_width` and `dim_underline` may need full paths if the imports
  aren't already at the top of the file (they should be from the earlier
  column-alignment work; verify).
- `dim` for the empty-body placeholder comes from `use crate::styles::dim;` —
  verify it's already imported.

- [ ] **Step 4: Rename `invocation_node_line` to `invocation_node_label` and
      drop the bullet**

The current function:

```rust
fn invocation_node_line(time_ago: &str, trigger: &str, short_id: &str) -> String {
    format!(
        "{} {} · {trigger} {}",
        dim("\u{25cf}"),
        dim(&format!("{time_ago} ago")),
        dim(&format!("[{short_id}]")),
    )
}
```

becomes:

```rust
fn invocation_node_label(time_ago: &str, trigger: &str, short_id: &str) -> String {
    format!(
        "{} · {trigger} {}",
        dim(&format!("{time_ago} ago")),
        dim(&format!("[{short_id}]")),
    )
}
```

The bullet is now the renderer's responsibility.

Update the unit test for this helper accordingly. Find it via:

```bash
grep -n "invocation_node_line\b" src/commands/hooks/jobs.rs
```

Rename it to `invocation_node_label_omits_bullet_and_dims_time_and_id` and
adjust the assertions: the returned string must NOT start with `●` (or contain
it), must contain the dim time-ago and the dim `[short_id]` segment.

- [ ] **Step 5: Delete the now-unused spine helpers and their tests**

Delete from `src/commands/hooks/jobs.rs`:

- `fn spine_blank()` and its test (`spine_blank_is_dim_pipe_flush_left`)
- `fn spine_prefixed()` and its test
  (`spine_prefixed_inserts_pipe_and_three_space_gutter`)
- `fn empty_invocation_placeholder()` and its test
  (`empty_invocation_placeholder_aligns_with_tabled_first_column`)
- `fn spine_terminator()` and its test
  (`spine_terminator_is_dim_corner_with_taper`)

Their behavior is now covered by `crate::output::outline::tests`.

- [ ] **Step 6: Run unit tests**

Run: `cargo test -p daft --lib` Expected: all tests PASS. The deleted helper
tests are gone; the outline tests cover their behavior.

- [ ] **Step 7: Run clippy + fmt**

Run: `mise run clippy && mise run fmt:check` Expected: zero warnings. (Ignore
plan-markdown fmt complaints — only Rust source counts.)

- [ ] **Step 8: Run hooks scenarios**

Run: `mise run test:manual -- --ci tests/manual/scenarios/hooks` (The
`--ci hooks` shortcut may resolve incorrectly; passing the directory path
works.) Expected: 62/62 pass. Substring matchers in scenarios are unaffected by
this internal refactor.

- [ ] **Step 9: Manual sandbox sanity check**

In a sandbox repo with at least one worktree containing background-job
invocations:

```bash
daft hooks jobs
daft hooks jobs --all
```

Confirm the listing is visually identical to the pre-refactor output:

- Section header
- Blank line below the header
- `● label` per invocation, separated by `│` between adjacent nodes
- Body table with 4-char inset
- `╰─╴` closing the section
- Trailing blank line between sections
- Cleanup footer (under `--all`) unchanged

- [ ] **Step 10: Commit**

```bash
git add src/commands/hooks/jobs.rs
git commit -m "refactor(hooks-jobs): render timeline via shared outline component"
```

---

## Out of scope

- Adding a second consumer of the outline component. The whole point is that one
  will appear; that's a separate change.
- Restyling the spine glyphs or gutter widths (visual contract is frozen).
- Adding `Body` variants beyond `Lines` and `Empty` (e.g., `Markdown`, `Tree`).
  Add when needed.
- Moving `worktree_header` into `outline.rs`. It's specific to the
  current-worktree marker pattern; leaving it in `jobs.rs` keeps `outline`
  domain-agnostic.

## Self-review notes

- **Spec coverage:** the spec was tweaked (one word) to refer to the "outline
  renderer" instead of the "spine helper". The visual contract is unchanged —
  all rendering details (gutter widths, dim glyphs, terminator, blank lines)
  survive intact in the renderer's body.
- **Type consistency:** `Outline`/`Section`/`Node`/`Body` are owned-string
  types; `render` takes `&Outline` and emits via callback. No lifetimes
  required.
- **Placeholders:** none. Every code block is the literal code to write or the
  literal command to run.
- **Test surface:** 7 outline tests cover structure, separators, terminators,
  dim styling, gutter widths, and empty edge cases. The 4 helper tests in
  `jobs.rs` are deleted (their behavior is now covered inside `outline.rs`);
  `invocation_node_label`'s test is updated to drop the bullet assertion.
