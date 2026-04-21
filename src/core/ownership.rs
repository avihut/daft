//! Branch ownership detection strategies.
//!
//! Computes who "owns" a branch from the commit range `base..branch`,
//! per the strategy configured in `daft.ownership.strategy`. See
//! `docs/superpowers/specs/2026-04-21-ownership-detection-strategies.md`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

/// Strategy for deducing branch ownership from a commit range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnershipStrategy {
    /// Owner = author of the newest commit. Original daft behavior.
    Tip,
    /// Owner = the current user if they authored any commit in range;
    /// otherwise the tip author.
    Any,
    /// Owner = author of the oldest commit in range.
    First,
    /// Owner = author with the most commits. Ties broken by recency.
    Plurality,
    /// Owner = author with > 50% of commits. No owner if no majority.
    Majority,
    /// Owner = author with highest recency-weighted score: commit at
    /// rank k from tip (k=0 = tip) contributes 1/(k+1). Ties broken by
    /// recency. This is the default.
    RecencyPlurality,
}

impl OwnershipStrategy {
    /// Parse a string value from git config.
    ///
    /// Accepts exact lowercase strings as documented:
    /// `tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`.
    /// Matching is case-insensitive. Returns `None` for unknown values.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "tip" => Some(Self::Tip),
            "any" => Some(Self::Any),
            "first" => Some(Self::First),
            "plurality" => Some(Self::Plurality),
            "majority" => Some(Self::Majority),
            "recency-plurality" => Some(Self::RecencyPlurality),
            _ => None,
        }
    }
}

/// A commit's author identity + recency, as needed by the resolver.
///
/// Commits are expected in **reverse-chronological order** (newest first) —
/// the natural order of `git log`. Rank 0 = tip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub author_name: String,
    pub author_email: String,
    /// Committer unix timestamp. Used as tie-breaker (largest wins).
    pub committer_timestamp: i64,
}

/// Resolved branch owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchOwner {
    /// Author name of the winning commit (used for display).
    pub name: String,
    /// Author email of the winning author (used for comparison /
    /// `--include <email>` matching).
    pub email: String,
    /// True iff `email` matches the current user's `git config user.email`.
    /// Precomputed so downstream code doesn't need `user_email` plumbing.
    pub is_current_user: bool,
}

/// Resolve the owner of a branch from its `base..branch` commit range.
///
/// `commits` MUST be reverse-chronological (newest first) — the natural
/// order of `git log`. Rank 0 = tip.
///
/// `user_email` is the current user's `git config user.email`, if set.
/// When `None`, the resolved owner is never `is_current_user = true`.
pub fn resolve_owner_from_records(
    commits: &[CommitRecord],
    strategy: OwnershipStrategy,
    user_email: Option<&str>,
) -> Option<BranchOwner> {
    if commits.is_empty() {
        return None;
    }

    debug_assert!(
        commits
            .windows(2)
            .all(|w| w[0].committer_timestamp >= w[1].committer_timestamp),
        "commits must be reverse-chronological (newest first)"
    );

    let winning_email: String = match strategy {
        OwnershipStrategy::Tip => commits[0].author_email.clone(),
        OwnershipStrategy::First => commits.last().unwrap().author_email.clone(),
        OwnershipStrategy::Any => {
            if let Some(user) = user_email {
                if commits
                    .iter()
                    .any(|c| c.author_email.eq_ignore_ascii_case(user))
                {
                    user.to_string()
                } else {
                    commits[0].author_email.clone()
                }
            } else {
                commits[0].author_email.clone()
            }
        }
        OwnershipStrategy::Plurality => pick_plurality(commits)?,
        OwnershipStrategy::Majority => pick_majority(commits)?,
        OwnershipStrategy::RecencyPlurality => pick_recency_plurality(commits)?,
    };

    let name = most_recent_name_for_email(commits, &winning_email)?;

    let is_current_user = user_email
        .map(|u| u.eq_ignore_ascii_case(&winning_email))
        .unwrap_or(false);

    Some(BranchOwner {
        name,
        email: winning_email,
        is_current_user,
    })
}

/// Return the author name from the most recent commit whose author email
/// matches `email` (case-insensitive). Assumes `commits` is
/// reverse-chronological; returns the first matching name.
fn most_recent_name_for_email(commits: &[CommitRecord], email: &str) -> Option<String> {
    commits
        .iter()
        .find(|c| c.author_email.eq_ignore_ascii_case(email))
        .map(|c| c.author_name.clone())
}

/// Plurality: author with the most commits. Ties broken by
/// most-recent-commit-of-tied-author (largest committer_timestamp wins).
fn pick_plurality(commits: &[CommitRecord]) -> Option<String> {
    let mut counts: HashMap<String, (u32, i64)> = HashMap::new();
    for c in commits {
        let key = c.author_email.to_ascii_lowercase();
        let entry = counts.entry(key).or_insert((0, i64::MIN));
        entry.0 += 1;
        if c.committer_timestamp > entry.1 {
            entry.1 = c.committer_timestamp;
        }
    }
    let winner = counts
        .into_iter()
        .max_by(|(ka, a), (kb, b)| {
            a.0.cmp(&b.0)
                .then(a.1.cmp(&b.1))
                // Deterministic tertiary tiebreak so the resolver is
                // pure-functional: when count + recency are identical
                // (common after squash-merges with reused timestamps),
                // pick the alphabetically later email. Matches
                // pick_recency_plurality.
                .then(ka.cmp(kb))
        })?
        .0;
    // Return canonical-cased email from the original record (first match).
    commits
        .iter()
        .find(|c| c.author_email.eq_ignore_ascii_case(&winner))
        .map(|c| c.author_email.clone())
}

/// Majority: author with > 50% of commits. Returns `None` if no author
/// strictly exceeds 50%.
fn pick_majority(commits: &[CommitRecord]) -> Option<String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for c in commits {
        let key = c.author_email.to_ascii_lowercase();
        *counts.entry(key).or_insert(0) += 1;
    }
    let total = commits.len() as u32;
    let (winner, count) = counts.into_iter().max_by_key(|(_, n)| *n)?;
    if count * 2 > total {
        commits
            .iter()
            .find(|c| c.author_email.eq_ignore_ascii_case(&winner))
            .map(|c| c.author_email.clone())
    } else {
        None
    }
}

/// Recency-plurality: author with the highest sum of `1/(k+1)` over
/// their commits, where k is the commit's zero-based rank from the tip.
/// Ties broken by most-recent-commit-of-tied-author.
fn pick_recency_plurality(commits: &[CommitRecord]) -> Option<String> {
    let mut scores: HashMap<String, (f64, i64)> = HashMap::new();
    for (k, c) in commits.iter().enumerate() {
        let weight = 1.0 / (k as f64 + 1.0);
        let key = c.author_email.to_ascii_lowercase();
        let entry = scores.entry(key).or_insert((0.0, i64::MIN));
        entry.0 += weight;
        if c.committer_timestamp > entry.1 {
            entry.1 = c.committer_timestamp;
        }
    }
    let winner = scores
        .into_iter()
        .max_by(|(ka, a), (kb, b)| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
                .then(ka.cmp(kb))
        })?
        .0;
    commits
        .iter()
        .find(|c| c.author_email.eq_ignore_ascii_case(&winner))
        .map(|c| c.author_email.clone())
}

/// Fetch commit records for `base..branch` from git, newest first.
///
/// Returns an empty Vec on any git error (unreachable base, malformed
/// branch, etc.) — ownership is best-effort and must never block daft.
pub fn fetch_commit_records(base: &str, branch: &str, cwd: &Path) -> Vec<CommitRecord> {
    let range = format!("{base}..{branch}");
    let output = match Command::new("git")
        .args(["log", &range, "--format=%an%x09%ae%x09%ct"])
        .current_dir(cwd)
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let name = parts.next()?.to_string();
            let email = parts.next()?.to_string();
            let ts = parts.next()?.trim().parse::<i64>().ok()?;
            if email.is_empty() {
                return None;
            }
            Some(CommitRecord {
                author_name: name,
                author_email: email,
                committer_timestamp: ts,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a CommitRecord. Timestamps are arbitrary; use the numeric
    /// order implied by the Vec (older last) when timestamps matter.
    fn rec(name: &str, email: &str, ts: i64) -> CommitRecord {
        CommitRecord {
            author_name: name.into(),
            author_email: email.into(),
            committer_timestamp: ts,
        }
    }

    fn me() -> Option<&'static str> {
        Some("me@example.com")
    }

    // ── Empty range ────────────────────────────────────────────────────
    #[test]
    fn resolve_returns_none_for_empty_range() {
        for strategy in [
            OwnershipStrategy::Tip,
            OwnershipStrategy::Any,
            OwnershipStrategy::First,
            OwnershipStrategy::Plurality,
            OwnershipStrategy::Majority,
            OwnershipStrategy::RecencyPlurality,
        ] {
            assert_eq!(resolve_owner_from_records(&[], strategy, me()), None);
        }
    }

    // ── Single-commit range: all strategies agree ──────────────────────
    #[test]
    fn resolve_single_commit_same_across_strategies() {
        let commits = vec![rec("Alice", "alice@x.com", 100)];
        for strategy in [
            OwnershipStrategy::Tip,
            OwnershipStrategy::Any,
            OwnershipStrategy::First,
            OwnershipStrategy::Plurality,
            OwnershipStrategy::Majority,
            OwnershipStrategy::RecencyPlurality,
        ] {
            let owner = resolve_owner_from_records(&commits, strategy, me()).unwrap();
            assert_eq!(owner.name, "Alice", "{strategy:?}");
            assert_eq!(owner.email, "alice@x.com");
            assert!(!owner.is_current_user);
        }
    }

    // ── Tip strategy ───────────────────────────────────────────────────
    #[test]
    fn tip_picks_newest_commit_author() {
        // Newest first.
        let commits = vec![
            rec("Bob", "bob@x.com", 200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Tip, me()).unwrap();
        assert_eq!(owner.name, "Bob");
    }

    // ── Any strategy ───────────────────────────────────────────────────
    #[test]
    fn any_matches_current_user_anywhere_in_range() {
        let commits = vec![
            rec("Bob", "bob@x.com", 300),
            rec("Me", "me@example.com", 200),
            rec("Bob", "bob@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Any, me()).unwrap();
        assert_eq!(owner.email, "me@example.com");
        assert!(owner.is_current_user);
    }

    #[test]
    fn any_falls_back_to_tip_when_user_absent() {
        let commits = vec![
            rec("Bob", "bob@x.com", 200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Any, me()).unwrap();
        assert_eq!(owner.email, "bob@x.com"); // tip
        assert!(!owner.is_current_user);
    }

    // ── First strategy ─────────────────────────────────────────────────
    #[test]
    fn first_picks_oldest_commit_author() {
        let commits = vec![
            rec("Bob", "bob@x.com", 300),
            rec("Bob", "bob@x.com", 200),
            rec("Alice", "alice@x.com", 100), // oldest
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::First, me()).unwrap();
        assert_eq!(owner.email, "alice@x.com");
    }

    // ── Plurality strategy ─────────────────────────────────────────────
    #[test]
    fn plurality_picks_most_frequent_author() {
        let commits = vec![
            rec("Bob", "bob@x.com", 500),
            rec("Alice", "alice@x.com", 400),
            rec("Alice", "alice@x.com", 300),
            rec("Alice", "alice@x.com", 200),
            rec("Bob", "bob@x.com", 100),
        ];
        let owner =
            resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, me()).unwrap();
        assert_eq!(owner.email, "alice@x.com");
    }

    #[test]
    fn plurality_tiebreak_by_most_recent_commit_of_tied_author() {
        // Alice and Bob each have 2 commits; Bob's most recent is newer.
        let commits = vec![
            rec("Bob", "bob@x.com", 400),
            rec("Alice", "alice@x.com", 300),
            rec("Bob", "bob@x.com", 200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner =
            resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, me()).unwrap();
        assert_eq!(owner.email, "bob@x.com");
    }

    // ── Majority strategy ──────────────────────────────────────────────
    #[test]
    fn majority_requires_strict_majority() {
        // 2 of 4 is not > 50% — no majority.
        let commits = vec![
            rec("Alice", "alice@x.com", 400),
            rec("Bob", "bob@x.com", 300),
            rec("Alice", "alice@x.com", 200),
            rec("Bob", "bob@x.com", 100),
        ];
        assert_eq!(
            resolve_owner_from_records(&commits, OwnershipStrategy::Majority, me()),
            None
        );
    }

    #[test]
    fn majority_picks_when_clear() {
        // 3 of 4 for Alice.
        let commits = vec![
            rec("Alice", "alice@x.com", 400),
            rec("Bob", "bob@x.com", 300),
            rec("Alice", "alice@x.com", 200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner =
            resolve_owner_from_records(&commits, OwnershipStrategy::Majority, me()).unwrap();
        assert_eq!(owner.email, "alice@x.com");
    }

    // ── Recency-plurality strategy ─────────────────────────────────────
    #[test]
    fn recency_plurality_user_wins_despite_teammate_tip() {
        // User authored 5 older commits; teammate authored the tip.
        //   user score: 1/2+1/3+1/4+1/5+1/6 ≈ 1.45
        //   bob  score: 1/1 = 1.00
        let commits = vec![
            rec("Bob", "bob@x.com", 600),
            rec("Me", "me@example.com", 500),
            rec("Me", "me@example.com", 400),
            rec("Me", "me@example.com", 300),
            rec("Me", "me@example.com", 200),
            rec("Me", "me@example.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::RecencyPlurality, me())
            .unwrap();
        assert_eq!(owner.email, "me@example.com");
        assert!(owner.is_current_user);
    }

    #[test]
    fn recency_plurality_three_recent_beat_one_old() {
        // Teammate rebase-on-top of one older commit of yours.
        //   bob score: 1/1 + 1/2 + 1/3 ≈ 1.83
        //   me  score: 1/4 = 0.25
        let commits = vec![
            rec("Bob", "bob@x.com", 400),
            rec("Bob", "bob@x.com", 300),
            rec("Bob", "bob@x.com", 200),
            rec("Me", "me@example.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::RecencyPlurality, me())
            .unwrap();
        assert_eq!(owner.email, "bob@x.com");
    }

    // ── Name disambiguation: most-recent name wins for same email ────
    #[test]
    fn same_email_different_names_uses_most_recent_name() {
        let commits = vec![
            rec("Avihu Turzion", "avihu@example.com", 200),
            rec("Avihu", "avihu@example.com", 100),
        ];
        let owner =
            resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, me()).unwrap();
        assert_eq!(owner.email, "avihu@example.com");
        assert_eq!(owner.name, "Avihu Turzion");
    }

    // ── is_current_user precomputation ─────────────────────────────────
    #[test]
    fn is_current_user_set_when_email_matches() {
        let commits = vec![rec("Me", "me@example.com", 100)];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Tip, me()).unwrap();
        assert!(owner.is_current_user);
    }

    #[test]
    fn is_current_user_false_when_no_user_email_configured() {
        let commits = vec![rec("Me", "me@example.com", 100)];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Tip, None).unwrap();
        assert!(!owner.is_current_user);
    }

    #[test]
    fn any_with_no_user_email_falls_back_to_tip() {
        let commits = vec![
            rec("Bob", "bob@x.com", 200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Any, None).unwrap();
        assert_eq!(owner.email, "bob@x.com");
        assert!(!owner.is_current_user);
    }

    #[test]
    fn plurality_collapses_case_insensitive_emails() {
        // Same author, inconsistent email casing (e.g. across machines).
        let commits = vec![
            rec("Alice", "Alice@X.com", 300),
            rec("Bob", "bob@x.com", 250),
            rec("Alice", "alice@x.com", 200),
            rec("Alice", "ALICE@X.COM", 100),
        ];
        let owner =
            resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, None).unwrap();
        // All three of Alice's commits collapse into one author; Alice (3) > Bob (1).
        assert!(
            owner.email.eq_ignore_ascii_case("alice@x.com"),
            "expected alice@x.com, got {:?}",
            owner.email
        );
        assert_eq!(owner.name, "Alice");
    }

    #[test]
    fn parse_accepts_all_known_strategies() {
        assert_eq!(
            OwnershipStrategy::parse("tip"),
            Some(OwnershipStrategy::Tip)
        );
        assert_eq!(
            OwnershipStrategy::parse("any"),
            Some(OwnershipStrategy::Any)
        );
        assert_eq!(
            OwnershipStrategy::parse("first"),
            Some(OwnershipStrategy::First)
        );
        assert_eq!(
            OwnershipStrategy::parse("plurality"),
            Some(OwnershipStrategy::Plurality)
        );
        assert_eq!(
            OwnershipStrategy::parse("majority"),
            Some(OwnershipStrategy::Majority)
        );
        assert_eq!(
            OwnershipStrategy::parse("recency-plurality"),
            Some(OwnershipStrategy::RecencyPlurality)
        );
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(
            OwnershipStrategy::parse("Recency-Plurality"),
            Some(OwnershipStrategy::RecencyPlurality)
        );
        assert_eq!(
            OwnershipStrategy::parse("TIP"),
            Some(OwnershipStrategy::Tip)
        );
    }

    #[test]
    fn parse_trims_whitespace() {
        assert_eq!(
            OwnershipStrategy::parse("  tip  "),
            Some(OwnershipStrategy::Tip)
        );
    }

    #[test]
    fn parse_returns_none_for_unknown() {
        assert_eq!(OwnershipStrategy::parse(""), None);
        assert_eq!(OwnershipStrategy::parse("owner"), None);
        assert_eq!(OwnershipStrategy::parse("recency"), None);
    }

    use std::process::Command as StdCommand;

    /// Create a minimal throwaway git repo for integration tests.
    /// Returns its path. Caller is responsible for cleanup — use tempfile.
    fn init_repo(dir: &std::path::Path) {
        StdCommand::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "--local", "user.email", "tester@example.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "--local", "user.name", "Tester"])
            .current_dir(dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "--local", "commit.gpgsign", "false"])
            .current_dir(dir)
            .output()
            .unwrap();
    }

    fn commit_as(dir: &std::path::Path, name: &str, email: &str, message: &str) {
        let path = dir.join(format!("{message}.txt"));
        std::fs::write(&path, message).unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", message])
            .env("GIT_AUTHOR_NAME", name)
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_COMMITTER_NAME", name)
            .env("GIT_COMMITTER_EMAIL", email)
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn fetch_commit_records_returns_newest_first_in_range() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        commit_as(repo, "Tester", "tester@example.com", "base");

        StdCommand::new("git")
            .args(["checkout", "-q", "-b", "feature"])
            .current_dir(repo)
            .output()
            .unwrap();
        commit_as(repo, "Alice", "alice@example.com", "first-feature");
        commit_as(repo, "Bob", "bob@example.com", "second-feature");

        let commits = fetch_commit_records("main", "feature", repo);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].author_name, "Bob", "newest first");
        assert_eq!(commits[1].author_name, "Alice");
    }

    #[test]
    fn fetch_commit_records_returns_empty_for_zero_divergence() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        init_repo(repo);
        commit_as(repo, "Tester", "tester@example.com", "base");

        let commits = fetch_commit_records("main", "main", repo);
        assert!(commits.is_empty());
    }

    #[test]
    fn fetch_commit_records_returns_empty_on_git_error() {
        let tmp = tempfile::tempdir().unwrap();
        let commits = fetch_commit_records("main", "feature", tmp.path());
        assert!(commits.is_empty(), "not a git repo → empty");
    }
}
