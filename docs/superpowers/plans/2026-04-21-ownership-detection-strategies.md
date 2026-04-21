# Ownership Detection Strategies Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace tip-author-email ownership detection with a strategy-driven
resolver over the full `base..branch` commit range, configurable via
`daft.ownership.strategy`, defaulting to recency-weighted plurality. Display
author name instead of email.

**Architecture:** One new module (`src/core/ownership.rs`) holds the pure
resolver. Each `WorktreeInfo` carries a new `owner: Option<BranchOwner>` struct
with `name`, `email`, and a precomputed `is_current_user` flag that eliminates
`user_email` plumbing throughout sync.rs. The old `owner_email` field is kept
during the transition so every commit compiles, then removed in a dedicated
cleanup task.

**Tech Stack:** Rust (clap, anyhow, serde_json), git CLI (`git log`,
`git config`), existing YAML integration test harness.

**Reference spec:**
`docs/superpowers/specs/2026-04-21-ownership-detection-strategies.md`

---

## File Structure

**New files:**

- `src/core/ownership.rs` — `OwnershipStrategy` enum, `BranchOwner` struct,
  `CommitRecord` struct, pure resolver functions, `fetch_commit_records()`.

**Modified files (data model / logic):**

- `src/core/mod.rs` — `pub mod ownership;`
- `src/core/settings.rs` — new field + keys + defaults + load wiring.
- `src/core/worktree/list.rs` — add `owner: Option<BranchOwner>` field; populate
  via `resolve_owner`; eventually drop `owner_email` and
  `get_author_email_for_ref`.
- `src/commands/sync.rs` — `is_branch_included` takes `Option<&BranchOwner>`;
  drop `user_email` plumbing.
- `src/commands/prune.rs` — update local-only stub population to new resolver.
- `src/core/sort.rs` — `SortColumn::Owner` compares on name.

**Modified files (display):**

- `src/output/format.rs` — Owner column renders `owner.name`.
- `src/commands/list.rs` — JSON `"owner"` emits `{name, email} | null`.
- `src/output/tui/columns.rs`, `src/output/tui/render.rs` — no code change
  beyond the `ColumnValues.owner` source string (now `owner.name`).

**Modified files (tests & docs):**

- `tests/manual/scenarios/list/owner-column.yml` — expect author name, not
  email.
- `tests/manual/scenarios/sync/ownership-rebase-push.yml` — author name in
  assertions; new `user.name` setup.
- New: `tests/manual/scenarios/list/owner-strategy-recency.yml`,
  `.../owner-strategy-tip.yml`, `.../owner-strategy-plurality.yml`.
- New: `tests/manual/scenarios/sync/ownership-strategy-recency.yml`.
- `docs/cli/daft-list.md` — owner column description, JSON field name.
- `docs/cli/daft-sync.md` — owner column description + mention strategy.
- `docs/cli/daft-prune.md` — owner column description.
- `docs/cli/git-worktree-list.md` — column description.
- `docs/guide/configuration.md` — new `daft.ownership.strategy` key.
- `SKILL.md` — one line on strategy-based ownership.
- `man/*.1` — regenerated.

---

## Task 1: Add `OwnershipStrategy` enum and parser

**Files:**

- Create: `src/core/ownership.rs` (new, minimal content in this task)
- Modify: `src/core/mod.rs` (add module declaration)

- [ ] **Step 1: Declare the new module**

In `src/core/mod.rs`, find the existing `pub mod` declarations and add:

```rust
pub mod ownership;
```

(Alongside `pub mod sort;`, `pub mod columns;`, `pub mod settings;`, etc.)

- [ ] **Step 2: Write failing parser tests**

Create `src/core/ownership.rs` with initial test-only content:

```rust
//! Branch ownership detection strategies.
//!
//! Computes who "owns" a branch from the commit range `base..branch`,
//! per the strategy configured in `daft.ownership.strategy`. See
//! `docs/superpowers/specs/2026-04-21-ownership-detection-strategies.md`.

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_all_known_strategies() {
        assert_eq!(OwnershipStrategy::parse("tip"), Some(OwnershipStrategy::Tip));
        assert_eq!(OwnershipStrategy::parse("any"), Some(OwnershipStrategy::Any));
        assert_eq!(OwnershipStrategy::parse("first"), Some(OwnershipStrategy::First));
        assert_eq!(OwnershipStrategy::parse("plurality"), Some(OwnershipStrategy::Plurality));
        assert_eq!(OwnershipStrategy::parse("majority"), Some(OwnershipStrategy::Majority));
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
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::ownership::tests` Expected: 4 passed.

- [ ] **Step 4: Commit**

```bash
git add src/core/mod.rs src/core/ownership.rs
git commit -m "feat(ownership): add OwnershipStrategy enum with parser"
```

---

## Task 2: Add `BranchOwner` + pure resolver functions

**Files:**

- Modify: `src/core/ownership.rs`

- [ ] **Step 1: Add the `BranchOwner` and `CommitRecord` types**

Append below the `OwnershipStrategy` block in `src/core/ownership.rs`:

```rust
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
```

- [ ] **Step 2: Write failing resolver tests**

Add at the top of the existing `#[cfg(test)] mod tests` block:

```rust
    /// Build a CommitRecord. Timestamps are arbitrary; use the numeric
    /// order implied by the Vec (older last) when timestamps matter.
    fn rec(name: &str, email: &str, ts: i64) -> CommitRecord {
        CommitRecord {
            author_name: name.into(),
            author_email: email.into(),
            committer_timestamp: ts,
        }
    }

    fn me() -> Option<&'static str> { Some("me@example.com") }

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
            rec("Bob",   "bob@x.com",   200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Tip, me()).unwrap();
        assert_eq!(owner.name, "Bob");
    }

    // ── Any strategy ───────────────────────────────────────────────────
    #[test]
    fn any_matches_current_user_anywhere_in_range() {
        let commits = vec![
            rec("Bob", "bob@x.com",      300),
            rec("Me",  "me@example.com", 200),
            rec("Bob", "bob@x.com",      100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Any, me()).unwrap();
        assert_eq!(owner.email, "me@example.com");
        assert!(owner.is_current_user);
    }

    #[test]
    fn any_falls_back_to_tip_when_user_absent() {
        let commits = vec![
            rec("Bob",   "bob@x.com",   200),
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
            rec("Bob",   "bob@x.com",   300),
            rec("Bob",   "bob@x.com",   200),
            rec("Alice", "alice@x.com", 100), // oldest
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::First, me()).unwrap();
        assert_eq!(owner.email, "alice@x.com");
    }

    // ── Plurality strategy ─────────────────────────────────────────────
    #[test]
    fn plurality_picks_most_frequent_author() {
        let commits = vec![
            rec("Bob",   "bob@x.com",   500),
            rec("Alice", "alice@x.com", 400),
            rec("Alice", "alice@x.com", 300),
            rec("Alice", "alice@x.com", 200),
            rec("Bob",   "bob@x.com",   100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, me()).unwrap();
        assert_eq!(owner.email, "alice@x.com");
    }

    #[test]
    fn plurality_tiebreak_by_most_recent_commit_of_tied_author() {
        // Alice and Bob each have 2 commits; Bob's most recent is newer.
        let commits = vec![
            rec("Bob",   "bob@x.com",   400),
            rec("Alice", "alice@x.com", 300),
            rec("Bob",   "bob@x.com",   200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, me()).unwrap();
        assert_eq!(owner.email, "bob@x.com");
    }

    // ── Majority strategy ──────────────────────────────────────────────
    #[test]
    fn majority_requires_strict_majority() {
        // 2 of 4 is not > 50% — no majority.
        let commits = vec![
            rec("Alice", "alice@x.com", 400),
            rec("Bob",   "bob@x.com",   300),
            rec("Alice", "alice@x.com", 200),
            rec("Bob",   "bob@x.com",   100),
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
            rec("Bob",   "bob@x.com",   300),
            rec("Alice", "alice@x.com", 200),
            rec("Alice", "alice@x.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Majority, me()).unwrap();
        assert_eq!(owner.email, "alice@x.com");
    }

    // ── Recency-plurality strategy ─────────────────────────────────────
    #[test]
    fn recency_plurality_user_wins_despite_teammate_tip() {
        // User authored 5 older commits; teammate authored the tip.
        //   user score: 1/2+1/3+1/4+1/5+1/6 ≈ 1.45
        //   bob  score: 1/1 = 1.00
        let commits = vec![
            rec("Bob", "bob@x.com",      600),
            rec("Me",  "me@example.com", 500),
            rec("Me",  "me@example.com", 400),
            rec("Me",  "me@example.com", 300),
            rec("Me",  "me@example.com", 200),
            rec("Me",  "me@example.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::RecencyPlurality, me()).unwrap();
        assert_eq!(owner.email, "me@example.com");
        assert!(owner.is_current_user);
    }

    #[test]
    fn recency_plurality_three_recent_beat_one_old() {
        // Teammate rebase-on-top of one older commit of yours.
        //   bob score: 1/1 + 1/2 + 1/3 ≈ 1.83
        //   me  score: 1/4 = 0.25
        let commits = vec![
            rec("Bob", "bob@x.com",      400),
            rec("Bob", "bob@x.com",      300),
            rec("Bob", "bob@x.com",      200),
            rec("Me",  "me@example.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::RecencyPlurality, me()).unwrap();
        assert_eq!(owner.email, "bob@x.com");
    }

    // ── Name disambiguation: most-recent name wins for same email ────
    #[test]
    fn same_email_different_names_uses_most_recent_name() {
        let commits = vec![
            rec("Avihu Turzion", "avihu@example.com", 200),
            rec("Avihu",         "avihu@example.com", 100),
        ];
        let owner = resolve_owner_from_records(&commits, OwnershipStrategy::Plurality, me()).unwrap();
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
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test -p daft --lib core::ownership::tests` Expected: compilation
error — `resolve_owner_from_records` not defined.

- [ ] **Step 4: Implement the resolver**

Append to `src/core/ownership.rs` (above the `#[cfg(test)]` block):

```rust
use std::collections::HashMap;

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

    let winning_email: String = match strategy {
        OwnershipStrategy::Tip => commits[0].author_email.clone(),
        OwnershipStrategy::First => commits.last().unwrap().author_email.clone(),
        OwnershipStrategy::Any => {
            if let Some(user) = user_email {
                if commits.iter().any(|c| c.author_email.eq_ignore_ascii_case(user)) {
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
        .max_by(|(_, a), (_, b)| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))?
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
        .max_by(|(_, a), (_, b)| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        })?
        .0;
    commits
        .iter()
        .find(|c| c.author_email.eq_ignore_ascii_case(&winner))
        .map(|c| c.author_email.clone())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::ownership::tests` Expected: all resolver
tests pass (14+ tests total including parser tests from Task 1).

- [ ] **Step 6: Commit**

```bash
git add src/core/ownership.rs
git commit -m "feat(ownership): add strategy-driven resolver over commit range"
```

---

## Task 3: Add `fetch_commit_records()` — the git layer

**Files:**

- Modify: `src/core/ownership.rs`

- [ ] **Step 1: Add the fetch function**

Append to `src/core/ownership.rs` (above `#[cfg(test)]`):

```rust
use std::path::Path;
use std::process::Command;

/// Fetch commit records for `base..branch` from git, newest first.
///
/// Returns an empty Vec on any git error (unreachable base, malformed
/// branch, etc.) — ownership is best-effort and must never block daft.
pub fn fetch_commit_records(base: &str, branch: &str, cwd: &Path) -> Vec<CommitRecord> {
    let range = format!("{base}..{branch}");
    let output = match Command::new("git")
        .args([
            "log",
            &range,
            "--format=%an%x09%ae%x09%ct",
        ])
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
```

- [ ] **Step 2: Add integration test**

Append inside the existing `#[cfg(test)] mod tests` block:

```rust
    use std::process::Command as StdCommand;

    /// Create a minimal throwaway git repo for integration tests.
    /// Returns its path. Caller is responsible for cleanup — use tempfile.
    fn init_repo(dir: &std::path::Path) {
        StdCommand::new("git").args(["init", "-q", "-b", "main"]).current_dir(dir).output().unwrap();
        StdCommand::new("git").args(["config", "--local", "user.email", "tester@example.com"]).current_dir(dir).output().unwrap();
        StdCommand::new("git").args(["config", "--local", "user.name", "Tester"]).current_dir(dir).output().unwrap();
        StdCommand::new("git").args(["config", "--local", "commit.gpgsign", "false"]).current_dir(dir).output().unwrap();
    }

    fn commit_as(dir: &std::path::Path, name: &str, email: &str, message: &str) {
        let path = dir.join(format!("{message}.txt"));
        std::fs::write(&path, message).unwrap();
        StdCommand::new("git").args(["add", "."]).current_dir(dir).output().unwrap();
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

        StdCommand::new("git").args(["checkout", "-q", "-b", "feature"]).current_dir(repo).output().unwrap();
        commit_as(repo, "Alice", "alice@example.com", "first-feature");
        commit_as(repo, "Bob",   "bob@example.com",   "second-feature");

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
```

- [ ] **Step 3: Verify `tempfile` is a dev-dependency**

Run: `grep -A2 '^\[dev-dependencies\]' Cargo.toml` Expected: `tempfile = ...`
present. If not, add it:

```toml
# in [dev-dependencies]
tempfile = "3"
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p daft --lib core::ownership` Expected: new
`fetch_commit_records_*` tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/ownership.rs Cargo.toml Cargo.lock
git commit -m "feat(ownership): add git-log-backed CommitRecord fetcher"
```

---

## Task 4: Wire `ownership_strategy` into `DaftSettings`

**Files:**

- Modify: `src/core/settings.rs`

- [ ] **Step 1: Add the default constant**

In `src/core/settings.rs` inside `pub mod defaults`, after
`BRANCH_DELETE_REMOTE`:

```rust
    /// Default value for ownership.strategy setting.
    pub const OWNERSHIP_STRATEGY: crate::core::ownership::OwnershipStrategy =
        crate::core::ownership::OwnershipStrategy::RecencyPlurality;
```

- [ ] **Step 2: Add the config key constant**

In `pub mod keys`, after `BRANCH_DELETE_REMOTE`:

```rust
    /// Config key for ownership.strategy setting.
    pub const OWNERSHIP_STRATEGY: &str = "daft.ownership.strategy";
```

- [ ] **Step 3: Add the field**

In `pub struct DaftSettings`, after `pub branch_delete_remote: bool,`:

```rust
    /// Strategy for deducing branch ownership from the commit range
    /// `base..branch`. Set via `daft.ownership.strategy`.
    pub ownership_strategy: crate::core::ownership::OwnershipStrategy,
```

- [ ] **Step 4: Initialize the field in `Default`**

In `impl Default for DaftSettings`, after
`branch_delete_remote: defaults::BRANCH_DELETE_REMOTE,`:

```rust
            ownership_strategy: defaults::OWNERSHIP_STRATEGY,
```

- [ ] **Step 5: Load from local + global config**

Inside `pub fn load()`, after the `BRANCH_DELETE_REMOTE` load block:

```rust
        if let Some(value) = git.config_get(keys::OWNERSHIP_STRATEGY)? {
            if !value.is_empty() {
                match crate::core::ownership::OwnershipStrategy::parse(&value) {
                    Some(strategy) => settings.ownership_strategy = strategy,
                    None => eprintln!(
                        "daft: unknown value for {}: {:?} — using default",
                        keys::OWNERSHIP_STRATEGY,
                        value
                    ),
                }
            }
        }
```

Add the equivalent block to `pub fn load_global()` (substituting
`config_get_global`).

- [ ] **Step 6: Add tests**

Inside `#[cfg(test)] mod tests` at the bottom of `settings.rs`, after the
existing tests:

```rust
    #[test]
    fn default_ownership_strategy_is_recency_plurality() {
        let settings = DaftSettings::default();
        assert_eq!(
            settings.ownership_strategy,
            crate::core::ownership::OwnershipStrategy::RecencyPlurality
        );
    }
```

- [ ] **Step 7: Update the existing `test_default_settings` assertion**

Find `fn test_default_settings()` near the top of the tests module. Add:

```rust
        assert_eq!(
            settings.ownership_strategy,
            crate::core::ownership::OwnershipStrategy::RecencyPlurality
        );
```

- [ ] **Step 8: Update the module-level doc comment**

At the top of `src/core/settings.rs`, find the "Config Keys" table and add a
row:

```
//! | `daft.ownership.strategy` | `recency-plurality` | Branch ownership detection strategy (`tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`) |
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p daft --lib core::settings::tests` Expected: all pass,
including the two new assertions.

- [ ] **Step 10: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat(settings): add daft.ownership.strategy config key"
```

---

## Task 5: Populate `owner: Option<BranchOwner>` on `WorktreeInfo`

**Files:**

- Modify: `src/core/worktree/list.rs`
- Modify: `src/commands/prune.rs`

NOTE: This task ADDS the new field alongside the existing `owner_email` field.
Both fields coexist during the transition (tasks 6–10); `owner_email` is removed
in Task 11.

- [ ] **Step 1: Re-export `BranchOwner` from list**

At the top of `src/core/worktree/list.rs`, near the other `use` statements:

```rust
use crate::core::ownership::{self, BranchOwner, OwnershipStrategy};
```

- [ ] **Step 2: Add the `owner` field to `WorktreeInfo`**

In `pub struct WorktreeInfo`, directly below `pub owner_email: Option<String>,`:

```rust
    /// Resolved branch owner per the configured strategy. `None` when
    /// `base..branch` is empty or git failed.
    pub owner: Option<BranchOwner>,
```

- [ ] **Step 3: Initialize `owner: None` in `empty()` and
      `local_branch_stub()`**

In `fn empty()`, add below `owner_email: None,`:

```rust
            owner: None,
```

In `fn local_branch_stub(name: &str, owner_email: Option<String>) -> Self`, add
below `owner_email,`:

```rust
            owner: None,
```

(`local_branch_stub` keeps its old signature in this task; the caller is updated
in Step 6.)

- [ ] **Step 4: Plumb strategy + user_email through `collect_worktree_info`**

Find the current signature:

```rust
pub fn collect_worktree_info(
    git: &GitCommand,
    base_branch: &str,
    stat: Stat,
    ...
) -> Result<Vec<WorktreeInfo>>
```

Change it to accept strategy and user_email. Add parameters:

```rust
    ownership_strategy: OwnershipStrategy,
    user_email: Option<&str>,
```

Update every call site (use Grep to find them):

```bash
rg "collect_worktree_info\(" src/ --type rust
```

For each caller, pass `settings.ownership_strategy` and obtain `user_email` from
`git.config_get("user.email").ok().flatten()` (already present at every call
site per the current sync.rs / list.rs / prune.rs flow).

- [ ] **Step 5: Populate `owner` inside `collect_worktree_info`**

Find the three occurrences of `let owner_email = ...` /
`let owner_email = get_author_email_for_ref(...)` in `list.rs` (currently at
~lines 834, 984, 1076).

At each site, directly after the `owner_email = ...` line, add:

```rust
        let owner = if !entry.is_detached {
            let commits = ownership::fetch_commit_records(base_branch, &branch_display, &entry.path);
            ownership::resolve_owner_from_records(&commits, ownership_strategy, user_email)
        } else {
            None
        };
```

(Adjust the `!entry.is_detached` guard / `branch_display` variable to match
whichever of the three sites you're in — in `collect_branch_info` for local
branches use `branch` directly and no detached guard; for remote branches use
`remote_branch`.)

Then in the `WorktreeInfo { ... }` literal, add below `owner_email,`:

```rust
            owner,
```

Do this at all three sites.

- [ ] **Step 6: Similarly plumb through `collect_branch_info`**

`collect_branch_info` is the second function with `get_author_email_for_ref`
calls (lines ~984 and ~1076). Apply the same treatment:

- Add parameters
  `ownership_strategy: OwnershipStrategy, user_email: Option<&str>` to its
  signature.
- Compute `owner` alongside `owner_email`.
- Include `owner` in the struct literal.

- [ ] **Step 7: Update `prune.rs` local-branch-stub population**

In `src/commands/prune.rs` at ~line 283:

```rust
// Before
let owner_email = list::get_author_email_for_ref(branch, &cwd);
stubs.push(list::WorktreeInfo::local_branch_stub(branch, owner_email));

// After
let commits = crate::core::ownership::fetch_commit_records(&default_branch, branch, &cwd);
let owner = crate::core::ownership::resolve_owner_from_records(
    &commits,
    settings.ownership_strategy,
    git.config_get("user.email").ok().flatten().as_deref(),
);
let owner_email = owner.as_ref().map(|o| o.email.clone());
let mut stub = list::WorktreeInfo::local_branch_stub(branch, owner_email);
stub.owner = owner;
stubs.push(stub);
```

(`default_branch` and `git` are already in scope at this call site in prune.rs;
if not, resolve them locally from `settings` and `GitCommand::new(true)`.)

- [ ] **Step 8: Build**

Run: `cargo build -p daft` Expected: compile succeeds (both fields populated).

- [ ] **Step 9: Run tests**

Run: `cargo test -p daft --lib` Expected: all existing tests pass. Sort tests
using `owner_email` still work.

- [ ] **Step 10: Commit**

```bash
git add src/core/worktree/list.rs src/commands/prune.rs
git commit -m "feat(ownership): populate BranchOwner on every WorktreeInfo"
```

---

## Task 6: Update CLI column rendering to use author name

**Files:**

- Modify: `src/output/format.rs`

- [ ] **Step 1: Switch the Owner column source**

In `src/output/format.rs` at ~line 298, replace:

```rust
    let owner = info.owner_email.clone().unwrap_or_default();
```

With:

```rust
    let owner = info
        .owner
        .as_ref()
        .map(|o| o.name.clone())
        .unwrap_or_default();
```

- [ ] **Step 2: Build and run existing format tests**

Run: `cargo test -p daft --lib output::format` Expected: pass (no format-level
tests assert on the specific rendering; any that do will be updated along with
the YAML scenarios in Task 12).

- [ ] **Step 3: Commit**

```bash
git add src/output/format.rs
git commit -m "refactor(list): render Owner column with author name"
```

---

## Task 7: Update JSON output shape

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Replace the JSON owner emit**

At `src/commands/list.rs:398`:

```rust
// Before
obj.insert("owner".into(), serde_json::json!(info.owner_email));

// After
let owner_json = info.owner.as_ref().map_or(serde_json::Value::Null, |o| {
    serde_json::json!({
        "name": o.name,
        "email": o.email,
    })
});
obj.insert("owner".into(), owner_json);
```

- [ ] **Step 2: Run**

Run: `cargo build -p daft` Expected: compile succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/commands/list.rs
git commit -m "feat(list): JSON owner becomes {name, email} object"
```

---

## Task 8: Update sort ordering by owner

**Files:**

- Modify: `src/core/sort.rs`

- [ ] **Step 1: Switch the `SortColumn::Owner` comparison**

At `src/core/sort.rs:142-146`, replace:

```rust
            SortColumn::Owner => {
                let a_owner = a.owner_email.as_deref().map(|s| s.to_lowercase());
                let b_owner = b.owner_email.as_deref().map(|s| s.to_lowercase());
                self.compare_optional(a_owner.as_deref(), b_owner.as_deref())
            }
```

With:

```rust
            SortColumn::Owner => {
                // Sort by owner name (case-insensitive), ties broken by email.
                let key = |w: &WorktreeInfo| -> Option<(String, String)> {
                    w.owner.as_ref().map(|o| (o.name.to_lowercase(), o.email.to_lowercase()))
                };
                let ord = match (key(a), key(b)) {
                    (Some(ak), Some(bk)) => ak.cmp(&bk),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                self.apply_direction(ord)
            }
```

(Use whichever of `compare_optional` / `apply_direction` matches the existing
pattern for tuple-keyed comparisons in this file. If `compare_optional` already
handles `Option<T: Ord>` uniformly, you can use it with the tuple.)

- [ ] **Step 2: Update the `info_with_owner` helper**

At ~line 456 in the test module, replace:

```rust
    fn info_with_owner(name: &str, owner: Option<&str>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.owner_email = owner.map(|s| s.to_string());
        i
    }
```

With:

```rust
    fn info_with_owner(name: &str, owner: Option<&str>) -> WorktreeInfo {
        let mut i = WorktreeInfo::empty(name);
        i.owner = owner.map(|email| crate::core::ownership::BranchOwner {
            name: email.split('@').next().unwrap_or(email).to_string(),
            email: email.to_string(),
            is_current_user: false,
        });
        i
    }
```

- [ ] **Step 3: Update the assertion at line ~813**

Find the assertion `.map(|i| i.owner_email.as_deref().unwrap())` and change to:

```rust
        .map(|i| i.owner.as_ref().unwrap().email.as_str())
```

(The test checks sort order; whether we check email or name is semantic. Email
is what the test helper sets distinctly, so keep the email assertion.)

- [ ] **Step 4: Run sort tests**

Run: `cargo test -p daft --lib core::sort` Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/sort.rs
git commit -m "refactor(sort): owner sort keys on name then email"
```

---

## Task 9: Audit TUI rendering

**Files:**

- Inspect: `src/output/tui/columns.rs`, `src/output/tui/render.rs`,
  `src/output/tui/state.rs`, `src/output/tui/driver.rs`,
  `src/output/tui/operation_table.rs`

- [ ] **Step 1: Grep for leftover `owner_email` usages**

Run:

```bash
rg "owner_email" src/output/tui/
```

Expected: the only references should be in code that feeds `ColumnValues` (which
came from `src/output/format.rs` — already updated in Task 6). Any direct
`info.owner_email` reference in the TUI layer needs conversion to
`info.owner.as_ref().map(|o| o.name.as_str()).unwrap_or_default()`.

- [ ] **Step 2: Grep for leftover divider/partition logic keyed off
      `owner_email`**

Run:

```bash
rg "owner_email" src/output/tui/ src/commands/
```

For each hit that compares `owner_email == user_email` as part of a
partition/divider decision, replace with
`info.owner.as_ref().is_some_and(|o| o.is_current_user)`. Note the real cleanup
happens in Task 10; this step only addresses TUI-layer display usages that are
NOT in `sync.rs`.

- [ ] **Step 3: Build + run unit tests**

Run: `cargo test -p daft --lib` Expected: pass.

- [ ] **Step 4: Commit (only if any TUI files changed)**

```bash
git add src/output/tui/
git commit -m "refactor(tui): read owner via BranchOwner instead of owner_email"
```

If `rg` found nothing to change, skip this commit.

---

## Task 10: Simplify `sync.rs` — drop `user_email` plumbing

**Files:**

- Modify: `src/commands/sync.rs`

- [ ] **Step 1: Change `is_branch_included` signature**

In `src/commands/sync.rs:64-93`, replace the function with:

```rust
/// Check if a branch is included by the filters or by ownership.
fn is_branch_included(
    branch: &str,
    owner: Option<&crate::core::ownership::BranchOwner>,
    filters: &[IncludeFilter],
) -> bool {
    if owner.is_some_and(|o| o.is_current_user) {
        return true;
    }
    for filter in filters {
        match filter {
            IncludeFilter::Unowned => return true,
            IncludeFilter::Email(email) => {
                if owner.is_some_and(|o| o.email.eq_ignore_ascii_case(email)) {
                    return true;
                }
            }
            IncludeFilter::Branch(name) => {
                if branch == name {
                    return true;
                }
            }
        }
    }
    false
}
```

- [ ] **Step 2: Update the three call sites in `sync.rs`**

Use Grep to locate them:

```bash
rg "is_branch_included\(" src/commands/sync.rs
```

There are three call sites — approximately lines 275, 293, 530, 674 (grep for
the exact set). Each currently passes
`owner.as_deref(), user_email.as_deref(), &include_filters`.

Replace each with the info's owner:

```rust
is_branch_included(branch, info.owner.as_ref(), &include_filters)
```

Where `info` is the relevant `WorktreeInfo` in scope. Two of the call sites
iterate over `worktrees` tuples `(path, branch)` without a full `WorktreeInfo` —
for those, use the `shared_owner_lookup: HashMap<String, Option<BranchOwner>>`
(see Step 4 below) to look up the owner by branch name.

- [ ] **Step 3: Drop the `user_email` lookups that no longer feed anything**

Search for `config_get("user.email")` in `sync.rs`:

```bash
rg 'config_get\("user\.email"\)' src/commands/sync.rs
```

Three lookups exist (approximately lines 264, 505, 637). The divider-gating one
at line ~529 (`user_email.as_ref().and_then(|_|`) is still needed as "skip the
divider entirely when user.email isn't configured" — keep it. The other two
become dead once call sites are updated; delete them.

- [ ] **Step 4: Update `shared_owner_lookup` type**

Currently:

```rust
let shared_owner_lookup: Arc<HashMap<String, Option<String>>> = Arc::new(
    worktree_infos
        .iter()
        .map(|info| (info.name.clone(), info.owner_email.clone()))
        .collect(),
);
```

Change to:

```rust
let shared_owner_lookup: Arc<HashMap<String, Option<BranchOwner>>> = Arc::new(
    worktree_infos
        .iter()
        .map(|info| (info.name.clone(), info.owner.clone()))
        .collect(),
);
```

Update every reader of this map — the partition in the orchestrator (~line 672)
currently does:

```rust
is_branch_included(
    branch,
    shared_owner_lookup.get(branch).and_then(|e| e.as_deref()),
    shared_user_email.as_deref(),
    &include_filters,
)
```

Change to:

```rust
is_branch_included(
    branch,
    shared_owner_lookup.get(branch).and_then(|o| o.as_ref()),
    &include_filters,
)
```

Delete `shared_user_email`.

- [ ] **Step 5: Delete or simplify the `included_branches` block for rebase/push
      gating**

The block at ~lines 267-306 currently re-derives ownership via
`list::get_author_email_for_ref`. Replace it with lookups into the already-
populated `shared_owner_lookup` from Step 4, or if the block runs before
`worktree_infos` are collected in its caller, build a local owner map using
`ownership::resolve_owner_from_records` / `fetch_commit_records` per the same
pattern.

Reference-implementation hint: this block runs AFTER `run_update_phase` but
BEFORE the TUI setup that builds `worktree_infos`. You have two choices:

1. Compute `owner` per branch inline here (one `fetch_commit_records` +
   `resolve_owner_from_records` call each). Same cost as the current
   `get_author_email_for_ref` call.
2. Hoist the `worktree_infos` collection earlier.

Choose (1) to keep the diff localized. After the change, the block looks like:

```rust
let include_filters: Vec<IncludeFilter> = args
    .include
    .iter()
    .map(|v| IncludeFilter::parse(v))
    .collect();

let git_for_email = GitCommand::new(true).with_gitoxide(settings.use_gitoxide);
let user_email: Option<String> = git_for_email.config_get("user.email").ok().flatten();

let included_branches: Option<HashSet<String>> = if user_email.is_some() || !include_filters.is_empty() {
    let worktrees = fetch::get_all_worktrees_with_branches(&git_for_email).unwrap_or_default();
    let project_root = get_project_root()?;
    let mut set = HashSet::new();
    for (path, branch) in &worktrees {
        let commits = crate::core::ownership::fetch_commit_records(&default_branch, branch, path);
        let owner = crate::core::ownership::resolve_owner_from_records(
            &commits,
            settings.ownership_strategy,
            user_email.as_deref(),
        );
        if is_branch_included(branch, owner.as_ref(), &include_filters) {
            set.insert(branch.clone());
        }
    }
    if let Ok(ref_output) = git_for_email.for_each_ref("%(refname:short)", "refs/heads") {
        let worktree_set: HashSet<&str> = worktrees.iter().map(|(_, b)| b.as_str()).collect();
        for branch in ref_output.lines() {
            let branch = branch.trim();
            if branch.is_empty() || worktree_set.contains(branch) {
                continue;
            }
            let commits = crate::core::ownership::fetch_commit_records(&default_branch, branch, &project_root);
            let owner = crate::core::ownership::resolve_owner_from_records(
                &commits,
                settings.ownership_strategy,
                user_email.as_deref(),
            );
            if is_branch_included(branch, owner.as_ref(), &include_filters) {
                set.insert(branch.to_string());
            }
        }
    }
    Some(set)
} else {
    None
};
```

- [ ] **Step 6: Build**

Run: `cargo build -p daft` Expected: compile succeeds.

- [ ] **Step 7: Run unit tests**

Run: `cargo test -p daft --lib` Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add src/commands/sync.rs
git commit -m "refactor(sync): is_branch_included takes BranchOwner, drop user_email plumbing"
```

---

## Task 11: Remove the obsolete `owner_email` field and `get_author_email_for_ref`

**Files:**

- Modify: `src/core/worktree/list.rs`

- [ ] **Step 1: Confirm no callers of `get_author_email_for_ref` remain**

Run:

```bash
rg "get_author_email_for_ref" src/
```

Expected: only the definition itself at `src/core/worktree/list.rs:439`. If any
callers remain, stop and port them — they should have been removed in Tasks 5
and 10.

- [ ] **Step 2: Delete the function**

Delete `fn get_author_email_for_ref(...) -> Option<String>` at
`src/core/worktree/list.rs:439-456`.

- [ ] **Step 3: Delete the `owner_email` field**

In `pub struct WorktreeInfo`, delete:

```rust
    pub owner_email: Option<String>,
```

And its initialization lines in `empty()`, `local_branch_stub()`, and the three
struct literals in `collect_worktree_info` / `collect_branch_info`.

- [ ] **Step 4: Update `local_branch_stub` signature**

Change from:

```rust
pub fn local_branch_stub(name: &str, owner_email: Option<String>) -> Self {
```

To:

```rust
pub fn local_branch_stub(name: &str, owner: Option<BranchOwner>) -> Self {
```

Update the caller in `prune.rs` (Task 5 Step 7) to pass `owner` directly —
remove the intermediate `owner_email` variable:

```rust
let mut stub = list::WorktreeInfo::local_branch_stub(branch, owner);
stubs.push(stub);
```

- [ ] **Step 5: Build**

Run: `cargo build -p daft` Expected: compile succeeds. If any `owner_email`
references remain in error output, delete or convert them.

- [ ] **Step 6: Run full unit test suite**

Run: `cargo test -p daft --lib` Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/list.rs src/commands/prune.rs
git commit -m "refactor(list): remove legacy owner_email field"
```

---

## Task 12: Update + add YAML integration scenarios

**Files:**

- Modify: `tests/manual/scenarios/list/owner-column.yml`
- Modify: `tests/manual/scenarios/sync/ownership-rebase-push.yml`
- Create: `tests/manual/scenarios/list/owner-strategy-tip.yml`
- Create: `tests/manual/scenarios/list/owner-strategy-plurality.yml`
- Create: `tests/manual/scenarios/list/owner-strategy-recency.yml`
- Create: `tests/manual/scenarios/sync/ownership-strategy-recency.yml`

- [ ] **Step 1: Update `list/owner-column.yml` to expect author name**

Replace the final assertion block with:

```yaml
- name: Set user.name so the Owner column shows it
  run: git config user.name "Me Testuser"
  cwd: "$WORK_DIR/test-repo/main"
  expect:
    exit_code: 0

- name: List with owner column shows author name
  run: NO_COLOR=1 git-worktree-list --columns branch,owner 2>&1
  cwd: "$WORK_DIR/test-repo/develop"
  expect:
    exit_code: 0
    output_contains:
      - "Owner"
      - "Me Testuser"
    output_not_contains:
      - "me@example.com"
```

Also update the "make a commit" step to set `GIT_AUTHOR_NAME` /
`GIT_COMMITTER_NAME` to `"Me Testuser"` so the commit carries a stable display
name.

- [ ] **Step 2: Update `sync/ownership-rebase-push.yml` to use the new default
      strategy**

The existing test asserts that a branch with a teammate's tip commit is NOT
rebased. Under the new default `recency-plurality` that only holds if you have
fewer weighted-commits than the teammate. Since the test has exactly one commit
authored by the teammate in range (no commits by "me@example.com"), the
recency-plurality result still assigns ownership to the teammate — **the test
keeps passing unchanged in semantics**.

Still, explicitly pin the strategy for determinism:

```yaml
- name: Pin ownership strategy to tip for this legacy assertion
  run: git config daft.ownership.strategy tip
  cwd: "$WORK_DIR/ownership-repo/main"
  expect:
    exit_code: 0
```

Insert after "Set local user email to current user". Add the same config to each
worktree that runs `daft` commands.

- [ ] **Step 3: Create `list/owner-strategy-tip.yml`**

```yaml
name: Owner strategy tip
description:
  With strategy=tip, the branch owner is the author of the most recent commit in
  base..branch.

repos:
  - name: strat-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_STRAT_REPO
    expect:
      exit_code: 0

  - name: Configure user and strategy
    run: |
      cd $WORK_DIR/strat-repo/main
      git config user.email "me@example.com"
      git config user.name "Me Testuser"
      git config daft.ownership.strategy tip
    expect:
      exit_code: 0

  - name: Checkout feature and stack commits; teammate writes the tip
    run: |
      git-worktree-checkout feature/strat
      cd $WORK_DIR/strat-repo/feature/strat
      for i in 1 2 3; do
        echo "me $i" > me-$i.txt
        git add me-$i.txt
        GIT_AUTHOR_NAME="Me Testuser" GIT_AUTHOR_EMAIL="me@example.com" \
        GIT_COMMITTER_NAME="Me Testuser" GIT_COMMITTER_EMAIL="me@example.com" \
          git commit -m "me $i"
      done
      echo "bob tip" > bob.txt
      git add bob.txt
      GIT_AUTHOR_NAME="Bob" GIT_AUTHOR_EMAIL="bob@example.com" \
      GIT_COMMITTER_NAME="Bob" GIT_COMMITTER_EMAIL="bob@example.com" \
        git commit -m "bob tip"
    cwd: "$WORK_DIR/strat-repo/main"
    expect:
      exit_code: 0

  - name: Owner column reports the tip author (Bob)
    run: NO_COLOR=1 git-worktree-list --columns branch,owner 2>&1
    cwd: "$WORK_DIR/strat-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "feature/strat"
        - "Bob"
      output_not_contains:
        - "Me Testuser"
```

- [ ] **Step 4: Create `list/owner-strategy-plurality.yml`**

```yaml
name: Owner strategy plurality
description:
  With strategy=plurality, the branch owner is the author with the most commits
  (me = 3 vs bob = 1 → me wins).

repos:
  - name: strat-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_STRAT_REPO
    expect:
      exit_code: 0

  - name: Configure user and strategy
    run: |
      cd $WORK_DIR/strat-repo/main
      git config user.email "me@example.com"
      git config user.name "Me Testuser"
      git config daft.ownership.strategy plurality
    expect:
      exit_code: 0

  - name: Checkout feature and stack commits; teammate writes the tip
    run: |
      git-worktree-checkout feature/strat
      cd $WORK_DIR/strat-repo/feature/strat
      for i in 1 2 3; do
        echo "me $i" > me-$i.txt
        git add me-$i.txt
        GIT_AUTHOR_NAME="Me Testuser" GIT_AUTHOR_EMAIL="me@example.com" \
        GIT_COMMITTER_NAME="Me Testuser" GIT_COMMITTER_EMAIL="me@example.com" \
          git commit -m "me $i"
      done
      echo "bob tip" > bob.txt
      git add bob.txt
      GIT_AUTHOR_NAME="Bob" GIT_AUTHOR_EMAIL="bob@example.com" \
      GIT_COMMITTER_NAME="Bob" GIT_COMMITTER_EMAIL="bob@example.com" \
        git commit -m "bob tip"
    cwd: "$WORK_DIR/strat-repo/main"
    expect:
      exit_code: 0

  - name: Owner column reports the plurality author (Me Testuser)
    run: NO_COLOR=1 git-worktree-list --columns branch,owner 2>&1
    cwd: "$WORK_DIR/strat-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "feature/strat"
        - "Me Testuser"
      output_not_contains:
        - "Bob"
```

- [ ] **Step 5: Create `list/owner-strategy-recency.yml`**

Same commit layout as Step 3, `recency-plurality` strategy. Expected winner
(ranks reverse-chrono, k=0 is tip):

- Bob (1 commit at rank 0): `1/1 = 1.0`
- Me (3 commits at ranks 1, 2, 3): `1/2 + 1/3 + 1/4 ≈ 1.083`

Me wins. This scenario validates the default — do NOT set the config key.

```yaml
name: Owner strategy recency-plurality (default)
description:
  Default strategy deduces owner via recency-weighted plurality; user wins
  despite teammate's tip commit.

repos:
  - name: strat-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_STRAT_REPO
    expect:
      exit_code: 0

  - name: Configure user identity (strategy unset — uses default)
    run: |
      cd $WORK_DIR/strat-repo/main
      git config user.email "me@example.com"
      git config user.name "Me Testuser"
    expect:
      exit_code: 0

  - name: Checkout feature and stack commits; teammate writes the tip
    run: |
      git-worktree-checkout feature/strat
      cd $WORK_DIR/strat-repo/feature/strat
      for i in 1 2 3; do
        echo "me $i" > me-$i.txt
        git add me-$i.txt
        GIT_AUTHOR_NAME="Me Testuser" GIT_AUTHOR_EMAIL="me@example.com" \
        GIT_COMMITTER_NAME="Me Testuser" GIT_COMMITTER_EMAIL="me@example.com" \
          git commit -m "me $i"
      done
      echo "bob tip" > bob.txt
      git add bob.txt
      GIT_AUTHOR_NAME="Bob" GIT_AUTHOR_EMAIL="bob@example.com" \
      GIT_COMMITTER_NAME="Bob" GIT_COMMITTER_EMAIL="bob@example.com" \
        git commit -m "bob tip"
    cwd: "$WORK_DIR/strat-repo/main"
    expect:
      exit_code: 0

  - name:
      Owner column reports the recency-weighted plurality author (Me Testuser)
    run: NO_COLOR=1 git-worktree-list --columns branch,owner 2>&1
    cwd: "$WORK_DIR/strat-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "feature/strat"
        - "Me Testuser"
      output_not_contains:
        - "Bob"
```

- [ ] **Step 6: Create `sync/ownership-strategy-recency.yml`**

This is the **regression test** for the user's original complaint. A branch
where the teammate wrote the tip but the user wrote most of the commits should
land in the owned-branch partition under the new default.

Set up a branch where `user.email` authored 3 commits and a teammate authored 1
tip commit. Run `daft sync --rebase main --push` (with a remote advance on
`main`). Assert the branch WAS rebased.

```yaml
name: Sync ownership strategy recency
description:
  "With the default recency-plurality strategy, a branch where the user authored
  most commits is still rebased even if a teammate wrote the tip."

repos:
  - name: recency-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_RECENCY_REPO
    expect:
      exit_code: 0

  - name: Configure user identity
    run: |
      cd $WORK_DIR/recency-repo/main
      git config user.email "me@example.com"
      git config user.name "Me Testuser"
    expect:
      exit_code: 0

  - name: Build feature branch; me writes 3, teammate writes the tip
    run: |
      git-worktree-checkout feature/recency
      cd $WORK_DIR/recency-repo/feature/recency
      for i in 1 2 3; do
        echo "me $i" > me-$i.txt
        git add me-$i.txt
        GIT_AUTHOR_NAME="Me Testuser" GIT_AUTHOR_EMAIL="me@example.com" \
        GIT_COMMITTER_NAME="Me Testuser" GIT_COMMITTER_EMAIL="me@example.com" \
          git commit -m "me $i"
      done
      echo "bob tip" > bob.txt
      git add bob.txt
      GIT_AUTHOR_NAME="Bob" GIT_AUTHOR_EMAIL="bob@example.com" \
      GIT_COMMITTER_NAME="Bob" GIT_COMMITTER_EMAIL="bob@example.com" \
        git commit -m "bob tip"
    cwd: "$WORK_DIR/recency-repo/main"
    expect:
      exit_code: 0

  - name: Advance main on remote
    run: |
      temp=$(mktemp -d)
      git clone "$REMOTE_RECENCY_REPO" "$temp" 2>/dev/null
      cd "$temp"
      echo "remote change" > r.txt
      git add r.txt
      git commit -m "advance main"
      git push origin main 2>/dev/null
      rm -rf "$temp"
    expect:
      exit_code: 0

  - name: Record feature hash before sync
    run: git rev-parse HEAD > /tmp/daft-recency-before
    cwd: "$WORK_DIR/recency-repo/feature/recency"
    expect:
      exit_code: 0

  - name: Sync with --rebase main
    run: git-worktree-sync --rebase main --verbose 2>&1 || true
    cwd: "$WORK_DIR/recency-repo/main"
    expect:
      exit_code: 0

  - name: Feature branch WAS rebased (hash changed)
    run: |
      before=$(cat /tmp/daft-recency-before)
      after=$(git rev-parse HEAD)
      echo "before=$before after=$after"
      [ "$before" != "$after" ]
    cwd: "$WORK_DIR/recency-repo/feature/recency"
    expect:
      exit_code: 0
```

- [ ] **Step 7: Run the updated + new scenarios**

```bash
mise run test:manual -- --ci list owner-column
mise run test:manual -- --ci list owner-strategy-tip
mise run test:manual -- --ci list owner-strategy-plurality
mise run test:manual -- --ci list owner-strategy-recency
mise run test:manual -- --ci sync ownership-rebase-push
mise run test:manual -- --ci sync ownership-strategy-recency
```

Expected: all pass.

- [ ] **Step 8: Commit**

```bash
git add tests/manual/scenarios/list/ tests/manual/scenarios/sync/
git commit -m "test(ownership): scenarios per strategy + recency regression"
```

---

## Task 13: Update documentation + regenerate man pages

**Files:**

- Modify: `docs/cli/daft-list.md`
- Modify: `docs/cli/daft-sync.md`
- Modify: `docs/cli/daft-prune.md` (if owner is mentioned)
- Modify: `docs/cli/git-worktree-list.md`
- Modify: `docs/guide/configuration.md`
- Modify: `SKILL.md`
- Regenerate: `man/*.1`

- [ ] **Step 1: Fix `daft-list.md` owner description (line 32)**

Replace:

```
- Owner: tip commit author email (available via `--columns owner`)
```

With:

```
- Owner: author name of the branch's commit range owner, per
  `daft.ownership.strategy` (available via `--columns owner`)
```

- [ ] **Step 2: Fix `daft-list.md` JSON field description (line 48)**

Replace:

```
`remote_ahead`, `remote_behind`, `branch_age`, and `owner_email`.
```

With:

```
`remote_ahead`, `remote_behind`, `branch_age`, and `owner` (an object
`{name, email}` or `null`).
```

- [ ] **Step 3: Fix `daft-sync.md` owner description (lines 40-51)**

Replace the `### Ownership-gated rebase and push` section so the paragraph
reads:

```
When `--rebase` or `--push` is specified, daft applies these operations
only to branches you own. Ownership is deduced from the branch's commit
range (`base..branch`) per the strategy set in
`daft.ownership.strategy` (default: `recency-plurality`). A branch is
"yours" when the resolved owner's email matches your `git config
user.email`.

The summary table's **Owner** column shows the author *name* of the
resolved owner.
```

- [ ] **Step 4: Fix `git-worktree-list.md` Owner column description**

Find any mention of "author email" in the owner column context and change to
"author name per `daft.ownership.strategy`".

- [ ] **Step 5: Add the config key to `docs/guide/configuration.md`**

Find the appropriate section (likely `## List Settings` or a new
`## Ownership Settings` section) and add:

```markdown
## Ownership Settings

| Key                       | Default             | Description                                                                                                                                                     |
| ------------------------- | ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `daft.ownership.strategy` | `recency-plurality` | Strategy for deducing branch ownership from the `base..branch` commit range. Valid values: `tip`, `any`, `first`, `plurality`, `majority`, `recency-plurality`. |
```

Add a short paragraph above the table explaining what each strategy does (port
the table from the spec's §2).

- [ ] **Step 6: Update `SKILL.md`**

Find any existing mention of ownership or add a short note under the
list-command or sync-command section:

```markdown
Branch ownership in `list` / `sync` / `prune` is deduced from the `base..branch`
commit range using a user-configurable strategy (git config
`daft.ownership.strategy`, default `recency-plurality`). The Owner column shows
the author name of the resolved owner.
```

- [ ] **Step 7: Regenerate man pages**

Run: `mise run man:gen` Expected: man pages updated in place.

- [ ] **Step 8: Verify man pages are consistent**

Run: `mise run man:verify` Expected: exits 0.

- [ ] **Step 9: Run docs site build to catch any broken links**

Run: `mise run docs:site:build` Expected: exits 0.

- [ ] **Step 10: Commit**

```bash
git add docs/ SKILL.md man/
git commit -m "docs: document daft.ownership.strategy and owner column"
```

---

## Task 14: Final verification

**Files:**

- None (verification only)

- [ ] **Step 1: Format**

Run: `mise run fmt` Expected: no changes needed, or auto-formats in place.

- [ ] **Step 2: Format check**

Run: `mise run fmt:check` Expected: exits 0.

- [ ] **Step 3: Clippy**

Run: `mise run clippy` Expected: zero warnings.

- [ ] **Step 4: Unit tests**

Run: `mise run test:unit` Expected: all pass, including the new
`core::ownership` suite.

- [ ] **Step 5: Full integration suite**

Run: `mise run test:integration` Expected: all pass. Pay attention to `list`,
`sync`, `prune` scenarios — if any unrelated scenario asserts on a specific
email in the Owner column, port it to expect the author name.

- [ ] **Step 6: Simulate CI locally**

Run: `mise run ci` Expected: exits 0.

- [ ] **Step 7: Sanity-test by hand in a throwaway repo**

Per CLAUDE.md: never use this repo for testing. Create a temp repo, populate
with commits from two authors, and run:

```bash
cd /tmp && mktemp -d
# build the repo with `git init`, set local user.email/name, two authors
# then:
daft list --columns branch,owner
git config daft.ownership.strategy plurality
daft list --columns branch,owner
git config daft.ownership.strategy tip
daft list --columns branch,owner
```

Confirm the Owner column shifts between strategies as expected.

- [ ] **Step 8: Commit any fixup from verification**

If any step required code tweaks (e.g. clippy fixes), commit them:

```bash
git commit -am "chore(ownership): address clippy / test fallout"
```

Otherwise the branch is ready for review.

---

## Self-Review Summary

Spec coverage audit:

| Spec section                                          | Task(s)                        |
| ----------------------------------------------------- | ------------------------------ |
| §1 Window `base..branch`                              | Task 2, Task 3                 |
| §2 Six aggregation strategies                         | Task 2                         |
| §3 `daft.ownership.strategy` config + default         | Task 4                         |
| §4 Data model: `BranchOwner`, drop `user_email` plumb | Task 2, Task 5, Task 10        |
| §5 Display: author name in CLI, TUI, JSON             | Task 6, Task 7, Task 9         |
| §5 Sort on name                                       | Task 8                         |
| §6 Owned-partition in sync (divider unchanged)        | Task 10                        |
| §7 Performance (`git log` per branch)                 | Task 3 (same shape as current) |
| §8 Unconfigured `user.email` preserved                | Task 2 (`None` user_email)     |
| Migration: JSON breaking change                       | Task 7 + Task 13 docs          |
| Testing: unit per strategy                            | Task 2                         |
| Testing: YAML integration per strategy + regression   | Task 12                        |
| Docs: cli pages, guide, SKILL.md, man                 | Task 13                        |

No open questions — all items from the spec have a task. Default is
`recency-plurality`; all six strategies implemented; docs include the
per-strategy explanation table; regression scenario for the user's original
complaint is Task 12 Step 6.
