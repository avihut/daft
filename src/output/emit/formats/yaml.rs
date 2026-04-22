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
