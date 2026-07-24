//! Row model for the `worktree_identities` table.

use chrono::{DateTime, Utc};

/// What kind of worktree an identity row describes.
///
/// Discriminates the two meanings the `branch` column can carry: the branch a
/// worktree is for (`Branch`), or the directory name of an anonymous sandbox
/// (`Canonical`/`Fork`). Every branch-keyed delete must filter on this, or a
/// branch removal could take a like-named sandbox's record with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreeKind {
    /// A worktree created for a branch — the 009 meaning, and the backfill
    /// default for rows written before the column existed.
    Branch,
    /// The shared detached sandbox `daft go <commit-ish>` visits: at most one
    /// per commit, matched by go's resolution.
    Canonical,
    /// A private detached sandbox from `daft start --fork`: never matched by
    /// go's resolution, reachable only by its directory name.
    Fork,
}

impl WorktreeKind {
    /// Stable on-disk / machine-facing name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Branch => "branch",
            Self::Canonical => "canonical",
            Self::Fork => "fork",
        }
    }

    /// Parse the on-disk value. Unknown values (a newer daft's kind) degrade
    /// to `Branch` — the conservative reading, since branch rows only ever
    /// gate display fallbacks.
    pub fn parse(s: &str) -> Self {
        match s {
            "canonical" => Self::Canonical,
            "fork" => Self::Fork,
            _ => Self::Branch,
        }
    }

    /// Whether the row describes an anonymous sandbox rather than a branch
    /// worktree.
    pub fn is_sandbox(self) -> bool {
        !matches!(self, Self::Branch)
    }
}

/// The branch a worktree was created for — or, for sandbox rows
/// (`kind != Branch`), the sandbox's directory-name identity.
///
/// Read only when live git state cannot name the branch — a plain detached
/// checkout — and as a cross-check for drift. Derived state always wins: this
/// row can be out of date, and an in-progress operation never can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeIdentityRow {
    pub repo_hash: String,
    /// The worktree's private-gitdir id: the directory name under
    /// `<common-dir>/worktrees/`. Stable across `git worktree move` and
    /// branch renames, which is why it is the key rather than the path.
    pub worktree_id: String,
    /// The branch this worktree is for, e.g. `feat/x` — or the directory name
    /// when `kind` says the row is a sandbox.
    pub branch: String,
    /// Absolute worktree path, for display and eviction.
    pub worktree_path: String,
    pub updated_at: DateTime<Utc>,
    /// Which meaning `branch` carries; see [`WorktreeKind`].
    pub kind: WorktreeKind,
    /// The spelling that created a sandbox (`v1.18.0`, `origin/master`,
    /// `HEAD~2`). `None` on branch rows.
    pub source_spelling: Option<String>,
    /// Full OID a sandbox was pinned at when created. `None` on branch rows.
    /// Lets removal warn when HEAD moved, and dedupes different spellings of
    /// one commit onto one canonical sandbox.
    pub pinned_commit: Option<String>,
}
