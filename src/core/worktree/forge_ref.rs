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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// CI rollup state of a PR/MR, as cached from the forge. Lives here (not in
/// `src/forge/`) for the same reason as [`ForgeBranchRef`]: the renderers that
/// decorate the `daft list` PR column are core/output-level and must not
/// depend on the forge CLI layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiStatus {
    Pass,
    Fail,
    Pending,
}

impl CiStatus {
    /// The TEXT value persisted in the forge-PR cache.
    pub fn as_str(self) -> &'static str {
        match self {
            CiStatus::Pass => "pass",
            CiStatus::Fail => "fail",
            CiStatus::Pending => "pending",
        }
    }

    /// Inverse of [`Self::as_str`] for cache reads; unknown text is `None`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pass" => Some(CiStatus::Pass),
            "fail" => Some(CiStatus::Fail),
            "pending" => Some(CiStatus::Pending),
            _ => None,
        }
    }

    /// The typographic glyph appended to the PR cell (`#723 ✓`). Colors
    /// reinforce but never carry the meaning alone (NO_COLOR, pipes,
    /// red/green colorblindness), so the glyph is the load-bearing signal.
    pub fn glyph(self) -> &'static str {
        match self {
            CiStatus::Pass => "\u{2713}",    // ✓
            CiStatus::Fail => "\u{2717}",    // ✗
            CiStatus::Pending => "\u{25cf}", // ●
        }
    }
}

/// PR decorations resolved from the forge-PR cache, keyed for the two match
/// directions the `daft list` PR column uses. Pure data: built by the command
/// layer (which owns store access), consumed by renderers — core never reads
/// the store.
#[derive(Debug, Default, Clone)]
pub struct ForgePrLookup {
    /// Outbound: the open same-repo PR whose head is this local branch.
    /// (Fork PRs and non-open PRs are excluded by the builder so a stranger's
    /// colliding branch name or a merged PR can't decorate a local branch.)
    pub by_branch: std::collections::HashMap<String, (ForgeBranchRef, Option<CiStatus>)>,
    /// Inbound: CI by the `(kind, number)` identity a `daft go pr:N` checkout
    /// recorded in `branch.<name>.merge` — any state, so a closed PR's
    /// worktree still shows its CI.
    pub ci_by_ref: std::collections::HashMap<ForgeBranchRef, Option<CiStatus>>,
}

impl ForgePrLookup {
    /// Resolve one row's PR cell: a config-recorded ref (inbound) is
    /// authoritative and only gains CI; otherwise the branch name may match an
    /// outbound PR from the cache.
    pub fn decorate(
        &self,
        branch: &str,
        config_ref: Option<ForgeBranchRef>,
    ) -> (Option<ForgeBranchRef>, Option<CiStatus>) {
        match config_ref {
            Some(r) => (Some(r), self.ci_by_ref.get(&r).copied().flatten()),
            None => match self.by_branch.get(branch) {
                Some((r, ci)) => (Some(*r), *ci),
                None => (None, None),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorate_prefers_config_ref_and_adds_ci() {
        let inbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 7);
        let mut lookup = ForgePrLookup::default();
        lookup.ci_by_ref.insert(inbound, Some(CiStatus::Fail));
        // An outbound row for the same branch name must NOT shadow config.
        lookup.by_branch.insert(
            "feat/x".into(),
            (ForgeBranchRef::new(ForgeRefKind::GithubPr, 99), None),
        );

        let (r, ci) = lookup.decorate("feat/x", Some(inbound));
        assert_eq!(r, Some(inbound));
        assert_eq!(ci, Some(CiStatus::Fail));
    }

    #[test]
    fn decorate_falls_back_to_outbound_match() {
        let outbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 42);
        let mut lookup = ForgePrLookup::default();
        lookup
            .by_branch
            .insert("feat/y".into(), (outbound, Some(CiStatus::Pass)));

        assert_eq!(
            lookup.decorate("feat/y", None),
            (Some(outbound), Some(CiStatus::Pass))
        );
        assert_eq!(lookup.decorate("feat/other", None), (None, None));
    }

    #[test]
    fn ci_status_round_trips_and_has_distinct_glyphs() {
        for s in [CiStatus::Pass, CiStatus::Fail, CiStatus::Pending] {
            assert_eq!(CiStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(CiStatus::parse("bogus"), None);
        let glyphs = [
            CiStatus::Pass.glyph(),
            CiStatus::Fail.glyph(),
            CiStatus::Pending.glyph(),
        ];
        assert_eq!(
            glyphs.len(),
            glyphs
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        );
    }

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
