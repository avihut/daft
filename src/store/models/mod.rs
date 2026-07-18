//! Typed row structs that mirror SQL columns 1:1.
//!
//! Models hold *only* the data shape. Conversion to/from store rows lives in
//! `repos/`; mapping between model types and domain types lives in adapters.
//! Models intentionally avoid any business-logic methods so the store layer
//! stays a pure data-access layer.

pub mod catalog_repo;
pub mod forge_pr;
pub mod governor_event;
pub mod hook_profile;
pub mod invocation;
pub mod job;
pub mod repo_policy;
pub mod repo_size;
pub mod visitor_seed;
pub mod worktree_size;

pub use catalog_repo::CatalogRepoRow;
pub use forge_pr::ForgePrRow;
pub use governor_event::GovernorEventRow;
pub use hook_profile::HookProfileRow;
pub use invocation::InvocationRow;
pub use job::JobRow;
pub use repo_policy::RepoPolicyRow;
pub use repo_size::RepoSizeRow;
pub use visitor_seed::VisitorSeedRow;
pub use worktree_size::WorktreeSizeRow;
