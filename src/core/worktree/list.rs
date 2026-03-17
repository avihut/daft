//! Core logic for the `git-worktree-list` / `daft list` command.
//!
//! Collects enriched worktree information (ahead/behind counts, dirty status,
//! last commit age and subject) for display.

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
    /// Commits ahead of the remote tracking branch (None if no upstream).
    pub remote_ahead: Option<usize>,
    /// Commits behind the remote tracking branch (None if no upstream).
    pub remote_behind: Option<usize>,
    /// Unix timestamp of the last commit (None if unavailable).
    pub last_commit_timestamp: Option<i64>,
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
    /// Author email of the branch tip commit (for ownership detection).
    pub owner_email: Option<String>,
    /// Total disk size of the worktree directory in bytes (None if not computed).
    pub size_bytes: Option<u64>,
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
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
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
            owner_email: None,
            size_bytes: None,
        }
    }

    /// Create a stub entry for a local-only branch (no worktree).
    pub fn local_branch_stub(name: &str, owner_email: Option<String>) -> Self {
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
            remote_ahead: None,
            remote_behind: None,
            last_commit_timestamp: None,
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
            owner_email,
            size_bytes: None,
        }
    }

    /// Re-compute the dynamic fields (ahead/behind, staged/unstaged, remote,
    /// last-commit) from the working tree on disk.  Static fields (kind, name,
    /// path, is_current, is_default_branch, branch_creation_timestamp) are
    /// left untouched.
    pub fn refresh_dynamic_fields(&mut self, base_branch: &str, stat: Stat) {
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
        let (staged, unstaged, untracked) = count_changed_files(path);
        self.staged = staged;
        self.unstaged = unstaged;
        self.untracked = untracked;

        // Remote ahead/behind
        let rab = get_upstream_ahead_behind(&self.name, path);
        self.remote_ahead = rab.map(|(a, _)| a);
        self.remote_behind = rab.map(|(_, b)| b);

        // Last commit
        let (ts, subj) = get_last_commit_info(path);
        self.last_commit_timestamp = ts;
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
}

/// Raw entry parsed from `git worktree list --porcelain`.
struct PorcelainEntry {
    path: PathBuf,
    branch: Option<String>,
    is_bare: bool,
    is_detached: bool,
}

/// Parse the porcelain output of `git worktree list --porcelain`.
///
/// Each entry is separated by a blank line and has the form:
/// ```text
/// worktree /path/to/worktree
/// HEAD <sha>
/// branch refs/heads/branch-name
/// ```
/// Bare entries have `bare` instead of `branch`.
/// Detached entries have `detached` instead of `branch`.
fn parse_porcelain(output: &str) -> Vec<PorcelainEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch: Option<String> = None;
    let mut is_bare = false;
    let mut is_detached = false;

    for line in output.lines() {
        if let Some(path_str) = line.strip_prefix("worktree ") {
            // Save previous entry if any
            if let Some(path) = current_path.take() {
                entries.push(PorcelainEntry {
                    path,
                    branch: current_branch.take(),
                    is_bare,
                    is_detached,
                });
            }
            current_path = Some(PathBuf::from(path_str));
            current_branch = None;
            is_bare = false;
            is_detached = false;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            current_branch = branch_ref.strip_prefix("refs/heads/").map(String::from);
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }
    // Don't forget the last entry
    if let Some(path) = current_path.take() {
        entries.push(PorcelainEntry {
            path,
            branch: current_branch.take(),
            is_bare,
            is_detached,
        });
    }

    entries
}

/// Get ahead/behind counts for a branch relative to a base branch.
///
/// Runs `git rev-list --left-right --count base...branch` in the given
/// worktree directory. Returns `(ahead, behind)` or `None` if the comparison
/// is not possible (e.g. unrelated histories, missing refs).
fn get_ahead_behind(
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

/// Get the last commit's Unix timestamp and subject for a worktree.
///
/// Returns `(timestamp, subject)` where timestamp is seconds since epoch.
fn get_last_commit_info(worktree_path: &Path) -> (Option<i64>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%s"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            if let Some((ts_str, subject)) = trimmed.split_once('\x1f') {
                let timestamp = ts_str.parse::<i64>().ok();
                (timestamp, subject.to_string())
            } else {
                (None, String::new())
            }
        }
        _ => (None, String::new()),
    }
}

/// Get the last commit's Unix timestamp and subject for a specific branch ref.
///
/// Unlike `get_last_commit_info`, this targets a named ref rather than HEAD,
/// so it can be called from any directory in the repository.
fn get_last_commit_info_for_ref(branch_ref: &str, cwd: &Path) -> (Option<i64>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%s", branch_ref])
        .current_dir(cwd)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            if let Some((ts_str, subject)) = trimmed.split_once('\x1f') {
                let timestamp = ts_str.parse::<i64>().ok();
                (timestamp, subject.to_string())
            } else {
                (None, String::new())
            }
        }
        _ => (None, String::new()),
    }
}

/// Get the author email of the tip commit on a given branch ref.
pub(crate) fn get_author_email_for_ref(branch_ref: &str, cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ae", branch_ref])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if email.is_empty() {
        None
    } else {
        Some(email)
    }
}

/// Get the Unix timestamp of when a branch was first created.
///
/// Primary: oldest reflog entry for the branch.
/// Fallback: timestamp of the first commit on the branch.
/// Returns `None` for detached HEAD or if both methods fail.
fn get_branch_creation_timestamp(branch: &str, worktree_path: &Path) -> Option<i64> {
    // Primary: oldest reflog entry
    let reflog_output = Command::new("git")
        .args(["reflog", "show", branch, "--format=%ct"])
        .current_dir(worktree_path)
        .output()
        .ok()?;

    if reflog_output.status.success() {
        let stdout = String::from_utf8_lossy(&reflog_output.stdout);
        // Last line is the oldest reflog entry
        if let Some(last_line) = stdout.trim().lines().last() {
            if let Ok(ts) = last_line.trim().parse::<i64>() {
                return Some(ts);
            }
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
        if let Some(first_line) = stdout.trim().lines().next() {
            if let Ok(ts) = first_line.trim().parse::<i64>() {
                return Some(ts);
            }
        }
    }

    None
}

/// Count staged, unstaged, and untracked files in a worktree.
///
/// Returns `(staged, unstaged, untracked)`.
fn count_changed_files(worktree_path: &Path) -> (usize, usize, usize) {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let mut staged = 0;
            let mut unstaged = 0;
            let mut untracked = 0;
            for line in stdout.lines() {
                if line.len() < 2 {
                    continue;
                }
                let bytes = line.as_bytes();
                let x = bytes[0]; // index (staged) status
                let y = bytes[1]; // worktree (unstaged) status
                if x == b'?' {
                    untracked += 1;
                } else {
                    if x != b' ' && x != b'?' {
                        staged += 1;
                    }
                    if y != b' ' && y != b'?' {
                        unstaged += 1;
                    }
                }
            }
            (staged, unstaged, untracked)
        }
        _ => (0, 0, 0),
    }
}

/// Get ahead/behind counts for a branch relative to its remote tracking branch.
fn get_upstream_ahead_behind(branch: &str, worktree_path: &Path) -> Option<(usize, usize)> {
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
fn count_changed_lines(worktree_path: &Path) -> ((usize, usize), (usize, usize)) {
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
fn get_base_line_counts(
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
fn get_remote_line_counts(branch: &str, worktree_path: &Path) -> Option<(usize, usize)> {
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
fn compute_directory_size(path: &Path) -> Option<u64> {
    use std::collections::HashSet;
    use std::os::unix::fs::MetadataExt;

    fn walk(dir: &Path, seen: &mut HashSet<(u64, u64)>) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut total = 0u64;
        for entry in entries {
            let Ok(entry) = entry else { continue };
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.is_dir() {
                total += walk(&entry.path(), seen);
            } else {
                // Skip hard links we've already counted (dev + ino pair).
                if meta.nlink() > 1 && !seen.insert((meta.dev(), meta.ino())) {
                    continue;
                }
                total += meta.len();
            }
        }
        total
    }

    let mut seen = HashSet::new();
    Some(walk(path, &mut seen))
}

/// Collect enriched worktree information for all worktrees in the project.
///
/// Parses the porcelain output, skips bare entries, enriches each entry with
/// ahead/behind, dirty status, and last commit info, then sorts alphabetically
/// by name (case-insensitive).
pub fn collect_worktree_info(
    git: &GitCommand,
    base_branch: &str,
    current_worktree_path: Option<&Path>,
    stat: Stat,
    compute_size: bool,
) -> Result<Vec<WorktreeInfo>> {
    let porcelain_output = git
        .worktree_list_porcelain()
        .context("Failed to list worktrees")?;

    let entries = parse_porcelain(&porcelain_output);
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

        // Count staged, unstaged, and untracked files
        let (staged, unstaged, untracked) = count_changed_files(&entry.path);

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
        let (last_commit_timestamp, last_commit_subject) = get_last_commit_info(&entry.path);

        // Owner email (author of branch tip commit)
        let owner_email = if !entry.is_detached {
            get_author_email_for_ref(&branch_display, &entry.path)
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

        let size_bytes = if compute_size {
            compute_directory_size(&entry.path)
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
            remote_ahead,
            remote_behind,
            last_commit_timestamp,
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
            owner_email,
            size_bytes,
        });
    }

    // Sort alphabetically by name (case-insensitive)
    infos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(infos)
}

/// Collect enriched information for branches that don't have active worktrees.
///
/// Enumerates local and/or remote branches, filters out those already represented
/// by a worktree, and enriches each with ahead/behind, commit info, and optionally
/// line-level stats.
pub fn collect_branch_info(
    git: &GitCommand,
    base_branch: &str,
    stat: Stat,
    include_local: bool,
    include_remote: bool,
    worktree_branches: &HashSet<String>,
    cwd: &Path,
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
            local_branch_names.insert(branch.to_string());

            let (ahead, behind) = match get_ahead_behind(base_branch, branch, cwd) {
                Some((a, b)) => (Some(a), Some(b)),
                None => (None, None),
            };

            let (remote_ahead, remote_behind) = match get_upstream_ahead_behind(branch, cwd) {
                Some((a, b)) => (Some(a), Some(b)),
                None => (None, None),
            };

            let (last_commit_timestamp, last_commit_subject) =
                get_last_commit_info_for_ref(branch, cwd);

            let owner_email = get_author_email_for_ref(branch, cwd);

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
                remote_ahead,
                remote_behind,
                last_commit_timestamp,
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
                owner_email,
                size_bytes: None,
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

            let (last_commit_timestamp, last_commit_subject) =
                get_last_commit_info_for_ref(remote_branch, cwd);

            let owner_email = get_author_email_for_ref(remote_branch, cwd);

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
                remote_ahead: None,
                remote_behind: None,
                last_commit_timestamp,
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
                owner_email,
                size_bytes: None,
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

    #[test]
    fn test_parse_porcelain_basic() {
        let output = "\
worktree /home/user/project/main
HEAD abc123
branch refs/heads/main

worktree /home/user/project/feature
HEAD def456
branch refs/heads/feature-branch
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 2);

        assert_eq!(entries[0].path, PathBuf::from("/home/user/project/main"));
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        assert!(!entries[0].is_bare);
        assert!(!entries[0].is_detached);

        assert_eq!(entries[1].path, PathBuf::from("/home/user/project/feature"));
        assert_eq!(entries[1].branch.as_deref(), Some("feature-branch"));
        assert!(!entries[1].is_bare);
        assert!(!entries[1].is_detached);
    }

    #[test]
    fn test_parse_porcelain_bare_skip() {
        let output = "\
worktree /home/user/project
HEAD abc123
bare

worktree /home/user/project/main
HEAD def456
branch refs/heads/main
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 2);

        // First entry is the bare root
        assert!(entries[0].is_bare);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/project"));

        // Second entry is a normal worktree
        assert!(!entries[1].is_bare);
        assert_eq!(entries[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn test_parse_porcelain_detached_head() {
        let output = "\
worktree /home/user/project/detached-wt
HEAD abc123
detached
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 1);

        assert!(entries[0].is_detached);
        assert!(!entries[0].is_bare);
        assert!(entries[0].branch.is_none());
        assert_eq!(
            entries[0].path,
            PathBuf::from("/home/user/project/detached-wt")
        );
    }

    #[test]
    fn test_parse_porcelain_empty() {
        let entries = parse_porcelain("");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_porcelain_mixed() {
        let output = "\
worktree /home/user/project
HEAD abc123
bare

worktree /home/user/project/main
HEAD def456
branch refs/heads/main

worktree /home/user/project/hotfix
HEAD 789abc
detached

worktree /home/user/project/feature
HEAD aaa111
branch refs/heads/feature/cool
";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 4);

        assert!(entries[0].is_bare);
        assert!(!entries[1].is_bare);
        assert!(!entries[1].is_detached);
        assert_eq!(entries[1].branch.as_deref(), Some("main"));
        assert!(entries[2].is_detached);
        assert!(!entries[3].is_bare);
        assert_eq!(entries[3].branch.as_deref(), Some("feature/cool"));
    }
}
