//! Serde-stable types shared between the coordinator IPC layer and the CLI.
//!
//! Lives outside `commands/` so the wire-protocol layer can reference these
//! types without taking a back-pointer into the high-level CLI module.

use serde::{Deserialize, Serialize};

/// Parsed composite job address: `[worktree:][invocation:]job_name`.
///
/// `job_name` is empty when the user supplied only an invocation prefix
/// (e.g. `daft hooks jobs logs 1f2b`); the resolver then auto-picks when
/// the invocation has a single job and prints a candidate list otherwise.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobAddress {
    pub worktree: Option<String>,
    pub invocation_prefix: Option<String>,
    pub job_name: String,
}

impl JobAddress {
    pub fn parse(input: &str) -> Self {
        // rsplitn splits from the right, so worktree (which may contain /)
        // stays intact as a single piece.
        let parts: Vec<&str> = input.rsplitn(3, ':').collect();
        match parts.len() {
            1 => {
                // Bare hex tokens (`1f2b`) are invocation prefixes, not job
                // names — same boundary as the `retry` resolver. A 9+ char
                // hex string or anything containing non-hex chars stays a
                // job name.
                if looks_like_inv_prefix(parts[0]) {
                    Self {
                        worktree: None,
                        invocation_prefix: Some(parts[0].to_string()),
                        job_name: String::new(),
                    }
                } else {
                    Self {
                        worktree: None,
                        invocation_prefix: None,
                        job_name: parts[0].to_string(),
                    }
                }
            }
            2 => {
                let left = parts[1];
                let right = parts[0];
                if left.contains('/') {
                    // Slash → unambiguously a worktree path. (`feature/auth:db-migrate`)
                    Self {
                        worktree: Some(left.to_string()),
                        invocation_prefix: None,
                        job_name: right.to_string(),
                    }
                } else if looks_like_inv_prefix(left) {
                    // Hex-shaped left → invocation:job. (`c9d4:db-migrate`)
                    // If both sides are hex, left wins as inv — worktrees
                    // are conventionally named, hex worktree names are an
                    // edge case the user can resolve via 3-segment input.
                    Self {
                        worktree: None,
                        invocation_prefix: Some(left.to_string()),
                        job_name: right.to_string(),
                    }
                } else if looks_like_inv_prefix(right) {
                    // Non-hex left + hex right → worktree:invocation drill-down,
                    // no job specified yet. (`feature:1f2b`) — the resolver
                    // auto-picks for single-job invocations or prints a
                    // candidate list otherwise.
                    Self {
                        worktree: Some(left.to_string()),
                        invocation_prefix: Some(right.to_string()),
                        job_name: String::new(),
                    }
                } else {
                    // Neither side hex-shaped, no slash → treat left as a
                    // worktree name. Real invocations are hex-only, so
                    // `feature:db-migrate` is overwhelmingly worktree:job.
                    Self {
                        worktree: Some(left.to_string()),
                        invocation_prefix: None,
                        job_name: right.to_string(),
                    }
                }
            }
            3 => Self {
                worktree: Some(parts[2].to_string()),
                invocation_prefix: Some(parts[1].to_string()),
                job_name: parts[0].to_string(),
            },
            _ => unreachable!(),
        }
    }

    pub fn with_inv_override(mut self, inv: Option<&str>) -> Self {
        if let Some(prefix) = inv {
            self.invocation_prefix = Some(prefix.to_string());
        }
        self
    }
}

/// True for tokens that match the invocation-short-id shape (2–8 ASCII hex
/// digits). Used by [`JobAddress::parse`] and re-exposed for `retry`'s
/// resolver so bare-token resolution stays consistent across `logs`,
/// `cancel`, and `retry`.
pub fn looks_like_inv_prefix(s: &str) -> bool {
    s.len() >= 2 && s.len() <= 8 && s.chars().all(|c| c.is_ascii_hexdigit())
}
