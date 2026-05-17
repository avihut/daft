//! Terminal text styling utilities.
//!
//! The implementation lives in the workspace-shared [`term_styles`] crate so
//! tooling (the YAML test runner in `xtask/`, future spin-outs) can reuse it
//! without depending on `daft` itself. This module re-exports the entire
//! public surface so existing call sites (`daft::styles::cyan`, etc.) keep
//! compiling.

pub use term_styles::*;
