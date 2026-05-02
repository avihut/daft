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
        assert!(
            s.contains("daft start <branch>"),
            "missing start line: {s:?}"
        );
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
