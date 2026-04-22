# Multi-Format Emit Support — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `--json` boolean flag with a unified `--format <FORMAT>`
and `--template <STR>` surface across seven daft commands, supporting seven
declarative formats (`json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`)
plus Tera-based templating.

**Architecture:** New module `src/output/emit/` owns the abstraction. Command
data is modelled as one of four shapes (`Tabular`, `Document`, `Matrix`,
`Sectioned`). Each format file implements serializers per-shape; a dispatcher
pairs `(shape, format)` at runtime and errors on unsupported combinations.
Commands flatten a shared `EmitArgs` clap struct and route to `emit::emit` when
structured output is requested; their existing human-friendly pretty printers
stay untouched.

**Tech Stack:** Rust, clap 4.5 (derive + flatten + ValueEnum + conflicts_with),
`serde_json`, `serde_yaml`, `csv`, `tera`, `toon-format`, lefthook, mise, YAML
integration scenarios (`tests/manual/scenarios/`).

**Spec:**
`docs/superpowers/specs/2026-04-22-multi-format-emit-support-design.md`

---

## File map

### Create

- `src/output/emit/mod.rs` — public API: `emit()`, re-exports.
- `src/output/emit/format.rs` — `Format` enum (clap ValueEnum), Display.
- `src/output/emit/args.rs` — `EmitArgs` clap struct with `#[command(flatten)]`
  usage.
- `src/output/emit/payload.rs` — `EmitPayload`, `Table`, `Matrix`, `Section`,
  `Cell`, shape enum, builders.
- `src/output/emit/dispatch.rs` — `(shape, format)` dispatch, `UnsupportedCombo`
  error, broken-pipe helper.
- `src/output/emit/test_fixtures.rs` — `fixture_tabular()`,
  `fixture_document()`, `fixture_matrix()`, `fixture_sectioned()`.
- `src/output/emit/formats/mod.rs` — module declarations.
- `src/output/emit/formats/json.rs` — all four shapes.
- `src/output/emit/formats/ndjson.rs` — tabular, matrix.
- `src/output/emit/formats/tsv.rs` — tabular, matrix (long-form).
- `src/output/emit/formats/csv.rs` — tabular, matrix (long-form).
- `src/output/emit/formats/yaml.rs` — all four shapes.
- `src/output/emit/formats/toon.rs` — all four shapes.
- `src/output/emit/formats/markdown.rs` — all four shapes (table for
  Tabular/Matrix, prose for Document).
- `src/output/emit/formats/template.rs` — Tera, all four shapes.
- `docs/guide/output-formats.md` — shared reference.
- `tests/manual/scenarios/list/format-tsv.yml`, `format-csv.yml`,
  `format-ndjson.yml`, `format-yaml.yml`, `format-toon.yml`,
  `format-markdown.yml`, `format-template.yml`, `format-errors.yml`.
- Same pattern of scenario files for the six other commands (details in Phase
  4).

### Modify

- `Cargo.toml` — add `csv`, `tera`, `toon-format` under `[dependencies]`.
- `src/output/mod.rs` — declare `pub mod emit;`.
- `src/commands/list.rs` — remove `json: bool`, flatten `EmitArgs`, replace
  `print_json` with `emit::emit(EmitPayload::Tabular(...))`, update `long_about`
  (remove `--json` reference).
- `src/commands/release_notes.rs` — remove `json: bool`, flatten `EmitArgs`,
  route to `emit::emit(EmitPayload::Document(...))`.
- `src/commands/hooks/trust.rs` — flatten `EmitArgs` into the `list` subcommand
  args, route to `emit::emit(EmitPayload::Tabular(...))`.
- `src/commands/layout.rs` — flatten `EmitArgs` into the `list` subcommand args,
  route.
- `src/commands/shared.rs` — flatten `EmitArgs` into the `status` subcommand
  args, route to `EmitPayload::Matrix(...)`.
- `src/commands/multi_remote.rs` — flatten `EmitArgs` into the `status`
  subcommand args, route to `EmitPayload::Sectioned(...)`.
- `src/commands/hooks/run_cmd.rs` — flatten `EmitArgs` into the listing-mode
  path, route to `EmitPayload::Sectioned(...)`.
- `tests/manual/scenarios/list/columns-json.yml`, `branches-json.yml`, any other
  existing `*--json*` scenarios → rewrite to `--format json`.
- `docs/cli/daft-list.md`, `daft-release-notes.md`, `daft-hooks-list.md`,
  `daft-layout-list.md`, `daft-shared-status.md`, `daft-multi-remote-status.md`,
  `daft-hooks-run.md` — add "Structured Output" sections linking to the shared
  guide.
- `docs/guide/*.md` — update any stray `--json` references.
- `SKILL.md` — document the new flag surface (per CLAUDE.md's rule for feature
  changes).
- `CHANGELOG.md` — breaking-change entry.
- `src/commands/completions/{bash,zsh,fish,fig}.rs` — verify flag completions
  auto-generate correctly for `--format` ValueEnum; update any hardcoded
  references.

### Regenerate

- `man/*.1` via `mise run man:gen`.

---

## Self-review requirements

Before marking the plan complete:

1. `mise run fmt && mise run clippy && mise run test:unit && mise run test:integration`
   all pass.
2. `mise run man:verify` passes (regenerated man pages are committed).
3. Running `daft list --json` exits non-zero with clap's "unknown argument"
   error (guards the break).

---

## Phase 1 — Core infrastructure (shapes, args, dispatch; no formats yet)

### Task 1: Add crate dependencies

**Files:**

- Modify: `Cargo.toml`

- [ ] **Step 1: Add deps to `[dependencies]` block**

In `Cargo.toml`, under `[dependencies]`, add (keeping alphabetical-ish grouping
near existing entries):

```toml
csv = "1.3"
tera = { version = "1.20", default-features = false }
toon-format = "0.2"
```

- [ ] **Step 2: Verify build**

Run: `cargo build 2>&1 | tail -20` Expected: builds cleanly; new crates resolve.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "build: add csv, tera, toon-format deps for multi-format emit"
```

---

### Task 2: Create emit module skeleton

**Files:**

- Create: `src/output/emit/mod.rs`
- Create: `src/output/emit/formats/mod.rs`
- Modify: `src/output/mod.rs`

- [ ] **Step 1: Create empty module files**

`src/output/emit/mod.rs`:

```rust
//! Structured output emission for commands that produce machine-readable data.
//!
//! Commands build an [`EmitPayload`] and pass it with an [`EmitArgs`] to
//! [`emit`], which dispatches to the correct serializer based on the payload
//! shape and the requested format.

pub mod args;
pub mod dispatch;
pub mod format;
pub mod payload;

mod formats;

#[cfg(test)]
mod test_fixtures;

pub use args::EmitArgs;
pub use dispatch::{emit, EmitError};
pub use format::Format;
pub use payload::{Cell, EmitPayload, Matrix, Section, Shape, Table};
```

`src/output/emit/formats/mod.rs`:

```rust
//! Per-format serializers. Each file implements the shapes it supports.

pub mod csv;
pub mod json;
pub mod markdown;
pub mod ndjson;
pub mod template;
pub mod toon;
pub mod tsv;
pub mod yaml;
```

In `src/output/mod.rs`, add alongside the existing `pub mod` entries:

```rust
pub mod emit;
```

- [ ] **Step 2: Build must fail because submodules are missing**

Run: `cargo build 2>&1 | head -10` Expected: errors about missing modules
(`args`, `dispatch`, etc.).

- [ ] **Step 3: Commit wiring only after the rest of Phase 1 is done** (end of
      Task 7, not here).

---

### Task 3: `Format` enum

**Files:**

- Create: `src/output/emit/format.rs`

- [ ] **Step 1: Write the format enum + tests**

```rust
//! The `Format` enum and helpers.

use std::fmt;

/// User-selectable output format, one per `--format <value>` enum variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum Format {
    Json,
    Ndjson,
    Tsv,
    Csv,
    Yaml,
    Toon,
    Markdown,
}

impl Format {
    pub const ALL: &'static [Format] = &[
        Format::Json,
        Format::Ndjson,
        Format::Tsv,
        Format::Csv,
        Format::Yaml,
        Format::Toon,
        Format::Markdown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "json",
            Format::Ndjson => "ndjson",
            Format::Tsv => "tsv",
            Format::Csv => "csv",
            Format::Yaml => "yaml",
            Format::Toon => "toon",
            Format::Markdown => "markdown",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_kebab_case() {
        assert_eq!(Format::Json.to_string(), "json");
        assert_eq!(Format::Ndjson.to_string(), "ndjson");
        assert_eq!(Format::Markdown.to_string(), "markdown");
    }

    #[test]
    fn all_covers_every_variant() {
        assert_eq!(Format::ALL.len(), 7);
        assert!(Format::ALL.contains(&Format::Toon));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::format -- --nocapture` Expected: 2 tests
PASS.

---

### Task 4: Payload shape types

**Files:**

- Create: `src/output/emit/payload.rs`

- [ ] **Step 1: Write payload types + builders + tests**

```rust
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
    pub fn str(s: impl Into<String>) -> Self { Cell::Str(s.into()) }
    pub fn int(i: impl Into<i64>) -> Self { Cell::Int(i.into()) }
    pub fn bool(b: bool) -> Self { Cell::Bool(b) }
    pub fn null() -> Self { Cell::Null }
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

    pub fn set(
        &mut self,
        row: impl Into<String>,
        col: impl Into<String>,
        value: Cell,
    ) {
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
        Self { name: name.into(), payload: Box::new(payload) }
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
            .row([Cell::str("x"), Cell::int(1)])
            .row([Cell::str("y"), Cell::int(2)]);
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
        assert_eq!(
            EmitPayload::Tabular(Table::new::<[&str; 0], &str>([])).shape(),
            Shape::Tabular
        );
        assert_eq!(
            EmitPayload::Document(serde_json::json!({})).shape(),
            Shape::Document
        );
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::payload -- --nocapture` Expected: 4 tests
PASS.

---

### Task 5: `EmitArgs` clap struct

**Files:**

- Create: `src/output/emit/args.rs`

- [ ] **Step 1: Write args + tests**

```rust
//! Shared clap args flattened into every command that supports structured emit.

use crate::output::emit::format::Format;

#[derive(clap::Args, Debug, Clone)]
pub struct EmitArgs {
    /// Output format. Mutually exclusive with --template.
    #[arg(long, value_enum, value_name = "FORMAT", conflicts_with = "template")]
    pub format: Option<Format>,

    /// Tera template string. Mutually exclusive with --format.
    #[arg(long, value_name = "STR", conflicts_with = "format")]
    pub template: Option<String>,

    /// Omit header row (tsv/csv only).
    #[arg(long)]
    pub no_headers: bool,
}

impl EmitArgs {
    /// True when the user requested structured emit (via --format or --template).
    pub fn is_structured(&self) -> bool {
        self.format.is_some() || self.template.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(flatten)]
        emit: EmitArgs,
    }

    #[test]
    fn default_is_unstructured() {
        let h = Harness::parse_from(["bin"]);
        assert!(!h.emit.is_structured());
    }

    #[test]
    fn format_sets_structured() {
        let h = Harness::parse_from(["bin", "--format", "json"]);
        assert!(h.emit.is_structured());
        assert_eq!(h.emit.format, Some(Format::Json));
    }

    #[test]
    fn template_sets_structured() {
        let h = Harness::parse_from(["bin", "--template", "{{ x }}"]);
        assert!(h.emit.is_structured());
    }

    #[test]
    fn format_and_template_conflict() {
        let err = Harness::try_parse_from([
            "bin", "--format", "json", "--template", "{{ x }}",
        ])
        .unwrap_err();
        assert!(err.to_string().contains("cannot be used with"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::args -- --nocapture` Expected: 4 tests
PASS.

---

### Task 6: Dispatch skeleton + `UnsupportedCombo` error

**Files:**

- Create: `src/output/emit/dispatch.rs`

- [ ] **Step 1: Write dispatch + error + supported-set helper + tests**

```rust
//! Shape × format dispatch and the `UnsupportedCombo` error.

use crate::output::emit::args::EmitArgs;
use crate::output::emit::format::Format;
use crate::output::emit::formats;
use crate::output::emit::payload::{EmitPayload, Shape};
use std::io::Write;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EmitError {
    #[error("'{command}' does not support --format {requested}\n  supported formats: {supported}")]
    UnsupportedCombo {
        command: String,
        requested: String,
        supported: String,
    },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

/// Formats declaratively supported by each shape.
///
/// This is the single source of truth for the support matrix in the spec.
pub fn supported_formats(shape: Shape) -> &'static [Format] {
    match shape {
        Shape::Tabular => &[
            Format::Json, Format::Ndjson, Format::Tsv, Format::Csv,
            Format::Yaml, Format::Toon, Format::Markdown,
        ],
        Shape::Document => &[
            Format::Json, Format::Yaml, Format::Toon, Format::Markdown,
        ],
        Shape::Matrix => &[
            Format::Json, Format::Ndjson, Format::Tsv, Format::Csv,
            Format::Yaml, Format::Toon, Format::Markdown,
        ],
        Shape::Sectioned => &[
            Format::Json, Format::Yaml, Format::Toon, Format::Markdown,
        ],
    }
}

/// Names a command for error messages. Commands pass their canonical invocation
/// (e.g. `"git-worktree-list"`, `"release-notes"`, `"hooks list"`).
pub fn emit<W: Write>(
    command: &str,
    payload: EmitPayload,
    args: &EmitArgs,
    writer: &mut W,
) -> Result<(), EmitError> {
    // --no-headers has no effect outside tsv/csv; warn once.
    if args.no_headers && !matches!(args.format, Some(Format::Tsv) | Some(Format::Csv)) {
        let fmt_name = args.format.map(|f| f.as_str()).unwrap_or("template");
        eprintln!(
            "warning: --no-headers has no effect with --format {fmt_name} (only tsv/csv)"
        );
    }

    let shape = payload.shape();

    if let Some(tmpl) = &args.template {
        return formats::template::emit(shape, &payload, tmpl, writer);
    }

    let format = args.format.ok_or_else(|| EmitError::Other(
        "emit() called without --format or --template".into()
    ))?;

    let supported = supported_formats(shape);
    if !supported.contains(&format) {
        let supported_list = supported
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(EmitError::UnsupportedCombo {
            command: command.to_string(),
            requested: format.to_string(),
            supported: supported_list,
        });
    }

    let headers = !args.no_headers;
    match (format, &payload) {
        (Format::Json, p) => formats::json::emit(p, writer),
        (Format::Ndjson, p) => formats::ndjson::emit(p, writer),
        (Format::Tsv, p) => formats::tsv::emit(p, headers, writer),
        (Format::Csv, p) => formats::csv::emit(p, headers, writer),
        (Format::Yaml, p) => formats::yaml::emit(p, writer),
        (Format::Toon, p) => formats::toon::emit(p, writer),
        (Format::Markdown, p) => formats::markdown::emit(p, writer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::emit::payload::{Section, Table};

    #[test]
    fn supported_sets_match_spec_matrix() {
        assert_eq!(supported_formats(Shape::Tabular).len(), 7);
        assert_eq!(supported_formats(Shape::Document).len(), 4);
        assert_eq!(supported_formats(Shape::Matrix).len(), 7);
        assert_eq!(supported_formats(Shape::Sectioned).len(), 4);
        assert!(!supported_formats(Shape::Document).contains(&Format::Tsv));
        assert!(!supported_formats(Shape::Sectioned).contains(&Format::Ndjson));
    }

    #[test]
    fn unsupported_combo_includes_supported_list_in_message() {
        let payload = EmitPayload::Document(serde_json::json!({}));
        let args = EmitArgs {
            format: Some(Format::Tsv),
            template: None,
            no_headers: false,
        };
        let mut buf = Vec::new();
        let err = emit("release-notes", payload, &args, &mut buf).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'release-notes' does not support --format tsv"));
        assert!(msg.contains("supported formats: json, yaml, toon, markdown"));
    }
}
```

- [ ] **Step 2: Add `thiserror` to deps if not present**

Check: `grep -n '^thiserror' Cargo.toml` If missing, add `thiserror = "2.0"`
under `[dependencies]`.

- [ ] **Step 3: Build fails because format stubs are missing**

Run: `cargo build 2>&1 | head -20` Expected: errors that `formats::json::emit`
etc. are undefined — we fix in Task 7.

---

### Task 7: Format stubs + fixtures + wiring

**Files:**

- Create: `src/output/emit/formats/json.rs`, `ndjson.rs`, `tsv.rs`, `csv.rs`,
  `yaml.rs`, `toon.rs`, `markdown.rs`, `template.rs` (stubs)
- Create: `src/output/emit/test_fixtures.rs`

- [ ] **Step 1: Create a stub for each format**

Each format file starts with:

```rust
//! <FORMAT> serializer. Filled in during Phase 2.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::EmitPayload;
use std::io::Write;

pub fn emit<W: Write>(_payload: &EmitPayload, _writer: &mut W) -> Result<(), EmitError> {
    Err(EmitError::Other(format!("{}: not implemented", module_path!())))
}
```

The tsv and csv stubs take an extra `_headers: bool` parameter:

```rust
//! TSV (or CSV) serializer. Filled in during Phase 2.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::EmitPayload;
use std::io::Write;

pub fn emit<W: Write>(
    _payload: &EmitPayload,
    _headers: bool,
    _writer: &mut W,
) -> Result<(), EmitError> {
    Err(EmitError::Other(format!("{}: not implemented", module_path!())))
}
```

The template stub takes shape + template + payload:

```rust
//! Template serializer. Filled in during Phase 2.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::payload::{EmitPayload, Shape};
use std::io::Write;

pub fn emit<W: Write>(
    _shape: Shape,
    _payload: &EmitPayload,
    _template: &str,
    _writer: &mut W,
) -> Result<(), EmitError> {
    Err(EmitError::Other(format!("{}: not implemented", module_path!())))
}
```

- [ ] **Step 2: Create shared test fixtures**

`src/output/emit/test_fixtures.rs`:

```rust
//! Shared fixtures used across per-format unit tests.
//!
//! Chosen to exercise: nulls, unicode, embedded commas/tabs/newlines, and a
//! realistic row count.

use crate::output::emit::payload::{Cell, EmitPayload, Matrix, Section, Table};

pub fn fixture_tabular() -> EmitPayload {
    EmitPayload::Tabular(
        Table::new(["name", "size", "enabled"])
            .row([Cell::str("alpha"), Cell::int(42), Cell::bool(true)])
            .row([Cell::str("béta, with comma"), Cell::int(0), Cell::bool(false)])
            .row([Cell::str("gamma"), Cell::null(), Cell::bool(true)]),
    )
}

pub fn fixture_document() -> EmitPayload {
    EmitPayload::Document(serde_json::json!({
        "title": "Release 1.2",
        "date": "2026-04-22",
        "sections": [
            {"heading": "Features", "items": ["foo", "bar"]},
            {"heading": "Fixes", "items": ["baz"]},
        ],
    }))
}

pub fn fixture_matrix() -> EmitPayload {
    let mut m = Matrix::new("path", "worktree", "state");
    m.set("shared/foo.txt", "main", Cell::str("linked"));
    m.set("shared/foo.txt", "feat", Cell::str("materialized"));
    m.set("shared/bar.txt", "main", Cell::str("linked"));
    m.set("shared/bar.txt", "feat", Cell::str("missing"));
    EmitPayload::Matrix(m)
}

pub fn fixture_sectioned() -> EmitPayload {
    EmitPayload::Sectioned(vec![
        Section::new(
            "remotes",
            EmitPayload::Tabular(
                Table::new(["name", "url", "is_default"])
                    .row([
                        Cell::str("origin"),
                        Cell::str("git@host:org/repo.git"),
                        Cell::bool(true),
                    ]),
            ),
        ),
        Section::new(
            "worktrees",
            EmitPayload::Tabular(
                Table::new(["branch", "remote", "path"])
                    .row([
                        Cell::str("main"),
                        Cell::str("origin"),
                        Cell::str("/w/main"),
                    ]),
            ),
        ),
    ])
}
```

- [ ] **Step 3: Build passes**

Run: `cargo build 2>&1 | tail -5` Expected: clean build.

- [ ] **Step 4: Full unit tests pass (stubs, payload, args, format, dispatch)**

Run: `cargo test --lib output::emit -- --nocapture` Expected: all Phase 1 tests
PASS.

- [ ] **Step 5: Commit Phase 1**

```bash
git add src/output Cargo.toml Cargo.lock
git commit -m "feat(emit): scaffold structured output module with shape-typed payloads"
```

---

## Phase 2 — Format serializers (TDD, one format at a time)

Each task follows the same pattern: write byte-exact tests using the shared
fixtures, implement the serializer, verify, commit.

### Task 8: JSON format (all four shapes)

**Files:**

- Modify: `src/output/emit/formats/json.rs`

- [ ] **Step 1: Write tests**

Replace stub content with:

```rust
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
    let rows: Vec<serde_json::Value> = t.rows.iter().map(|row| {
        let mut map = serde_json::Map::new();
        for (h, c) in t.headers.iter().zip(row) {
            map.insert(h.clone(), cell_value(c));
        }
        serde_json::Value::Object(map)
    }).collect();
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::formats::json` Expected: 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/output/emit/formats/json.rs
git commit -m "feat(emit): json serializer for all four shapes"
```

---

### Task 9: NDJSON format (tabular, matrix)

**Files:**

- Modify: `src/output/emit/formats/ndjson.rs`

- [ ] **Step 1: Write tests + implementation**

```rust
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
        assert!(lines.iter().any(|l| l.contains(r#""path":"shared/foo.txt""#)));
        assert!(lines.iter().any(|l| l.contains(r#""state":"missing""#)));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::formats::ndjson` Expected: 2 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/output/emit/formats/ndjson.rs
git commit -m "feat(emit): ndjson serializer for tabular and matrix shapes"
```

---

### Task 10: TSV format (tabular + matrix long-form, whitespace normalization)

**Files:**

- Modify: `src/output/emit/formats/tsv.rs`

- [ ] **Step 1: Write tests + implementation**

```rust
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
        writeln!(writer, "{}", t.headers.iter().map(|s| normalize(s)).collect::<Vec<_>>().join("\t"))?;
    }
    for row in &t.rows {
        let cells: Vec<String> = row.iter().map(cell_to_str).map(|s| normalize(&s)).collect();
        writeln!(writer, "{}", cells.join("\t"))?;
    }
    Ok(())
}

fn write_matrix_long<W: Write>(m: &Matrix, headers: bool, writer: &mut W) -> Result<(), EmitError> {
    if headers {
        writeln!(writer, "{}\t{}\t{}", normalize(&m.row_key), normalize(&m.col_key), normalize(&m.cell_key))?;
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
        .map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c })
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
        let p = EmitPayload::Tabular(
            Table::new(["a"])
                .row([Cell::str("x\ty\nz")])
        );
        let out = render(&p, true);
        assert_eq!(out, "a\nx y z\n");
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::formats::tsv` Expected: 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/output/emit/formats/tsv.rs
git commit -m "feat(emit): tsv serializer with whitespace normalization and matrix long-form"
```

---

### Task 11: CSV format (tabular + matrix long-form, RFC 4180)

**Files:**

- Modify: `src/output/emit/formats/csv.rs`

- [ ] **Step 1: Write tests + implementation**

```rust
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
    let mut w = ::csv::WriterBuilder::new().has_headers(false).from_writer(writer);
    if headers {
        w.write_record(&t.headers).map_err(|e| EmitError::Other(e.to_string()))?;
    }
    for row in &t.rows {
        let cells: Vec<String> = row.iter().map(cell_to_str).collect();
        w.write_record(&cells).map_err(|e| EmitError::Other(e.to_string()))?;
    }
    w.flush()?;
    Ok(())
}

fn write_matrix_long<W: Write>(m: &Matrix, headers: bool, writer: &mut W) -> Result<(), EmitError> {
    let mut w = ::csv::WriterBuilder::new().has_headers(false).from_writer(writer);
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
        let p = EmitPayload::Tabular(
            Table::new(["msg"])
                .row([Cell::str("she said \"hi\"\nand left")])
        );
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test --lib output::emit::formats::csv` Expected: 3 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/output/emit/formats/csv.rs
git commit -m "feat(emit): csv serializer via csv crate with matrix long-form"
```

---

### Task 12: YAML format (all four shapes)

**Files:**

- Modify: `src/output/emit/formats/yaml.rs`

- [ ] **Step 1: Write tests + implementation**

```rust
//! YAML serializer via serde_yaml. Delegates to json.rs for the `Value` model.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::formats::json::to_json_value;
use crate::output::emit::payload::EmitPayload;
use std::io::Write;

pub fn emit<W: Write>(payload: &EmitPayload, writer: &mut W) -> Result<(), EmitError> {
    let value = to_json_value(payload);
    let rendered = serde_yaml::to_string(&value).map_err(|e| EmitError::Other(e.to_string()))?;
    writer.write_all(rendered.as_bytes())?;
    Ok(())
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
    fn tabular_yaml_is_sequence_of_maps() {
        let out = render(&fixture_tabular());
        assert!(out.contains("- name: alpha"));
        assert!(out.contains("size: 42"));
        assert!(out.contains("béta, with comma"));
    }

    #[test]
    fn document_yaml_preserves_shape() {
        let out = render(&fixture_document());
        assert!(out.contains("title: Release 1.2"));
        assert!(out.contains("- foo"));
    }

    #[test]
    fn matrix_yaml_is_nested_map() {
        let out = render(&fixture_matrix());
        assert!(out.contains("path:"));
        assert!(out.contains("shared/foo.txt:"));
        assert!(out.contains("main: linked"));
    }

    #[test]
    fn sectioned_yaml_has_named_sections() {
        let out = render(&fixture_sectioned());
        assert!(out.contains("remotes:"));
        assert!(out.contains("worktrees:"));
    }
}
```

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib output::emit::formats::yaml
git add src/output/emit/formats/yaml.rs
git commit -m "feat(emit): yaml serializer for all four shapes"
```

---

### Task 13: TOON format (all four shapes)

**Files:**

- Modify: `src/output/emit/formats/toon.rs`

- [ ] **Step 1: Write tests + implementation**

```rust
//! TOON serializer via the `toon-format` crate. Delegates to json.rs for Value.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::formats::json::to_json_value;
use crate::output::emit::payload::EmitPayload;
use std::io::Write;

pub fn emit<W: Write>(payload: &EmitPayload, writer: &mut W) -> Result<(), EmitError> {
    let value = to_json_value(payload);
    let rendered = toon_format::to_string(&value)
        .map_err(|e| EmitError::Other(format!("toon: {e}")))?;
    writer.write_all(rendered.as_bytes())?;
    if !rendered.ends_with('\n') {
        writer.write_all(b"\n")?;
    }
    Ok(())
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
    fn tabular_toon_nonempty() {
        let out = render(&fixture_tabular());
        assert!(!out.is_empty());
        assert!(out.contains("alpha"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn document_toon_nonempty() {
        let out = render(&fixture_document());
        assert!(out.contains("title"));
        assert!(out.contains("Release 1.2"));
    }

    #[test]
    fn matrix_toon_nonempty() {
        let out = render(&fixture_matrix());
        assert!(out.contains("path"));
        assert!(out.contains("shared/foo.txt"));
    }

    #[test]
    fn sectioned_toon_nonempty() {
        let out = render(&fixture_sectioned());
        assert!(out.contains("remotes"));
        assert!(out.contains("worktrees"));
    }
}
```

Note: TOON's exact output is crate-version-specific, so these are substring
assertions rather than byte-exact. Byte-exact regression is covered by the JSON
→ Value intermediate; if TOON serialization changes version-to-version, the
`to_json_value` input is unchanged.

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib output::emit::formats::toon
git add src/output/emit/formats/toon.rs
git commit -m "feat(emit): toon serializer via toon-format crate"
```

---

### Task 14: Markdown format (table for Tabular/Matrix, prose for Document, H2-sectioned tables for Sectioned)

**Files:**

- Modify: `src/output/emit/formats/markdown.rs`

- [ ] **Step 1: Write tests + implementation**

````rust
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
    if t.headers.is_empty() { return Ok(()); }
    writeln!(writer, "| {} |", t.headers.join(" | "))?;
    writeln!(writer, "| {} |", t.headers.iter().map(|_| "---").collect::<Vec<_>>().join(" | "))?;
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
    writeln!(writer, "| {} |", header.iter().map(|_| "---").collect::<Vec<_>>().join(" | "))?;
    for row in &m.rows {
        let mut cells = vec![row.clone()];
        for col in &m.cols {
            let v = m.cells.get(&(row.clone(), col.clone()))
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
        if !s.ends_with('\n') { writer.write_all(b"\n")?; }
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
        if i > 0 { writeln!(writer)?; }
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
````

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib output::emit::formats::markdown
git add src/output/emit/formats/markdown.rs
git commit -m "feat(emit): markdown serializer with per-shape rendering"
```

---

### Task 15: Template format (Tera)

**Files:**

- Modify: `src/output/emit/formats/template.rs`

- [ ] **Step 1: Write tests + implementation**

```rust
//! Tera template serializer. Works for every shape; context is built by
//! converting the payload to its json-value representation.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::formats::json::to_json_value;
use crate::output::emit::payload::{EmitPayload, Shape};
use std::io::Write;
use tera::{Context, Tera};

pub fn emit<W: Write>(
    _shape: Shape,
    payload: &EmitPayload,
    template: &str,
    writer: &mut W,
) -> Result<(), EmitError> {
    let value = to_json_value(payload);

    let mut ctx = Context::new();
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map { ctx.insert(k, &v); }
        }
        other => { ctx.insert("data", &other); }
    }
    // Tabular emits as an array at the top level; lift to `items` for template use.
    if matches!(payload, EmitPayload::Tabular(_)) {
        ctx.insert("items", &to_json_value(payload));
    }

    let rendered = Tera::one_off(template, &ctx, false)
        .map_err(|e| EmitError::Other(format!("template error: {e}")))?;
    writer.write_all(rendered.as_bytes())?;
    if !rendered.ends_with('\n') {
        writer.write_all(b"\n")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::emit::test_fixtures::*;

    fn render(p: &EmitPayload, template: &str) -> String {
        let mut buf = Vec::new();
        emit(p.shape(), p, template, &mut buf).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn tabular_template_iterates_items() {
        let out = render(
            &fixture_tabular(),
            "{% for r in items %}{{ r.name }}:{{ r.size }}\n{% endfor %}",
        );
        assert!(out.contains("alpha:42\n"));
        assert!(out.contains("béta, with comma:0\n"));
    }

    #[test]
    fn document_template_reads_top_level_keys() {
        let out = render(&fixture_document(), "{{ title }} ({{ date }})");
        assert_eq!(out, "Release 1.2 (2026-04-22)\n");
    }

    #[test]
    fn syntax_error_produces_clear_error() {
        let mut buf = Vec::new();
        let err = emit(
            Shape::Document,
            &fixture_document(),
            "{{ unterminated",
            &mut buf,
        )
        .unwrap_err();
        assert!(err.to_string().contains("template error"));
    }
}
```

- [ ] **Step 2: Run tests + commit**

```bash
cargo test --lib output::emit::formats::template
git add src/output/emit/formats/template.rs
git commit -m "feat(emit): tera template serializer"
```

---

### Task 16: Broken-pipe handling in dispatch

**Files:**

- Modify: `src/output/emit/dispatch.rs`

- [ ] **Step 1: Add broken-pipe detection and a public helper**

Append to `src/output/emit/dispatch.rs`:

```rust
/// Returns true if an error is a broken-pipe IO error.
pub fn is_broken_pipe(err: &EmitError) -> bool {
    matches!(err, EmitError::Io(e) if e.kind() == std::io::ErrorKind::BrokenPipe)
}

#[cfg(test)]
mod pipe_tests {
    use super::*;

    #[test]
    fn broken_pipe_is_detected() {
        let e = EmitError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        assert!(is_broken_pipe(&e));
    }

    #[test]
    fn other_io_error_is_not_broken_pipe() {
        let e = EmitError::Io(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(!is_broken_pipe(&e));
    }
}
```

Export from `src/output/emit/mod.rs`:

```rust
pub use dispatch::{emit, is_broken_pipe, EmitError};
```

- [ ] **Step 2: Add the shared `emit_and_handle` convenience**

Append to `src/output/emit/mod.rs`:

```rust
/// Runs `emit` and converts broken-pipe errors into `Ok(())` — matches the
/// behaviour every command needs when their stdout is closed by `head`, `less q`,
/// etc. Returns (exit_code_hint, ...) via Result where broken pipe is Ok.
pub fn emit_and_handle<W: std::io::Write>(
    command: &str,
    payload: payload::EmitPayload,
    args: &EmitArgs,
    writer: &mut W,
) -> Result<(), EmitError> {
    match dispatch::emit(command, payload, args, writer) {
        Err(e) if dispatch::is_broken_pipe(&e) => Ok(()),
        other => other,
    }
}
```

- [ ] **Step 3: Test + commit**

```bash
cargo test --lib output::emit -- --nocapture
git add src/output/emit/dispatch.rs src/output/emit/mod.rs
git commit -m "feat(emit): broken-pipe passthrough helper"
```

---

## Phase 3 — Command integration

Each integration task follows the same pattern: flatten `EmitArgs`, build the
appropriate payload when structured output is requested, call `emit_and_handle`,
keep the existing human-friendly printer path for non-structured output.

### Task 17: Integrate `list` command

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Add the flatten + is_structured branch**

In `src/commands/list.rs`:

1. Add
   `use crate::output::emit::{self, EmitArgs, EmitPayload, Table, Cell, Section};`
   (adjust imports as needed).
2. Remove the `#[arg(long, help = "Output in JSON format")] json: bool,` field
   from `Args`.
3. In `Args`, add at the top of the field list:

```rust
#[command(flatten)]
emit: EmitArgs,
```

4. In `long_about` (around line 64), replace the `--json` paragraph:

```rust
Use --format to emit machine-readable output suitable for scripting.
Supported formats: json, ndjson, tsv, csv, yaml, toon, markdown. Use
--template '<tera>' for custom output. See the Structured Output guide
for details.
```

5. Replace the existing `if args.json { return print_json(...); }` block with:

```rust
if args.emit.is_structured() {
    let table = build_emit_table(&infos, &project_root, &cwd, stat, selected_columns, now);
    return emit::emit_and_handle(
        "git-worktree-list",
        EmitPayload::Tabular(table),
        &args.emit,
        &mut std::io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

6. Port the body of the existing `print_json` into a new `build_emit_table` that
   returns `Table`. Preserve field order and names exactly as they appear today
   in the JSON output (the current implementation is authoritative). Map
   `Option<u64>` sizes to `Cell::Int` or `Cell::Null`, booleans to `Cell::Bool`,
   strings to `Cell::Str`, numeric ahead/behind to `Cell::Int`. Append the size
   summary row only when `ListColumn::Size` is selected, using `"TOTAL"` as the
   path sentinel and empty cells for non-summary columns.
7. Delete the old `print_json` fn.

- [ ] **Step 2: Add a unit test over the builder**

In `src/commands/list.rs` under `#[cfg(test)]`:

```rust
#[test]
fn build_emit_table_preserves_column_order() {
    // Construct a minimal WorktreeInfo fixture and call build_emit_table.
    // Assert headers match the legacy --json field order for default columns.
    // (Exact fixture construction follows existing tests in this file.)
}
```

Flesh out the fixture using the existing test helpers in this file — reuse any
`WorktreeInfo` mock constructors already present. If none exist, keep the test
minimal: build one `WorktreeInfo` with literal values and assert `table.headers`
is the exact legacy default-column list.

- [ ] **Step 3: Verify**

```bash
cargo test --lib commands::list
cargo build --release
./target/release/daft list --format json | head
./target/release/daft list --format tsv | head
./target/release/daft list --json 2>&1 | head    # Expect: "unknown argument"
```

Expected: first two produce valid output; third errors with clap's "unexpected
argument '--json'".

- [ ] **Step 4: Commit**

```bash
git add src/commands/list.rs
git commit -m "feat(list): migrate --json to --format with full format matrix"
```

---

### Task 18: Integrate `release-notes` command

**Files:**

- Modify: `src/commands/release_notes.rs`

- [ ] **Step 1: Flatten EmitArgs + replace `--json` branch**

1. Remove `json: bool` from `Args`; add `#[command(flatten)] emit: EmitArgs,` at
   the top of the fields.
2. Replace the `if args.json { output_json(...) }` branch with:

```rust
if args.emit.is_structured() {
    let payload = build_release_notes_payload(&filtered_releases, args.list)?;
    return emit::emit_and_handle(
        "release-notes",
        payload,
        &args.emit,
        &mut io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

3. Add `build_release_notes_payload` that returns `EmitPayload::Document`:

```rust
fn build_release_notes_payload(
    releases: &[Release],
    list_mode: bool,
) -> Result<EmitPayload> {
    // When rendering as markdown, emit the raw release-notes prose as a JSON string
    // so the markdown format prints it verbatim (see formats/markdown.rs).
    // For all other formats we emit a structured document.
    let value = if list_mode {
        serde_json::to_value(
            releases.iter().map(|r| serde_json::json!({
                "version": r.version,
                "date": r.date,
            })).collect::<Vec<_>>()
        )?
    } else {
        serde_json::to_value(releases)?
    };
    Ok(EmitPayload::Document(value))
}
```

4. Markdown of release-notes: if the user requests `--format markdown`, we want
   the current prose rendering (not a JSON code block). Detect this in `run()`
   before `emit_and_handle` and short-circuit:

```rust
if args.emit.format == Some(emit::Format::Markdown) {
    // Pre-render to markdown prose and wrap as Document(String) — the markdown
    // format prints top-level strings verbatim.
    let prose = render_releases_markdown(&filtered_releases, args.list);
    let payload = EmitPayload::Document(serde_json::Value::String(prose));
    return emit::emit_and_handle(
        "release-notes",
        payload,
        &args.emit,
        &mut io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

`render_releases_markdown` reuses the existing pager-content rendering (extract
the prose-building portion of the current `output_full` / `output_list` into a
pure function that returns `String`).

5. Delete the old `output_json` fn.

- [ ] **Step 2: Verify and commit**

```bash
cargo build --release
./target/release/daft release-notes --format json | head
./target/release/daft release-notes --format markdown | head -20
./target/release/daft release-notes --format tsv 2>&1 | head
# Expect: "does not support --format tsv"
./target/release/daft release-notes --json 2>&1 | head
# Expect: "unknown argument"

git add src/commands/release_notes.rs
git commit -m "feat(release-notes): migrate --json to --format"
```

---

### Task 19: Integrate `hooks list`

**Files:**

- Modify: `src/commands/hooks/trust.rs`

- [ ] **Step 1: Locate the `list` subcommand's Args struct and `run` fn**

Find the list command in `trust.rs` (around lines 232–314 per the earlier
audit). Add `#[command(flatten)] emit: EmitArgs,` to its args.

- [ ] **Step 2: Replace the manual tabular output with a payload**

Create a builder `build_trust_table(entries: &[TrustEntry]) -> Table` that
builds a
`Table::new(["repo_path", "trust_level", "remote_fingerprint", "timestamp"])`
and appends one row per trust entry. In the list fn:

```rust
if args.emit.is_structured() {
    let table = build_trust_table(&entries);
    return emit::emit_and_handle(
        "hooks list",
        EmitPayload::Tabular(table),
        &args.emit,
        &mut std::io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
// existing human table rendering unchanged
```

- [ ] **Step 3: Verify and commit**

```bash
cargo build --release
./target/release/daft hooks list --format tsv | head
./target/release/daft hooks list --format yaml | head

git add src/commands/hooks/trust.rs
git commit -m "feat(hooks list): add --format support"
```

---

### Task 20: Integrate `layout list`

**Files:**

- Modify: `src/commands/layout.rs`

- [ ] **Step 1: Add flatten + routing**

Find the `list` subcommand args in `layout.rs` (around lines 142–244). Add
`#[command(flatten)] emit: EmitArgs,`. Before the existing tabular rendering:

```rust
if args.emit.is_structured() {
    let table = Table::new(["name", "template", "is_default", "is_selected"]);
    let table = layouts.iter().fold(table, |t, l| {
        t.row([
            Cell::str(&l.name),
            Cell::str(&l.template),
            Cell::bool(l.is_default),
            Cell::bool(l.is_selected),
        ])
    });
    return emit::emit_and_handle(
        "layout list",
        EmitPayload::Tabular(table),
        &args.emit,
        &mut std::io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

- [ ] **Step 2: Verify and commit**

```bash
cargo build --release
./target/release/daft layout list --format json

git add src/commands/layout.rs
git commit -m "feat(layout list): add --format support"
```

---

### Task 21: Integrate `shared status` (Matrix)

**Files:**

- Modify: `src/commands/shared.rs`

- [ ] **Step 1: Find the `status` subcommand args (around lines 459–560)**

Add `#[command(flatten)] emit: EmitArgs,`.

- [ ] **Step 2: Build a Matrix payload**

```rust
if args.emit.is_structured() {
    let mut m = Matrix::new("path", "worktree", "state");
    for (path, per_wt) in &status_by_path {
        for (wt_name, state) in per_wt {
            m.set(path.clone(), wt_name.clone(), Cell::str(state.as_str()));
        }
    }
    return emit::emit_and_handle(
        "shared status",
        EmitPayload::Matrix(m),
        &args.emit,
        &mut std::io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

(Adjust `status_by_path` / `state.as_str()` to match this module's actual types;
the shape is the point.)

- [ ] **Step 3: Verify and commit**

```bash
cargo build --release
./target/release/daft shared status --format tsv | head   # long-form
./target/release/daft shared status --format markdown     # wide pivot

git add src/commands/shared.rs
git commit -m "feat(shared status): add --format with Matrix payload"
```

---

### Task 22: Integrate `multi-remote status` (Sectioned)

**Files:**

- Modify: `src/commands/multi_remote.rs`

- [ ] **Step 1: Flatten EmitArgs into the `status` subcommand**

- [ ] **Step 2: Build Sectioned payload**

```rust
if args.emit.is_structured() {
    let remotes_table = remotes.iter().fold(
        Table::new(["name", "url", "is_default"]),
        |t, r| t.row([
            Cell::str(&r.name),
            Cell::str(&r.url),
            Cell::bool(r.is_default),
        ]),
    );
    let worktrees_table = worktrees.iter().fold(
        Table::new(["branch", "remote", "path"]),
        |t, w| t.row([
            Cell::str(&w.branch),
            Cell::str(&w.remote),
            Cell::str(w.path.to_string_lossy().as_ref()),
        ]),
    );
    let payload = EmitPayload::Sectioned(vec![
        Section::new("remotes", EmitPayload::Tabular(remotes_table)),
        Section::new("worktrees", EmitPayload::Tabular(worktrees_table)),
    ]);
    return emit::emit_and_handle(
        "multi-remote status",
        payload,
        &args.emit,
        &mut std::io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

- [ ] **Step 3: Verify and commit**

```bash
cargo build --release
./target/release/daft multi-remote status --format yaml
./target/release/daft multi-remote status --format tsv 2>&1 | head
# Expect: "does not support --format tsv"

git add src/commands/multi_remote.rs
git commit -m "feat(multi-remote status): add --format with Sectioned payload"
```

---

### Task 23: Integrate `hooks run` listing mode (Sectioned)

**Files:**

- Modify: `src/commands/hooks/run_cmd.rs`

- [ ] **Step 1: Flatten EmitArgs into the no-hook-specified listing path**

Around the existing `cmd_run_list_hooks` (per audit, ~lines 181-200). Build a
Sectioned payload with one section per hook type; each section holds a Tabular
with columns `job_name`, `description`, `tags`:

```rust
if args.emit.is_structured() {
    let sections: Vec<Section> = hooks_by_type
        .iter()
        .map(|(hook_type, jobs)| {
            let table = jobs.iter().fold(
                Table::new(["job_name", "description", "tags"]),
                |t, j| t.row([
                    Cell::str(j.name.as_deref().unwrap_or("")),
                    Cell::str(j.description.as_deref().unwrap_or("")),
                    Cell::str(j.tags.join(",")),
                ]),
            );
            Section::new(hook_type.as_str(), EmitPayload::Tabular(table))
        })
        .collect();
    return emit::emit_and_handle(
        "hooks run",
        EmitPayload::Sectioned(sections),
        &args.emit,
        &mut std::io::stdout(),
    ).map_err(|e| anyhow::anyhow!("{e}"));
}
```

- [ ] **Step 2: Verify and commit**

```bash
cargo build --release
./target/release/daft hooks run --format json

git add src/commands/hooks/run_cmd.rs
git commit -m "feat(hooks run): add --format to listing mode with Sectioned payload"
```

---

## Phase 4 — Integration tests (YAML scenarios)

### Task 24: Migrate existing `--json` scenarios for `list`

**Files:**

- Modify: `tests/manual/scenarios/list/columns-json.yml`, `branches-json.yml`,
  any other `*json*.yml` under `list/`.

- [ ] **Step 1: `grep -rn -- '--json' tests/manual/scenarios/list/` to
      enumerate**

Run:

```bash
grep -rn -- '--json' tests/manual/scenarios/list/
```

For each match, change `--json` to `--format json` in-place (same call,
equivalent output for json format).

- [ ] **Step 2: Run the list scenarios**

```bash
mise run test:manual -- --ci list
```

Expected: all scenarios pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/list/
git commit -m "test(list): migrate --json scenarios to --format json"
```

---

### Task 25: New format-coverage scenarios for `list`

**Files:**

- Create: `tests/manual/scenarios/list/format-ndjson.yml`, `format-tsv.yml`,
  `format-csv.yml`, `format-yaml.yml`, `format-toon.yml`, `format-markdown.yml`,
  `format-template.yml`, `format-errors.yml`.

- [ ] **Step 1: Template + implement each scenario**

Use an existing scenario in `tests/manual/scenarios/list/` as a structural
template (e.g. `columns-json.yml`). Each new file exercises one format with
sensible assertions. Example — `format-tsv.yml`:

```yaml
name: List TSV format
description: --format tsv emits tab-separated rows

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: TSV has header and tabbed rows
    run: git-worktree-list --format tsv
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_matches_regex: "^kind\tname"

  - name: --no-headers omits header row
    run: git-worktree-list --format tsv --no-headers
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_not_contains:
        - "kind\tname"
```

(If the scenario DSL does not support `output_matches_regex`, use
`output_contains`. Check the existing scenarios to see what the harness supports
before writing new ones.)

`format-errors.yml` covers:

- `git-worktree-list --json` → exit 2, stderr contains "unexpected argument".
- `git-worktree-list --format bogus` → exit 2, stderr lists valid values.
- `git-worktree-list --format json --template 'x'` → exit 2.

`format-template.yml` covers:

- `git-worktree-list --template '{% for r in items %}{{ r.name }}|{% endfor %}'`
  → exit 0, stdout contains `"|"`.

- [ ] **Step 2: Run the full list suite**

```bash
mise run test:manual -- --ci list
```

Expected: every new scenario passes.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/list/
git commit -m "test(list): format coverage scenarios for every --format value"
```

---

### Task 26: Format-coverage scenarios for the other six commands

**Files:**

- Create: `tests/manual/scenarios/<cmd>/format-*.yml` for each of:
  `release-notes` (new dir if needed), `hooks list` (under `hooks/`), `layout`,
  `shared`, `multi-remote`, `hooks run`.

- [ ] **Step 1: For each command, create at minimum:**

- `format-json.yml` — `--format json` produces valid JSON.
- `format-yaml.yml` — `--format yaml` exit 0, non-empty output.
- `format-errors.yml` — unsupported format × command errors; `--json` errors;
  `--format invalid` errors.
- `format-template.yml` — `--template '{{ ... }}'` works for that shape.

For tabular-shape commands (`hooks list`, `layout list`, `shared status`) also
include `format-tsv.yml`.

- [ ] **Step 2: Run them**

```bash
mise run test:manual -- --ci release-notes
mise run test:manual -- --ci hooks
mise run test:manual -- --ci layout
mise run test:manual -- --ci shared
mise run test:manual -- --ci multi-remote
```

Expected: every new scenario passes.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/
git commit -m "test: format coverage for release-notes, hooks, layout, shared, multi-remote"
```

---

## Phase 5 — Documentation

### Task 27: Shared output-formats guide

**Files:**

- Create: `docs/guide/output-formats.md`

- [ ] **Step 1: Write the page**

```markdown
---
title: Output Formats
description: Structured output via --format and --template across daft commands
---

# Output Formats

Seven daft commands can emit machine-readable output via the shared `--format`
flag: `list`, `release-notes`, `hooks list`, `layout list`, `shared status`,
`multi-remote status`, and `hooks run` (when called without a specific hook).

## Flags

- `--format <FORMAT>` — pick one of: `json`, `ndjson`, `tsv`, `csv`, `yaml`,
  `toon`, `markdown`. Mutually exclusive with `--template`.
- `--template <STR>` — render output with a
  [Tera](https://keats.github.io/tera/) template. Mutually exclusive with
  `--format`.
- `--no-headers` — omit the header row in `tsv` / `csv` output. Ignored (with a
  warning) for other formats.

## Per-command support

Not every format applies to every command. The supported sets:

| Command               | json | ndjson | tsv | csv | yaml | toon | markdown | template |
| --------------------- | :--: | :----: | :-: | :-: | :--: | :--: | :------: | :------: |
| `list`                |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks list`          |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `layout list`         |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `release-notes`       |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `shared status`       |  ✓   |   ✓    |  ✓  |  ✓  |  ✓   |  ✓   |    ✓     |    ✓     |
| `multi-remote status` |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |
| `hooks run` (listing) |  ✓   |   —    |  —  |  —  |  ✓   |  ✓   |    ✓     |    ✓     |

Requesting an unsupported combination prints a clear error listing the supported
formats for that command and exits 2.

## Formats

### json

Pretty-printed JSON, two-space indent. Safe to pipe into `jq`. Use `jq -c .` for
single-line output.

### ndjson

One JSON object per line. Streams naturally into line processors like `jq`,
`fq`, `mlr`, or `awk`. Tabular commands emit one row per line; matrix commands
emit one object per populated cell in long form.

### tsv

Tab-separated rows with a header row unless `--no-headers` is set. Cell values
containing tabs or newlines are replaced with a single space before emission —
TSV has no standard escaping, and preserving those bytes would break awk
pipelines. If you need raw content, use `csv` or `json`.

### csv

RFC 4180 CSV. Fields with commas, quotes, or newlines are double-quoted; quotes
inside a field are escaped by doubling.

### yaml

YAML 1.2. Preserves nested structure; good for configs and human reading.

### toon

[TOON](https://github.com/toon-format/spec) — token-efficient structured data,
designed for piping into LLM context. Roughly 30-50% fewer tokens than
equivalent JSON.

### markdown

For tabular commands, a GitHub-flavored markdown table. For `release-notes`, the
rendered prose notes (ready to paste into a GitHub release). For
`shared status`, a wide-form pivot table for quick visual reading.

## Templates

`--template` takes a [Tera](https://keats.github.io/tera/) template string. Tera
is a Jinja-inspired engine with good error messages and full control-flow.

### Context

- For `list`, `hooks list`, `layout list`, `shared status` — the template
  context exposes `items` as the array of rows.
- For `release-notes`, the context is the top-level document fields as
  variables.
- For `multi-remote status` and `hooks run` (listing) — the context is each
  section name as a variable binding the section's data.

### Examples

Print one branch per line from `daft list`:

    daft list --template "{% for r in items %}{{ r.name }}
    {% endfor %}"

Custom summary:

    daft list --template "{{ items | length }} worktrees"

Release titles only:

    daft release-notes --template "{% for r in releases %}{{ r.version }}
    {% endfor %}"

Syntax errors in your template print a line-and-column pointer to stderr and
exit 2.

## Errors

    error: 'release-notes' does not support --format tsv
      supported formats: json, yaml, toon, markdown

    error: invalid value 'bogus' for '--format <FORMAT>'
      [possible values: json, ndjson, tsv, csv, yaml, toon, markdown]

    error: the argument '--format <FORMAT>' cannot be used with '--template <STR>'
```

- [ ] **Step 2: Sanity-build docs site**

```bash
mise run docs:site:build
```

Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add docs/guide/output-formats.md
git commit -m "docs(guide): shared structured-output reference"
```

---

### Task 28: Per-command doc updates

**Files:**

- Modify: `docs/cli/daft-list.md`, `daft-release-notes.md`,
  `daft-hooks-list.md`, `daft-layout-list.md`, `daft-shared-status.md`,
  `daft-multi-remote-status.md`, `daft-hooks-run.md`.

- [ ] **Step 1: Add a `## Structured Output` section to each**

Use this template; adjust the supported-formats line and the example to match
the command. For `daft-list.md`:

````markdown
## Structured Output

`daft list` supports machine-readable output via `--format`: `json`, `ndjson`,
`tsv`, `csv`, `yaml`, `toon`, `markdown`, plus `--template <tera>` for custom
output.

```sh
# Two columns for awk / cut
daft list --format tsv --no-headers | cut -f2,3

# Pipe to jq
daft list --format json | jq '.[] | select(.is_current == true)'

# Custom one-liner per worktree
daft list --template '{% for r in items %}{{ r.name }} -> {{ r.path }}
{% endfor %}'
```
````

See the [Output Formats guide](../guide/output-formats.md) for format details
and Tera syntax.

````

For `daft-release-notes.md`, list supported formats as `json, yaml, toon,
markdown, template`. Example:

```sh
# Markdown prose, ready to paste into a GitHub release
daft release-notes 1.2.0 --format markdown

# JSON for tooling
daft release-notes --format json | jq '.[0].version'
````

- [ ] **Step 2: Remove any references to `--json` from the other guide pages**

```bash
grep -rn -- '--json' docs/
```

Rewrite each hit to `--format json` (or the new flag appropriate to the
context).

- [ ] **Step 3: Commit**

```bash
git add docs/
git commit -m "docs(cli): per-command structured-output sections; drop --json references"
```

---

### Task 29: Update `SKILL.md` and `CHANGELOG.md`

**Files:**

- Modify: `SKILL.md`, `CHANGELOG.md`

- [ ] **Step 1: Add a new section to SKILL.md**

Under whatever the existing structure is, add:

```markdown
## Structured output

Seven daft commands emit machine-readable output via `--format`:

- Flat-list: `list`, `hooks list`, `layout list`
- Document: `release-notes`
- Matrix: `shared status`
- Sectioned: `multi-remote status`, `hooks run` (listing mode)

Valid formats: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`.
Unsupported combinations print a clear error listing the supported set.

Use `--template '<tera-template>'` for custom output. Tera syntax: `{{ var }}`,
`{% for x in items %}...{% endfor %}`, `{% if %}...{% endif %}`.
```

- [ ] **Step 2: CHANGELOG entry**

Add an `## [Unreleased]` section at the top with:

```markdown
## [Unreleased]

### BREAKING

- The `--json` flag is removed from `daft list` and `daft release-notes`. Use
  `--format json` instead.

### Added

- `--format` on `list`, `release-notes`, `hooks list`, `layout list`,
  `shared status`, `multi-remote status`, `hooks run` with seven declarative
  formats: `json`, `ndjson`, `tsv`, `csv`, `yaml`, `toon`, `markdown`.
- `--template <tera>` for custom output on the same commands.
- `--no-headers` to suppress header rows in `tsv` / `csv` output.
```

- [ ] **Step 3: Commit**

```bash
git add SKILL.md CHANGELOG.md
git commit -m "docs: SKILL and CHANGELOG entries for multi-format emit"
```

---

### Task 30: Regenerate man pages

**Files:**

- Regenerate: `man/*.1`

- [ ] **Step 1: Regenerate**

```bash
mise run man:gen
```

- [ ] **Step 2: Verify**

```bash
mise run man:verify
```

Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add man/
git commit -m "docs(man): regenerate after --format flag addition"
```

---

## Phase 6 — Final validation & release prep

### Task 31: Shell completions sanity check

**Files:**

- Possibly modify: `src/commands/completions/{bash,zsh,fish,fig}.rs`

- [ ] **Step 1: Grep for any hardcoded `--json` references**

```bash
grep -n -- '--json' src/commands/completions/
```

If any hits, remove them. Then check that `--format` value completion works:

```bash
cargo build --release
./target/release/daft completions bash | grep -A2 -- '--format' | head
./target/release/daft completions zsh  | grep -A2 -- '--format' | head
./target/release/daft completions fish | grep -A2 -- '--format' | head
```

Expected: the seven format values appear as completion options (clap's ValueEnum
auto-generates these when `#[arg(value_enum)]` is used, as we did in Task 3).

- [ ] **Step 2: Commit (if changes needed)**

```bash
git add src/commands/completions/
git commit -m "build(completions): drop --json, propagate --format value set"
```

If no changes needed, skip this commit.

---

### Task 32: Full CI locally

- [ ] **Step 1: Run the full CI suite**

```bash
mise run ci
```

Expected: clean pass — `fmt:check`, `clippy` (zero warnings), `test:unit`,
`test:integration`, `man:verify`.

- [ ] **Step 2: If anything fails, fix the root cause, not the symptom.**

Re-run `mise run ci` after each fix. Commit fixes individually with clear
messages (`fix(emit): ...`).

- [ ] **Step 3: Confirm the regression guard manually**

```bash
./target/release/daft list --json 2>&1 | tee /dev/stderr | grep -qi "unexpected\|unknown"
./target/release/daft release-notes --json 2>&1 | grep -qi "unexpected\|unknown"
```

Expected: both print clap's "unexpected argument" message and exit non-zero.

---

### Task 33: Final push / PR

- [ ] **Step 1: Push the branch**

```bash
git push -u origin feat/multi-format-emit-support
```

- [ ] **Step 2: Open the PR**

Title: `feat!: multi-format emit support with --format flag`

Body:

```markdown
## Summary

- Replaces `--json` with a unified `--format <FORMAT>` across seven commands.
- Supports seven declarative formats (`json`, `ndjson`, `tsv`, `csv`, `yaml`,
  `toon`, `markdown`) plus `--template <tera>` for custom output.
- `--no-headers` suppresses header rows in `tsv` / `csv`.

## BREAKING CHANGE

`--json` is removed. Use `--format json` instead. See the Output Formats guide
at `docs/guide/output-formats.md` for the full surface.

## Test plan

- [ ] `mise run ci` passes locally.
- [ ] `daft list --json` errors cleanly.
- [ ] `daft list --format tsv | cut -f2,3` pipes sensibly.
- [ ] `daft release-notes --format markdown` produces paste-ready release notes.
- [ ] `daft shared status --format tsv` emits long-form triples.

Fixes the "multi-format emit support" future-work item.

Spec: `docs/superpowers/specs/2026-04-22-multi-format-emit-support-design.md`
Plan: `docs/superpowers/plans/2026-04-22-multi-format-emit-support.md`
```

- [ ] **Step 3: Tag per CLAUDE.md**

Assignee `avihut`, label `feat`, milestone `Public Launch`.

- [ ] **Step 4: Merge on green** (user action, not plan step).

---

## Appendix A — Common failure modes & fixes

- **Clippy fails on an unused import in a format stub.** Stubs in Task 7 keep
  unused `payload`/`format` imports. After the format is implemented in Phase 2,
  those imports become used. If you build clippy after Task 7 alone, use
  `#[allow(unused)]` inline or sequence Phase 2 before running clippy.
- **`toon-format` version drift changes test output.** Use substring assertions
  in TOON tests (as written in Task 13) — don't pin byte-exact.
- **YAML `-` prefix for sequences vs inline.** `serde_yaml` emits block style
  for structured fixtures and inline for empty ones. Tests use substring
  assertions to tolerate this.
- **`--no-headers` warning bleeds into test-captured stderr.** That's expected —
  scenarios check stderr for the warning when passing `--no-headers` with e.g.
  `--format json`.
- **Broken pipe integration test.** Hard to write portably in YAML scenarios.
  The unit test in `dispatch.rs::pipe_tests` is the authoritative check; trust
  it.
