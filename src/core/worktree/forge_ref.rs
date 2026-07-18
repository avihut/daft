//! Shared PR/MR branch-tracking ref format.
//!
//! A forge PR/MR checkout records its provenance in the branch's `merge` config
//! (`branch.<name>.merge = refs/pull/123/head` for GitHub,
//! `refs/merge-requests/45/head` for GitLab). This module is the single source
//! of truth for that ref shape — written by forge checkout
//! (`GitCommand::set_branch_tracking`) and read back by `daft list`'s PR column
//! (`get_forge_branch_ref`), so the two can never drift.
//!
//! It lives in `core` (not `src/forge/`) deliberately: `core` must not depend on
//! the forge CLI layer, and both the writer and the list reader are core-level.

/// The forge a tracked PR/MR ref belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeRefKind {
    /// GitHub Pull Request (`refs/pull/<n>/head`).
    GithubPr,
    /// GitLab Merge Request (`refs/merge-requests/<n>/head`).
    GitlabMr,
}

impl ForgeRefKind {
    /// Stable short tag: `"pr"` / `"mr"`. The canonical spelling everywhere a
    /// kind becomes a string — the forge-PR cache's TEXT column, the fork
    /// tracking-ref namespace (`refs/remotes/<remote>/pr/N`), the user-facing
    /// `pr:`/`mr:` prefixes.
    pub fn tag(self) -> &'static str {
        match self {
            ForgeRefKind::GithubPr => "pr",
            ForgeRefKind::GitlabMr => "mr",
        }
    }
}

/// A PR/MR reference recovered from (or written to) a branch's tracking config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgeBranchRef {
    pub kind: ForgeRefKind,
    pub number: u32,
}

impl ForgeBranchRef {
    pub fn new(kind: ForgeRefKind, number: u32) -> Self {
        Self { kind, number }
    }

    /// The full merge ref written to `branch.<name>.merge`.
    pub fn merge_ref(&self) -> String {
        match self.kind {
            ForgeRefKind::GithubPr => format!("refs/pull/{}/head", self.number),
            ForgeRefKind::GitlabMr => format!("refs/merge-requests/{}/head", self.number),
        }
    }

    /// Parse a `branch.<name>.merge` value back into a `ForgeBranchRef`.
    ///
    /// Returns `None` for ordinary branch merge refs (`refs/heads/...`) and any
    /// value that doesn't match the exact `refs/{pull,merge-requests}/<n>/head`
    /// shape.
    pub fn parse_merge_ref(merge_ref: &str) -> Option<Self> {
        for (prefix, kind) in [
            ("refs/pull/", ForgeRefKind::GithubPr),
            ("refs/merge-requests/", ForgeRefKind::GitlabMr),
        ] {
            if let Some(rest) = merge_ref.strip_prefix(prefix) {
                let number: u32 = rest.strip_suffix("/head")?.parse().ok()?;
                return Some(Self { kind, number });
            }
        }
        None
    }

    /// Compact display for the `daft list` PR column: `#123` for a GitHub PR,
    /// `!45` for a GitLab MR (each platform's native notation).
    pub fn short(&self) -> String {
        match self.kind {
            ForgeRefKind::GithubPr => format!("#{}", self.number),
            ForgeRefKind::GitlabMr => format!("!{}", self.number),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_ref_matches_platform_convention() {
        assert_eq!(
            ForgeBranchRef::new(ForgeRefKind::GithubPr, 123).merge_ref(),
            "refs/pull/123/head"
        );
        assert_eq!(
            ForgeBranchRef::new(ForgeRefKind::GitlabMr, 45).merge_ref(),
            "refs/merge-requests/45/head"
        );
    }

    #[test]
    fn short_uses_native_notation() {
        assert_eq!(
            ForgeBranchRef::new(ForgeRefKind::GithubPr, 123).short(),
            "#123"
        );
        assert_eq!(
            ForgeBranchRef::new(ForgeRefKind::GitlabMr, 45).short(),
            "!45"
        );
    }

    #[test]
    fn parse_recovers_github_pr() {
        assert_eq!(
            ForgeBranchRef::parse_merge_ref("refs/pull/123/head"),
            Some(ForgeBranchRef::new(ForgeRefKind::GithubPr, 123))
        );
    }

    #[test]
    fn parse_recovers_gitlab_mr() {
        assert_eq!(
            ForgeBranchRef::parse_merge_ref("refs/merge-requests/45/head"),
            Some(ForgeBranchRef::new(ForgeRefKind::GitlabMr, 45))
        );
    }

    #[test]
    fn parse_rejects_ordinary_branches() {
        assert_eq!(ForgeBranchRef::parse_merge_ref("refs/heads/main"), None);
        assert_eq!(
            ForgeBranchRef::parse_merge_ref("refs/heads/feature/x"),
            None
        );
        assert_eq!(ForgeBranchRef::parse_merge_ref(""), None);
    }

    #[test]
    fn parse_rejects_malformed_pr_refs() {
        // Missing /head suffix.
        assert_eq!(ForgeBranchRef::parse_merge_ref("refs/pull/123"), None);
        // Non-numeric number.
        assert_eq!(ForgeBranchRef::parse_merge_ref("refs/pull/abc/head"), None);
        // Empty number.
        assert_eq!(ForgeBranchRef::parse_merge_ref("refs/pull//head"), None);
        // Trailing garbage after the shape.
        assert_eq!(
            ForgeBranchRef::parse_merge_ref("refs/pull/123/head/extra"),
            None
        );
    }

    #[test]
    fn parse_merge_ref_roundtrips() {
        for r in [
            ForgeBranchRef::new(ForgeRefKind::GithubPr, 1),
            ForgeBranchRef::new(ForgeRefKind::GithubPr, 99999),
            ForgeBranchRef::new(ForgeRefKind::GitlabMr, 7),
        ] {
            assert_eq!(ForgeBranchRef::parse_merge_ref(&r.merge_ref()), Some(r));
        }
    }
}
