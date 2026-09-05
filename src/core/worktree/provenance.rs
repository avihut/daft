//! Branch provenance — what daft knows about where a branch has been.
//!
//! Prune's safety turns on a question git answers nowhere durably: *was this
//! branch ever published to this remote?* An upstream config exists only if
//! someone passed `-u`; `refs/remotes/<remote>/<branch>` is a cache of the
//! remote's *current* state, and `git fetch --prune` deletes it together with
//! its reflog — the very operation that would prove the branch used to be
//! there destroys the evidence that it was. Absence therefore looks identical
//! on a branch created thirty seconds ago and on one deleted upstream last
//! week, which is how #858 reaped fresh worktrees.
//!
//! So daft records the fact at the two moments it knows it, in git config
//! under the branch's own section:
//!
//! | key                        | written at   | meaning                                    |
//! |----------------------------|--------------|--------------------------------------------|
//! | `branch.<n>.daftOrigin`    | `daft start` | `local` — created here, never published    |
//! | `branch.<n>.daftPublished` | the push seam| `"<remote> <rfc3339>"` — daft pushed it    |
//!
//! `daftPublished` is the only record that can *authorize* a deletion, so it
//! is written through one function ([`mark_published`]), called at daft's push
//! seams (the single-branch path and the batched one) for a push git itself
//! reported, and by prune's backfill for a branch whose *config* says it
//! tracks `<remote>/<same name>` and whose remote-tracking ref is present.
//! It carries the remote it attests to — a stamp for `origin` must not
//! authorize pruning against a different `daft.remote`. `daftOrigin` can only
//! withhold or annotate.
//!
//! The backfill is what keeps this honest in a repo daft does not exclusively
//! own: a teammate's `git push -u` is a fact daft can see, and a record of
//! daft's own pushes alone would treat their branches as unknown forever.
//!
//! Git maintains the lifecycle for free: `git branch -m` renames the whole
//! `branch.<name>` section and `git branch -d`/`-D` removes it, and daft's own
//! rename and delete paths go through both ([`crate::git`] `branch.rs`). A
//! branch deleted and recreated under the same name therefore starts clean.
//!
//! Absence of a record means *unknown*, and unknown must resolve to the
//! non-destructive answer — see ARCHITECTURE.md "Bridge git's evidence gaps
//! with daft's own records".

use crate::git::GitCommand;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Written by `daft start`; the value is always [`ORIGIN_LOCAL`].
const ORIGIN_KEY: &str = "daftOrigin";
/// Written by the push seam; the value is `"<remote> <rfc3339>"`.
const PUBLISHED_KEY: &str = "daftPublished";
/// The only [`ORIGIN_KEY`] value daft writes.
const ORIGIN_LOCAL: &str = "local";
/// Written at daft's rename seam; the value is always [`DEFAULT_TRUE`].
const DEFAULT_KEY: &str = "daftDefault";
/// The only [`DEFAULT_KEY`] value daft writes.
const DEFAULT_TRUE: &str = "true";

/// Git reports config variable names lowercased regardless of how they were
/// written, so the parser matches these rather than the camelCase spellings.
const ORIGIN_KEY_REPORTED: &str = ".daftorigin";
const PUBLISHED_KEY_REPORTED: &str = ".daftpublished";
const DEFAULT_KEY_REPORTED: &str = ".daftdefault";

/// Record that daft created `branch` here and has not published it.
///
/// Informational: it sharpens prune's message and answers "did daft make
/// this?", but nothing destructive keys off it. Failing to write it costs a
/// better diagnostic, never safety.
pub fn mark_local(git: &GitCommand, cwd: &Path, branch: &str) -> Result<()> {
    git.config_set_at(&format!("branch.{branch}.{ORIGIN_KEY}"), ORIGIN_LOCAL, cwd)
}

/// Record that daft pushed `branch` to `remote`, just now.
///
/// The only writer of the one record that can authorize a deletion. Call it
/// where git has confirmed the push succeeded — its own report, not a bare
/// exit status — or where daft has directly observed the branch on the remote,
/// and never for a delete push, which proves the branch is *gone* from the
/// remote: the opposite attestation.
pub fn mark_published(git: &GitCommand, cwd: &Path, remote: &str, branch: &str) -> Result<()> {
    let stamp = format!("{remote} {}", chrono::Utc::now().to_rfc3339());
    git.config_set_at(&format!("branch.{branch}.{PUBLISHED_KEY}"), &stamp, cwd)
}

/// Record that `branch` is this repository's default branch, as daft
/// determined at the moment it renamed the previous one.
///
/// This fact has nowhere else durable to live. `<remote>/HEAD` names the
/// *remote's* default, and a default-branch rename can never move it: a remote
/// refuses to delete the branch its own HEAD names (`receive.denyDeleteCurrent`,
/// bare repositories included), so daft's rename leaves the old branch standing
/// on the remote and the symref rightly keeps pointing at it. The catalog is
/// then re-derived from that symref at every registration, overwriting the new
/// name with the old one — and `daft go` reopens a branch the user renamed
/// away (#933).
///
/// "The record is stale" and "the derivation is stale" are locally
/// indistinguishable, which is the case ARCHITECTURE.md says must be written
/// down rather than inferred.
///
/// Written only where daft has just observed the repository's own `HEAD`
/// naming the branch it is renaming — an observation, never a guess about
/// which branch is default. Exactly one branch carries the marker: any other's
/// is cleared first, so a repo renamed twice cannot hold two answers.
pub fn mark_default(git: &GitCommand, cwd: &Path, branch: &str) -> Result<()> {
    for stale in recorded_default_branches(cwd) {
        if stale != branch {
            let _ = git.config_unset_at(&format!("branch.{stale}.{DEFAULT_KEY}"), cwd);
        }
    }
    git.config_set_at(&format!("branch.{branch}.{DEFAULT_KEY}"), DEFAULT_TRUE, cwd)
}

/// The branch daft recorded as this repository's default.
///
/// `None` — unknown, so the ladder's remaining rungs answer — when nothing is
/// marked, when more than one branch is, or when the marker is not corroborated
/// by the repository's own `HEAD`.
///
/// That last condition is what makes the marker trustworthy. Its job is to
/// outrank `<remote>/HEAD`, never to outrank the repository itself, so it is
/// read as *permission to believe this repo's own declaration over the
/// remote's* rather than as a standalone claim. Both records are maintained by
/// git across `git branch -m` — it moves `HEAD` and renames the whole
/// `branch.<name>` section — so a genuine rename keeps them agreeing.
///
/// It also closes the one way a false claimant appears: `git branch -c` copies
/// the entire `branch.<name>` section, marker included, so copying the default
/// branch would otherwise leave a second branch quietly claiming to be it.
/// The copy is not what `HEAD` names, so it does not corroborate. Verified
/// against real git, along with the rename and delete lifecycle.
pub fn recorded_default_branch(dir: &Path) -> Option<String> {
    let mut marked = recorded_default_branches(dir);
    let branch = match marked.len() {
        1 => marked.pop()?,
        _ => return None,
    };
    (crate::core::remote::local_head_branch(dir).as_deref() == Some(branch.as_str()))
        .then_some(branch)
}

/// Branch names carrying the default marker.
///
/// The name is recovered by stripping the known suffix rather than splitting
/// on dots, for the same reason [`Provenance::parse`] does it that way: a
/// branch called `release.2.x` would otherwise be filed under `release`.
fn recorded_default_branches(dir: &Path) -> Vec<String> {
    let Ok(out) = crate::utils::git_command_at(dir)
        .args([
            "config",
            "--local",
            "--get-regexp",
            r"^branch\..*\.daftdefault$",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (key, value) = line.trim_end().split_once(' ')?;
            (value.trim() == DEFAULT_TRUE).then_some(key)
        })
        .filter_map(|key| key.strip_prefix("branch."))
        .filter_map(|rest| rest.strip_suffix(DEFAULT_KEY_REPORTED))
        .map(String::from)
        .collect()
}

/// Every branch's provenance in one repository, read once per prune run.
#[derive(Debug, Default)]
pub struct Provenance {
    /// branch → the remote daft published it to.
    published: HashMap<String, String>,
    /// Branches daft created locally.
    local: HashSet<String>,
}

impl Provenance {
    /// Read every provenance key in the current repository.
    pub fn load(git: &GitCommand) -> Result<Self> {
        Ok(Self::parse(&git.branch_provenance_entries()?))
    }

    /// Parse `git config --get-regexp` output: one `branch.<name>.<key> value`
    /// per line.
    ///
    /// The branch name is recovered by stripping the known suffix rather than
    /// by splitting on dots — `branch.release.2.x.daftpublished` is a branch
    /// called `release.2.x`, and a dot-splitting parser would silently file it
    /// under `release`.
    pub fn parse(entries: &str) -> Self {
        let mut out = Self::default();

        for line in entries.lines() {
            let Some((key, value)) = line.trim_end().split_once(' ') else {
                continue;
            };
            let Some(rest) = key.strip_prefix("branch.") else {
                continue;
            };

            if let Some(branch) = rest.strip_suffix(PUBLISHED_KEY_REPORTED) {
                // "<remote> <timestamp>" — the timestamp is for humans reading
                // the config; only the remote is load-bearing.
                let remote = value.split_once(' ').map_or(value, |(remote, _)| remote);
                out.published
                    .insert(branch.to_string(), remote.trim().to_string());
            } else if let Some(branch) = rest.strip_suffix(ORIGIN_KEY_REPORTED)
                && value.trim() == ORIGIN_LOCAL
            {
                out.local.insert(branch.to_string());
            }
        }

        out
    }

    /// Whether daft published `branch` to this exact `remote`.
    ///
    /// The remote has to match: a branch published to `origin` says nothing
    /// about a repository pruning against `upstream`, and this is the answer
    /// that authorizes deleting a worktree.
    pub fn published_on(&self, branch: &str, remote: &str) -> bool {
        self.published.get(branch).is_some_and(|r| r == remote)
    }

    /// Note in this snapshot that `branch` has just been recorded as published
    /// to `remote`, so a caller that stamps and then queries stays consistent
    /// without re-reading the whole config.
    pub fn record_published(&mut self, branch: &str, remote: &str) {
        self.published
            .insert(branch.to_string(), remote.to_string());
    }

    /// Whether daft created `branch` locally and never published it.
    pub fn is_local_only(&self, branch: &str) -> bool {
        self.local.contains(branch) && !self.published.contains_key(branch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git_at(dir: &Path) -> Command {
        crate::utils::git_command_at(dir)
    }

    fn init_repo(dir: &Path) {
        git_at(dir).args(["init", "-q"]).output().unwrap();
    }

    fn local_value(dir: &Path, key: &str) -> Option<String> {
        let output = git_at(dir)
            .args(["config", "--local", "--get", key])
            .output()
            .unwrap();
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    #[test]
    fn published_stamp_records_the_remote_it_attests_to() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let git = GitCommand::new(true);

        mark_published(&git, dir.path(), "origin", "feat/x").unwrap();

        let raw = local_value(dir.path(), "branch.feat/x.daftPublished").unwrap();
        assert!(
            raw.starts_with("origin "),
            "the remote must lead the value: {raw}"
        );

        let entries = git_at(dir.path())
            .args(["config", "--get-regexp", r"^branch\..*\.daft"])
            .output()
            .unwrap();
        let provenance = Provenance::parse(&String::from_utf8_lossy(&entries.stdout));

        assert!(provenance.published_on("feat/x", "origin"));
        assert!(
            !provenance.published_on("feat/x", "upstream"),
            "a stamp for origin must not authorize pruning against another remote"
        );
    }

    #[test]
    fn local_marker_does_not_read_as_published() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let git = GitCommand::new(true);

        mark_local(&git, dir.path(), "feat/fresh").unwrap();

        let entries = git_at(dir.path())
            .args(["config", "--get-regexp", r"^branch\..*\.daft"])
            .output()
            .unwrap();
        let provenance = Provenance::parse(&String::from_utf8_lossy(&entries.stdout));

        assert!(provenance.is_local_only("feat/fresh"));
        assert!(!provenance.published_on("feat/fresh", "origin"));
    }

    #[test]
    fn git_carries_the_section_through_rename_and_drops_it_on_delete() {
        let dir = tempfile::tempdir().unwrap();
        init_repo(dir.path());
        let git = GitCommand::new(true);
        git_at(dir.path())
            .args(["commit", "-q", "--allow-empty", "-m", "seed"])
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        git_at(dir.path())
            .args(["branch", "feat/x"])
            .output()
            .unwrap();

        mark_published(&git, dir.path(), "origin", "feat/x").unwrap();

        git_at(dir.path())
            .args(["branch", "-m", "feat/x", "feat/renamed"])
            .output()
            .unwrap();
        assert!(
            local_value(dir.path(), "branch.feat/renamed.daftPublished").is_some(),
            "rename must carry the stamp, or the branch silently loses its provenance"
        );

        git_at(dir.path())
            .args(["branch", "-D", "feat/renamed"])
            .output()
            .unwrap();
        assert!(
            local_value(dir.path(), "branch.feat/renamed.daftPublished").is_none(),
            "delete must drop the stamp, or a recreated branch inherits a stale one"
        );
    }

    #[test]
    fn branch_names_containing_dots_survive_parsing() {
        let provenance = Provenance::parse(
            "branch.release.2.x.daftpublished origin 2026-08-22T00:00:00Z\n\
             branch.release.2.x.daftorigin local\n",
        );

        assert!(provenance.published_on("release.2.x", "origin"));
        assert!(
            !provenance.published_on("release", "origin"),
            "a dot-splitting parser would file this under the wrong branch"
        );
    }

    #[test]
    fn unknown_branches_are_unknown_not_published() {
        let provenance = Provenance::parse("");

        assert!(!provenance.published_on("anything", "origin"));
        assert!(!provenance.is_local_only("anything"));
    }

    // ── the default-branch record (#933) ──────────────────────────────────

    /// A bare repo holding `branch`, with `HEAD` naming it — the shape every
    /// daft-managed repo has. `local_head_branch` only reads a bare common
    /// dir, so the corroboration these tests exercise needs one.
    fn bare_repo_with_branch(parent: &Path, branch: &str) -> std::path::PathBuf {
        let src = parent.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for args in [
            vec!["init", "-q", "-b", branch],
            vec!["commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            git_at(&src)
                .args(&args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .unwrap();
        }
        let bare = parent.join("bare.git");
        git_at(parent)
            .args(["clone", "-q", "--bare"])
            .arg(&src)
            .arg(&bare)
            .output()
            .unwrap();
        bare
    }

    fn mark(dir: &Path, branch: &str) {
        mark_default(&GitCommand::new(true), dir, branch).unwrap();
    }

    #[test]
    fn a_recorded_default_branch_reads_back() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = bare_repo_with_branch(tmp.path(), "trunk");
        mark(&bare, "trunk");
        assert_eq!(recorded_default_branch(&bare).as_deref(), Some("trunk"));
    }

    /// The lifecycle the `branch.<name>` namespace is chosen for: `git branch
    /// -m` moves HEAD and renames the whole section, so both halves of the
    /// record follow the branch with no migration.
    #[test]
    fn the_record_follows_a_branch_rename() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = bare_repo_with_branch(tmp.path(), "trunk");
        mark(&bare, "trunk");
        git_at(&bare)
            .args(["branch", "-m", "trunk", "mainline"])
            .output()
            .unwrap();
        assert_eq!(recorded_default_branch(&bare).as_deref(), Some("mainline"));
    }

    /// `git branch -c` copies the entire `branch.<name>` section, marker
    /// included. Two claimants is unknown; and once the original is gone the
    /// survivor is still not what HEAD names, so it never inherits the claim.
    #[test]
    fn a_copied_branch_does_not_inherit_the_claim() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = bare_repo_with_branch(tmp.path(), "trunk");
        mark(&bare, "trunk");
        git_at(&bare)
            .args(["branch", "-c", "trunk", "copy"])
            .output()
            .unwrap();
        assert_eq!(
            recorded_default_branch(&bare),
            None,
            "two marked branches is unknown, not a coin flip"
        );
        git_at(&bare)
            .args(["branch", "-D", "copy"])
            .output()
            .unwrap();
        assert_eq!(recorded_default_branch(&bare).as_deref(), Some("trunk"));
    }

    /// The marker outranks `<remote>/HEAD`, never the repository itself, so on
    /// its own it is not evidence.
    #[test]
    fn an_uncorroborated_marker_is_not_evidence() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = bare_repo_with_branch(tmp.path(), "trunk");
        git_at(&bare)
            .args(["branch", "other", "trunk"])
            .output()
            .unwrap();
        mark(&bare, "other");
        assert_eq!(recorded_default_branch(&bare), None);
    }

    /// A repo renamed twice must not end up with two answers.
    #[test]
    fn marking_clears_the_previous_claimant() {
        let tmp = tempfile::tempdir().unwrap();
        let bare = bare_repo_with_branch(tmp.path(), "trunk");
        git_at(&bare)
            .args(["branch", "second", "trunk"])
            .output()
            .unwrap();
        mark(&bare, "second");
        mark(&bare, "trunk");
        assert_eq!(
            local_value(&bare, "branch.second.daftDefault"),
            None,
            "the stale claim is cleared, not left to collide"
        );
        assert_eq!(recorded_default_branch(&bare).as_deref(), Some("trunk"));
    }
}
