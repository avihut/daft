//! The provider trait and the context threaded into a fetch.

use std::path::Path;

use anyhow::Result;

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::forge::info::RemoteRefInfo;
use crate::git::GitCommand;

/// A base repository's `owner`/`repo` (or `group/sub/repo` for GitLab).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoCoords {
    pub owner: String,
    pub repo: String,
}

/// Everything a provider needs to run one `fetch_info`.
pub struct ForgeContext<'a> {
    /// For remote inspection (URL parsing, listing) when coords must be
    /// derived from the repo.
    pub git: &'a GitCommand,
    /// Working directory the CLI runs in (auth + repo context).
    pub repo_root: &'a Path,
    /// Authoritative coords from a pasted URL. When `None`, the provider
    /// derives them itself (GitHub: `gh repo set-default` then the remote URL;
    /// GitLab: glab's `:id` placeholder from repo context).
    pub explicit_coords: Option<RepoCoords>,
    /// Config override of the CLI binary name (self-hosted Enterprise wrappers).
    /// `None` uses the platform default (`gh` / `glab`).
    pub tool: Option<&'a str>,
    /// Config forge hostname (GHE / self-hosted), passed to the CLI as
    /// `--hostname` when set.
    pub hostname: Option<&'a str>,
}

impl ForgeContext<'_> {
    /// The CLI binary to invoke — the config override, else the default.
    pub fn tool_or(&self, default: &'static str) -> &str {
        self.tool.unwrap_or(default)
    }
}

/// Platform-specific PR/MR resolution over a forge CLI.
pub trait RemoteRefProvider {
    /// The ref kind this provider yields.
    fn kind(&self) -> ForgeRefKind;

    /// Stable identifier for diagnostics/tests: `"github"` / `"gitlab"`.
    fn platform_label(&self) -> &'static str;

    /// Resolve a PR/MR number to its metadata via the CLI.
    fn fetch_info(&self, number: u32, ctx: &ForgeContext<'_>) -> Result<RemoteRefInfo>;
}
