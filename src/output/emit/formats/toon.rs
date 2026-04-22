//! TOON serializer via the `toon-format` crate. Delegates to json.rs for Value.
//!
//! Note: the `toon-format` crate exposes `encode_default(&value)` (not
//! `to_string`). The plan assumed a `to_string` API; the actual public function
//! is `toon_format::encode_default`.

use crate::output::emit::dispatch::EmitError;
use crate::output::emit::formats::json::to_json_value;
use crate::output::emit::payload::EmitPayload;
use std::io::Write;

pub fn emit<W: Write>(payload: &EmitPayload, writer: &mut W) -> Result<(), EmitError> {
    let value = to_json_value(payload);
    let rendered =
        toon_format::encode_default(&value).map_err(|e| EmitError::Other(format!("toon: {e}")))?;
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
