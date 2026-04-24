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
