//! TSV serializer.
//!
//! Cells containing tabs or newlines are whitespace-normalized to a single
//! space before emission; TSV has no standard quoting, so preservation would
//! break awk pipelines. Users who need raw content should use csv or json.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::{Cell, EmitPayload, Matrix, Table};
use std::io::Write;

pub fn emit<W: Write>(
    payload: &EmitPayload,
    headers: bool,
    writer: &mut W,
) -> Result<(), EmitError> {
    match payload {
        EmitPayload::Tabular(t) => write_tabular(t, headers, writer),
        EmitPayload::Matrix(m) => write_matrix_long(m, headers, writer),
        _ => unreachable!("dispatcher should have rejected non-tabular/matrix for tsv"),
    }
}

fn write_tabular<W: Write>(t: &Table, headers: bool, writer: &mut W) -> Result<(), EmitError> {
    if headers {
        writeln!(
            writer,
            "{}",
            t.headers
                .iter()
                .map(|s| normalize(s))
                .collect::<Vec<_>>()
                .join("\t")
        )?;
    }
    for row in &t.rows {
        let cells: Vec<String> = row.iter().map(cell_to_str).map(|s| normalize(&s)).collect();
        writeln!(writer, "{}", cells.join("\t"))?;
    }
    Ok(())
}

fn write_matrix_long<W: Write>(m: &Matrix, headers: bool, writer: &mut W) -> Result<(), EmitError> {
    if headers {
        writeln!(
            writer,
            "{}\t{}\t{}",
            normalize(&m.row_key),
            normalize(&m.col_key),
            normalize(&m.cell_key)
        )?;
    }
    for row in &m.rows {
        for col in &m.cols {
            if let Some(v) = m.cells.get(&(row.clone(), col.clone())) {
                writeln!(
                    writer,
                    "{}\t{}\t{}",
                    normalize(row),
                    normalize(col),
                    normalize(&cell_to_str(v))
                )?;
            }
        }
    }
    Ok(())
}

fn cell_to_str(c: &Cell) -> String {
    match c {
        Cell::Str(s) => s.clone(),
        Cell::Int(i) => i.to_string(),
        Cell::Bool(b) => b.to_string(),
        Cell::Null => String::new(),
    }
}

fn normalize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '\t' || c == '\n' || c == '\r' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::emit::payload::Table;
    use crate::output::emit::test_fixtures::*;

    fn render(p: &EmitPayload, headers: bool) -> String {
        let mut buf = Vec::new();
        emit(p, headers, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn tabular_tsv_with_headers() {
        let out = render(&fixture_tabular(), true);
        let expected = "name\tsize\tenabled\n\
                        alpha\t42\ttrue\n\
                        béta, with comma\t0\tfalse\n\
                        gamma\t\ttrue\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn tabular_tsv_without_headers() {
        let out = render(&fixture_tabular(), false);
        assert!(!out.starts_with("name\t"));
        assert!(out.starts_with("alpha\t"));
    }

    #[test]
    fn matrix_tsv_is_long_form_with_three_columns() {
        let out = render(&fixture_matrix(), true);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "path\tworktree\tstate");
        assert_eq!(lines.len(), 5);
        assert!(lines[1..].iter().all(|l| l.matches('\t').count() == 2));
    }

    #[test]
    fn tabs_and_newlines_become_spaces() {
        let p = EmitPayload::Tabular(Table::new(["a"]).row([Cell::str("x\ty\nz")]));
        let out = render(&p, true);
        assert_eq!(out, "a\nx y z\n");
    }
}
