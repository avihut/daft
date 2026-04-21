//! Core logic for `daft worktree-exec`.
//!
//! Target resolution, per-worktree command pipeline, scheduler, and the
//! `ExecReport` data type that the command layer renders. No IO to stdout
//! lives here; renderers are separate.

use std::path::PathBuf;
use std::time::Duration;

/// A worktree that has been matched by the user's selectors and is ready
/// to receive commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub worktree_path: PathBuf,
    pub branch_name: String,
}

/// One command in a per-worktree pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSpec {
    /// Direct argv exec, e.g. from `-- CMD ARGS…`. First element is the
    /// program; remaining are its arguments. Never touches a shell.
    Argv(Vec<String>),
    /// Shell-parsed string, e.g. from `-x 'CMD'`. Run via `$SHELL -c`,
    /// falling back to `sh -c` if `$SHELL` is unset.
    Shell(String),
}

impl CommandSpec {
    /// A short one-line representation of the command for UI display.
    pub fn display(&self) -> String {
        match self {
            CommandSpec::Argv(parts) => parts.join(" "),
            CommandSpec::Shell(s) => s.clone(),
        }
    }
}

/// How the scheduler fans work out across worktrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecMode {
    /// All worktrees concurrently. Default.
    Parallel,
    /// One worktree at a time; stop on the first failing worktree.
    Sequential,
    /// One worktree at a time; continue through failures.
    KeepGoing,
}

/// Outcome of running the full pipeline on one worktree.
#[derive(Debug)]
pub struct WorktreeOutcome {
    pub target: ResolvedTarget,
    /// Index into the pipeline of the last command attempted (0-based). If
    /// the first command failed, this is 0. If all succeeded, this equals
    /// pipeline.len() - 1.
    pub last_command_index: usize,
    /// Exit code of the last command attempted. 0 when all succeeded.
    pub exit_code: i32,
    /// Wall-clock duration from spawn of first command to finish of last.
    pub elapsed: Duration,
    /// Captured stdout+stderr, truncated at `OUTPUT_CAP_BYTES` keeping the tail.
    pub captured_output: Vec<u8>,
    /// Whether the worktree was cancelled by SIGINT before finishing.
    pub cancelled: bool,
}

impl WorktreeOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0 && !self.cancelled
    }
}

/// The aggregate result of an exec invocation used by renderers and the
/// outer command layer.
#[derive(Debug, Default)]
pub struct ExecReport {
    pub outcomes: Vec<WorktreeOutcome>,
    pub orphan_branches_skipped: Vec<String>,
}

impl ExecReport {
    /// 0 if every worktree succeeded, 1 otherwise. Single-target
    /// pass-through never builds an `ExecReport`; it propagates the child
    /// exit code directly.
    pub fn aggregate_exit_code(&self) -> i32 {
        if self.outcomes.iter().all(|o| o.succeeded()) {
            0
        } else {
            1
        }
    }
}

/// Output-capture cap per worktree (ring buffer keeps the tail). Internal
/// constant — not user-configurable in v1.
pub const OUTPUT_CAP_BYTES: usize = 1024 * 1024; // 1 MiB

/// Byte tail-buffer: writes are appended; when total exceeds `cap`, the
/// oldest bytes are dropped so that only the last `cap` bytes remain.
///
/// Does not attempt to preserve UTF-8 boundaries. Callers that need string
/// output should `String::from_utf8_lossy(buf.tail())`.
pub struct TailBuffer {
    buf: Vec<u8>,
    cap: usize,
}

impl TailBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap.min(64 * 1024)),
            cap,
        }
    }

    pub fn extend(&mut self, bytes: &[u8]) {
        if bytes.len() >= self.cap {
            // New chunk alone fills (or overfills) the cap.
            let start = bytes.len() - self.cap;
            self.buf.clear();
            self.buf.extend_from_slice(&bytes[start..]);
            return;
        }
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
        }
    }

    pub fn tail(&self) -> &[u8] {
        &self.buf
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.buf
    }
}

/// Minimal worktree view that `resolve_targets` needs. In production we
/// build this from `crate::core::worktree::prune::parse_worktree_list`,
/// which already gives us `(path, branch)`.
#[derive(Clone, Debug)]
pub struct WorktreeSnapshot {
    pub path: std::path::PathBuf,
    pub branch: Option<String>,
}

impl WorktreeSnapshot {
    /// True if this snapshot represents a branch that has a real checked-out
    /// worktree. Orphan branches (branch exists in refs but no worktree)
    /// are represented with `path` pointing to an unusable sentinel.
    pub fn has_worktree(&self) -> bool {
        // In production, `parse_worktree_list` only ever returns real
        // worktrees; orphan branches are collected separately and fed in
        // with the sentinel path "::orphan::" from the command-layer
        // helper. Resolve on the sentinel to distinguish the two.
        self.path.to_str() != Some("::orphan::")
    }
}

fn is_glob(tok: &str) -> bool {
    tok.contains('*') || tok.contains('?') || tok.contains('[')
}

/// Errors surfaced during target resolution. Kept as a plain enum so
/// callers render friendly messages rather than parsing anyhow strings.
#[derive(Debug)]
pub enum ResolveError {
    NoTargets,
    AllWithPositionals,
    Unmatched(Vec<String>),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::NoTargets => {
                write!(
                    f,
                    "no targets: pass one or more branch/dir names, a glob, or --all"
                )
            }
            ResolveError::AllWithPositionals => {
                write!(f, "--all cannot be combined with positional targets")
            }
            ResolveError::Unmatched(toks) => {
                write!(f, "no worktree matched: {}", toks.join(", "))
            }
        }
    }
}

impl std::error::Error for ResolveError {}

pub fn resolve_targets(
    positionals: &[String],
    all: bool,
    worktrees: &[WorktreeSnapshot],
) -> Result<Vec<ResolvedTarget>, ResolveError> {
    if positionals.is_empty() && !all {
        return Err(ResolveError::NoTargets);
    }
    if !positionals.is_empty() && all {
        return Err(ResolveError::AllWithPositionals);
    }

    if all {
        let mut out: Vec<ResolvedTarget> = worktrees
            .iter()
            .filter_map(|w| {
                w.branch.as_ref().map(|b| ResolvedTarget {
                    worktree_path: w.path.clone(),
                    branch_name: b.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.branch_name.cmp(&b.branch_name));
        return Ok(out);
    }

    let mut out: Vec<ResolvedTarget> = Vec::new();
    let mut seen_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut unmatched: Vec<String> = Vec::new();

    for tok in positionals {
        let mut matched = false;

        // 1. Exact branch match.
        for wt in worktrees {
            if let Some(branch) = &wt.branch {
                if branch == tok {
                    matched = true;
                    if seen_paths.insert(wt.path.clone()) {
                        out.push(ResolvedTarget {
                            worktree_path: wt.path.clone(),
                            branch_name: branch.clone(),
                        });
                    }
                    break;
                }
            }
        }
        if matched {
            continue;
        }

        // 2. Exact directory-name match.
        for wt in worktrees {
            let dir_name = wt.path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if dir_name == tok {
                if let Some(branch) = &wt.branch {
                    matched = true;
                    if seen_paths.insert(wt.path.clone()) {
                        out.push(ResolvedTarget {
                            worktree_path: wt.path.clone(),
                            branch_name: branch.clone(),
                        });
                    }
                    break;
                }
            }
        }

        if !matched {
            unmatched.push(tok.clone());
        }
    }

    if !unmatched.is_empty() {
        return Err(ResolveError::Unmatched(unmatched));
    }

    Ok(out)
}

/// Like `resolve_targets`, but supports globs and reports orphan
/// branches (branches matched by a glob that have no worktree). Orphans
/// are surfaced so the renderer can print a one-line warning; they do not
/// count as unmatched for error purposes *unless* the glob matched nothing
/// actionable at all (only orphans, or nothing).
pub fn resolve_targets_with_orphans(
    positionals: &[String],
    all: bool,
    worktrees: &[WorktreeSnapshot],
) -> Result<(Vec<ResolvedTarget>, Vec<String>), ResolveError> {
    if positionals.is_empty() && !all {
        return Err(ResolveError::NoTargets);
    }
    if !positionals.is_empty() && all {
        return Err(ResolveError::AllWithPositionals);
    }

    if all {
        let mut out: Vec<ResolvedTarget> = worktrees
            .iter()
            .filter(|w| w.has_worktree())
            .filter_map(|w| {
                w.branch.as_ref().map(|b| ResolvedTarget {
                    worktree_path: w.path.clone(),
                    branch_name: b.clone(),
                })
            })
            .collect();
        out.sort_by(|a, b| a.branch_name.cmp(&b.branch_name));
        return Ok((out, Vec::new()));
    }

    use globset::{Glob, GlobSetBuilder};
    let mut out: Vec<ResolvedTarget> = Vec::new();
    let mut seen_paths: std::collections::HashSet<std::path::PathBuf> =
        std::collections::HashSet::new();
    let mut orphans: Vec<String> = Vec::new();
    let mut unmatched: Vec<String> = Vec::new();

    for tok in positionals {
        if is_glob(tok) {
            let glob = match Glob::new(tok) {
                Ok(g) => g,
                Err(_) => {
                    unmatched.push(tok.clone());
                    continue;
                }
            };
            let mut set_builder = GlobSetBuilder::new();
            set_builder.add(glob);
            let set = match set_builder.build() {
                Ok(s) => s,
                Err(_) => {
                    unmatched.push(tok.clone());
                    continue;
                }
            };

            let mut snapshot_branches: Vec<(&WorktreeSnapshot, &String)> = worktrees
                .iter()
                .filter_map(|w| w.branch.as_ref().map(|b| (w, b)))
                .filter(|(_, b)| set.is_match(b))
                .collect();
            snapshot_branches.sort_by(|a, b| a.1.cmp(b.1));

            let mut actionable_this_glob: usize = 0;
            let mut orphans_this_glob: Vec<String> = Vec::new();

            for (wt, branch) in snapshot_branches {
                if !wt.has_worktree() {
                    orphans_this_glob.push(branch.clone());
                } else if seen_paths.insert(wt.path.clone()) {
                    out.push(ResolvedTarget {
                        worktree_path: wt.path.clone(),
                        branch_name: branch.clone(),
                    });
                    actionable_this_glob += 1;
                } else {
                    // Already pulled in by an earlier positional; still
                    // counts as "this glob produced something actionable."
                    actionable_this_glob += 1;
                }
            }

            if actionable_this_glob == 0 {
                // Either matched nothing, or matched only orphans. Both
                // are errors; don't report orphans as "skipped" because
                // the run isn't happening.
                unmatched.push(tok.clone());
            } else {
                orphans.extend(orphans_this_glob);
            }
            continue;
        }

        // Exact fallthrough: reuse the non-glob resolver's logic.
        let sub = resolve_targets(std::slice::from_ref(tok), false, worktrees);
        match sub {
            Ok(exact) => {
                for t in exact {
                    if seen_paths.insert(t.worktree_path.clone()) {
                        out.push(t);
                    }
                }
            }
            Err(ResolveError::Unmatched(ref toks)) => {
                unmatched.extend_from_slice(toks);
            }
            Err(other) => return Err(other),
        }
    }

    if !unmatched.is_empty() {
        return Err(ResolveError::Unmatched(unmatched));
    }

    Ok((out, orphans))
}

/// Build a `Vec<WorktreeSnapshot>` from the repo's current worktree list
/// plus all local branches. Branches without an associated worktree are
/// included as orphan snapshots (sentinel path "::orphan::"), which
/// `resolve_targets_with_orphans` filters during glob expansion.
pub fn collect_snapshot(git: &crate::git::GitCommand) -> anyhow::Result<Vec<WorktreeSnapshot>> {
    use crate::core::worktree::prune::parse_worktree_list;

    let wt_entries = parse_worktree_list(git)?;
    let mut snaps: Vec<WorktreeSnapshot> = Vec::new();
    let mut branches_with_worktrees: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for entry in &wt_entries {
        if let Some(branch) = &entry.branch {
            branches_with_worktrees.insert(branch.clone());
            snaps.push(WorktreeSnapshot {
                path: entry.path.clone(),
                branch: Some(branch.clone()),
            });
        }
    }

    // Orphan branches: local branches that have no worktree. These are
    // only included for glob-expansion reporting; they carry the
    // "::orphan::" sentinel so `has_worktree()` returns false.
    let branch_output = git
        .for_each_ref("%(refname:short)", "refs/heads/")
        .unwrap_or_default();
    for line in branch_output.lines() {
        let branch = line.trim();
        if branch.is_empty() {
            continue;
        }
        if !branches_with_worktrees.contains(branch) {
            snaps.push(WorktreeSnapshot {
                path: std::path::PathBuf::from("::orphan::"),
                branch: Some(branch.to_string()),
            });
        }
    }

    Ok(snaps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_everything_under_cap() {
        let mut r = TailBuffer::new(16);
        r.extend(b"hello world");
        assert_eq!(r.tail(), b"hello world");
    }

    #[test]
    fn ring_keeps_only_tail_over_cap() {
        let mut r = TailBuffer::new(4);
        r.extend(b"abcdefghij");
        assert_eq!(r.tail(), b"ghij");
    }

    #[test]
    fn ring_exact_cap_boundary() {
        let mut r = TailBuffer::new(4);
        r.extend(b"abcd");
        assert_eq!(r.tail(), b"abcd");
        r.extend(b"e");
        assert_eq!(r.tail(), b"bcde");
    }

    #[test]
    fn ring_multi_extend_accumulates_tail() {
        let mut r = TailBuffer::new(5);
        r.extend(b"aaa");
        r.extend(b"bbb");
        r.extend(b"ccc");
        assert_eq!(r.tail(), b"bbccc");
    }

    use std::path::PathBuf;

    /// A lean snapshot of a worktree for resolver tests. Produced from
    /// `parse_worktree_list` in production; built by hand in tests.
    fn snap(path: &str, branch: &str) -> WorktreeSnapshot {
        WorktreeSnapshot {
            path: PathBuf::from(path),
            branch: Some(branch.to_string()),
        }
    }

    fn all_worktrees() -> Vec<WorktreeSnapshot> {
        vec![
            snap("/r/master", "master"),
            snap("/r/feat-a", "feat/a"),
            snap("/r/feat-b", "feat/b"),
            snap("/r/fix-crash", "fix/crash"),
        ]
    }

    #[test]
    fn exact_branch_name_resolves() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/a".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch_name, "feat/a");
        assert_eq!(got[0].worktree_path, PathBuf::from("/r/feat-a"));
    }

    #[test]
    fn exact_dir_name_resolves_when_branch_miss() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat-b".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].branch_name, "feat/b");
    }

    #[test]
    fn branch_takes_precedence_over_dir_name_collision() {
        // A worktree whose dir name collides with another worktree's branch.
        let wts = vec![
            snap("/r/x", "branch-x"), // dir name "x", branch "branch-x"
            snap("/r/y", "x"),        // branch "x", dir name "y"
        ];
        let got = resolve_targets(&["x".into()], false, &wts).unwrap();
        assert_eq!(got[0].branch_name, "x");
        assert_eq!(got[0].worktree_path, PathBuf::from("/r/y"));
    }

    #[test]
    fn dedupe_by_path() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/a".into(), "feat-a".into()], false, &wts).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn preserves_user_order() {
        let wts = all_worktrees();
        let got = resolve_targets(&["feat/b".into(), "feat/a".into()], false, &wts).unwrap();
        assert_eq!(got[0].branch_name, "feat/b");
        assert_eq!(got[1].branch_name, "feat/a");
    }

    fn snap_no_wt(branch: &str) -> WorktreeSnapshot {
        // A branch that exists in the repo but has no worktree checked out.
        // For test purposes we represent this as a snapshot whose path is
        // a sentinel — `has_worktree()` returns false for this path.
        WorktreeSnapshot {
            path: PathBuf::from("::orphan::"),
            branch: Some(branch.to_string()),
        }
    }

    fn all_with_orphans() -> Vec<WorktreeSnapshot> {
        let mut v = all_worktrees();
        v.push(snap_no_wt("feat/orphan-1"));
        v.push(snap_no_wt("feat/orphan-2"));
        v
    }

    fn resolve_report(
        positionals: &[String],
        all: bool,
        worktrees: &[WorktreeSnapshot],
    ) -> (Vec<ResolvedTarget>, Vec<String>) {
        let (tgts, orphans) = resolve_targets_with_orphans(positionals, all, worktrees).unwrap();
        (tgts, orphans)
    }

    #[test]
    fn glob_matches_branches_alphabetically() {
        let wts = all_worktrees();
        let (got, orphans) = resolve_report(&["feat/*".into()], false, &wts);
        assert_eq!(
            got.iter()
                .map(|t| t.branch_name.as_str())
                .collect::<Vec<_>>(),
            vec!["feat/a", "feat/b"]
        );
        assert!(orphans.is_empty());
    }

    #[test]
    fn glob_skips_orphan_branches_but_reports_them() {
        let wts = all_with_orphans();
        let (got, orphans) = resolve_report(&["feat/*".into()], false, &wts);
        let branches: Vec<_> = got.iter().map(|t| t.branch_name.as_str()).collect();
        assert_eq!(branches, vec!["feat/a", "feat/b"]);
        assert_eq!(orphans, vec!["feat/orphan-1", "feat/orphan-2"]);
    }

    #[test]
    fn glob_that_matches_nothing_is_error() {
        let wts = all_worktrees();
        let err = resolve_targets_with_orphans(&["zzz*".into()], false, &wts).unwrap_err();
        assert!(matches!(err, ResolveError::Unmatched(_)));
    }

    #[test]
    fn glob_that_only_matches_orphans_is_error_not_silent_skip() {
        // "feat/orphan-*" only matches branches with no worktree; nothing
        // actionable, so we error rather than run zero commands silently.
        let wts = all_with_orphans();
        let err = resolve_targets_with_orphans(&["feat/orphan-*".into()], false, &wts).unwrap_err();
        assert!(matches!(err, ResolveError::Unmatched(_)));
    }
}
