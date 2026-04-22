//! CSV serializer — RFC 4180 via the `csv` crate.

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
        _ => unreachable!("dispatcher should have rejected non-tabular/matrix for csv"),
    }
}

fn write_tabular<W: Write>(t: &Table, headers: bool, writer: &mut W) -> Result<(), EmitError> {
    let mut w = ::csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(writer);
    if headers {
        w.write_record(&t.headers)
            .map_err(|e| EmitError::Other(e.to_string()))?;
    }
    for row in &t.rows {
        let cells: Vec<String> = row.iter().map(cell_to_str).collect();
        w.write_record(&cells)
            .map_err(|e| EmitError::Other(e.to_string()))?;
    }
    w.flush()?;
    Ok(())
}

fn write_matrix_long<W: Write>(m: &Matrix, headers: bool, writer: &mut W) -> Result<(), EmitError> {
    let mut w = ::csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(writer);
    if headers {
        w.write_record([&m.row_key, &m.col_key, &m.cell_key])
            .map_err(|e| EmitError::Other(e.to_string()))?;
    }
    for row in &m.rows {
        for col in &m.cols {
            if let Some(v) = m.cells.get(&(row.clone(), col.clone())) {
                w.write_record([row.as_str(), col.as_str(), &cell_to_str(v)])
                    .map_err(|e| EmitError::Other(e.to_string()))?;
            }
        }
    }
    w.flush()?;
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
    fn tabular_csv_quotes_comma_fields() {
        let out = render(&fixture_tabular(), true);
        let expected = "name,size,enabled\n\
                        alpha,42,true\n\
                        \"béta, with comma\",0,false\n\
                        gamma,,true\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn csv_quotes_embedded_quotes_and_newlines() {
        let p =
            EmitPayload::Tabular(Table::new(["msg"]).row([Cell::str("she said \"hi\"\nand left")]));
        let out = render(&p, true);
        assert_eq!(out, "msg\n\"she said \"\"hi\"\"\nand left\"\n");
    }

    #[test]
    fn matrix_csv_long_form() {
        let out = render(&fixture_matrix(), true);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "path,worktree,state");
        assert_eq!(lines.len(), 5);
    }
}
