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
pub use dispatch::{EmitError, emit, is_broken_pipe};
pub use format::Format;
pub use payload::{Cell, EmitPayload, Matrix, Section, Shape, Table};

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
