//! Shape-typed data payloads passed to `emit()`.
//!
//! A command builds one of the four shapes and hands it off to the emit
//! dispatcher, which picks the right format serializer.

use std::collections::BTreeMap;

/// Coarse classification of an `EmitPayload`'s shape; used by the dispatcher
/// to pick the right format serializer and to derive the supported-format set.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Shape {
    Tabular,
    Document,
    Matrix,
    Sectioned,
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Shape::Tabular => "tabular",
            Shape::Document => "document",
            Shape::Matrix => "matrix",
            Shape::Sectioned => "sectioned",
        }
    }
}

/// Atomic cell value in a Tabular or Matrix payload.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Str(String),
    Int(i64),
    Bool(bool),
    Null,
}

impl Cell {
    pub fn str(s: impl Into<String>) -> Self {
        Cell::Str(s.into())
    }
    pub fn int(i: impl Into<i64>) -> Self {
        Cell::Int(i.into())
    }
    pub fn bool(b: bool) -> Self {
        Cell::Bool(b)
    }
    pub fn null() -> Self {
        Cell::Null
    }
}

/// A flat rows-and-columns table.
#[derive(Clone, Debug)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    pub fn new(headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    /// Append a row. Panics if the row length does not match the header count.
    pub fn row(mut self, row: impl IntoIterator<Item = Cell>) -> Self {
        let row: Vec<Cell> = row.into_iter().collect();
        assert_eq!(
            row.len(),
            self.headers.len(),
            "Table row has {} cells but table has {} headers",
            row.len(),
            self.headers.len()
        );
        self.rows.push(row);
        self
    }
}

/// A 2D matrix of cells indexed by (row_label, col_label).
///
/// Long-form TSV/CSV emits one output row per populated (row_label, col_label)
/// pair using `row_key`, `col_key`, and `cell_key` as column names.
#[derive(Clone, Debug)]
pub struct Matrix {
    pub row_key: String,
    pub col_key: String,
    pub cell_key: String,
    pub rows: Vec<String>,
    pub cols: Vec<String>,
    pub cells: BTreeMap<(String, String), Cell>,
}

impl Matrix {
    pub fn new(
        row_key: impl Into<String>,
        col_key: impl Into<String>,
        cell_key: impl Into<String>,
    ) -> Self {
        Self {
            row_key: row_key.into(),
            col_key: col_key.into(),
            cell_key: cell_key.into(),
            rows: Vec::new(),
            cols: Vec::new(),
            cells: BTreeMap::new(),
        }
    }

    pub fn add_row(&mut self, label: impl Into<String>) {
        let label = label.into();
        if !self.rows.iter().any(|r| r == &label) {
            self.rows.push(label);
        }
    }

    pub fn add_col(&mut self, label: impl Into<String>) {
        let label = label.into();
        if !self.cols.iter().any(|c| c == &label) {
            self.cols.push(label);
        }
    }

    pub fn set(&mut self, row: impl Into<String>, col: impl Into<String>, value: Cell) {
        let row = row.into();
        let col = col.into();
        self.add_row(row.clone());
        self.add_col(col.clone());
        self.cells.insert((row, col), value);
    }
}

/// A named section inside a Sectioned payload. Payloads may nest.
#[derive(Clone, Debug)]
pub struct Section {
    pub name: String,
    pub payload: Box<EmitPayload>,
}

impl Section {
    pub fn new(name: impl Into<String>, payload: EmitPayload) -> Self {
        Self {
            name: name.into(),
            payload: Box::new(payload),
        }
    }
}

/// Top-level payload variants.
#[derive(Clone, Debug)]
pub enum EmitPayload {
    Tabular(Table),
    Document(serde_json::Value),
    Matrix(Matrix),
    Sectioned(Vec<Section>),
}

impl EmitPayload {
    pub fn shape(&self) -> Shape {
        match self {
            EmitPayload::Tabular(_) => Shape::Tabular,
            EmitPayload::Document(_) => Shape::Document,
            EmitPayload::Matrix(_) => Shape::Matrix,
            EmitPayload::Sectioned(_) => Shape::Sectioned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_builder_records_rows() {
        let t = Table::new(["a", "b"])
            .row([Cell::str("x"), Cell::int(1i64)])
            .row([Cell::str("y"), Cell::int(2i64)]);
        assert_eq!(t.headers, vec!["a", "b"]);
        assert_eq!(t.rows.len(), 2);
    }

    #[test]
    #[should_panic(expected = "Table row has 1 cells but table has 2 headers")]
    fn table_row_mismatch_panics() {
        Table::new(["a", "b"]).row([Cell::str("only")]);
    }

    #[test]
    fn matrix_tracks_distinct_rows_and_cols_in_insertion_order() {
        let mut m = Matrix::new("path", "worktree", "state");
        m.set("a.txt", "main", Cell::str("linked"));
        m.set("b.txt", "main", Cell::str("missing"));
        m.set("a.txt", "feature", Cell::str("materialized"));
        assert_eq!(m.rows, vec!["a.txt", "b.txt"]);
        assert_eq!(m.cols, vec!["main", "feature"]);
        assert_eq!(m.cells.len(), 3);
    }

    #[test]
    fn shape_returns_variant() {
        let empty_headers: [&str; 0] = [];
        assert_eq!(
            EmitPayload::Tabular(Table::new(empty_headers)).shape(),
            Shape::Tabular
        );
        assert_eq!(
            EmitPayload::Document(serde_json::json!({})).shape(),
            Shape::Document
        );
    }
}
