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
pub fn render(outline: &Outline, mut emit: impl FnMut(&str)) {
    const SPINE: &str = "\u{2502}"; // │
    const BULLET: &str = "\u{25cf}"; // ●
    const TERMINATOR: &str = "\u{2570}\u{2500}\u{2574}"; // ╰─╴
    const LINES_GUTTER: &str = "   "; // 3 spaces — body lines carry their own padding
    const EMPTY_GUTTER: &str = "    "; // 4 spaces — placeholder text aligns with table content

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
                Body::Empty(text) => {
                    emit(&format!("{}{EMPTY_GUTTER}{text}", dim(SPINE)));
                }
            }
        }

        emit(&dim(TERMINATOR));
        emit("");
    }
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
        assert!(
            lines[2].contains("\u{25cf}"),
            "node line missing bullet: {:?}",
            lines[2]
        );
        assert!(lines[2].contains("node-a"));
        assert!(lines[3].contains("\u{2502}"), "spine breath missing │");
        assert!(lines[4].contains("\u{2502}") && lines[4].contains("row1"));
        assert!(lines[4].ends_with("row1"));
        assert!(lines[5].ends_with("row2"));
        assert!(
            lines[6].contains("\u{2570}\u{2500}\u{2574}"),
            "terminator missing ╰─╴: {:?}",
            lines[6]
        );
        assert_eq!(lines[7], "");
    }

    #[test]
    fn render_node_with_empty_body_uses_4_space_gutter() {
        let outline = Outline {
            sections: vec![Section {
                header: "h".into(),
                nodes: vec![Node {
                    label: "n".into(),
                    body: Body::Empty("(no body)".into()),
                }],
            }],
        };
        let lines = collect(&outline);
        // Find the placeholder line — it's after header, blank, node, spine.
        let placeholder = &lines[4];
        // Strip ANSI to count visible characters between │ and the text.
        let visible = crate::output::format::strip_ansi(placeholder);
        assert!(
            visible.starts_with("\u{2502}    (no body)"),
            "expected `│    (no body)`, got {visible:?}"
        );
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
        assert!(
            lines[5].contains("\u{2502}") && !lines[5].contains("b1") && !lines[5].contains("n2"),
            "expected spine separator between nodes, got {:?}",
            lines[5]
        );
        assert!(
            lines[5].len() < 15,
            "expected just `│` (with ANSI), got {:?}",
            lines[5]
        );
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
                assert!(
                    line.contains("\x1b[2m"),
                    "expected dim escape on spine line: {line:?}"
                );
                assert!(
                    line.contains("\x1b[0m"),
                    "expected reset escape on spine line: {line:?}"
                );
            }
        }
    }
}
