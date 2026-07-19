//! GitHub PR provider (`gh api`).

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::forge::cli::{self, CliApiRequest};
use crate::forge::info::{BaseRepo, CiStatus, PrListEntry, RemoteRefInfo, parse_forge_timestamp};
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
            extra_env: &[],
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

    fn fetch_list(&self, ctx: &ForgeContext<'_>) -> Result<Vec<PrListEntry>> {
        // Every open PR (up to gh's practical page cap), plus a window of
        // recently merged ones so `daft list` can mark a local branch's PR as
        // merged — the "this worktree is done, prune it" signal. Merged PRs
        // skip the check rollup: their CI is dead information, and dropping
        // the field keeps the GraphQL cost of the second call down.
        let mut entries = run_pr_list(ctx, "open", "200", true)?;
        entries.extend(run_pr_list(ctx, "merged", "50", false)?);
        Ok(entries)
    }
}

/// One `gh pr list` invocation for one `--state`. The repo resolves from the
/// cwd's remotes (honoring `gh repo set-default` in fork workflows) — no
/// explicit coords needed. `statusCheckRollup` rides along in the same single
/// invocation when requested, which is the whole reason this uses
/// `gh pr list --json` (GraphQL-backed) rather than the REST listing endpoint
/// (no check data).
fn run_pr_list(
    ctx: &ForgeContext<'_>,
    state: &str,
    limit: &str,
    with_rollup: bool,
) -> Result<Vec<PrListEntry>> {
    let mut extra_env: Vec<(&str, &str)> = Vec::new();
    if let Some(host) = ctx.hostname {
        // `gh pr list` has no `--hostname` flag (only `gh api` does);
        // GH_HOST is gh's documented equivalent.
        extra_env.push(("GH_HOST", host));
    }

    let base_fields = "number,title,state,headRefName,isCrossRepository,url,author,headRepositoryOwner,updatedAt,headRefOid,baseRefName";
    let fields = if with_rollup {
        format!("{base_fields},statusCheckRollup")
    } else {
        base_fields.to_string()
    };

    let output = cli::run_cli_api(CliApiRequest {
        tool: ctx.tool_or("gh"),
        args: &[
            "pr", "list", "--state", state, "--limit", limit, "--json", &fields,
        ],
        repo_root: ctx.repo_root,
        prompt_env: GH_PROMPT_ENV,
        extra_env: &extra_env,
        install_hint: INSTALL_HINT,
        run_context: "failed to run gh pr list",
    })?;

    if !output.status.success() {
        let context = format!("could not list {state} pull requests via gh");
        return Err(match listing_failure(&output) {
            Some(kind) => anyhow::Error::new(kind)
                .context(format!("{context}: {}", cli::error_details(&output))),
            None => cli::generic_api_error(&context, &output),
        });
    }

    parse_pr_list(&output.stdout)
}

/// Recognize the "keeps failing until the user acts" shapes in a failed
/// `gh pr list`, for forge-health classification: gh reserves exit code 4
/// for authentication failures (its auth prompt also names `gh auth login`),
/// and an authenticated-but-invisible repo (revoked access, deleted,
/// renamed) errors with "Could not resolve to a Repository". Anything else —
/// network, rate limit, API drift — stays untyped, i.e. transient.
fn listing_failure(output: &std::process::Output) -> Option<cli::ForgeUnavailable> {
    if output.status.code() == Some(4) {
        return Some(cli::ForgeUnavailable::Unauthenticated);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("gh auth login") {
        return Some(cli::ForgeUnavailable::Unauthenticated);
    }
    if stderr.contains("Could not resolve to a Repository") {
        return Some(cli::ForgeUnavailable::RepoAccess);
    }
    None
}

/// Parse `gh pr list --json ...` output into list entries. Pure, so the JSON
/// shapes (and the rollup derivation) unit-test without a subprocess.
fn parse_pr_list(json: &[u8]) -> Result<Vec<PrListEntry>> {
    let items: Vec<GhPrListItem> = serde_json::from_slice(json)
        .context("could not parse the gh pr list response (a GitHub API change?)")?;
    Ok(items
        .into_iter()
        .map(|item| PrListEntry {
            kind: ForgeRefKind::GithubPr,
            number: item.number,
            title: item.title,
            state: item.state.to_lowercase(),
            head_branch: item.head_ref_name,
            is_cross_repo: item.is_cross_repository,
            ci_status: derive_ci_status(&item.status_check_rollup),
            url: item.url,
            author: item.author.unwrap_or_default().display_name(),
            head_repo_owner: item.head_repository_owner.unwrap_or_default().login,
            updated_at: item.updated_at.as_deref().and_then(parse_forge_timestamp),
            head_oid: item.head_ref_oid.filter(|oid| !oid.is_empty()),
            base_branch: item.base_ref_name.filter(|name| !name.is_empty()),
        })
        .collect())
}

/// Roll a PR's check contexts up to one [`CiStatus`]: any failing context
/// dominates, then any still-running one; all conclusive-and-benign is a
/// pass. No contexts at all → `None` (the PR has no CI, distinct from green).
///
/// The rollup mixes two GraphQL shapes — CheckRun (`status` + `conclusion`)
/// and StatusContext (`state`) — whose vocabularies differ; both are folded
/// through the same three buckets.
fn derive_ci_status(contexts: &[GhCheckContext]) -> Option<CiStatus> {
    if contexts.is_empty() {
        return None;
    }
    let mut pending = false;
    for ctx in contexts {
        // CheckRun: `conclusion` is authoritative once COMPLETED; until then
        // `status` (QUEUED / IN_PROGRESS / ...) means the run is still going.
        // StatusContext: `state` is the whole story.
        let verdict = ctx
            .conclusion
            .as_deref()
            .filter(|c| !c.is_empty())
            .or(ctx.state.as_deref());
        match verdict {
            Some(
                "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED"
                | "STARTUP_FAILURE",
            ) => return Some(CiStatus::Fail),
            Some("PENDING" | "EXPECTED") => pending = true,
            Some(_) => {}           // SUCCESS / NEUTRAL / SKIPPED and friends
            None => pending = true, // a CheckRun that hasn't concluded
        }
    }
    Some(if pending {
        CiStatus::Pending
    } else {
        CiStatus::Pass
    })
}

#[derive(Deserialize)]
struct GhPrListItem {
    number: u32,
    title: String,
    state: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "isCrossRepository")]
    is_cross_repository: bool,
    url: String,
    /// Nullable: a deleted account serializes as `author: null` (gh emits `{}`
    /// or `null`), so a single such PR must not abort the whole listing parse.
    #[serde(default)]
    author: Option<GhListAuthor>,
    /// Nullable: a deleted fork leaves the head repository ownerless.
    #[serde(rename = "headRepositoryOwner", default)]
    head_repository_owner: Option<GhListAuthor>,
    #[serde(rename = "updatedAt", default)]
    updated_at: Option<String>,
    #[serde(rename = "statusCheckRollup", default)]
    status_check_rollup: Vec<GhCheckContext>,
    /// The head branch's commit — pins a merged PR to the work it carried
    /// (#737).
    #[serde(rename = "headRefOid", default)]
    head_ref_oid: Option<String>,
    /// The branch the PR targets.
    #[serde(rename = "baseRefName", default)]
    base_ref_name: Option<String>,
}

#[derive(Deserialize, Default)]
struct GhListAuthor {
    #[serde(default)]
    login: String,
    /// The profile's display name; often absent (bots, spartan profiles).
    #[serde(default)]
    name: Option<String>,
}

impl GhListAuthor {
    /// The human-facing name: the profile's full name when set, else the
    /// login. Owner cells and completion columns show a person, so the
    /// display name wins; namespace uses (fork `owner:branch`) read `login`
    /// directly and never come through here.
    fn display_name(self) -> String {
        match self.name {
            Some(name) if !name.trim().is_empty() => name,
            _ => self.login,
        }
    }
}

#[derive(Deserialize)]
struct GhCheckContext {
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
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
        // REST's user summary carries no display name (unlike the GraphQL
        // listing), so the login stands in; the next listing refresh
        // overwrites the cached author with the full name. A deleted author
        // (`user: null`) leaves it blank rather than failing the resolve.
        author: response.user.map(|u| u.login).unwrap_or_default(),
        state: if response.merged {
            "merged".to_string()
        } else {
            response.state.to_lowercase()
        },
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
    /// Nullable: a deleted PR author serializes as `user: null`, which must
    /// not fail the checkout resolve.
    #[serde(default)]
    user: Option<GhUser>,
    state: String,
    /// REST reports a merged PR as `state: closed` + `merged: true`; daft
    /// folds that back into the `merged` state everywhere else uses.
    #[serde(default)]
    merged: bool,
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

    fn check(conclusion: Option<&str>, state: Option<&str>) -> GhCheckContext {
        GhCheckContext {
            conclusion: conclusion.map(String::from),
            state: state.map(String::from),
        }
    }

    #[test]
    fn ci_rollup_no_contexts_is_none() {
        assert_eq!(derive_ci_status(&[]), None);
    }

    #[test]
    fn ci_rollup_any_failure_dominates() {
        // A failing CheckRun outweighs a pending StatusContext and a success.
        let contexts = [
            check(Some("SUCCESS"), None),
            check(None, Some("PENDING")),
            check(Some("FAILURE"), None),
        ];
        assert_eq!(derive_ci_status(&contexts), Some(CiStatus::Fail));
        // StatusContext ERROR also fails.
        assert_eq!(
            derive_ci_status(&[check(None, Some("ERROR"))]),
            Some(CiStatus::Fail)
        );
    }

    #[test]
    fn ci_rollup_pending_beats_pass() {
        // A CheckRun with no conclusion yet (still running) → pending.
        let contexts = [check(Some("SUCCESS"), None), check(None, None)];
        assert_eq!(derive_ci_status(&contexts), Some(CiStatus::Pending));
    }

    #[test]
    fn ci_rollup_all_benign_is_pass() {
        let contexts = [
            check(Some("SUCCESS"), None),
            check(Some("NEUTRAL"), None),
            check(Some("SKIPPED"), None),
            check(None, Some("SUCCESS")),
        ];
        assert_eq!(derive_ci_status(&contexts), Some(CiStatus::Pass));
    }

    #[test]
    fn listing_failure_recognizes_deep_shapes() {
        use crate::forge::cli::{ForgeUnavailable, exit_status_with_code};
        let failed = |code: i32, stderr: &str| std::process::Output {
            status: exit_status_with_code(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        };

        // gh reserves exit code 4 for authentication failures.
        assert_eq!(
            listing_failure(&failed(4, "")),
            Some(ForgeUnavailable::Unauthenticated)
        );
        // Belt-and-suspenders: the auth prompt names `gh auth login`.
        assert_eq!(
            listing_failure(&failed(
                1,
                "To get started with GitHub CLI, please run:  gh auth login"
            )),
            Some(ForgeUnavailable::Unauthenticated)
        );
        // Authenticated but the repo is invisible (revoked/renamed/deleted).
        assert_eq!(
            listing_failure(&failed(
                1,
                "GraphQL: Could not resolve to a Repository with the name 'acme/widget'."
            )),
            Some(ForgeUnavailable::RepoAccess)
        );
        // Network trouble stays untyped — transient, never flips health.
        assert_eq!(
            listing_failure(&failed(1, "dial tcp: network is unreachable")),
            None
        );
    }

    #[test]
    fn parses_pr_list_with_rollup() {
        let json = r#"[
            {"number": 7, "title": "feat: x", "state": "OPEN",
             "headRefName": "feat/x", "isCrossRepository": false,
             "url": "https://github.com/acme/widget/pull/7",
             "author": {"login": "octocat", "name": "The Octocat"},
             "headRefOid": "abc123def456", "baseRefName": "master",
             "statusCheckRollup": [
                {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"__typename": "StatusContext", "state": "SUCCESS"}
             ]},
            {"number": 9, "title": "fix: y", "state": "OPEN",
             "headRefName": "fix/y", "isCrossRepository": true,
             "url": "https://github.com/acme/widget/pull/9",
             "author": {"login": "contributor"},
             "statusCheckRollup": []}
        ]"#;
        let entries = parse_pr_list(json.as_bytes()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].number, 7);
        assert_eq!(entries[0].state, "open", "gh's OPEN is lowercased");
        assert_eq!(entries[0].head_branch, "feat/x");
        assert_eq!(entries[0].ci_status, Some(CiStatus::Pass));
        assert_eq!(
            entries[0].author, "The Octocat",
            "the profile's full name beats the login"
        );
        assert!(entries[1].is_cross_repo);
        assert_eq!(entries[1].ci_status, None, "no checks ≠ green");
        assert_eq!(
            entries[1].author, "contributor",
            "no display name (bots, spartan profiles) falls back to the login"
        );
        // The merge witness's two pins (#737).
        assert_eq!(entries[0].head_oid.as_deref(), Some("abc123def456"));
        assert_eq!(entries[0].base_branch.as_deref(), Some("master"));
        // Absent on the second row: the witness abstains rather than guess.
        assert_eq!(entries[1].head_oid, None);
        assert_eq!(entries[1].base_branch, None);
    }

    #[test]
    fn pr_list_tolerates_null_author() {
        // A deleted account serializes as author: null (gh returns {} or null),
        // and a set login can still carry a null or blank display name. A single
        // such row must never abort the whole listing (a null `author` fails to
        // deserialize unless the field is `Option`).
        let json = r#"[
            {"number": 3, "title": "t", "state": "MERGED",
             "headRefName": "b", "isCrossRepository": false,
             "url": "u", "author": {}},
            {"number": 4, "title": "t", "state": "OPEN",
             "headRefName": "c", "isCrossRepository": false,
             "url": "u", "author": {"login": "bot", "name": null}},
            {"number": 5, "title": "t", "state": "OPEN",
             "headRefName": "d", "isCrossRepository": false,
             "url": "u", "author": {"login": "ghost", "name": "  "}},
            {"number": 6, "title": "t", "state": "OPEN",
             "headRefName": "e", "isCrossRepository": false,
             "url": "u", "author": null}
        ]"#;
        let entries = parse_pr_list(json.as_bytes()).unwrap();
        assert_eq!(
            entries.len(),
            4,
            "a null-author row must not drop the listing"
        );
        assert_eq!(entries[0].author, "");
        assert_eq!(entries[0].state, "merged");
        assert_eq!(entries[1].author, "bot", "null name falls back to login");
        assert_eq!(entries[2].author, "ghost", "blank name falls back to login");
        assert_eq!(
            entries[3].author, "",
            "a null author object is blank, not an error"
        );
    }

    #[test]
    fn single_pr_tolerates_null_user() {
        // `gh api .../pulls/N` returns `user: null` for a deleted author; the
        // checkout resolve must not fail on it (non-Option `user` panics here).
        let json = r#"{
            "title": "t", "state": "open", "draft": false, "user": null,
            "html_url": "https://github.com/acme/widget/pull/8",
            "head": {"ref": "b", "repo": {"name": "widget", "owner": {"login": "acme"}}},
            "base": {"ref": "main", "repo": {"name": "widget", "owner": {"login": "acme"}}}
        }"#;
        let info = parse(json, 8).unwrap();
        assert_eq!(info.author, "", "a null user leaves the author blank");
        assert_eq!(info.source_branch, "b");
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
    fn merged_flag_overrides_rest_closed_state() {
        // REST reports a merged PR as state=closed + merged=true; daft folds
        // that into the `merged` state (drives the purple PR-column fate and
        // the state note's wording).
        let json = r#"{
            "title": "Done", "state": "closed", "merged": true, "draft": false,
            "user": {"login": "x"},
            "html_url": "https://github.com/a/b/pull/9",
            "head": {"ref": "old", "repo": {"name": "b", "owner": {"login": "a"}}},
            "base": {"ref": "main", "repo": {"name": "b", "owner": {"login": "a"}}}
        }"#;
        let info = parse(json, 9).unwrap();
        assert_eq!(info.state, "merged");
        assert_eq!(info.state_note().as_deref(), Some("PR #9 is merged"));
    }

    #[test]
    fn pr_list_normalizes_merged_state() {
        // The merged-window listing call carries no statusCheckRollup.
        let json = r#"[{
            "number": 6, "title": "Landed", "state": "MERGED",
            "headRefName": "feat/done", "isCrossRepository": false,
            "url": "https://github.com/a/b/pull/6",
            "author": {"login": "x"}
        }]"#;
        let entries = parse_pr_list(json.as_bytes()).unwrap();
        assert_eq!(entries[0].state, "merged");
        assert_eq!(entries[0].ci_status, None);
        // Absent row fields degrade to empty/None, never a parse failure.
        assert_eq!(entries[0].head_repo_owner, "");
        assert_eq!(entries[0].updated_at, None);
    }

    #[test]
    fn pr_list_carries_head_owner_and_updated_at() {
        let json = r#"[{
            "number": 9, "title": "Fork work", "state": "OPEN",
            "headRefName": "patch-1", "isCrossRepository": true,
            "url": "https://github.com/a/b/pull/9",
            "author": {"login": "alice"},
            "headRepositoryOwner": {"login": "alice-fork"},
            "updatedAt": "2026-07-18T09:30:00Z"
        }, {
            "number": 8, "title": "Deleted fork", "state": "OPEN",
            "headRefName": "gone", "isCrossRepository": true,
            "url": "https://github.com/a/b/pull/8",
            "author": {"login": "bob"},
            "headRepositoryOwner": null,
            "updatedAt": "not-a-timestamp"
        }]"#;
        let entries = parse_pr_list(json.as_bytes()).unwrap();
        assert_eq!(entries[0].head_repo_owner, "alice-fork");
        assert_eq!(
            entries[0].updated_at.unwrap().to_rfc3339(),
            "2026-07-18T09:30:00+00:00"
        );
        // A deleted fork's null owner and a malformed timestamp both degrade
        // gracefully — the listing itself must never fail on one bad row.
        assert_eq!(entries[1].head_repo_owner, "");
        assert_eq!(entries[1].updated_at, None);
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
