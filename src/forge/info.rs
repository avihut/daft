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
