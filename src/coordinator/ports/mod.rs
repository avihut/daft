//! Hexagonal *ports* for the coordinator.
//!
//! Ports are pure trait declarations. The domain layer depends on these
//! abstractions; adapters in [`crate::coordinator::adapters`] supply the
//! concrete implementations (SQLite, the system clock, Unix signals).
//!
//! Hard rules (see `CLAUDE.md` "Store / Ports / Adapters / Domain Pattern"):
//!
//! * Domain code imports from `ports/`, never from `store::*` or `nix::*`.
//! * Ports may surface store row types (the data shape *is* the contract);
//!   they must not surface raw `rusqlite::Connection` or other adapter
//!   internals.

pub mod clock;
pub mod process;
pub mod store;

pub use clock::Clock;
pub use process::ProcessControl;
pub use store::JobsStorePort;
