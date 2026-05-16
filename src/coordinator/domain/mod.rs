//! Coordinator *domain* layer.
//!
//! Pure business logic that talks to the world only through the traits in
//! [`crate::coordinator::ports`]. No `rusqlite`, no `nix`, no `Utc::now` —
//! tests inject mock adapters and assert behavior without any external
//! state.
//!
//! As the SQLite cutover progresses, additional functions currently in
//! [`crate::coordinator::process`] will be lifted in here (lifecycle,
//! cancel, …). [`reconcile`] lands first to validate the layering and
//! give future extractions a working template.

pub mod reconcile;
pub mod retention;

pub use reconcile::reconcile_active_jobs;
