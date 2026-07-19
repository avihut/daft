//! Forge (GitHub / GitLab) PR/MR integration for `daft checkout`.
//!
//! Pure CLI passthrough (#127): daft resolves `pr:123` / `mr:45` / a pasted
//! PR/MR URL by shelling out to `gh` / `glab`, which inherit the user's existing
//! auth — daft never speaks HTTP, stores tokens, or touches a keychain. The
//! layer is deliberately thin: parse the target, pick a provider from the repo's
//! remote (the `pr`/`mr` prefix is a friendly alias, not the platform selector),
//! ask the CLI for the PR/MR metadata, and hand back a platform-neutral
//! [`RemoteRefInfo`] plus the local remote its head ref is fetched from.

pub mod cli;
pub mod github;
pub mod gitlab;
pub mod info;
pub mod parse;
pub mod provider;

pub use cli::{ForgeUnavailable, classify_unavailable};
pub use info::{BaseRepo, CiStatus, PrListEntry, RemoteRefInfo};
pub use parse::{ForgeTarget, TargetSource};
pub use provider::{ForgeContext, RemoteRefProvider, RepoCoords};

use std::path::Path;

use anyhow::{Result, bail};

use crate::core::worktree::forge_ref::ForgeRefKind;
use crate::git::GitCommand;
use github::GitHubProvider;
use gitlab::GitLabProvider;

/// daft-owned forge configuration (auth stays in `gh`/`glab`). Populated from
/// settings by the command layer; defaults resolve everything from the remote.
#[derive(Debug, Default, Clone)]
pub struct ForgeConfig {
    /// Force the platform when the remote is ambiguous (`github` / `gitlab`).
    pub platform: Option<String>,
    /// Override the `gh` binary (Enterprise wrappers).
    pub github_cli: Option<String>,
    /// Override the `glab` binary.
    pub gitlab_cli: Option<String>,
    /// Forge hostname for self-hosted / Enterprise instances (`--hostname`).
    pub hostname: Option<String>,
}

impl ForgeConfig {
    /// Load daft's forge config from git config (`daft.forge.*`). Auth is never
    /// read here — it stays in `gh`/`glab`. Cheap enough to read on the (rare)
    /// PR-checkout path rather than bloating `DaftSettings`.
    pub fn load(git: &GitCommand) -> Self {
        use crate::core::settings::keys;
        let get = |key: &str| git.config_get(key).ok().flatten().filter(|v| !v.is_empty());
        Self {
            platform: get(keys::FORGE_PLATFORM),
            github_cli: get(keys::FORGE_GITHUB_CLI),
            gitlab_cli: get(keys::FORGE_GITLAB_CLI),
            hostname: get(keys::FORGE_HOSTNAME),
        }
    }
}

/// A resolved PR/MR: its metadata plus the local remote its head ref lives on.
#[derive(Debug, Clone)]
pub struct ResolvedRef {
    pub info: RemoteRefInfo,
    /// Local remote name the PR/MR head ref is fetched from (the base repo).
    pub base_remote: String,
}

/// Resolve a [`ForgeTarget`] to its PR/MR metadata and base remote.
///
/// Selects the provider (URL: from the URL's platform; prefix: config override
/// → remote host → GitHub), runs the CLI, validates the source branch (it feeds
/// a fetch refspec and, for a fork PR, comes from an untrusted repo), and finds
/// the local remote that points at the base repository.
pub fn resolve(
    target: &ForgeTarget,
    git: &GitCommand,
    repo_root: &Path,
    default_remote: &str,
    config: &ForgeConfig,
) -> Result<ResolvedRef> {
    let (provider, explicit_coords): (Box<dyn RemoteRefProvider>, Option<RepoCoords>) =
        match &target.source {
            TargetSource::Url {
                kind, owner, repo, ..
            } => (
                provider_for_kind(*kind),
                Some(RepoCoords {
                    owner: owner.clone(),
                    repo: repo.clone(),
                }),
            ),
            TargetSource::Prefix { .. } => (select_provider(git, config)?, None),
        };

    let tool = match provider.kind() {
        ForgeRefKind::GithubPr => config.github_cli.as_deref(),
        ForgeRefKind::GitlabMr => config.gitlab_cli.as_deref(),
    };

    let ctx = ForgeContext {
        git,
        repo_root,
        explicit_coords,
        tool,
        hostname: hostname_for(target, config),
    };

    let info = provider.fetch_info(target.number, &ctx)?;

    // The source branch feeds a fetch refspec; on a fork PR it comes from an
    // attacker-influenceable repo. Reject anything that isn't a plain branch
    // name before it reaches git (core validates again at execute()).
    crate::utils::validate_branch_name(&info.source_branch).map_err(|e| {
        anyhow::anyhow!(
            "{}'s source branch {:?} is not a valid branch name: {e}",
            info.display(),
            info.source_branch
        )
    })?;

    let base_remote = find_base_remote(git, &info.base, default_remote);
    Ok(ResolvedRef { info, base_remote })
}

/// The forge host to talk to. A pasted PR/MR URL names its host authoritatively
/// (GitHub Enterprise, self-hosted GitLab), so it wins over the configured
/// default; a bare `pr:`/`mr:` prefix carries no host and falls back to the
/// config (and, when that too is unset, the CLI's own default host).
fn hostname_for<'a>(target: &'a ForgeTarget, config: &'a ForgeConfig) -> Option<&'a str> {
    match &target.source {
        TargetSource::Url { host, .. } => Some(host.as_str()),
        TargetSource::Prefix { .. } => config.hostname.as_deref(),
    }
}

/// Fetch the repo's PR/MR snapshot (open + recently merged) from its selected
/// platform — the forge-cache refresh payload. Provider selection follows the
/// same chain as a bare `pr:`/`mr:` target (config override → remote host →
/// GitHub); the listing always runs from repo context, so a pasted URL never
/// reaches here.
pub fn fetch_snapshot(
    git: &GitCommand,
    repo_root: &Path,
    config: &ForgeConfig,
) -> Result<(ForgeRefKind, Vec<PrListEntry>)> {
    let provider = select_provider(git, config)?;
    let tool = match provider.kind() {
        ForgeRefKind::GithubPr => config.github_cli.as_deref(),
        ForgeRefKind::GitlabMr => config.gitlab_cli.as_deref(),
    };
    let ctx = ForgeContext {
        git,
        repo_root,
        explicit_coords: None,
        tool,
        hostname: config.hostname.as_deref(),
    };
    Ok((provider.kind(), provider.fetch_list(&ctx)?))
}

/// Whether checking out a fork PR/MR would clobber an unrelated local branch.
/// `true` means "bail". Pure core of [`preflight_fork_collision`]: a same-named
/// local branch exists that doesn't already track this head ref.
fn is_fork_collision(branch_exists: bool, current_merge: Option<&str>, head_ref: &str) -> bool {
    branch_exists && current_merge != Some(head_ref)
}

/// Guard a fork PR/MR checkout from hijacking an unrelated local branch.
///
/// Fork PRs are often opened from the fork's default branch, so
/// `source_branch` is frequently `main`/`master` — a branch the user already
/// has. Without this guard, checking out would either navigate to that branch's
/// existing worktree (core's name-based shortcut can't see the mismatch) or
/// rewrite its `branch.<name>.merge` to the PR head ref, hijacking it so the
/// next `git pull` pulls PR code into it. Runs in the command layer *before*
/// `execute`, so it pre-empts both. Same-repo refs need no guard — the
/// same-named branch genuinely is the PR/MR's branch. A branch already tracking
/// this head ref is a legitimate re-checkout and passes.
pub fn preflight_fork_collision(git: &GitCommand, info: &RemoteRefInfo) -> Result<()> {
    if !info.is_cross_repo {
        return Ok(());
    }
    let branch = &info.source_branch;
    let branch_exists = git
        .show_ref_exists(&format!("refs/heads/{branch}"))
        .unwrap_or(false);
    let current_merge = git
        .config_get(&format!("branch.{branch}.merge"))
        .ok()
        .flatten();
    if is_fork_collision(branch_exists, current_merge.as_deref(), &info.head_ref()) {
        bail!(
            "local branch '{branch}' already exists and does not track {display}.\n  \
             tip: `{go}` opens that branch as-is; rename or delete it to check out \
             {display} fresh.",
            display = info.display(),
            go = crate::daft_cmd(&format!("go {branch}")),
        );
    }
    Ok(())
}

fn provider_for_kind(kind: ForgeRefKind) -> Box<dyn RemoteRefProvider> {
    match kind {
        ForgeRefKind::GithubPr => Box::new(GitHubProvider),
        ForgeRefKind::GitlabMr => Box::new(GitLabProvider),
    }
}

/// Choose a provider for a bare `pr:`/`mr:` reference. Order: config override,
/// then the first remote whose host names a known forge, then GitHub (the
/// common case — its error then hints to set `forge.platform`).
pub fn select_provider(
    git: &GitCommand,
    config: &ForgeConfig,
) -> Result<Box<dyn RemoteRefProvider>> {
    if let Some(platform) = &config.platform {
        return match platform.to_ascii_lowercase().as_str() {
            "github" => Ok(Box::new(GitHubProvider)),
            "gitlab" => Ok(Box::new(GitLabProvider)),
            other => bail!("invalid forge.platform {other:?}; expected `github` or `gitlab`"),
        };
    }

    let hosts = remote_forge_hosts(git);
    if hosts.iter().any(|h| h.contains("github")) {
        return Ok(Box::new(GitHubProvider));
    }
    if hosts.iter().any(|h| h.contains("gitlab")) {
        return Ok(Box::new(GitLabProvider));
    }
    // Ambiguous host: default to GitHub. A wrong guess surfaces as a GitHub
    // error whose remedy is `forge.platform`.
    Ok(Box::new(GitHubProvider))
}

/// Hosts of every configured remote that parses into a forge slug.
fn remote_forge_hosts(git: &GitCommand) -> Vec<String> {
    git.remote_list()
        .unwrap_or_default()
        .iter()
        .filter_map(|r| git.remote_get_url(r).ok())
        .filter_map(|url| split_repo_key(&url).map(|(host, _, _)| host))
        .collect()
}

/// Whether this repo plausibly has a forge to talk to at all — the local,
/// no-network gate for the default `pr` column in `daft list` and for the
/// background snapshot refresh. True when `daft.forge.platform` names a
/// platform (self-hosted / Enterprise hosts are opaque, so the explicit
/// override is the signal there) or any remote URL's host names a known
/// forge — the same signal [`select_provider`] keys on. A repo that fails
/// this never spawns a refresh and never shows the default `pr` column; an
/// explicit `--columns +pr` still renders config-recorded refs.
pub fn repo_forge_capable(git: &GitCommand) -> bool {
    let platform = git
        .config_get(crate::core::settings::keys::FORGE_PLATFORM)
        .ok()
        .flatten();
    names_a_forge(platform.as_deref(), &remote_forge_hosts(git))
}

/// Pure core of [`repo_forge_capable`].
fn names_a_forge(platform: Option<&str>, hosts: &[String]) -> bool {
    if platform.is_some_and(|p| matches!(p.to_ascii_lowercase().as_str(), "github" | "gitlab")) {
        return true;
    }
    hosts
        .iter()
        .any(|h| h.contains("github") || h.contains("gitlab"))
}

/// Find the local remote pointing at the PR/MR's base repository, matching by
/// `owner`/`repo` (host-tolerant, to survive SSH host aliases). Falls back to
/// `default_remote` — the base repo is usually just `origin`, and local fixture
/// remotes don't parse into a forge slug.
pub fn find_base_remote(git: &GitCommand, base: &BaseRepo, default_remote: &str) -> String {
    for remote in git.remote_list().unwrap_or_default() {
        let Ok(url) = git.remote_get_url(&remote) else {
            continue;
        };
        if let Some((_, owner, repo)) = split_repo_key(&url)
            && owner.eq_ignore_ascii_case(&base.owner)
            && repo.eq_ignore_ascii_case(&base.repo)
        {
            return remote;
        }
    }
    default_remote.to_string()
}

/// Split a remote URL into `(host, owner, repo)` via the catalog's canonical
/// normalizer (handles scp/ssh/https/ports, `.git`, subgroups). `None` for
/// local-path remotes (fewer than 3 segments), which aren't forge repos.
pub(crate) fn split_repo_key(url: &str) -> Option<(String, String, String)> {
    let key = crate::catalog::normalize::normalize_url(url);
    let segments: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 3 {
        return None;
    }
    let host = segments[0].to_string();
    let repo = (*segments.last()?).to_string();
    let owner = segments[1..segments.len() - 1].join("/");
    Some((host, owner, repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::info::BaseRepo;

    #[test]
    fn hostname_prefers_the_pasted_url_over_config() {
        let config = ForgeConfig {
            hostname: Some("configured.example".to_string()),
            ..Default::default()
        };
        // A pasted enterprise URL names its host authoritatively.
        let url = ForgeTarget {
            number: 9,
            source: TargetSource::Url {
                kind: ForgeRefKind::GithubPr,
                host: "github.acme.com".to_string(),
                owner: "team".to_string(),
                repo: "repo".to_string(),
            },
        };
        assert_eq!(hostname_for(&url, &config), Some("github.acme.com"));

        // A bare prefix carries no host, so the configured default stands.
        let prefix = ForgeTarget {
            number: 9,
            source: TargetSource::Prefix {
                hint: ForgeRefKind::GithubPr,
            },
        };
        assert_eq!(hostname_for(&prefix, &config), Some("configured.example"));

        // No URL host and no config: the CLI's own default host (None) wins.
        assert_eq!(hostname_for(&prefix, &ForgeConfig::default()), None);
    }

    #[test]
    fn split_repo_key_handles_forms() {
        assert_eq!(
            split_repo_key("git@github.com:acme/widget.git"),
            Some(("github.com".into(), "acme".into(), "widget".into()))
        );
        assert_eq!(
            split_repo_key("https://gitlab.com/group/sub/widget.git"),
            Some(("gitlab.com".into(), "group/sub".into(), "widget".into()))
        );
        // Local-path fixture remotes don't parse into a forge slug.
        assert_eq!(split_repo_key("/remotes/test-repo"), None);
    }

    #[test]
    fn forge_capability_needs_a_known_host_or_an_override() {
        let gh = || vec!["github.com".to_string()];
        let opaque = || vec!["git.corp.example".to_string()];

        assert!(names_a_forge(None, &gh()));
        assert!(names_a_forge(None, &["gitlab.corp.example".to_string()]));
        // Opaque hosts and local-path fixtures (no parsed host at all) are
        // not forge-capable on their own…
        assert!(!names_a_forge(None, &opaque()));
        assert!(!names_a_forge(None, &[]));
        // …but the explicit platform override vouches for them.
        assert!(names_a_forge(Some("github"), &opaque()));
        assert!(names_a_forge(Some("GitLab"), &[]));
        // An invalid override vouches for nothing.
        assert!(!names_a_forge(Some("bitbucket"), &opaque()));
    }

    #[test]
    fn fork_collision_verdict() {
        let head = "refs/pull/50/head";
        // No local branch → no collision.
        assert!(!is_fork_collision(false, None, head));
        // Branch exists, untracked → collision (would hijack).
        assert!(is_fork_collision(true, None, head));
        // Branch exists, tracks a *different* ref → collision.
        assert!(is_fork_collision(true, Some("refs/heads/main"), head));
        assert!(is_fork_collision(true, Some("refs/pull/99/head"), head));
        // Branch already tracks this PR → legitimate re-checkout, no collision.
        assert!(!is_fork_collision(true, Some(head), head));
    }

    fn fork_info(source_branch: &str, number: u32) -> RemoteRefInfo {
        RemoteRefInfo {
            kind: ForgeRefKind::GithubPr,
            number,
            title: "t".into(),
            author: "a".into(),
            state: "open".into(),
            draft: false,
            source_branch: source_branch.into(),
            is_cross_repo: true,
            url: "https://github.com/acme/widget/pull/50".into(),
            base: BaseRepo {
                host: "github.com".into(),
                owner: "acme".into(),
                repo: "widget".into(),
            },
        }
    }

    /// Restores the process cwd on drop, even if an assertion panics — so a
    /// failing cwd-dependent test can't leave the cwd pointing at a since-
    /// deleted tempdir and poison parallel tests (the documented cwd-race flake).
    struct CwdGuard(Option<std::path::PathBuf>);
    impl Drop for CwdGuard {
        fn drop(&mut self) {
            if let Some(dir) = self.0.take() {
                let _ = std::env::set_current_dir(dir);
            }
        }
    }

    /// Real-git guard: a fork PR opened from the fork's `main` must not hijack
    /// the user's own local `main`.
    #[test]
    #[serial_test::serial]
    fn preflight_bails_on_conflicting_local_branch() {
        let _cwd = CwdGuard(std::env::current_dir().ok());
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            crate::utils::git_command_at(tmp.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "T")
                .env("GIT_AUTHOR_EMAIL", "t@t.co")
                .env("GIT_COMMITTER_NAME", "T")
                .env("GIT_COMMITTER_EMAIL", "t@t.co")
                .output()
                .unwrap();
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["commit", "--allow-empty", "-qm", "init"]);
        std::env::set_current_dir(tmp.path()).unwrap();
        let git = GitCommand::new(true);

        // PR #50 from a fork's `main` collides with the user's `main`.
        let err = preflight_fork_collision(&git, &fork_info("main", 50)).unwrap_err();
        assert!(err.to_string().contains("already exists"), "got: {err}");

        // Re-checkout: `main` already tracks this PR → passes. A fresh
        // GitCommand reads current config (gix_repo caches a config snapshot;
        // in production preflight runs in a new process, so it always sees what
        // a prior `daft checkout pr:` invocation wrote).
        git.set_branch_tracking("main", "origin", "refs/pull/50/head")
            .unwrap();
        let git = GitCommand::new(true);
        assert!(preflight_fork_collision(&git, &fork_info("main", 50)).is_ok());

        // A branch that doesn't exist locally → passes.
        assert!(preflight_fork_collision(&git, &fork_info("brand-new-feature", 51)).is_ok());
    }

    /// `ForgeConfig::load` maps the `daft.forge.*` git-config keys.
    #[test]
    #[serial_test::serial]
    fn forge_config_reads_git_config_keys() {
        let _cwd = CwdGuard(std::env::current_dir().ok());
        let tmp = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            crate::utils::git_command_at(tmp.path())
                .args(args)
                .output()
                .unwrap();
        };
        run(&["init", "-q"]);
        run(&["config", "daft.forge.platform", "gitlab"]);
        run(&["config", "daft.forge.githubCli", "gh-enterprise"]);
        run(&["config", "daft.forge.hostname", "git.corp.example"]);
        std::env::set_current_dir(tmp.path()).unwrap();

        let cfg = ForgeConfig::load(&GitCommand::new(true));
        assert_eq!(cfg.platform.as_deref(), Some("gitlab"));
        assert_eq!(cfg.github_cli.as_deref(), Some("gh-enterprise"));
        assert_eq!(cfg.gitlab_cli, None); // unset
        assert_eq!(cfg.hostname.as_deref(), Some("git.corp.example"));
    }
}
