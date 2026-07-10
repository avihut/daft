//! Inline TUI renderer for sync and prune operations.
//!
//! Uses ratatui with Viewport::Inline to render an operation header
//! and worktree status table that update in-place as tasks execute.

mod columns;
mod driver;
pub mod live_table;
pub mod operation_table;
mod presenter;
mod render;
pub mod shared_picker;
mod state;

pub use columns::Column;
pub use driver::{RawModeGuard, TuiRenderer, enable_raw_mode_guard};
pub use live_table::{LiveTable, LiveTableConfig};
pub use operation_table::{CompletedTable, OperationTable, TableConfig};
pub use presenter::TuiPresenter;
pub use state::{FinalStatus, PhaseState, PhaseStatus, TuiState, WorktreeRow, WorktreeStatus};
