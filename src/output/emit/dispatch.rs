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
            Format::Json,
            Format::Ndjson,
            Format::Tsv,
            Format::Csv,
            Format::Yaml,
            Format::Toon,
            Format::Markdown,
        ],
        Shape::Document => &[Format::Json, Format::Yaml, Format::Toon, Format::Markdown],
        Shape::Matrix => &[
            Format::Json,
            Format::Ndjson,
            Format::Tsv,
            Format::Csv,
            Format::Yaml,
            Format::Toon,
            Format::Markdown,
        ],
        Shape::Sectioned => &[Format::Json, Format::Yaml, Format::Toon, Format::Markdown],
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
        eprintln!("warning: --no-headers has no effect with --format {fmt_name} (only tsv/csv)");
    }

    let shape = payload.shape();

    if let Some(tmpl) = &args.template {
        return formats::template::emit(shape, &payload, tmpl, writer);
    }

    let format = args
        .format
        .ok_or_else(|| EmitError::Other("emit() called without --format or --template".into()))?;

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
