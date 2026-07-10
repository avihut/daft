//! Global repo catalog — the data backbone of daft's Graph pillar.
//!
//! The catalog is a machine-local registry of every repo daft has touched:
//! identity (the `daft-id` UUID), name, location, remote, default branch,
//! and a removed-state that outlives the repo itself (so job logs stay
//! addressable and `daft clone <name>` can restore from the recorded
//! remote). It is daft's first *global* store — one SQLite file under the
//! data dir — as opposed to the per-repo coordinator DBs under the state
//! dir.
//!
//! ## Layering
//!
//! This module is a service layer over `store::{models, repos}`:
//! [`service::Catalog`] is the single choke point (no command imports
//! `rusqlite` or the repos layer), and the business rules — URL/name
//! normalization, collision suffixing, resolution precedence — are pure
//! functions in [`normalize`] tested without any database. That covers
//! what the ports/adapters spine exists to protect; a `CatalogPort` trait
//! belongs in `coordinator/ports/` the day the coordinator becomes a
//! consumer, not before.
//!
//! The trust registry (`repos.json`) is intentionally untouched: this
//! module performs **zero** `TrustDatabase` reads or writes. Migrating
//! trust/layout into the store is its own future PR.

pub mod fleet;
pub mod normalize;
pub mod registration;
pub mod relations;
pub mod service;

pub use registration::{gather_facts, note_repo_removed, register_repo, touch_current_repo};
pub use service::{
    Catalog, CatalogError, RegistrationFacts, RegistrationOutcome, effective_default_branch,
    resolve_repo_arg,
};
