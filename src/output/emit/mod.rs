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
