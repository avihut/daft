//! Parse a checkout target into a forge PR/MR reference.
//!
//! Recognises three spellings, all pure (no I/O):
//! - `pr:123` / `mr:45` — a bare reference; the platform is resolved later from
//!   the repo's remote, so the `pr`/`mr` prefix is only a hint (they are
//!   aliases: `pr:` on a GitLab remote resolves the MR, and vice versa).
//! - A pasted PR/MR web URL (`https://github.com/o/r/pull/123`,
//!   `https://gitlab.com/g/r/-/merge_requests/45`) — which names the platform
//!   and repo authoritatively.
//!
//! `:` and `/` are illegal in git ref names, so a `pr:N` prefix or a URL can
//! never collide with a real branch name — [`ForgeTarget::parse`] returning
//! `None` means "an ordinary branch", with no false positives.

use crate::core::worktree::forge_ref::ForgeRefKind;

/// A resolved forge reference from the checkout positional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForgeTarget {
    /// PR/MR number.
    pub number: u32,
    /// Where the reference came from — a bare prefix or a full web URL.
    pub source: TargetSource,
}

/// Origin of a [`ForgeTarget`], which decides how the platform + repo are found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSource {
    /// `pr:N` / `mr:N`. `hint` is the prefix's kind, used only for wording;
    /// the actual provider is chosen from the repo's remote (aliases).
    Prefix { hint: ForgeRefKind },
    /// A pasted PR/MR web URL — authoritative about platform and repo.
    Url {
        kind: ForgeRefKind,
        host: String,
        owner: String,
        repo: String,
    },
}

impl ForgeTarget {
    /// Parse a checkout positional. Returns `None` for an ordinary branch name.
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if let Some(t) = parse_url(input) {
            return Some(t);
        }
        parse_prefix(input)
    }

    /// The prefix hint or the URL's kind — for wording only.
    pub fn kind_hint(&self) -> ForgeRefKind {
        match &self.source {
            TargetSource::Prefix { hint } => *hint,
            TargetSource::Url { kind, .. } => *kind,
        }
    }
}

/// `pr:N` / `mr:N` → a `Prefix` target. Rejects anything else.
fn parse_prefix(input: &str) -> Option<ForgeTarget> {
    let (prefix, rest) = input.split_once(':')?;
    let hint = match prefix {
        "pr" => ForgeRefKind::GithubPr,
        "mr" => ForgeRefKind::GitlabMr,
        _ => return None,
    };
    let number: u32 = rest.parse().ok()?;
    Some(ForgeTarget {
        number,
        source: TargetSource::Prefix { hint },
    })
}

/// Shape-based parse of a PR/MR web URL. Host-agnostic (so GitHub Enterprise /
/// self-hosted GitLab work): the marker segment (`pull` / `merge_requests`)
/// decides the platform, not the host. Trailing path (`/files`), query, and
/// fragment are ignored.
fn parse_url(input: &str) -> Option<ForgeTarget> {
    let (scheme, rest) = input.split_once("://")?;
    if scheme != "https" && scheme != "http" {
        return None;
    }
    // Drop query/fragment.
    let path = rest.split(['?', '#']).next()?;
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // Minimum: host / owner / repo / marker / N.
    if segments.len() < 5 {
        return None;
    }
    let host = segments[0].to_string();

    // Find `<marker> <number>` and take everything between the host and the
    // marker as the repository path (GitLab nests subgroups, and prefixes the
    // marker with `/-/`).
    for (i, pair) in segments.windows(2).enumerate() {
        let Ok(number) = pair[1].parse::<u32>() else {
            continue;
        };
        let kind = match pair[0] {
            "pull" => ForgeRefKind::GithubPr,
            "merge_requests" => ForgeRefKind::GitlabMr,
            _ => continue,
        };
        // The marker can't be the host segment: the repo path lives between the
        // host and the marker, so the slice below needs `i >= 1`. A host
        // literally named `pull`/`merge_requests` followed by a number (e.g.
        // `https://pull/9/a/b/c`) would otherwise panic on the reversed range
        // `&segments[1..0]`. Skip it and keep scanning for a real marker.
        if i == 0 {
            continue;
        }
        // Repo path segments: after the host, up to the marker. GitLab places
        // a `/-/` separator before `merge_requests`; drop the trailing `-`.
        let mut repo_segs = &segments[1..i];
        if repo_segs.last() == Some(&"-") {
            repo_segs = &repo_segs[..repo_segs.len() - 1];
        }
        // Need at least owner + repo.
        if repo_segs.len() < 2 {
            return None;
        }
        let repo = (*repo_segs.last()?).to_string();
        let owner = repo_segs[..repo_segs.len() - 1].join("/");
        return Some(ForgeTarget {
            number,
            source: TargetSource::Url {
                kind,
                host,
                owner,
                repo,
            },
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pr_and_mr_prefixes() {
        let pr = ForgeTarget::parse("pr:123").unwrap();
        assert_eq!(pr.number, 123);
        assert_eq!(
            pr.source,
            TargetSource::Prefix {
                hint: ForgeRefKind::GithubPr
            }
        );
        let mr = ForgeTarget::parse("mr:45").unwrap();
        assert_eq!(mr.number, 45);
        assert_eq!(mr.kind_hint(), ForgeRefKind::GitlabMr);
    }

    #[test]
    fn ordinary_branch_names_are_not_forge_targets() {
        assert_eq!(ForgeTarget::parse("main"), None);
        assert_eq!(ForgeTarget::parse("feature/pr-work"), None);
        assert_eq!(ForgeTarget::parse("pr:"), None); // no number
        assert_eq!(ForgeTarget::parse("pr:abc"), None); // non-numeric
        assert_eq!(ForgeTarget::parse("release:1"), None); // unknown prefix
        assert_eq!(ForgeTarget::parse(""), None);
    }

    #[test]
    fn parses_github_pr_url() {
        let t = ForgeTarget::parse("https://github.com/owner/repo/pull/123").unwrap();
        assert_eq!(t.number, 123);
        assert_eq!(
            t.source,
            TargetSource::Url {
                kind: ForgeRefKind::GithubPr,
                host: "github.com".to_string(),
                owner: "owner".to_string(),
                repo: "repo".to_string(),
            }
        );
    }

    #[test]
    fn parses_github_url_with_trailing_and_fragment() {
        assert_eq!(
            ForgeTarget::parse("https://github.com/owner/repo/pull/2895/files")
                .unwrap()
                .number,
            2895
        );
        assert_eq!(
            ForgeTarget::parse("https://github.com/o/r/pull/77#discussion_r1")
                .unwrap()
                .number,
            77
        );
    }

    #[test]
    fn parses_enterprise_github_host() {
        let t = ForgeTarget::parse("https://github.acme.com/team/repo/pull/9").unwrap();
        assert_eq!(t.kind_hint(), ForgeRefKind::GithubPr);
        assert!(matches!(t.source, TargetSource::Url { host, .. } if host == "github.acme.com"));
    }

    #[test]
    fn parses_gitlab_mr_url_with_subgroups() {
        let t = ForgeTarget::parse("https://gitlab.com/group/sub/repo/-/merge_requests/7/diffs")
            .unwrap();
        assert_eq!(t.number, 7);
        assert_eq!(
            t.source,
            TargetSource::Url {
                kind: ForgeRefKind::GitlabMr,
                host: "gitlab.com".to_string(),
                owner: "group/sub".to_string(),
                repo: "repo".to_string(),
            }
        );
    }

    #[test]
    fn rejects_non_pr_urls() {
        assert_eq!(ForgeTarget::parse("https://github.com/owner/repo"), None);
        assert_eq!(ForgeTarget::parse("https://github.com/o/r/issues/5"), None);
        assert_eq!(ForgeTarget::parse("https://github.com/o/r/pull/new"), None);
        assert_eq!(ForgeTarget::parse("https://example.com/pull/1"), None); // too shallow
        assert_eq!(ForgeTarget::parse("ftp://github.com/o/r/pull/1"), None); // wrong scheme
    }

    #[test]
    fn marker_at_host_position_does_not_panic() {
        // The host segment is literally the marker, followed by a number, with
        // enough trailing segments to clear the length guard. The repo path
        // (host+1 .. marker) is then empty — this must yield None, not panic on
        // the reversed slice `&segments[1..0]`.
        assert_eq!(ForgeTarget::parse("https://pull/9/a/b/c"), None);
        assert_eq!(ForgeTarget::parse("https://merge_requests/9/a/b/c"), None);
    }
}
