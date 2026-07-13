//! GitHub PR provider (`gh api`).

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::forge::cli::{self, CliApiRequest};
use crate::forge::info::{BaseRepo, RemoteRefInfo};
use crate::forge::provider::{ForgeContext, RemoteRefProvider, RepoCoords};
use crate::git::GitCommand;

const GH_PROMPT_ENV: (&str, &str) = ("GH_PROMPT_DISABLED", "1");
const INSTALL_HINT: &str = "GitHub CLI (gh) is not installed. Install it from https://cli.github.com/ and run `gh auth login`.";

pub struct GitHubProvider;

impl RemoteRefProvider for GitHubProvider {
    fn kind(&self) -> ForgeRefKind {
        ForgeRefKind::GithubPr
    }

    fn platform_label(&self) -> &'static str {
        "github"
    }

    fn fetch_info(&self, number: u32, ctx: &ForgeContext<'_>) -> Result<RemoteRefInfo> {
        let coords = resolve_coords(ctx)?;
        let tool = ctx.tool_or("gh");
        let api_path = format!("repos/{}/{}/pulls/{}", coords.owner, coords.repo, number);

        let mut args = vec!["api", api_path.as_str()];
        if let Some(host) = ctx.hostname {
            args.extend(["--hostname", host]);
        }

        let output = cli::run_cli_api(CliApiRequest {
            tool,
            args: &args,
            repo_root: ctx.repo_root,
            prompt_env: GH_PROMPT_ENV,
            install_hint: INSTALL_HINT,
            run_context: "failed to run gh api",
        })?;

        if !output.status.success() {
            return Err(classify_error(number, &coords, &output));
        }

        let response: GhPrResponse = serde_json::from_slice(&output.stdout).with_context(|| {
            format!("could not parse the gh response for PR #{number} (a GitHub API change?)")
        })?;
        into_info(number, response)
    }
}

/// Which owner/repo to query: the pasted URL's, else `gh repo set-default`
/// (fork-workflow aware), else a GitHub remote's URL.
fn resolve_coords(ctx: &ForgeContext<'_>) -> Result<RepoCoords> {
    if let Some(coords) = &ctx.explicit_coords {
        return Ok(coords.clone());
    }
    if let Some(coords) = gh_default_repo(ctx) {
        return Ok(coords);
    }
    coords_from_github_remote(ctx.git).context(
        "no GitHub remote found. Set the default with `gh repo set-default`, \
         or run inside a repository with a github.com remote.",
    )
}

/// Read `gh repo set-default --view` → `owner/repo`. `None` if gh is missing,
/// unset, or the output isn't a slug.
fn gh_default_repo(ctx: &ForgeContext<'_>) -> Option<RepoCoords> {
    let output = std::process::Command::new(ctx.tool_or("gh"))
        .args(["repo", "set-default", "--view"])
        .current_dir(ctx.repo_root)
        .env(GH_PROMPT_ENV.0, GH_PROMPT_ENV.1)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let slug = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let (owner, repo) = slug.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(RepoCoords {
        owner: owner.to_string(),
        repo: repo.to_string(),
    })
}

/// First remote whose host looks like GitHub, split into owner/repo.
fn coords_from_github_remote(git: &GitCommand) -> Result<RepoCoords> {
    for remote in git.remote_list().unwrap_or_default() {
        let Ok(url) = git.remote_get_url(&remote) else {
            continue;
        };
        if let Some((host, owner, repo)) = super::split_repo_key(&url)
            && host.contains("github")
        {
            return Ok(RepoCoords { owner, repo });
        }
    }
    bail!("no github.com remote configured")
}

fn into_info(number: u32, response: GhPrResponse) -> Result<RemoteRefInfo> {
    if response.head.ref_name.is_empty() {
        bail!("PR #{number} has an empty head branch; it may be in an invalid state");
    }
    let base_repo = response
        .base
        .repo
        .context("PR base repository is null (unexpected GitHub API response)")?;
    let head_repo = response.head.repo.ok_or_else(|| {
        anyhow::anyhow!(
            "PR #{number}'s source repository was deleted — the fork it was opened from \
             no longer exists, so its branch can't be checked out"
        )
    })?;

    let is_cross_repo = !base_repo
        .owner
        .login
        .eq_ignore_ascii_case(&head_repo.owner.login)
        || !base_repo.name.eq_ignore_ascii_case(&head_repo.name);

    let host = cli::host_from_url(&response.html_url)?;

    Ok(RemoteRefInfo {
        kind: ForgeRefKind::GithubPr,
        number,
        title: response.title,
        author: response.user.login,
        state: response.state.to_lowercase(),
        draft: response.draft,
        source_branch: response.head.ref_name,
        is_cross_repo,
        url: response.html_url,
        base: BaseRepo {
            host,
            owner: base_repo.owner.login,
            repo: base_repo.name,
        },
    })
}

/// Turn a failed `gh api` into an actionable error, recognising the common
/// HTTP statuses gh reports in its JSON error body.
fn classify_error(
    number: u32,
    coords: &RepoCoords,
    output: &std::process::Output,
) -> anyhow::Error {
    if let Ok(err) = serde_json::from_slice::<GhErrorResponse>(&output.stdout) {
        match err.status.as_str() {
            "404" => {
                return anyhow::anyhow!(
                    "PR #{number} was not found on {}/{}. Check the number, or run \
                     `gh repo set-default` if this repo isn't the PR's base repository.",
                    coords.owner,
                    coords.repo
                );
            }
            "401" => {
                return anyhow::anyhow!("GitHub CLI isn't authenticated. Run `gh auth login`.");
            }
            "403" if err.message.to_lowercase().contains("rate limit") => {
                return anyhow::anyhow!(
                    "GitHub API rate limit exceeded. Wait a few minutes and retry."
                );
            }
            "403" => {
                return anyhow::anyhow!("GitHub API access forbidden: {}", err.message);
            }
            _ => {}
        }
    }
    cli::generic_api_error(&format!("gh api failed for PR #{number}"), output)
}

// ── GitHub API JSON shapes (subset we read) ──────────────────────────────────

#[derive(Debug, Deserialize)]
struct GhPrResponse {
    title: String,
    user: GhUser,
    state: String,
    #[serde(default)]
    draft: bool,
    head: GhPrRef,
    base: GhPrRef,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhPrRef {
    #[serde(rename = "ref")]
    ref_name: String,
    repo: Option<GhPrRepo>,
}

#[derive(Debug, Deserialize)]
struct GhPrRepo {
    name: String,
    owner: GhOwner,
}

#[derive(Debug, Deserialize)]
struct GhOwner {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhErrorResponse {
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str, number: u32) -> Result<RemoteRefInfo> {
        let response: GhPrResponse = serde_json::from_str(json).unwrap();
        into_info(number, response)
    }

    #[test]
    fn parses_same_repo_pr() {
        let json = r#"{
            "title": "Fix the bug", "state": "open", "draft": false,
            "user": {"login": "octocat"},
            "html_url": "https://github.com/acme/widget/pull/12",
            "head": {"ref": "fix-bug", "repo": {"name": "widget", "owner": {"login": "acme"}}},
            "base": {"ref": "main", "repo": {"name": "widget", "owner": {"login": "acme"}}}
        }"#;
        let info = parse(json, 12).unwrap();
        assert_eq!(info.kind, ForgeRefKind::GithubPr);
        assert_eq!(info.source_branch, "fix-bug");
        assert_eq!(info.title, "Fix the bug");
        assert_eq!(info.author, "octocat");
        assert!(!info.is_cross_repo);
        assert_eq!(info.base.host, "github.com");
        assert_eq!(info.base.owner, "acme");
        assert_eq!(info.base.repo, "widget");
        assert_eq!(info.head_ref(), "refs/pull/12/head");
        assert_eq!(info.display(), "PR #12");
        assert!(info.state_note().is_none());
    }

    #[test]
    fn detects_fork_pr() {
        let json = r#"{
            "title": "Contribution", "state": "open", "draft": true,
            "user": {"login": "contributor"},
            "html_url": "https://github.com/acme/widget/pull/34",
            "head": {"ref": "feature", "repo": {"name": "widget", "owner": {"login": "contributor"}}},
            "base": {"ref": "main", "repo": {"name": "widget", "owner": {"login": "acme"}}}
        }"#;
        let info = parse(json, 34).unwrap();
        assert!(info.is_cross_repo, "different head owner => fork");
        assert!(info.draft);
        assert_eq!(info.base.owner, "acme", "base names the target repo");
    }

    #[test]
    fn merged_state_produces_a_note() {
        let json = r#"{
            "title": "Old", "state": "closed", "draft": false,
            "user": {"login": "x"},
            "html_url": "https://github.com/a/b/pull/9",
            "head": {"ref": "old", "repo": {"name": "b", "owner": {"login": "a"}}},
            "base": {"ref": "main", "repo": {"name": "b", "owner": {"login": "a"}}}
        }"#;
        let info = parse(json, 9).unwrap();
        assert_eq!(info.state_note().as_deref(), Some("PR #9 is closed"));
    }

    #[test]
    fn deleted_fork_head_repo_errors() {
        let json = r#"{
            "title": "Gone", "state": "open", "draft": false,
            "user": {"login": "x"},
            "html_url": "https://github.com/a/b/pull/5",
            "head": {"ref": "gone", "repo": null},
            "base": {"ref": "main", "repo": {"name": "b", "owner": {"login": "a"}}}
        }"#;
        let err = parse(json, 5).unwrap_err().to_string();
        assert!(err.contains("deleted"), "got: {err}");
    }
}
