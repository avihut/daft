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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
}

/// Lifecycle + CI state of a PR/MR, folded into the one signal the PR cell
/// communicates. Color carries it in color-capable renderers (green/red/
/// yellow CI, purple merged, dim closed); [`Self::glyph`] is the colorless
/// encoding appended to the cell text when color is off (`NO_COLOR`, pipes),
/// so the signal never exists as color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrStatus {
    /// Open with no CI information (the PR has no checks, or the platform's
    /// listing carries none — GitLab). Renders plain, like an unknown state.
    Open,
    /// Open with a CI rollup.
    Ci(CiStatus),
    Merged,
    /// Closed without merging.
    Closed,
}

impl PrStatus {
    /// Fold a cached row's `state` + CI rollup into one display status.
    /// Unknown states (GitLab's transitional `locked`, future additions) are
    /// treated as open.
    pub fn from_state_and_ci(state: &str, ci: Option<CiStatus>) -> Self {
        match state {
            "merged" => PrStatus::Merged,
            "closed" => PrStatus::Closed,
            _ => ci.map_or(PrStatus::Open, PrStatus::Ci),
        }
    }

    /// The colorless encoding for the PR cell (`#723 ✓`), appended only when
    /// the renderer applies no color; empty for [`PrStatus::Open`] (a bare
    /// number already reads as "open, nothing notable").
    pub fn glyph(self) -> &'static str {
        match self {
            PrStatus::Open => "",
            PrStatus::Ci(CiStatus::Pass) => "\u{2713}", // ✓
            PrStatus::Ci(CiStatus::Fail) => "\u{2717}", // ✗
            PrStatus::Ci(CiStatus::Pending) => "\u{25cf}", // ●
            PrStatus::Merged => "\u{25c6}",             // ◆
            PrStatus::Closed => "\u{25cb}",             // ○
        }
    }

    /// The semantic color slot this status renders in, or `None` when the
    /// number stays plain (open without CI). The live (ratatui) and blocking
    /// (term-styles) tables use different color types, so they share this
    /// classification rather than the concrete color — the status→color
    /// mapping then lives in exactly one place, and a new `PrStatus` variant
    /// can't drift the two tables out of agreement.
    pub fn semantic_color(self) -> Option<PrStatusColor> {
        match self {
            PrStatus::Ci(CiStatus::Pass) => Some(PrStatusColor::Pass),
            PrStatus::Ci(CiStatus::Fail) => Some(PrStatusColor::Fail),
            PrStatus::Ci(CiStatus::Pending) => Some(PrStatusColor::Pending),
            PrStatus::Merged => Some(PrStatusColor::Merged),
            PrStatus::Closed => Some(PrStatusColor::Closed),
            PrStatus::Open => None,
        }
    }
}

/// The color a PR's fate maps to, shared by the live and blocking renderers.
/// Each translates it into its own color type (ratatui `Color` /
/// term-styles), keeping the two tables in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrStatusColor {
    /// CI passing — green.
    Pass,
    /// CI failing — red.
    Fail,
    /// CI running — yellow.
    Pending,
    /// Merged — purple (the "done, prune it" signal).
    Merged,
    /// Closed without merging — dim.
    Closed,
}

/// One resolved PR-cell decoration: the ref plus everything the renderers
/// need to style it. `status`/`url`/`author` are `None` when the ref is known
/// only from branch config (inbound checkout with no cache row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDecoration {
    pub r: ForgeBranchRef,
    pub status: Option<PrStatus>,
    /// Web URL of the PR/MR — plain-print renderers wrap the cell in an
    /// OSC 8 terminal hyperlink.
    pub url: Option<String>,
    /// The PR author's login. Wherever a decoration attaches to a row, the
    /// Owner cell prefers this over branch-history deduction — the forge's
    /// answer to "whose PR" is canonical where the history walk is a
    /// winning-commit heuristic (and synthesized fork rows have no history
    /// at all).
    pub author: Option<String>,
}

impl PrDecoration {
    /// A decoration known only by its ref — the config-recorded fallback when
    /// no cache row backs it (no status, no URL, no author).
    pub fn bare(r: ForgeBranchRef) -> Self {
        Self {
            r,
            status: None,
            url: None,
            author: None,
        }
    }
}

/// One open PR from the cache snapshot — the seed for a synthesized `daft
/// list` row when no worktree or local branch already represents it. Carries
/// exactly what such a row can display: identity + fate via `decoration`,
/// and the cache-sourced stand-ins for the local columns (title for the
/// last-commit cell, `updated_at` for age).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub decoration: PrDecoration,
    /// Branch name in the head repository.
    pub head_branch: String,
    /// The head lives in a fork of the base repository.
    pub is_cross_repo: bool,
    /// Owner login of the head (fork) repository; empty when the platform's
    /// listing doesn't carry it (GitLab) or the write-through path recorded
    /// the row.
    pub head_repo_owner: String,
    /// Sanitized PR title (control characters stripped at the cache's
    /// persistence boundary).
    pub title: String,
    /// The PR's last-activity timestamp, when the platform supplied one.
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl OpenPr {
    /// The branch-cell text for a synthesized row. Fork branch names live in
    /// per-fork namespaces — two contributors' `patch-1`s collide, and a
    /// fork's `main` must not read as yours — so cross-repo rows render
    /// GitHub-style `owner:branch`. Same-repo rows show the plain name (that
    /// IS an origin branch); a fork whose owner the listing couldn't supply
    /// falls back to the plain name too.
    pub fn display_branch(&self) -> String {
        if self.is_cross_repo && !self.head_repo_owner.is_empty() {
            format!("{}:{}", self.head_repo_owner, self.head_branch)
        } else {
            self.head_branch.clone()
        }
    }
}

/// PR decorations resolved from the forge-PR cache, keyed for the two match
/// directions the `daft list` PR column uses. Pure data: built by the command
/// layer (which owns store access), consumed by renderers — core never reads
/// the store. `PartialEq` is load-bearing: the live table's refresh poll
/// compares snapshots to detect that the background refresh landed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ForgePrLookup {
    /// Outbound: the best same-repo PR whose head is this local branch —
    /// open beats merged, newer number wins, closed and fork PRs never map
    /// (a stranger's colliding branch name must not decorate a local branch).
    pub by_branch: std::collections::HashMap<String, PrDecoration>,
    /// Inbound: every cached row by its `(kind, number)` identity, as
    /// recorded in `branch.<name>.merge` by a `daft go pr:N` checkout — any
    /// state, so a merged/closed PR's worktree still shows its fate.
    pub by_ref: std::collections::HashMap<ForgeBranchRef, PrDecoration>,
    /// Every open PR in the snapshot, newest number first — the seeds for the
    /// default open-PR rows in `daft list`. Includes cross-repo PRs (they
    /// never match `by_branch` but do get rows); the list layer dedups
    /// against worktrees and local branches.
    pub open: Vec<OpenPr>,
}

impl ForgePrLookup {
    /// Resolve one row's PR cell: a config-recorded ref (inbound) is
    /// authoritative and only gains status/URL; otherwise the branch name may
    /// match an outbound PR from the cache.
    pub fn decorate(
        &self,
        branch: &str,
        config_ref: Option<ForgeBranchRef>,
    ) -> Option<PrDecoration> {
        match config_ref {
            Some(r) => Some(
                self.by_ref
                    .get(&r)
                    .cloned()
                    .unwrap_or_else(|| PrDecoration::bare(r)),
            ),
            None => self.by_branch.get(branch).cloned(),
        }
    }

    /// The same decorations stripped to bare identity — no status, no URL.
    /// The live table seeds with this while a background refresh is in
    /// flight: a possibly-stale fate must not render as current, while
    /// identity (numbers, branch names, titles, authors) may be a run stale
    /// and shows immediately; statuses arrive with the refresh (or not at
    /// all this run).
    pub fn identity_only(mut self) -> Self {
        let strip = |d: &mut PrDecoration| {
            d.status = None;
            d.url = None;
        };
        self.by_branch.values_mut().for_each(strip);
        self.by_ref.values_mut().for_each(strip);
        self.open.iter_mut().for_each(|p| strip(&mut p.decoration));
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decor(r: ForgeBranchRef, status: Option<PrStatus>) -> PrDecoration {
        PrDecoration {
            status,
            ..PrDecoration::bare(r)
        }
    }

    #[test]
    fn decorate_prefers_config_ref_and_adds_status() {
        let inbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 7);
        let mut lookup = ForgePrLookup::default();
        lookup
            .by_ref
            .insert(inbound, decor(inbound, Some(PrStatus::Ci(CiStatus::Fail))));
        // An outbound row for the same branch name must NOT shadow config.
        lookup.by_branch.insert(
            "feat/x".into(),
            decor(ForgeBranchRef::new(ForgeRefKind::GithubPr, 99), None),
        );

        let d = lookup.decorate("feat/x", Some(inbound)).unwrap();
        assert_eq!(d.r, inbound);
        assert_eq!(d.status, Some(PrStatus::Ci(CiStatus::Fail)));
    }

    #[test]
    fn decorate_config_ref_without_cache_row_is_bare() {
        let inbound = ForgeBranchRef::new(ForgeRefKind::GitlabMr, 45);
        let lookup = ForgePrLookup::default();
        assert_eq!(
            lookup.decorate("feat/x", Some(inbound)),
            Some(decor(inbound, None))
        );
    }

    #[test]
    fn decorate_falls_back_to_outbound_match() {
        let outbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 42);
        let mut lookup = ForgePrLookup::default();
        lookup.by_branch.insert(
            "feat/y".into(),
            decor(outbound, Some(PrStatus::Ci(CiStatus::Pass))),
        );

        assert_eq!(
            lookup.decorate("feat/y", None),
            Some(decor(outbound, Some(PrStatus::Ci(CiStatus::Pass))))
        );
        assert_eq!(lookup.decorate("feat/other", None), None);
    }

    #[test]
    fn identity_only_strips_status_and_url_but_keeps_identity() {
        let outbound = ForgeBranchRef::new(ForgeRefKind::GithubPr, 42);
        let inbound = ForgeBranchRef::new(ForgeRefKind::GitlabMr, 7);
        let mut lookup = ForgePrLookup::default();
        lookup.by_branch.insert(
            "feat/y".into(),
            PrDecoration {
                r: outbound,
                status: Some(PrStatus::Ci(CiStatus::Fail)),
                url: Some("https://github.com/a/b/pull/42".into()),
                author: Some("alice".into()),
            },
        );
        lookup.by_ref.insert(
            inbound,
            PrDecoration {
                r: inbound,
                status: Some(PrStatus::Merged),
                url: Some("https://gitlab.com/a/b/-/merge_requests/7".into()),
                author: None,
            },
        );
        lookup.open.push(OpenPr {
            decoration: PrDecoration {
                r: outbound,
                status: Some(PrStatus::Ci(CiStatus::Fail)),
                url: Some("https://github.com/a/b/pull/42".into()),
                author: Some("alice".into()),
            },
            head_branch: "feat/y".into(),
            is_cross_repo: false,
            head_repo_owner: String::new(),
            title: "feat: y".into(),
            updated_at: None,
        });

        let bare = lookup.identity_only();
        let d = &bare.by_branch["feat/y"];
        assert_eq!(d.r, outbound, "the number itself stays");
        assert_eq!((d.status, d.url.as_deref()), (None, None));
        assert_eq!(d.author.as_deref(), Some("alice"), "author is identity");
        let d = &bare.by_ref[&inbound];
        assert_eq!(d.r, inbound);
        assert_eq!((d.status, d.url.as_deref()), (None, None));
        let p = &bare.open[0];
        assert_eq!(
            (p.decoration.status, p.decoration.url.as_deref()),
            (None, None)
        );
        assert_eq!(p.title, "feat: y", "row identity survives the strip");
        assert_eq!(p.decoration.author.as_deref(), Some("alice"));
    }

    #[test]
    fn display_branch_prefixes_forks_only_when_owner_is_known() {
        let base = OpenPr {
            decoration: decor(ForgeBranchRef::new(ForgeRefKind::GithubPr, 9), None),
            head_branch: "patch-1".into(),
            is_cross_repo: true,
            head_repo_owner: "alice".into(),
            title: String::new(),
            updated_at: None,
        };
        assert_eq!(base.display_branch(), "alice:patch-1");

        let same_repo = OpenPr {
            is_cross_repo: false,
            ..base.clone()
        };
        assert_eq!(
            same_repo.display_branch(),
            "patch-1",
            "a same-repo head IS an origin branch — no prefix"
        );

        let unknown_owner = OpenPr {
            head_repo_owner: String::new(),
            ..base
        };
        assert_eq!(
            unknown_owner.display_branch(),
            "patch-1",
            "fork with no owner in the listing (GitLab) falls back plain"
        );
    }

    #[test]
    fn ci_status_round_trips() {
        for s in [CiStatus::Pass, CiStatus::Fail, CiStatus::Pending] {
            assert_eq!(CiStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(CiStatus::parse("bogus"), None);
    }

    #[test]
    fn pr_status_folds_state_and_ci() {
        use PrStatus as S;
        assert_eq!(S::from_state_and_ci("open", None), S::Open);
        assert_eq!(
            S::from_state_and_ci("open", Some(CiStatus::Pass)),
            S::Ci(CiStatus::Pass)
        );
        assert_eq!(
            S::from_state_and_ci("merged", Some(CiStatus::Fail)),
            S::Merged
        );
        assert_eq!(S::from_state_and_ci("closed", None), S::Closed);
        // Unknown states behave like open.
        assert_eq!(
            S::from_state_and_ci("locked", Some(CiStatus::Pending)),
            S::Ci(CiStatus::Pending)
        );
    }

    #[test]
    fn pr_status_glyphs_are_distinct_except_open() {
        let statuses = [
            PrStatus::Ci(CiStatus::Pass),
            PrStatus::Ci(CiStatus::Fail),
            PrStatus::Ci(CiStatus::Pending),
            PrStatus::Merged,
            PrStatus::Closed,
        ];
        let glyphs: std::collections::HashSet<_> = statuses.iter().map(|s| s.glyph()).collect();
        assert_eq!(glyphs.len(), statuses.len());
        assert!(!glyphs.contains(""), "every non-open status has a glyph");
        assert_eq!(PrStatus::Open.glyph(), "");
    }

    #[test]
    fn semantic_color_classifies_every_status() {
        use PrStatusColor as C;
        assert_eq!(PrStatus::Ci(CiStatus::Pass).semantic_color(), Some(C::Pass));
        assert_eq!(PrStatus::Ci(CiStatus::Fail).semantic_color(), Some(C::Fail));
        assert_eq!(
            PrStatus::Ci(CiStatus::Pending).semantic_color(),
            Some(C::Pending)
        );
        assert_eq!(PrStatus::Merged.semantic_color(), Some(C::Merged));
        assert_eq!(PrStatus::Closed.semantic_color(), Some(C::Closed));
        // Open renders plain — no color in either table.
        assert_eq!(PrStatus::Open.semantic_color(), None);
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
