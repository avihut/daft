//! Markdown serializer. Shape-dependent rendering:
//! - Tabular: markdown table.
//! - Matrix: wide-form markdown pivot table (rows = row labels, cols = col labels).
//! - Document: JSON pretty-printed in a fenced `json` code block. Commands that
//!   have a natural prose rendering (release-notes) pre-render their markdown
//!   and pass it as `Document(Value::String(markdown_body))`; the emitter
//!   detects a top-level string and emits it as-is.
//! - Sectioned: one `## <name>` per section, each rendered per-shape.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::{Cell, EmitPayload, Matrix, Section, Table};
use std::io::Write;

pub fn emit<W: Write>(payload: &EmitPayload, writer: &mut W) -> Result<(), EmitError> {
    render(payload, writer, 2)
}

fn render<W: Write>(
    payload: &EmitPayload,
    writer: &mut W,
    section_heading_level: usize,
) -> Result<(), EmitError> {
    match payload {
        EmitPayload::Tabular(t) => write_tabular(t, writer),
        EmitPayload::Document(v) => write_document(v, writer),
        EmitPayload::Matrix(m) => write_matrix_wide(m, writer),
        EmitPayload::Sectioned(sections) => write_sections(sections, writer, section_heading_level),
    }
}

fn write_tabular<W: Write>(t: &Table, writer: &mut W) -> Result<(), EmitError> {
    if t.headers.is_empty() {
        return Ok(());
    }
    writeln!(writer, "| {} |", t.headers.join(" | "))?;
    writeln!(
        writer,
        "| {} |",
        t.headers
            .iter()
            .map(|_| "---")
            .collect::<Vec<_>>()
            .join(" | ")
    )?;
    for row in &t.rows {
        let cells: Vec<String> = row.iter().map(cell_to_md).collect();
        writeln!(writer, "| {} |", cells.join(" | "))?;
    }
    Ok(())
}

fn write_matrix_wide<W: Write>(m: &Matrix, writer: &mut W) -> Result<(), EmitError> {
    let mut header = vec![m.row_key.clone()];
    header.extend(m.cols.iter().cloned());
    writeln!(writer, "| {} |", header.join(" | "))?;
    writeln!(
        writer,
        "| {} |",
        header.iter().map(|_| "---").collect::<Vec<_>>().join(" | ")
    )?;
    for row in &m.rows {
        let mut cells = vec![row.clone()];
        for col in &m.cols {
            let v = m
                .cells
                .get(&(row.clone(), col.clone()))
                .map(cell_to_md)
                .unwrap_or_default();
            cells.push(v);
        }
        writeln!(writer, "| {} |", cells.join(" | "))?;
    }
    Ok(())
}

fn write_document<W: Write>(v: &serde_json::Value, writer: &mut W) -> Result<(), EmitError> {
    // Release-notes (and any other prose-rendering Document) sets a top-level
    // string so the markdown emitter prints it verbatim.
    if let serde_json::Value::String(s) = v {
        writer.write_all(s.as_bytes())?;
        if !s.ends_with('\n') {
            writer.write_all(b"\n")?;
        }
    } else {
        let pretty = serde_json::to_string_pretty(v)?;
        writeln!(writer, "```json")?;
        writer.write_all(pretty.as_bytes())?;
        writeln!(writer, "\n```")?;
    }
    Ok(())
}

fn write_sections<W: Write>(
    sections: &[Section],
    writer: &mut W,
    level: usize,
) -> Result<(), EmitError> {
    let hashes = "#".repeat(level);
    for (i, s) in sections.iter().enumerate() {
        if i > 0 {
            writeln!(writer)?;
        }
        writeln!(writer, "{hashes} {}", s.name)?;
        writeln!(writer)?;
        render(&s.payload, writer, level + 1)?;
    }
    Ok(())
}

fn cell_to_md(c: &Cell) -> String {
    match c {
        Cell::Str(s) => escape_pipes(s),
        Cell::Int(i) => i.to_string(),
        Cell::Bool(b) => b.to_string(),
        Cell::Null => String::new(),
    }
}

fn escape_pipes(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::emit::test_fixtures::*;

    fn render_str(p: &EmitPayload) -> String {
        let mut buf = Vec::new();
        emit(p, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn tabular_markdown_table() {
        let out = render_str(&fixture_tabular());
        assert!(out.contains("| name | size | enabled |"));
        assert!(out.contains("| --- | --- | --- |"));
        assert!(out.contains("| alpha | 42 | true |"));
    }

    #[test]
    fn matrix_markdown_is_wide_pivot() {
        let out = render_str(&fixture_matrix());
        assert!(out.contains("| path | main | feat |"));
        assert!(out.contains("| shared/foo.txt | linked | materialized |"));
    }

    #[test]
    fn sectioned_markdown_uses_h2() {
        let out = render_str(&fixture_sectioned());
        assert!(out.contains("## remotes"));
        assert!(out.contains("## worktrees"));
        assert!(out.contains("| name | url | is_default |"));
    }

    #[test]
    fn document_string_emitted_verbatim() {
        let p = EmitPayload::Document(serde_json::json!("# Release 1.2\n\nBody."));
        let out = render_str(&p);
        assert_eq!(out, "# Release 1.2\n\nBody.\n");
    }

    #[test]
    fn document_object_rendered_as_fenced_json() {
        let out = render_str(&fixture_document());
        assert!(out.starts_with("```json"));
        assert!(out.trim_end().ends_with("```"));
    }
}
