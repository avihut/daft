//! redb table definitions and schema versioning.
//!
//! Values are bincode-encoded; keys are composite UTF-8 strings so range scans
//! over a `repo_hash` or `(repo_hash, invocation_id)` prefix work cheaply.

use redb::TableDefinition;

/// Per-invocation header. Key: `"{repo_hash}:{invocation_id}"`.
pub const INVOCATIONS: TableDefinition<&str, &[u8]> = TableDefinition::new("invocations_v1");

/// Per-job record. Key: `"{repo_hash}:{invocation_id}:{job_name}"`.
pub const JOBS: TableDefinition<&str, &[u8]> = TableDefinition::new("jobs_v1");

/// Per-repo cleanup policy. Key: `"{repo_hash}"`.
pub const REPO_POLICY: TableDefinition<&str, &[u8]> = TableDefinition::new("repo_policy_v1");

/// Scalar metadata. The store records its schema version under key `"schema_version"`.
pub const META: TableDefinition<&str, u64> = TableDefinition::new("meta_v1");

/// Current on-disk schema version.
///
/// Bump together with a migration in `migrate.rs`. Stored under `META["schema_version"]`.
/// On open, a binary refuses to write to a database whose stored version is higher than
/// `SCHEMA_VERSION` — older binaries don't pretend to understand newer data.
pub const SCHEMA_VERSION: u64 = 1;

pub const SCHEMA_VERSION_KEY: &str = "schema_version";
