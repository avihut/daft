//! Port: persistent job/invocation/policy storage for the coordinator.
//!
//! The contract surfaces the existing JobStore method shape directly. The
//! port returns store row models (`InvocationRow`, `JobRow`) rather than
//! a bespoke `JobView` — the row *is* the contract today, and a JobView
//! abstraction would just rename fields without adding meaning. If a
//! second consumer ever needs a different projection, introduce it then.
//!
//! `RepoPolicy` is a domain type from [`crate::coordinator::clean_policy`],
//! not a row model — adapters translate row ↔ policy internally so callers
//! deal with the type the rest of the coordinator already uses.

use crate::coordinator::clean_policy::RepoPolicy;
use crate::store::models::{InvocationRow, JobRow};
use anyhow::Result;

pub trait JobsStorePort: Send + Sync {
    fn upsert_invocation(&self, row: &InvocationRow) -> Result<()>;

    fn get_invocation(&self, repo_hash: &str, invocation_id: &str)
    -> Result<Option<InvocationRow>>;

    fn upsert_job(&self, row: &JobRow) -> Result<()>;

    fn get_job(&self, repo_hash: &str, invocation_id: &str, name: &str) -> Result<Option<JobRow>>;

    /// All jobs for a repo across every invocation, ordered by
    /// `started_at ASC`.
    fn list_jobs_for_repo(&self, repo_hash: &str) -> Result<Vec<JobRow>>;

    /// All jobs in one invocation, ordered by `started_at ASC`. Tab
    /// completion batches with this to make one DB query instead of N.
    fn list_jobs_for_invocation(&self, repo_hash: &str, invocation_id: &str)
    -> Result<Vec<JobRow>>;

    /// Jobs whose status is `Running` or `Cancelling` — i.e. anything the
    /// reconciler might need to confirm or mark `Crashed`.
    fn list_active_jobs(&self, repo_hash: &str) -> Result<Vec<JobRow>>;

    /// Return the persisted policy, or [`RepoPolicy::defaults`] if no row
    /// exists.
    fn read_repo_policy(&self, repo_hash: &str) -> Result<RepoPolicy>;

    /// Field-merge write. Explicit `Some(_)` in `policy` wins; `None`
    /// preserves the stored value. Matches the previous JSON-sidecar
    /// behavior — hooks without a `log:` block produce an all-`None`
    /// policy and must not clobber persisted tuning.
    fn write_repo_policy(&self, repo_hash: &str, policy: &RepoPolicy) -> Result<()>;
}
