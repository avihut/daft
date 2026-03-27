//! Interactive TUI for collecting declared-but-uncollected shared files.
//!
//! Presents a tabbed interface where each tab represents a declared shared
//! file. The user selects which worktree's copy to promote to shared storage,
//! with a syntax-highlighted preview of the file content.

mod highlight;
mod input;
mod render;
pub mod state;

pub use state::CollectPickerState;
