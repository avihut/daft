//! Multi-remote support for daft.
//!
//! This module provides functionality for organizing worktrees by remote
//! when working with multiple remotes (e.g., fork workflows with `origin` and `upstream`).
//!
//! # Directory Structure
//!
//! When multi-remote mode is disabled (default):
//! ```text
//! project/
//! ├── .git/
//! ├── main/
//! └── feature/foo/
//! ```
//!
//! When multi-remote mode is enabled:
//! ```text
//! project/
//! ├── .git/
//! ├── origin/
//! │   ├── main/
//! │   └── feature/foo/
//! └── upstream/
//!     └── main/
//! ```
//!
//! # Configuration
//!
//! Multi-remote mode is controlled by git config:
//! - `daft.multiRemote.enabled` - Enable/disable remote-prefixed paths
//! - `daft.multiRemote.defaultRemote` - Default remote for new branches

pub mod config;
pub mod migration;
pub mod path;

pub use config::*;
pub use migration::*;
pub use path::*;
