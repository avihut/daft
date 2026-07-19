//! Core logic for the `git-worktree-list` / `daft list` command.
//!
//! Collects enriched worktree information (ahead/behind counts, dirty status,
//! last commit age and subject) for display.

use crate::core::ownership::{self, BranchOwner, OwnershipStrategy};
use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Statistics mode for diff counts in the list output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Stat {
    /// Summary counts: commits for base/remote, files for changes (default).
    #[default]
    Summary,
    /// Line-level counts: insertions/deletions for all columns.
    Lines,
}

impl Stat {
    /// Parse a string value into a Stat mode.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "summary" => Some(Self::Summary),
            "lines" => Some(Self::Lines),
            _ => None,
        }
    }
}

/// The kind of entry in the list output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    /// Branch checked out in an active worktree.
    Worktree,
    /// Local branch without an active worktree.
    LocalBranch,
    /// Remote tracking branch without a local branch or worktree.
    RemoteBranch,
    /// An open PR with no local presence, synthesized from the forge-PR
    /// cache (`daft list` shows every open PR by default).
    ForgePr,
}

impl EntryKind {
    /// Display-section order: worktrees first, then local branches, then
    /// remote branches, then foreign open PRs — most local to least. Every
    /// list sort composes this before the user's sort key.
    pub fn section_order(self) -> u8 {
        match self {
            EntryKind::Worktree => 0,
            EntryKind::LocalBranch => 1,
            EntryKind::RemoteBranch => 2,
            EntryKind::ForgePr => 3,
        }
    }
}

/// Enriched information about a single worktree or branch.
#[derive(Clone, Debug)]
pub struct WorktreeInfo {
    /// The kind of entry.
    pub kind: EntryKind,
    /// Branch name (stripped of `refs/heads/` prefix), or `"(detached)"`.
    pub name: String,
    /// Absolute path to the worktree (None for non-worktree branches).
    pub path: Option<PathBuf>,
    /// Whether this worktree is the one the user is currently inside.
    pub is_current: bool,
    /// Whether this is the default (base) branch.
    pub is_default_branch: bool,
    /// Commits ahead of the base branch (None if not computable).
    pub ahead: Option<usize>,
    /// Commits behind the base branch (None if not computable).
    pub behind: Option<usize>,
    /// Number of staged files.
    pub staged: usize,
    /// Number of unstaged (modified/deleted) tracked files.
    pub unstaged: usize,
    /// Number of untracked files.
    pub untracked: usize,
    /// Number of files with unresolved merge conflicts. Counted separately
    /// from `staged`/`unstaged`, never in addition to them.
    pub conflicted: usize,
    /// Commits ahead of the remote tracking branch (None if no upstream).
    pub remote_ahead: Option<usize>,
    /// Commits behind the remote tracking branch (None if no upstream).
    pub remote_behind: Option<usize>,
    /// Unix timestamp of the last commit (None if unavailable).
    pub last_commit_timestamp: Option<i64>,
    /// Abbreviated commit hash of the last commit (None if unavailable).
    pub last_commit_hash: Option<String>,
    /// Subject line of the last commit.
    pub last_commit_subject: String,
    /// Unix timestamp of branch creation (None for detached HEAD or if unavailable).
    pub branch_creation_timestamp: Option<i64>,
    /// Lines inserted vs base branch (None if not computed or not applicable).
    pub base_lines_inserted: Option<usize>,
    /// Lines deleted vs base branch (None if not computed or not applicable).
    pub base_lines_deleted: Option<usize>,
    /// Lines inserted in staged changes (None if not computed).
    pub staged_lines_inserted: Option<usize>,
    /// Lines deleted in staged changes (None if not computed).
    pub staged_lines_deleted: Option<usize>,
    /// Lines inserted in unstaged changes (None if not computed).
    pub unstaged_lines_inserted: Option<usize>,
    /// Lines deleted in unstaged changes (None if not computed).
    pub unstaged_lines_deleted: Option<usize>,
    /// Lines inserted vs remote tracking branch (None if not computed or no upstream).
    pub remote_lines_inserted: Option<usize>,
    /// Lines deleted vs remote tracking branch (None if not computed or no upstream).
    pub remote_lines_deleted: Option<usize>,
    /// Resolved branch owner per the configured strategy. `None` when
    /// `base..branch` is empty or git failed.
    pub owner: Option<BranchOwner>,
    /// Total disk size of the worktree directory in bytes (None if not computed).
    pub size_bytes: Option<u64>,
    /// Most recent mtime of changed/untracked files (None if clean or not computed).
    pub working_tree_mtime: Option<i64>,
    /// Whether this is a detached HEAD sandbox (no branch).
    pub is_sandbox: bool,
    /// The PR/MR this branch tracks (from `branch.<name>.merge`), or `None`.
    /// Local config only — no network.
    pub forge_ref: Option<super::forge_ref::ForgeBranchRef>,
}

impl WorktreeInfo {
    /// Create a minimal `WorktreeInfo` with just a branch name and default values.
    /// Used by the TUI to create placeholder rows for dynamically discovered branches.
    pub fn empty(name: &str) -> Self {
        Self {
            kind: EntryKind::Worktree,
            name: name.to_string(),
            path: None,
            is_current: false,
            is_default_branch: false,
            ahead: None,
            behind: None,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
            last_commit_hash: None,
            last_commit_subject: String::new(),
            branch_creation_timestamp: None,
            base_lines_inserted: None,
            base_lines_deleted: None,
            staged_lines_inserted: None,
            staged_lines_deleted: None,
            unstaged_lines_inserted: None,
            unstaged_lines_deleted: None,
            remote_lines_inserted: None,
            remote_lines_deleted: None,
            owner: None,
            size_bytes: None,
            working_tree_mtime: None,
            is_sandbox: false,
            forge_ref: None,
        }
    }

    /// Create a stub entry for a local-only branch (no worktree).
    pub fn local_branch_stub(name: &str, owner: Option<BranchOwner>) -> Self {
        Self {
            kind: EntryKind::LocalBranch,
            name: name.to_string(),
            path: None,
            is_current: false,
            is_default_branch: false,
            ahead: None,
            behind: None,
            staged: 0,
            unstaged: 0,
            untracked: 0,
            conflicted: 0,
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
            last_commit_hash: None,
            last_commit_subject: String::new(),
            branch_creation_timestamp: None,
            base_lines_inserted: None,
            base_lines_deleted: None,
            staged_lines_inserted: None,
            staged_lines_deleted: None,
            unstaged_lines_inserted: None,
            unstaged_lines_deleted: None,
            remote_lines_inserted: None,
            remote_lines_deleted: None,
            owner,
            size_bytes: None,
            working_tree_mtime: None,
            is_sandbox: false,
            forge_ref: None,
        }
    }

    /// Re-compute the dynamic fields (ahead/behind, staged/unstaged, remote,
    /// last-commit) from the working tree on disk.  Static fields (kind, name,
    /// path, is_current, is_default_branch, branch_creation_timestamp) are
    /// left untouched.
    pub fn refresh_dynamic_fields(&mut self, base_branch: &str, stat: Stat, git: &GitCommand) {
        let Some(path) = self.path.as_deref() else {
            return;
        };

        // Base ahead/behind
        if self.name != "(detached)" {
            let ab = get_ahead_behind(base_branch, &self.name, path);
            self.ahead = ab.map(|(a, _)| a);
            self.behind = ab.map(|(_, b)| b);
        }

        // Working tree status
        let changed = count_changed_files(path);
        let (staged, unstaged, untracked) = (changed.staged, changed.unstaged, changed.untracked);
        self.staged = staged;
        self.unstaged = unstaged;
        self.untracked = untracked;

        // Remote ahead/behind
        let rab = get_upstream_ahead_behind(&self.name, path);
        self.remote_ahead = rab.map(|(a, _)| a);
        self.remote_behind = rab.map(|(_, b)| b);

        // Last commit
        let (ts, hash, subj) = get_commit_metadata(path, git);
        self.last_commit_timestamp = ts;
        self.last_commit_hash = hash;
        self.last_commit_subject = subj;

        // Line-level stats (only when Stat::Lines mode)
        if stat == Stat::Lines {
            let base_lines = get_base_line_counts(base_branch, &self.name, path);
            self.base_lines_inserted = base_lines.map(|(i, _)| i);
            self.base_lines_deleted = base_lines.map(|(_, d)| d);

            let ((si, sd), (ui, ud)) = count_changed_lines(path);
            self.staged_lines_inserted = Some(si);
            self.staged_lines_deleted = Some(sd);
            self.unstaged_lines_inserted = Some(ui);
            self.unstaged_lines_deleted = Some(ud);

            let remote_lines = get_remote_line_counts(&self.name, path);
            self.remote_lines_inserted = remote_lines.map(|(i, _)| i);
            self.remote_lines_deleted = remote_lines.map(|(_, d)| d);
        }

        // Re-compute size if it was previously computed
        if self.size_bytes.is_some() {
            self.size_bytes = compute_directory_size(path);
        }
    }

    /// Apply a typed patch in place. Returns the `FieldSet` of fields the
    /// patch addressed (the caller — typically `LiveTable` — uses this to
    /// decide whether to re-sort). The returned set reflects which cluster
    /// the patch belongs to, not whether values actually differ from prior
    /// state; idempotent re-sort is acceptable per spec.
    pub fn apply_patch(
        &mut self,
        patch: &crate::core::worktree::sync_dag::WorktreeInfoPatch,
    ) -> crate::core::worktree::info_field::FieldSet {
        use crate::core::worktree::info_field::FieldSet;
        use crate::core::worktree::sync_dag::WorktreeInfoPatch as P;

        match patch {
            P::BaseAheadBehind(v) => {
                (self.ahead, self.behind) = match v {
                    Some((a, b)) => (Some(*a), Some(*b)),
                    None => (None, None),
                };
                FieldSet::BASE_AHEAD_BEHIND
            }
            P::RemoteAheadBehind(v) => {
                (self.remote_ahead, self.remote_behind) = match v {
                    Some((a, b)) => (Some(*a), Some(*b)),
                    None => (None, None),
                };
                FieldSet::REMOTE_AHEAD_BEHIND
            }
            P::Changes {
                staged,
                unstaged,
                untracked,
                conflicted,
            } => {
                self.staged = *staged;
                self.unstaged = *unstaged;
                self.untracked = *untracked;
                self.conflicted = *conflicted;
                FieldSet::CHANGES
            }
            P::LastCommit {
                timestamp,
                hash,
                subject,
            } => {
                self.last_commit_timestamp = *timestamp;
                self.last_commit_hash = hash.clone();
                self.last_commit_subject = subject.clone();
                FieldSet::LAST_COMMIT
            }
            P::BranchAge(v) => {
                self.branch_creation_timestamp = *v;
                FieldSet::BRANCH_AGE
            }
            P::Owner(v) => {
                self.owner = v.clone();
                FieldSet::OWNER
            }
            P::BaseLines(v) => {
                (self.base_lines_inserted, self.base_lines_deleted) = match v {
                    Some((i, d)) => (Some(*i), Some(*d)),
                    None => (None, None),
                };
                FieldSet::BASE_LINES
            }
            P::ChangesLines { staged, unstaged } => {
                self.staged_lines_inserted = Some(staged.0);
                self.staged_lines_deleted = Some(staged.1);
                self.unstaged_lines_inserted = Some(unstaged.0);
                self.unstaged_lines_deleted = Some(unstaged.1);
                FieldSet::CHANGES_LINES
            }
            P::RemoteLines(v) => {
                (self.remote_lines_inserted, self.remote_lines_deleted) = match v {
                    Some((i, d)) => (Some(*i), Some(*d)),
                    None => (None, None),
                };
                FieldSet::REMOTE_LINES
            }
            P::Size(v) => {
                self.size_bytes = *v;
                FieldSet::SIZE
            }
            P::Mtime(v) => {
                self.working_tree_mtime = *v;
                FieldSet::MTIME
            }
            P::ForgeRef(v) => {
                self.forge_ref = *v;
                FieldSet::FORGE_REF
            }
        }
    }
}

/// Get ahead/behind counts for a branch relative to a base branch.
///
/// Runs `git rev-list --left-right --count base...branch` in the given
/// worktree directory. Returns `(ahead, behind)` or `None` if the comparison
/// is not possible (e.g. unrelated histories, missing refs).
pub(crate) fn get_ahead_behind(
    base_branch: &str,
    branch: &str,
    worktree_path: &Path,
) -> Option<(usize, usize)> {
    let range = format!("{base_branch}...{branch}");
    let output = Command::new("git")
        .args(["rev-list", "--left-right", "--count", &range])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let parts: Vec<&str> = stdout.trim().split('\t').collect();
    if parts.len() == 2 {
        let behind = parts[0].parse::<usize>().ok()?;
        let ahead = parts[1].parse::<usize>().ok()?;
        Some((ahead, behind))
    } else {
        None
    }
}

/// Dispatch commit metadata retrieval for a worktree HEAD, using gitoxide when
/// enabled with a fallback to the git subprocess.
pub(crate) fn get_commit_metadata(
    worktree_path: &Path,
    git: &GitCommand,
) -> (Option<i64>, Option<String>, String) {
    if git.use_gitoxide
        && let Ok((ts, hash, subj)) = crate::git::oxide::get_commit_metadata_for_head(worktree_path)
    {
        return (Some(ts), Some(hash), subj);
    }
    get_last_commit_info(worktree_path)
}

/// Dispatch commit metadata retrieval for a named ref, using gitoxide when
/// enabled with a fallback to the git subprocess.
fn get_commit_metadata_for_ref_dispatched(
    branch_ref: &str,
    cwd: &Path,
    git: &GitCommand,
) -> (Option<i64>, Option<String>, String) {
    if git.use_gitoxide
        && let Ok(repo) = git.gix_repo()
    {
        let full_ref = if branch_ref.starts_with("refs/") {
            branch_ref.to_string()
        } else if branch_ref.contains('/') {
            // Remote branch like "origin/feature-x"
            format!("refs/remotes/{branch_ref}")
        } else {
            format!("refs/heads/{branch_ref}")
        };
        if let Ok((ts, hash, subj)) =
            crate::git::oxide::get_commit_metadata_for_ref(&repo, &full_ref)
        {
            return (Some(ts), Some(hash), subj);
        }
    }
    get_last_commit_info_for_ref(branch_ref, cwd)
}

/// Get the last commit's Unix timestamp, abbreviated hash, and subject for a worktree.
///
/// Returns `(timestamp, hash, subject)` where timestamp is seconds since epoch.
fn get_last_commit_info(worktree_path: &Path) -> (Option<i64>, Option<String>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%h\x1f%s"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            let mut parts = trimmed.splitn(3, '\x1f');
            let ts_str = parts.next().unwrap_or("");
            let hash_str = parts.next().unwrap_or("");
            let subject = parts.next().unwrap_or("");
            if ts_str.is_empty() {
                (None, None, String::new())
            } else {
                let timestamp = ts_str.parse::<i64>().ok();
                let hash = if hash_str.is_empty() {
                    None
                } else {
                    Some(hash_str.to_string())
                };
                (timestamp, hash, subject.to_string())
            }
        }
        _ => (None, None, String::new()),
    }
}

/// Get the last commit's Unix timestamp, abbreviated hash, and subject for a specific branch ref.
///
/// Unlike `get_last_commit_info`, this targets a named ref rather than HEAD,
/// so it can be called from any directory in the repository.
fn get_last_commit_info_for_ref(
    branch_ref: &str,
    cwd: &Path,
) -> (Option<i64>, Option<String>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%h\x1f%s", branch_ref])
        .current_dir(cwd)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            let mut parts = trimmed.splitn(3, '\x1f');
            let ts_str = parts.next().unwrap_or("");
            let hash_str = parts.next().unwrap_or("");
            let subject = parts.next().unwrap_or("");
            if ts_str.is_empty() {
                (None, None, String::new())
            } else {
                let timestamp = ts_str.parse::<i64>().ok();
                let hash = if hash_str.is_empty() {
                    None
                } else {
                    Some(hash_str.to_string())
                };
                (timestamp, hash, subject.to_string())
            }
        }
        _ => (None, None, String::new()),
    }
}

/// Get the Unix timestamp of when a branch was first created.
///
/// Primary: oldest reflog entry for the branch.
/// Fallback: timestamp of the first commit on the branch.
/// Returns `None` for detached HEAD or if both methods fail.
pub(crate) fn get_branch_creation_timestamp(branch: &str, worktree_path: &Path) -> Option<i64> {
    // Primary: oldest reflog entry
    let reflog_output = Command::new("git")
        .args(["reflog", "show", branch, "--format=%ct"])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if reflog_output.status.success() {
        let stdout = String::from_utf8_lossy(&reflog_output.stdout);
        // Last line is the oldest reflog entry
        if let Some(last_line) = stdout.trim().lines().last()
            && let Ok(ts) = last_line.trim().parse::<i64>()
        {
            return Some(ts);
        }
    }

    // Fallback: first commit on the branch
    let log_output = Command::new("git")
        .args(["log", "--reverse", "--format=%ct", branch])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if log_output.status.success() {
        let stdout = String::from_utf8_lossy(&log_output.stdout);
        if let Some(first_line) = stdout.trim().lines().next()
            && let Ok(ts) = first_line.trim().parse::<i64>()
        {
            return Some(ts);
        }
    }

    None
}

/// Result of counting changed files in a worktree.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ChangedFiles {
    pub(crate) staged: usize,
    pub(crate) unstaged: usize,
    pub(crate) untracked: usize,
    /// Files with unresolved merge conflicts. Counted on their own, never as
    /// staged or unstaged — see [`classify_porcelain_status`].
    pub(crate) conflicted: usize,
    /// Relative paths of all changed/untracked files (for mtime computation).
    pub(crate) paths: Vec<String>,
}

/// The seven `XY` pairs `git status --porcelain` uses for unmerged paths.
/// Both letters have to match: `AU` and `UA` are conflicts, but `AM` and `MA`
/// are ordinary staged-and-modified files.
const UNMERGED_PAIRS: [[u8; 2]; 7] = [
    *b"DD", // both deleted
    *b"AU", // added by us
    *b"UD", // deleted by them
    *b"UA", // added by them
    *b"DU", // deleted by us
    *b"AA", // both added
    *b"UU", // both modified
];

/// Count staged, unstaged, untracked and conflicted files in a worktree.
pub(crate) fn count_changed_files(worktree_path: &Path) -> ChangedFiles {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            classify_porcelain_status(&String::from_utf8_lossy(&out.stdout))
        }
        _ => ChangedFiles::default(),
    }
}

/// Classify `git status --porcelain` output into per-state counts.
///
/// Split from the git invocation so the `XY` classification — the part with
/// the interesting edge cases — is testable without standing up a repo.
///
/// Conflicts are classified **first**, and exclusively. Their `XY` pairs
/// otherwise read as "something in the index *and* something in the worktree",
/// so a conflicted file used to be counted twice over — once as staged, once
/// as unstaged — and a two-file conflict rendered as `+2 -2`, which reads like
/// four files of ordinary work rather than two files needing a decision.
pub(crate) fn classify_porcelain_status(stdout: &str) -> ChangedFiles {
    let mut counts = ChangedFiles::default();

    for line in stdout.lines() {
        if line.len() < 2 {
            continue;
        }
        let bytes = line.as_bytes();
        let x = bytes[0]; // index (staged) status
        let y = bytes[1]; // worktree (unstaged) status

        if UNMERGED_PAIRS.contains(&[x, y]) {
            counts.conflicted += 1;
        } else if x == b'?' {
            counts.untracked += 1;
        } else {
            if x != b' ' && x != b'?' {
                counts.staged += 1;
            }
            if y != b' ' && y != b'?' {
                counts.unstaged += 1;
            }
        }

        // Extract filename (starts after "XY " at byte 3). Conflicted files
        // are included: they are modified on disk, so they count toward the
        // working-tree mtime like any other change.
        if line.len() > 3 {
            let path = &line[3..];
            // Renames show "old -> new"; take the new path.
            let path = path.rsplit_once(" -> ").map_or(path, |(_, new)| new);
            counts.paths.push(path.to_string());
        }
    }

    counts
}

/// Get ahead/behind counts for a branch relative to its remote tracking branch.
pub(crate) fn get_upstream_ahead_behind(
    branch: &str,
    worktree_path: &Path,
) -> Option<(usize, usize)> {
    let range = format!("{branch}@{{upstream}}...{branch}");
    let output = Command::new("git")
        .args(["rev-list", "--left-right", "--count", &range])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8(output.stdout).ok()?;
    let parts: Vec<&str> = stdout.trim().split('\t').collect();
    if parts.len() == 2 {
        let behind = parts[0].parse::<usize>().ok()?;
        let ahead = parts[1].parse::<usize>().ok()?;
        Some((ahead, behind))
    } else {
        None
    }
}

/// The PR/MR a branch tracks, read from its `branch.<name>.merge` config
/// (`refs/pull/N/head` / `refs/merge-requests/N/head`, written by a `pr:`/`mr:`
/// checkout). Local config only — no network. `None` for ordinary branches.
pub(crate) fn get_forge_branch_ref(
    branch: &str,
    worktree_path: &Path,
) -> Option<super::forge_ref::ForgeBranchRef> {
    let key = format!("branch.{branch}.merge");
    let output = Command::new("git")
        .args(["config", "--get", &key])
        .current_dir(worktree_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let merge = String::from_utf8(output.stdout).ok()?;
    super::forge_ref::ForgeBranchRef::parse_merge_ref(merge.trim())
}

/// Parse `git diff --numstat` output and return (total_insertions, total_deletions).
///
/// Each line has the format: `insertions\tdeletions\tfilename`.
/// Binary files show `-\t-\tfilename` and are counted as (0, 0).
fn parse_numstat(output: &str) -> (usize, usize) {
    let mut insertions = 0;
    let mut deletions = 0;
    for line in output.lines() {
        let mut parts = line.splitn(3, '\t');
        if let (Some(ins), Some(del)) = (parts.next(), parts.next()) {
            insertions += ins.parse::<usize>().unwrap_or(0);
            deletions += del.parse::<usize>().unwrap_or(0);
        }
    }
    (insertions, deletions)
}

/// Count lines inserted/deleted for staged and unstaged changes in a worktree.
///
/// Returns `((staged_ins, staged_del), (unstaged_ins, unstaged_del))`.
pub(crate) fn count_changed_lines(worktree_path: &Path) -> ((usize, usize), (usize, usize)) {
    let staged = Command::new("git")
        .args(["diff", "--cached", "--numstat"])
        .current_dir(worktree_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_numstat(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or((0, 0));

    let unstaged = Command::new("git")
        .args(["diff", "--numstat"])
        .current_dir(worktree_path)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_numstat(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or((0, 0));

    (staged, unstaged)
}

/// Get line counts between base branch and current branch.
///
/// Runs `git diff --numstat base...branch`.
/// Returns `(insertions, deletions)` or `None` if not computable.
pub(crate) fn get_base_line_counts(
    base_branch: &str,
    branch: &str,
    worktree_path: &Path,
) -> Option<(usize, usize)> {
    let range = format!("{base_branch}...{branch}");
    let output = Command::new("git")
        .args(["diff", "--numstat", &range])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(parse_numstat(&String::from_utf8_lossy(&output.stdout)))
}

/// Get line counts between branch and its remote tracking branch.
///
/// Runs `git diff --numstat branch@{upstream}...branch`.
/// Returns `(insertions, deletions)` or `None` if no upstream.
pub(crate) fn get_remote_line_counts(branch: &str, worktree_path: &Path) -> Option<(usize, usize)> {
    let range = format!("{branch}@{{upstream}}...{branch}");
    let output = Command::new("git")
        .args(["diff", "--numstat", &range])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    Some(parse_numstat(&String::from_utf8_lossy(&output.stdout)))
}

/// Recursively compute the total size of a directory in bytes.
///
/// Skips unreadable files/directories rather than aborting the entire traversal,
/// so a worktree with a few permission-denied entries still reports the sum of
/// all readable files. Tracks seen inodes to count hard-linked files only once
/// (matching `du` behavior). Does not follow symlinks.
pub(crate) fn compute_directory_size(path: &Path) -> Option<u64> {
    // Single-path convenience over the shared bounded walker. Preserves the old
    // always-`Some` contract (a missing/unreadable dir reports `Some(0)`); the
    // walker still parallelises this one tree's subdirectories.
    let roots = [path.to_path_buf()];
    crate::core::size_walk::walk_all(&roots, None, crate::core::size_walk::resolve_jobs(None))
        .pop()
        .flatten()
}

/// Return the most recent mtime among a set of changed/untracked files.
///
/// `worktree_path` is the root of the worktree; `relative_paths` are the
/// paths reported by `git status --porcelain` (relative to the worktree root).
/// Returns `None` if the list is empty or no file could be stat-ed.
pub(crate) fn max_mtime_of_files(worktree_path: &Path, relative_paths: &[String]) -> Option<i64> {
    let mut max_mtime: Option<i64> = None;
    for rel in relative_paths {
        let full = worktree_path.join(rel);
        if let Ok(meta) = std::fs::symlink_metadata(&full)
            && let Ok(modified) = meta.modified()
        {
            let ts = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            max_mtime = Some(max_mtime.map_or(ts, |cur| cur.max(ts)));
        }
    }
    max_mtime
}

/// Collect enriched worktree information for all worktrees in the project.
///
/// Parses the porcelain output, skips bare entries, enriches each entry with
/// ahead/behind, dirty status, and last commit info, then sorts alphabetically
/// by name (case-insensitive).
#[allow(clippy::too_many_arguments)]
pub fn collect_worktree_info(
    git: &GitCommand,
    base_branch: &str,
    current_worktree_path: Option<&Path>,
    stat: Stat,
    compute_size: bool,
    compute_mtime: bool,
    compute_forge_ref: bool,
    ownership_strategy: OwnershipStrategy,
    user_email: Option<&str>,
    remote_name: &str,
    size_jobs: usize,
) -> Result<Vec<WorktreeInfo>> {
    let porcelain_output = git
        .worktree_list_porcelain()
        .context("Failed to list worktrees")?;

    let entries = super::porcelain::parse_worktree_list_porcelain(&porcelain_output);
    let mut infos = Vec::new();

    for entry in entries {
        // Skip bare entries (the bare repo root)
        if entry.is_bare {
            continue;
        }

        let branch_display = if entry.is_detached {
            "(detached)".to_string()
        } else {
            entry.branch.clone().unwrap_or_default()
        };

        // Canonicalize for comparison (ignore errors, fall back to raw path)
        let canonical_entry = entry
            .path
            .canonicalize()
            .unwrap_or_else(|_| entry.path.clone());
        let is_current = current_worktree_path == Some(canonical_entry.as_path());

        // Ahead/behind relative to base branch
        let (ahead, behind) = if !entry.is_detached {
            if let Some(branch) = &entry.branch {
                match get_ahead_behind(base_branch, branch, &entry.path) {
                    Some((a, b)) => (Some(a), Some(b)),
                    None => (None, None),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Count staged, unstaged, untracked and conflicted files
        let changed = count_changed_files(&entry.path);
        let (staged, unstaged, untracked, conflicted) = (
            changed.staged,
            changed.unstaged,
            changed.untracked,
            changed.conflicted,
        );

        // Ahead/behind relative to upstream tracking branch
        let (remote_ahead, remote_behind) = if !entry.is_detached {
            if let Some(branch) = &entry.branch {
                match get_upstream_ahead_behind(branch, &entry.path) {
                    Some((a, b)) => (Some(a), Some(b)),
                    None => (None, None),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        // Last commit info
        let (last_commit_timestamp, last_commit_hash, last_commit_subject) =
            get_commit_metadata(&entry.path, git);

        let owner = if !entry.is_detached {
            ownership::resolve_owner_with_fallbacks(
                base_branch,
                &branch_display,
                &entry.path,
                ownership_strategy,
                user_email,
                Some(remote_name),
            )
        } else {
            None
        };

        // Branch creation timestamp (only for non-detached worktrees)
        let branch_creation_timestamp = if !entry.is_detached {
            entry
                .branch
                .as_deref()
                .and_then(|b| get_branch_creation_timestamp(b, &entry.path))
        } else {
            None
        };

        // Whether this is the default (base) branch
        let is_default_branch = entry.branch.as_deref().is_some_and(|b| b == base_branch);

        // Line-level diff counts (only when stat is Lines)
        let (base_lines_inserted, base_lines_deleted) = if stat == Stat::Lines && !entry.is_detached
        {
            if let Some(branch) = &entry.branch {
                match get_base_line_counts(base_branch, branch, &entry.path) {
                    Some((ins, del)) => (Some(ins), Some(del)),
                    None => (None, None),
                }
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let (
            staged_lines_inserted,
            staged_lines_deleted,
            unstaged_lines_inserted,
            unstaged_lines_deleted,
        ) = if stat == Stat::Lines {
            let ((si, sd), (ui, ud)) = count_changed_lines(&entry.path);
            (Some(si), Some(sd), Some(ui), Some(ud))
        } else {
            (None, None, None, None)
        };

        let (remote_lines_inserted, remote_lines_deleted) =
            if stat == Stat::Lines && !entry.is_detached {
                if let Some(branch) = &entry.branch {
                    match get_remote_line_counts(branch, &entry.path) {
                        Some((ins, del)) => (Some(ins), Some(del)),
                        None => (None, None),
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };

        let working_tree_mtime = if compute_mtime && !changed.paths.is_empty() {
            max_mtime_of_files(&entry.path, &changed.paths)
        } else {
            None
        };

        let forge_ref = if compute_forge_ref && !entry.is_detached {
            entry
                .branch
                .as_deref()
                .and_then(|b| get_forge_branch_ref(b, &entry.path))
        } else {
            None
        };

        infos.push(WorktreeInfo {
            kind: EntryKind::Worktree,
            name: branch_display,
            path: Some(entry.path),
            is_current,
            is_default_branch,
            ahead,
            behind,
            staged,
            unstaged,
            untracked,
            conflicted,
            remote_ahead,
            remote_behind,
            last_commit_timestamp,
            last_commit_hash,
            last_commit_subject,
            branch_creation_timestamp,
            base_lines_inserted,
            base_lines_deleted,
            staged_lines_inserted,
            staged_lines_deleted,
            unstaged_lines_inserted,
            unstaged_lines_deleted,
            remote_lines_inserted,
            remote_lines_deleted,
            owner,
            size_bytes: None,
            working_tree_mtime,
            is_sandbox: entry.is_detached,
            forge_ref,
        });
    }

    // Size walk: batched across all worktrees so their trees walk concurrently
    // under one shared job budget (see core::size_walk), instead of the old
    // one-worktree-at-a-time sequential walk. `size_jobs` is resolved by the
    // caller (DAFT_SIZE_WALK_JOBS / daft.list.sizeConcurrency / auto).
    if compute_size {
        let indexed: Vec<(usize, PathBuf)> = infos
            .iter()
            .enumerate()
            .filter_map(|(i, info)| info.path.clone().map(|p| (i, p)))
            .collect();
        let paths: Vec<PathBuf> = indexed.iter().map(|(_, p)| p.clone()).collect();
        let sizes = crate::core::size_walk::walk_all(&paths, None, size_jobs);
        for ((i, _), size) in indexed.into_iter().zip(sizes) {
            infos[i].size_bytes = size;
        }
    }

    Ok(infos)
}

/// Collect enriched information for branches that don't have active worktrees.
///
/// Enumerates local and/or remote branches, filters out those already represented
/// by a worktree, and enriches each with ahead/behind, commit info, and optionally
/// line-level stats.
///
/// `only_local` restricts the local arm to the named branches — the default
/// open-PR rows surface a handful of PR-bearing branches, and enriching every
/// local branch (several git calls each) just to discard most would tax every
/// bare `daft list`. `None` enriches all (the `--branches` path).
#[allow(clippy::too_many_arguments)]
pub fn collect_branch_info(
    git: &GitCommand,
    base_branch: &str,
    stat: Stat,
    include_local: bool,
    include_remote: bool,
    worktree_branches: &HashSet<String>,
    only_local: Option<&HashSet<String>>,
    cwd: &Path,
    ownership_strategy: OwnershipStrategy,
    user_email: Option<&str>,
    remote_name: &str,
) -> Result<Vec<WorktreeInfo>> {
    let mut infos = Vec::new();
    let mut local_branch_names: HashSet<String> = HashSet::new();

    // Collect local branches without worktrees
    if include_local {
        let output = git
            .for_each_ref("%(refname:short)", "refs/heads/")
            .context("Failed to list local branches")?;

        for branch in output.lines() {
            let branch = branch.trim();
            if branch.is_empty() || worktree_branches.contains(branch) {
                continue;
            }
            if let Some(only) = only_local
                && !only.contains(branch)
            {
                continue;
            }
            local_branch_names.insert(branch.to_string());

            let (ahead, behind) = match get_ahead_behind(base_branch, branch, cwd) {
                Some((a, b)) => (Some(a), Some(b)),
                None => (None, None),
            };

            let (remote_ahead, remote_behind) = match get_upstream_ahead_behind(branch, cwd) {
                Some((a, b)) => (Some(a), Some(b)),
                None => (None, None),
            };

            let (last_commit_timestamp, last_commit_hash, last_commit_subject) =
                get_commit_metadata_for_ref_dispatched(branch, cwd, git);

            let owner = ownership::resolve_owner_with_fallbacks(
                base_branch,
                branch,
                cwd,
                ownership_strategy,
                user_email,
                Some(remote_name),
            );

            let branch_creation_timestamp = get_branch_creation_timestamp(branch, cwd);

            let is_default_branch = branch == base_branch;

            // Line-level stats (base and remote only — no working dir for staged/unstaged)
            let (base_lines_inserted, base_lines_deleted) = if stat == Stat::Lines {
                match get_base_line_counts(base_branch, branch, cwd) {
                    Some((ins, del)) => (Some(ins), Some(del)),
                    None => (None, None),
                }
            } else {
                (None, None)
            };

            let (remote_lines_inserted, remote_lines_deleted) = if stat == Stat::Lines {
                match get_remote_line_counts(branch, cwd) {
                    Some((ins, del)) => (Some(ins), Some(del)),
                    None => (None, None),
                }
            } else {
                (None, None)
            };

            infos.push(WorktreeInfo {
                kind: EntryKind::LocalBranch,
                name: branch.to_string(),
                path: None,
                is_current: false,
                is_default_branch,
                ahead,
                behind,
                staged: 0,
                unstaged: 0,
                untracked: 0,
                conflicted: 0,
                remote_ahead,
                remote_behind,
                last_commit_timestamp,
                last_commit_hash,
                last_commit_subject,
                branch_creation_timestamp,
                base_lines_inserted,
                base_lines_deleted,
                staged_lines_inserted: None,
                staged_lines_deleted: None,
                unstaged_lines_inserted: None,
                unstaged_lines_deleted: None,
                remote_lines_inserted,
                remote_lines_deleted,
                owner,
                size_bytes: None,
                working_tree_mtime: None,
                is_sandbox: false,
                forge_ref: None,
            });
        }
    }

    // Collect remote branches without local branches or worktrees
    if include_remote {
        let output = git
            .for_each_ref("%(refname:short)", "refs/remotes/origin/")
            .context("Failed to list remote branches")?;

        for remote_branch in output.lines() {
            let remote_branch = remote_branch.trim();
            // %(refname:short) renders origin/HEAD as just "origin"
            if remote_branch.is_empty()
                || remote_branch == "origin/HEAD"
                || remote_branch == "origin"
            {
                continue;
            }

            // Strip origin/ prefix for deduplication check
            let short_name = remote_branch
                .strip_prefix("origin/")
                .unwrap_or(remote_branch);

            // Skip if already represented by a worktree or local branch
            if worktree_branches.contains(short_name) || local_branch_names.contains(short_name) {
                continue;
            }

            let (ahead, behind) = match get_ahead_behind(base_branch, remote_branch, cwd) {
                Some((a, b)) => (Some(a), Some(b)),
                None => (None, None),
            };

            let (last_commit_timestamp, last_commit_hash, last_commit_subject) =
                get_commit_metadata_for_ref_dispatched(remote_branch, cwd, git);

            let owner = ownership::resolve_owner(
                base_branch,
                remote_branch,
                cwd,
                ownership_strategy,
                user_email,
            );

            // Line-level stats (base only — no upstream concept for remote branches)
            let (base_lines_inserted, base_lines_deleted) = if stat == Stat::Lines {
                match get_base_line_counts(base_branch, remote_branch, cwd) {
                    Some((ins, del)) => (Some(ins), Some(del)),
                    None => (None, None),
                }
            } else {
                (None, None)
            };

            infos.push(WorktreeInfo {
                kind: EntryKind::RemoteBranch,
                name: remote_branch.to_string(),
                path: None,
                is_current: false,
                is_default_branch: false,
                ahead,
                behind,
                staged: 0,
                unstaged: 0,
                untracked: 0,
                conflicted: 0,
                remote_ahead: None,
                remote_behind: None,
                last_commit_timestamp,
                last_commit_hash,
                last_commit_subject,
                branch_creation_timestamp: None,
                base_lines_inserted,
                base_lines_deleted,
                staged_lines_inserted: None,
                staged_lines_deleted: None,
                unstaged_lines_inserted: None,
                unstaged_lines_deleted: None,
                remote_lines_inserted: None,
                remote_lines_deleted: None,
                owner,
                size_bytes: None,
                working_tree_mtime: None,
                is_sandbox: false,
                forge_ref: None,
            });
        }
    }

    Ok(infos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_numstat_basic() {
        assert_eq!(parse_numstat("10\t5\tfile.rs\n3\t1\tother.rs\n"), (13, 6));
    }

    #[test]
    fn test_parse_numstat_binary() {
        assert_eq!(parse_numstat("-\t-\timage.png\n5\t2\tcode.rs\n"), (5, 2));
    }

    #[test]
    fn test_parse_numstat_empty() {
        assert_eq!(parse_numstat(""), (0, 0));
    }

    // ── porcelain status classification ─────────────────────────────────────

    /// Every `XY` pair git documents as unmerged counts as exactly one
    /// conflict, and as nothing else. The regression: these pairs read as
    /// "changed in the index" *and* "changed in the worktree", so each
    /// conflicted file used to land in both the staged and the unstaged
    /// tally — two conflicts rendered as `+2 -2`.
    #[test]
    fn every_unmerged_pair_counts_once_as_a_conflict() {
        for pair in ["DD", "AU", "UD", "UA", "DU", "AA", "UU"] {
            let counts = classify_porcelain_status(&format!("{pair} f.txt\n"));
            assert_eq!(
                counts,
                ChangedFiles {
                    conflicted: 1,
                    staged: 0,
                    unstaged: 0,
                    untracked: 0,
                    paths: vec!["f.txt".to_string()],
                },
                "porcelain pair {pair} misclassified"
            );
        }
    }

    /// The pairs that merely *look* unmerged because one letter matches.
    /// `AM` is staged-add plus worktree-modify; `MA` and `MU` are not
    /// conflicts either. Only both letters together make a conflict.
    #[test]
    fn lookalike_pairs_are_not_conflicts() {
        let counts = classify_porcelain_status("AM a.txt\nMD b.txt\n");
        assert_eq!(counts.conflicted, 0);
        assert_eq!(counts.staged, 2);
        assert_eq!(counts.unstaged, 2);
    }

    #[test]
    fn ordinary_states_are_unchanged() {
        // One staged, one unstaged, one both, one untracked.
        let counts = classify_porcelain_status("M  a.txt\n M b.txt\nMM c.txt\n?? d.txt\n");
        assert_eq!(counts.staged, 2, "a.txt and c.txt");
        assert_eq!(counts.unstaged, 2, "b.txt and c.txt");
        assert_eq!(counts.untracked, 1);
        assert_eq!(counts.conflicted, 0);
    }

    #[test]
    fn conflicts_mix_with_ordinary_changes_without_inflating_them() {
        let counts = classify_porcelain_status("UU a.txt\nM  b.txt\n?? c.txt\n");
        assert_eq!(counts.conflicted, 1);
        assert_eq!(counts.staged, 1, "only b.txt — never the conflicted a.txt");
        assert_eq!(counts.unstaged, 0);
        assert_eq!(counts.untracked, 1);
    }

    /// Conflicted files are still modified on disk, so they must keep
    /// contributing to the working-tree mtime like any other change.
    #[test]
    fn conflicted_paths_are_still_collected_for_mtime() {
        let counts = classify_porcelain_status("UU src/a.txt\n");
        assert_eq!(counts.paths, vec!["src/a.txt".to_string()]);
    }

    #[test]
    fn rename_entries_report_the_new_path() {
        let counts = classify_porcelain_status("R  old.txt -> new.txt\n");
        assert_eq!(counts.paths, vec!["new.txt".to_string()]);
        assert_eq!(counts.staged, 1);
    }

    #[test]
    fn empty_and_truncated_lines_are_skipped() {
        assert_eq!(classify_porcelain_status(""), ChangedFiles::default());
        let counts = classify_porcelain_status("\nM\nM  a.txt\n");
        assert_eq!(counts.staged, 1);
        assert_eq!(counts.paths, vec!["a.txt".to_string()]);
    }

    /// End-to-end against a real repo: a conflicting merge, read through the
    /// actual `git status --porcelain` invocation rather than a fixture
    /// string, so the classification is pinned to git's real output.
    #[test]
    fn counts_a_real_conflict_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        let git = |args: &[&str]| {
            let out = crate::utils::git_command_at(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .output()
                .expect("git");
            out.status.success()
        };
        assert!(git(&["init", "-q", "-b", "main"]));
        std::fs::write(path.join("f.txt"), "base\n").unwrap();
        assert!(git(&["add", "f.txt"]));
        assert!(git(&["commit", "-qm", "base"]));
        assert!(git(&["checkout", "-qb", "other"]));
        std::fs::write(path.join("f.txt"), "other\n").unwrap();
        assert!(git(&["commit", "-qam", "other"]));
        assert!(git(&["checkout", "-q", "main"]));
        std::fs::write(path.join("f.txt"), "main\n").unwrap();
        assert!(git(&["commit", "-qam", "main"]));
        // Expected to fail — that is the conflict we are counting.
        assert!(!git(&["merge", "other"]));

        let counts = count_changed_files(path);
        assert_eq!(counts.conflicted, 1);
        assert_eq!(counts.staged, 0, "a conflict is not a staged change");
        assert_eq!(counts.unstaged, 0, "nor an unstaged one");
    }
}

#[cfg(test)]
mod apply_patch_tests {
    use super::*;
    use crate::core::worktree::info_field::FieldSet;
    use crate::core::worktree::sync_dag::WorktreeInfoPatch;

    fn empty_info() -> WorktreeInfo {
        WorktreeInfo::empty("test")
    }

    #[test]
    fn base_ahead_behind_some_fills_both_and_returns_the_field() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::BaseAheadBehind(Some((3, 1))));
        assert_eq!(info.ahead, Some(3));
        assert_eq!(info.behind, Some(1));
        assert_eq!(touched, FieldSet::BASE_AHEAD_BEHIND);
    }

    #[test]
    fn base_ahead_behind_none_clears_both() {
        let mut info = empty_info();
        info.ahead = Some(5);
        info.behind = Some(2);
        let touched = info.apply_patch(&WorktreeInfoPatch::BaseAheadBehind(None));
        assert_eq!(info.ahead, None);
        assert_eq!(info.behind, None);
        assert_eq!(touched, FieldSet::BASE_AHEAD_BEHIND);
    }

    #[test]
    fn changes_fills_three_fields() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::Changes {
            staged: 2,
            unstaged: 1,
            untracked: 4,
            conflicted: 0,
        });
        assert_eq!((info.staged, info.unstaged, info.untracked), (2, 1, 4));
        assert_eq!(touched, FieldSet::CHANGES);
    }

    #[test]
    fn last_commit_fills_three_fields() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::LastCommit {
            timestamp: Some(1700000000),
            hash: Some("abc1234".into()),
            subject: "fix bug".into(),
        });
        assert_eq!(info.last_commit_timestamp, Some(1700000000));
        assert_eq!(info.last_commit_hash, Some("abc1234".into()));
        assert_eq!(info.last_commit_subject, "fix bug");
        assert_eq!(touched, FieldSet::LAST_COMMIT);
    }

    #[test]
    fn size_fills_size_bytes() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::Size(Some(2048)));
        assert_eq!(info.size_bytes, Some(2048));
        assert_eq!(touched, FieldSet::SIZE);
    }

    #[test]
    fn changes_lines_fills_four_fields() {
        let mut info = empty_info();
        let touched = info.apply_patch(&WorktreeInfoPatch::ChangesLines {
            staged: (10, 2),
            unstaged: (5, 1),
        });
        assert_eq!(info.staged_lines_inserted, Some(10));
        assert_eq!(info.staged_lines_deleted, Some(2));
        assert_eq!(info.unstaged_lines_inserted, Some(5));
        assert_eq!(info.unstaged_lines_deleted, Some(1));
        assert_eq!(touched, FieldSet::CHANGES_LINES);
    }
}
