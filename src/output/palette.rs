//! Shared 256-color accents for progress surfaces.
//!
//! The hook-progress block and the plan-execute timeline render as one
//! composed region (#651), so their scaffolding greys must match exactly.
//! These constants are the single source of truth; `hook_progress::formatting`
//! re-exports them for its internal use.

/// Mid grey — structural scaffolding (rails, box frames, durations).
pub(crate) const GREY: &str = "\x1b[38;5;245m";

/// Dark grey — quieter scaffolding (pending rows, expected skips).
pub(crate) const DARK_GREY: &str = "\x1b[38;5;240m";

/// Warm yellow — attention without alarm (skips worth noticing).
pub(crate) const YELLOW: &str = "\x1b[38;5;220m";

/// Soft blue — work handed to the background (coordinator jobs).
pub(crate) const BLUE: &str = "\x1b[38;5;75m";

/// Check whether progress visuals should be suppressed entirely.
///
/// True when running unit tests (`cfg!(test)`) or when `DAFT_TESTING` is set
/// (integration tests invoking the binary as a subprocess). Lifted from
/// `hook_progress::formatting::output_suppressed` so the timeline and the
/// hook renderer share one predicate.
pub(crate) fn testing_suppressed() -> bool {
    cfg!(test) || std::env::var("DAFT_TESTING").is_ok()
}
