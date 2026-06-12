//! `invocations` row.
//!
//! `status` / `skip_reason` are free strings at the storage boundary (same
//! convention as `jobs.status`); the constants below define the vocabulary
//! at the edges so unknown values written by a newer daft still round-trip.

use chrono::{DateTime, Utc};

/// The hook fire ran (the historical meaning of an invocation row).
pub const INVOCATION_STATUS_COMPLETED: &str = "completed";
/// The hook fire did not run; `skip_reason` says why.
pub const INVOCATION_STATUS_SKIPPED: &str = "skipped";

/// Skipped because the repository's trust level is Deny.
pub const SKIP_REASON_UNTRUSTED: &str = "untrusted";
/// Skipped because trust level is Prompt and no interactive callback was
/// available (includes fingerprint-mismatch downgrades).
pub const SKIP_REASON_PROMPT_UNAVAILABLE: &str = "prompt-unavailable";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvocationRow {
    pub repo_hash: String,
    pub invocation_id: String,
    pub trigger_command: String,
    pub hook_type: String,
    pub worktree: String,
    pub created_at: DateTime<Utc>,
    pub coordinator_pid: Option<u32>,
    pub status: String,
    pub skip_reason: Option<String>,
}
