//! Typed row structs that mirror SQL columns 1:1.
//!
//! Models hold *only* the data shape. Conversion to/from store rows lives in
//! `repos/`; mapping between model types and domain types lives in adapters.
//! Models intentionally avoid any business-logic methods so the store layer
//! stays a pure data-access layer.

pub mod catalog_repo;
pub mod invocation;
pub mod job;
pub mod repo_policy;
pub mod visitor_seed;

pub use catalog_repo::CatalogRepoRow;
pub use invocation::InvocationRow;
pub use job::JobRow;
pub use repo_policy::RepoPolicyRow;
pub use visitor_seed::VisitorSeedRow;
