//! Tera template serializer. Works for every shape; context is built by
//! converting the payload to its json-value representation.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::formats::json::to_json_value;
use crate::output::emit::payload::{EmitPayload, Shape};
use std::io::Write;
use tera::{Context, Error, Kwargs, State, Tera, TeraResult, Value};

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
            for (k, v) in map {
                ctx.insert(k, &v);
            }
        }
        other => {
            ctx.insert("data", &other);
        }
    }
    // Tabular emits as an array at the top level; lift to `items` for template use.
    if matches!(payload, EmitPayload::Tabular(_)) {
        ctx.insert("items", &to_json_value(payload));
    }

    // tera 2.0.0 rewrote its template engine and trimmed the built-in filter
    // set, dropping `json_encode`. Re-register it so `--template` users keep the
    // ability to serialize a value to JSON (e.g. `{{ items | json_encode() }}`).
    // `Tera::one_off` builds a throwaway instance that can't carry custom
    // filters, so construct one explicitly and render with autoescape off.
    let mut tera = Tera::default();
    tera.register_filter("json_encode", json_encode);
    let rendered = tera
        .render_str(template, &ctx, false)
        .map_err(|e| EmitError::Other(format!("template error: {e}")))?;
    writer.write_all(rendered.as_bytes())?;
    if !rendered.ends_with('\n') {
        writer.write_all(b"\n")?;
    }
    Ok(())
}

/// Serialize a value to a compact JSON string.
///
/// Restores the `json_encode` filter that Tera shipped as a built-in through
/// 1.x and removed in 2.0.0. Tera's `Value` implements `serde::Serialize`, so
/// this routes through `serde_json` just as the old built-in did.
fn json_encode(value: Value, _: Kwargs, _: &State) -> TeraResult<String> {
    serde_json::to_string(&value).map_err(Error::message)
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

    #[test]
    fn json_encode_filter_serializes_values() {
        // Regression: tera 2.0.0 removed the built-in `json_encode` filter;
        // daft re-registers it so `--template` keeps JSON-encoding support.
        let out = render(&fixture_document(), "{{ title | json_encode() }}");
        assert_eq!(out, "\"Release 1.2\"\n");

        let arr = render(&fixture_tabular(), "{{ items | json_encode() }}");
        assert!(
            arr.trim_start().starts_with('['),
            "expected a JSON array, got: {arr}"
        );
        assert!(
            arr.contains("alpha"),
            "expected item name in JSON, got: {arr}"
        );
    }
}
