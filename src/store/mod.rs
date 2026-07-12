//! Daft's structured-data store.
//!
//! This module is the **pure data layer**: it knows how to open a SQLite
//! file with daft's security defaults, run migrations, and round-trip rows
//! through the [`repos`] query layer. It deliberately knows nothing about
//! the coordinator, jobs as a concept, hooks, or CLI commands — those live
//! one layer up in the adapters and domain modules.
//!
//! See `CLAUDE.md`'s "Database & Storage" section for the conventions every
//! consumer inherits (PRAGMA bring-up, perm tightening, env scrub).
//!
//! ```text
//! Application
//! ├─ uses domain (pure)
//! │   └─ uses ports (traits)
//! │       └─ implemented by adapters
//! │           └─ uses store::{repos, models}     ← this module
//! │               └─ uses store::{connection, pool, migrate, paths}
//! ```

pub mod connection;
pub mod env_scrub;
pub mod error;
pub mod migrate;
pub mod models;
pub mod paths;
pub mod pool;
pub mod repos;

pub use error::{Result, StoreError};
pub use models::{CatalogRepoRow, InvocationRow, JobRow, RepoPolicyRow, VisitorSeedRow};
pub use pool::Pool;
pub use repos::{CatalogReposRepo, InvocationsRepo, JobsRepo, RepoPoliciesRepo, VisitorSeedsRepo};
