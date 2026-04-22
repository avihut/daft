//! JSON serializer. Pretty-printed, two-space indent.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::{Cell, EmitPayload, Matrix, Section, Table};
use std::io::Write;

pub fn emit<W: Write>(payload: &EmitPayload, writer: &mut W) -> Result<(), EmitError> {
    let value = to_json_value(payload);
    let rendered = serde_json::to_string_pretty(&value)?;
    writer.write_all(rendered.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(())
}

pub fn to_json_value(payload: &EmitPayload) -> serde_json::Value {
    match payload {
        EmitPayload::Tabular(t) => tabular_value(t),
        EmitPayload::Document(d) => d.clone(),
        EmitPayload::Matrix(m) => matrix_value(m),
        EmitPayload::Sectioned(s) => sectioned_value(s),
    }
}

fn tabular_value(t: &Table) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = t
        .rows
        .iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for (h, c) in t.headers.iter().zip(row) {
                map.insert(h.clone(), cell_value(c));
            }
            serde_json::Value::Object(map)
        })
        .collect();
    serde_json::Value::Array(rows)
}

fn matrix_value(m: &Matrix) -> serde_json::Value {
    let mut outer = serde_json::Map::new();
    for row in &m.rows {
        let mut inner = serde_json::Map::new();
        for col in &m.cols {
            if let Some(v) = m.cells.get(&(row.clone(), col.clone())) {
                inner.insert(col.clone(), cell_value(v));
            }
        }
        outer.insert(row.clone(), serde_json::Value::Object(inner));
    }
    serde_json::json!({ &m.row_key: outer })
}

fn sectioned_value(sections: &[Section]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for s in sections {
        map.insert(s.name.clone(), to_json_value(&s.payload));
    }
    serde_json::Value::Object(map)
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
    fn tabular_json() {
        let out = render(&fixture_tabular());
        let expected = r#"[
  {
    "name": "alpha",
    "size": 42,
    "enabled": true
  },
  {
    "name": "béta, with comma",
    "size": 0,
    "enabled": false
  },
  {
    "name": "gamma",
    "size": null,
    "enabled": true
  }
]
"#;
        assert_eq!(out, expected);
    }

    #[test]
    fn matrix_json_is_nested_object() {
        let out = render(&fixture_matrix());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["path"]["shared/foo.txt"]["main"],
            serde_json::Value::String("linked".into())
        );
        assert_eq!(
            v["path"]["shared/bar.txt"]["feat"],
            serde_json::Value::String("missing".into())
        );
    }

    #[test]
    fn sectioned_json_has_named_sections() {
        let out = render(&fixture_sectioned());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(v["remotes"].is_array());
        assert!(v["worktrees"].is_array());
    }

    #[test]
    fn document_json_preserves_shape() {
        let out = render(&fixture_document());
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["title"], "Release 1.2");
    }
}
