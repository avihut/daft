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
    Err(EmitError::Other(format!(
        "{}: not implemented",
        module_path!()
    )))
}
