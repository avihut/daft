//! Platform-neutral PR/MR metadata produced by a provider.

use crate::core::worktree::forge_ref::ForgeRefKind;

/// Resolved metadata for a single PR/MR, everything the checkout needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteRefInfo {
    /// Which forge + ref kind this is (drives display and the branch-tracking
    /// ref format).
    pub kind: ForgeRefKind,
    /// PR/MR number.
    pub number: u32,
    /// PR/MR title.
    pub title: String,
    /// Author's login/username.
    pub author: String,
    /// State (`open`, `closed`, `merged`, …), lowercased.
    pub state: String,
    /// Whether it's a draft.
    pub draft: bool,
    /// Branch name in the source (head) repository — the local branch daft
    /// creates.
    pub source_branch: String,
    /// Whether the head is a fork (cross-repo) of the base repository.
    pub is_cross_repo: bool,
    /// Web URL of the PR/MR.
    pub url: String,
    /// Host + base (target) repo, used to find the local remote to fetch from.
    pub base: BaseRepo,
}

/// The base (target) repository a PR/MR was opened against — where its head
/// ref (`refs/pull/N/head` / `refs/merge-requests/N/head`) lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRepo {
    pub host: String,
    pub owner: String,
    pub repo: String,
}

// The CI rollup enum lives in core next to `ForgeBranchRef` (renderers use it
// without depending on the forge CLI layer); re-exported here so provider code
// and consumers keep one import path.
pub use crate::core::worktree::forge_ref::CiStatus;

/// One entry of a provider's open-PR/MR listing — the forge-cache refresh
/// payload. Leaner than [`RemoteRefInfo`]: a listing decorates `daft list`
/// and tab completion, it doesn't drive a checkout, so base-repo coords and
/// draft state aren't carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrListEntry {
    pub kind: ForgeRefKind,
    pub number: u32,
    /// Raw forge title — sanitized at the cache's persistence boundary, not
    /// here.
    pub title: String,
    /// Lowercased state (`open`, `merged`, `closed`).
    pub state: String,
    /// Branch name in the head repository.
    pub head_branch: String,
    /// Whether the head lives in a fork of the base repository.
    pub is_cross_repo: bool,
    /// CI rollup, `None` when the PR has no checks (or the platform's listing
    /// doesn't carry pipeline status — GitLab, deferred).
    pub ci_status: Option<CiStatus>,
    pub url: String,
    pub author: String,
    /// Owner login of the head (fork) repository — the `owner:` prefix on a
    /// synthesized fork row in `daft list`. Empty when the platform's listing
    /// doesn't carry it (GitLab's REST listing names only project IDs).
    pub head_repo_owner: String,
    /// The PR's last-activity timestamp — the Age cell on a synthesized row.
    /// `None` when the platform didn't supply one.
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Commit the PR's head branch pointed at. Pins a merged PR to the exact
    /// work it carried, so squash-merge detection can tell "this branch was
    /// merged" from "a branch that once had this name was merged"
    /// (`ForgeMergedWitness`, #737). `None` when the platform didn't supply
    /// one — the witness then abstains rather than guess.
    pub head_oid: Option<String>,
    /// Branch the PR was opened against. A merged PR only proves the work
    /// reached *its own* base: a stacked PR merges into another feature
    /// branch, a backport into a release line. `None` when the platform
    /// didn't supply one.
    pub base_branch: Option<String>,
}

/// Parse a forge timestamp (RFC 3339, e.g. `2026-07-18T12:00:00Z`) into UTC.
/// `None` on any parse failure — a malformed timestamp must never fail a
/// listing, it just costs one Age cell.
pub(crate) fn parse_forge_timestamp(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

impl RemoteRefInfo {
    /// Compact identity for rail rows / result line: `PR #123` / `MR !45`.
    pub fn display(&self) -> String {
        match self.kind {
            ForgeRefKind::GithubPr => format!("PR #{}", self.number),
            ForgeRefKind::GitlabMr => format!("MR !{}", self.number),
        }
    }

    /// The branch-tracking ref this PR/MR head is fetched into config as
    /// (`refs/pull/123/head` / `refs/merge-requests/45/head`).
    pub fn head_ref(&self) -> String {
        crate::core::worktree::forge_ref::ForgeBranchRef::new(self.kind, self.number).merge_ref()
    }

    /// An advisory line when the PR/MR isn't open (still worth checking out),
    /// or `None` when it's open.
    pub fn state_note(&self) -> Option<String> {
        match self.state.as_str() {
            "open" | "opened" | "" => None,
            other => Some(format!("{} is {}", self.display(), other)),
        }
    }
}
