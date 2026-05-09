//! NDJSON serializer — one JSON object per line.
//!
//! Tabular: one row → one line.
//! Matrix:  one cell → one line, long-form `{row_key, col_key, cell_key}`.
//! Other shapes: unreachable (dispatcher excludes them).

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::{Cell, EmitPayload, Matrix, Table};
use std::io::Write;

pub fn emit<W: Write>(payload: &EmitPayload, writer: &mut W) -> Result<(), EmitError> {
    match payload {
        EmitPayload::Tabular(t) => write_tabular(t, writer),
        EmitPayload::Matrix(m) => write_matrix(m, writer),
        _ => unreachable!("dispatcher should have rejected non-tabular/matrix for ndjson"),
    }
}

fn write_tabular<W: Write>(t: &Table, writer: &mut W) -> Result<(), EmitError> {
    for row in &t.rows {
        let mut map = serde_json::Map::new();
        for (h, c) in t.headers.iter().zip(row) {
            map.insert(h.clone(), cell_value(c));
        }
        let line = serde_json::to_string(&serde_json::Value::Object(map))?;
        writeln!(writer, "{line}")?;
    }
    Ok(())
}

fn write_matrix<W: Write>(m: &Matrix, writer: &mut W) -> Result<(), EmitError> {
    for row in &m.rows {
        for col in &m.cols {
            if let Some(v) = m.cells.get(&(row.clone(), col.clone())) {
                let obj = serde_json::json!({
                    &m.row_key: row,
                    &m.col_key: col,
                    &m.cell_key: cell_value(v),
                });
                let line = serde_json::to_string(&obj)?;
                writeln!(writer, "{line}")?;
            }
        }
    }
    Ok(())
}

fn cell_value(c: &Cell) -> serde_json::Value {
    match c {
        Cell::Str(s) => serde_json::Value::String(s.clone()),
        Cell::Int(i) => serde_json::Value::Number((*i).into()),
        Cell::Bool(b) => serde_json::Value::Bool(*b),
        Cell::Null => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::emit::test_fixtures::*;

    fn render(p: &EmitPayload) -> String {
        let mut buf = Vec::new();
        emit(p, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn tabular_ndjson_line_per_row() {
        let out = render(&fixture_tabular());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("{\"name\":\"alpha\""));
        assert!(lines[1].contains(r#""name":"béta, with comma""#));
    }

    #[test]
    fn matrix_ndjson_long_form() {
        let out = render(&fixture_matrix());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "2 paths × 2 worktrees = 4 cells");
        assert!(
            lines
                .iter()
                .any(|l| l.contains(r#""path":"shared/foo.txt""#))
        );
        assert!(lines.iter().any(|l| l.contains(r#""state":"missing""#)));
    }
}
