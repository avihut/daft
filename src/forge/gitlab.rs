//! GitLab MR provider (`glab api`).
//!
//! Simpler than the GitHub provider: daft fetches the MR head from the target
//! project's `refs/merge-requests/<iid>/head` ref (available on the base repo
//! for both same-repo and fork MRs), so — unlike worktrunk, which resolves the
//! fork's clone URL to configure push — daft needs no extra project-URL calls.
//! Push-to-fork is deferred (#127 out of scope).

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::forge::cli::{self, CliApiRequest};
use crate::forge::info::{BaseRepo, RemoteRefInfo};
use crate::forge::provider::{ForgeContext, RemoteRefProvider};

const GLAB_PROMPT_ENV: (&str, &str) = ("GLAB_NO_PROMPT", "1");
const INSTALL_HINT: &str = "GitLab CLI (glab) is not installed. Install it from https://gitlab.com/gitlab-org/cli#installation and run `glab auth login`.";

pub struct GitLabProvider;

impl RemoteRefProvider for GitLabProvider {
    fn kind(&self) -> ForgeRefKind {
        ForgeRefKind::GitlabMr
    }

    fn platform_label(&self) -> &'static str {
        "gitlab"
    }

    fn fetch_info(&self, number: u32, ctx: &ForgeContext<'_>) -> Result<RemoteRefInfo> {
        // Project id: the pasted URL's path (URL-encoded), else glab's `:id`
        // placeholder resolved from the repo's remote.
        let project = match &ctx.explicit_coords {
            Some(coords) => encode_project(&format!("{}/{}", coords.owner, coords.repo)),
            None => ":id".to_string(),
        };
        let api_path = format!("projects/{project}/merge_requests/{number}");

        let mut args = vec!["api", api_path.as_str()];
        if let Some(host) = ctx.hostname {
            args.extend(["--hostname", host]);
        }

        let output = cli::run_cli_api(CliApiRequest {
            tool: ctx.tool_or("glab"),
            args: &args,
            repo_root: ctx.repo_root,
            prompt_env: GLAB_PROMPT_ENV,
            install_hint: INSTALL_HINT,
            run_context: "failed to run glab api",
        })?;

        if !output.status.success() {
            return Err(classify_error(number, &output));
        }

        let response: GlabMrResponse =
            serde_json::from_slice(&output.stdout).with_context(|| {
                format!("could not parse the glab response for MR !{number} (a GitLab API change?)")
            })?;
        into_info(number, response)
    }
}

/// Percent-encode the `/` separators in a project path (`group/sub/repo` →
/// `group%2Fsub%2Frepo`) so it's a single path segment for the projects API.
fn encode_project(path: &str) -> String {
    path.replace('/', "%2F")
}

fn into_info(number: u32, response: GlabMrResponse) -> Result<RemoteRefInfo> {
    if response.source_branch.is_empty() {
        bail!("MR !{number} has an empty source branch; it may be in an invalid state");
    }
    let is_cross_repo = response.source_project_id != response.target_project_id;

    // Base (target) project: everything left of `/-/` in the web URL.
    let (project_url, _) = response
        .web_url
        .split_once("/-/")
        .with_context(|| format!("MR !{number} web URL missing `/-/`: {}", response.web_url))?;
    let (host, owner, repo) = split_project_url(project_url)
        .with_context(|| format!("could not parse project from MR URL: {project_url}"))?;

    Ok(RemoteRefInfo {
        kind: ForgeRefKind::GitlabMr,
        number,
        title: response.title,
        author: response.author.username,
        state: response.state.to_lowercase(),
        draft: response.draft,
        source_branch: response.source_branch,
        is_cross_repo,
        url: response.web_url,
        base: BaseRepo { host, owner, repo },
    })
}

/// `https://host/group/sub/repo` → `(host, "group/sub", "repo")`.
fn split_project_url(url: &str) -> Option<(String, String, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 3 {
        return None;
    }
    let host = segments[0].to_string();
    let repo = (*segments.last()?).to_string();
    let owner = segments[1..segments.len() - 1].join("/");
    Some((host, owner, repo))
}

/// Turn a failed `glab api` into an actionable error.
fn classify_error(number: u32, output: &std::process::Output) -> anyhow::Error {
    if let Ok(err) = serde_json::from_slice::<GlabErrorResponse>(&output.stdout) {
        let text = if !err.message.is_empty() {
            &err.message
        } else {
            &err.error
        };
        if text.starts_with("404") {
            return anyhow::anyhow!("MR !{number} was not found. Check the number.");
        }
        if text.starts_with("401") {
            return anyhow::anyhow!("GitLab CLI isn't authenticated. Run `glab auth login`.");
        }
        if text.starts_with("403") {
            return anyhow::anyhow!("GitLab API access forbidden for MR !{number}.");
        }
    }
    cli::generic_api_error(&format!("glab api failed for MR !{number}"), output)
}

// ── GitLab API JSON shapes (subset we read) ──────────────────────────────────

#[derive(Debug, Deserialize)]
struct GlabMrResponse {
    title: String,
    author: GlabAuthor,
    state: String,
    #[serde(default)]
    draft: bool,
    source_branch: String,
    source_project_id: u64,
    target_project_id: u64,
    web_url: String,
}

#[derive(Debug, Deserialize)]
struct GlabAuthor {
    username: String,
}

#[derive(Debug, Deserialize)]
struct GlabErrorResponse {
    #[serde(default)]
    message: String,
    #[serde(default)]
    error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str, number: u32) -> Result<RemoteRefInfo> {
        let response: GlabMrResponse = serde_json::from_str(json).unwrap();
        into_info(number, response)
    }

    #[test]
    fn parses_same_repo_mr() {
        let json = r#"{
            "title": "Add feature", "state": "opened", "draft": false,
            "author": {"username": "dev"},
            "source_branch": "feature-x",
            "source_project_id": 1, "target_project_id": 1,
            "web_url": "https://gitlab.com/group/widget/-/merge_requests/45"
        }"#;
        let info = parse(json, 45).unwrap();
        assert_eq!(info.kind, ForgeRefKind::GitlabMr);
        assert_eq!(info.source_branch, "feature-x");
        assert_eq!(info.author, "dev");
        assert!(!info.is_cross_repo);
        assert_eq!(info.base.host, "gitlab.com");
        assert_eq!(info.base.owner, "group");
        assert_eq!(info.base.repo, "widget");
        assert_eq!(info.head_ref(), "refs/merge-requests/45/head");
        assert_eq!(info.display(), "MR !45");
        assert!(info.state_note().is_none(), "opened is not noteworthy");
    }

    #[test]
    fn detects_fork_mr_and_subgroups() {
        let json = r#"{
            "title": "Contribution", "state": "opened", "draft": false,
            "author": {"username": "contributor"},
            "source_branch": "patch",
            "source_project_id": 9, "target_project_id": 1,
            "web_url": "https://gitlab.com/group/sub/widget/-/merge_requests/7"
        }"#;
        let info = parse(json, 7).unwrap();
        assert!(info.is_cross_repo, "different project ids => fork");
        assert_eq!(info.base.owner, "group/sub");
        assert_eq!(info.base.repo, "widget");
    }

    #[test]
    fn merged_state_produces_a_note() {
        let json = r#"{
            "title": "Old", "state": "merged", "draft": false,
            "author": {"username": "x"},
            "source_branch": "old",
            "source_project_id": 1, "target_project_id": 1,
            "web_url": "https://gitlab.com/g/r/-/merge_requests/3"
        }"#;
        let info = parse(json, 3).unwrap();
        assert_eq!(info.state_note().as_deref(), Some("MR !3 is merged"));
    }

    #[test]
    fn encodes_project_path() {
        assert_eq!(encode_project("group/repo"), "group%2Frepo");
        assert_eq!(encode_project("group/sub/repo"), "group%2Fsub%2Frepo");
    }
}
