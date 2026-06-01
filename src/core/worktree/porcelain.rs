//! The single parser for `git worktree list --porcelain` output.
//!
//! Every consumer that needs structured worktree information — layout
//! detection, prune / branch-delete / rename, `daft list`, repo enumeration,
//! the doctor, layout transforms — routes through
//! [`parse_worktree_list_porcelain`]. It is pure (string in, structs out):
//! callers own the git invocation (typically via
//! [`crate::utils::git_command_at`], which scrubs inherited `GIT_*` so a `-C`
//! directory stays authoritative) and pass the captured stdout here.
//!
//! Bare entries are **retained** (with `is_bare = true`) so callers that need
//! to reason about the bare repo can. Callers that only want checked-out
//! worktrees filter on `!is_bare` — or use [`first_main_index`] to find the
//! first non-bare ("main") entry.

use std::path::PathBuf;

/// A single entry from `git worktree list --porcelain`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorktreeListEntry {
    /// The worktree's root path (from the `worktree <path>` line).
    pub path: PathBuf,
    /// The checked-out branch short name, or `None` for a detached HEAD or the
    /// bare entry. Only `refs/heads/<branch>` refs populate this.
    pub branch: Option<String>,
    /// True for the repository's bare entry (the `bare` line).
    pub is_bare: bool,
    /// True for a detached-HEAD worktree (the `detached` line) — e.g. a daft
    /// sandbox.
    pub is_detached: bool,
}

/// Parse `git worktree list --porcelain` output into [`WorktreeListEntry`]s.
///
/// Stanzas are delimited by blank lines, each opening with a `worktree <path>`
/// line. The final stanza needs no trailing blank line or newline — it is
/// flushed at end of input. Bare entries are retained with `is_bare = true`;
/// a `detached` worktree yields `branch = None`.
pub fn parse_worktree_list_porcelain(porcelain: &str) -> Vec<WorktreeListEntry> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut is_bare = false;
    let mut is_detached = false;

    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(p) = path.take() {
                entries.push(WorktreeListEntry {
                    path: p,
                    branch: branch.take(),
                    is_bare,
                    is_detached,
                });
            }
            branch = None;
            is_bare = false;
            is_detached = false;
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch refs/heads/") {
            branch = Some(rest.to_string());
        } else if line == "bare" {
            is_bare = true;
        } else if line == "detached" {
            is_detached = true;
        }
    }
    if let Some(p) = path.take() {
        entries.push(WorktreeListEntry {
            path: p,
            branch: branch.take(),
            is_bare,
            is_detached,
        });
    }

    entries
}

/// Index of the first non-bare entry — the "main" worktree — or `None` when the
/// list is empty or every entry is bare.
pub fn first_main_index(entries: &[WorktreeListEntry]) -> Option<usize> {
    entries.iter().position(|e| !e.is_bare)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parses_multiple_entries_with_branches() {
        let input = "\
worktree /home/user/proj/main
HEAD abc123
branch refs/heads/main

worktree /home/user/proj/develop
HEAD def456
branch refs/heads/develop
";
        let entries = parse_worktree_list_porcelain(input);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0],
            WorktreeListEntry {
                path: PathBuf::from("/home/user/proj/main"),
                branch: Some("main".to_string()),
                is_bare: false,
                is_detached: false,
            }
        );
        assert_eq!(
            entries[1],
            WorktreeListEntry {
                path: PathBuf::from("/home/user/proj/develop"),
                branch: Some("develop".to_string()),
                is_bare: false,
                is_detached: false,
            }
        );
    }

    #[test]
    fn retains_bare_entry_with_is_bare_set() {
        let input = "\
worktree /home/user/proj/.git
HEAD abc123
bare

worktree /home/user/proj/develop
HEAD def456
branch refs/heads/develop
";
        let entries = parse_worktree_list_porcelain(input);
        // Bare is RETAINED (unlike the layout-detection facade, which drops it).
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_bare);
        assert_eq!(entries[0].branch, None);
        assert!(!entries[1].is_bare);
    }

    #[test]
    fn detached_head_has_no_branch() {
        let input = "\
worktree /home/user/proj/main
HEAD abc123
branch refs/heads/main

worktree /home/user/proj/sandbox
HEAD deadbeef
detached
";
        let entries = parse_worktree_list_porcelain(input);
        assert_eq!(entries.len(), 2);
        assert!(entries[1].is_detached);
        assert_eq!(entries[1].branch, None);
        assert_eq!(entries[1].path, PathBuf::from("/home/user/proj/sandbox"));
    }

    #[test]
    fn flushes_final_entry_without_trailing_newline() {
        // No trailing blank line / newline — the last stanza must still flush.
        let input = "worktree /home/user/proj/main\nHEAD abc123\nbranch refs/heads/main";
        let entries = parse_worktree_list_porcelain(input);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("/home/user/proj/main"));
        assert_eq!(entries[0].branch, Some("main".to_string()));
    }

    #[test]
    fn empty_input_yields_no_entries() {
        assert!(parse_worktree_list_porcelain("").is_empty());
    }

    #[test]
    fn preserves_slashed_branch_short_name() {
        // A `branch refs/heads/feature/cool` line yields the short slashed name.
        let input = "worktree /p/feature\nHEAD aaa111\nbranch refs/heads/feature/cool\n";
        let entries = parse_worktree_list_porcelain(input);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch.as_deref(), Some("feature/cool"));
    }

    #[test]
    fn mixed_bare_detached_and_branches() {
        // A full repo: bare root + main + detached sandbox + slashed feature.
        let input = "\
worktree /p/.git
HEAD abc123
bare

worktree /p/main
HEAD def456
branch refs/heads/main

worktree /p/hotfix
HEAD 789abc
detached

worktree /p/feature
HEAD aaa111
branch refs/heads/feature/cool
";
        let entries = parse_worktree_list_porcelain(input);
        assert_eq!(entries.len(), 4);
        assert!(entries[0].is_bare);
        assert!(!entries[1].is_bare && !entries[1].is_detached);
        assert_eq!(entries[1].branch.as_deref(), Some("main"));
        assert!(entries[2].is_detached);
        assert_eq!(entries[2].branch, None);
        assert_eq!(entries[3].branch.as_deref(), Some("feature/cool"));
    }

    #[test]
    fn first_main_index_skips_leading_bare() {
        let input = "\
worktree /home/user/proj/.git
HEAD abc123
bare

worktree /home/user/proj/main
HEAD def456
branch refs/heads/main

worktree /home/user/proj/develop
HEAD aaa111
branch refs/heads/develop
";
        let entries = parse_worktree_list_porcelain(input);
        // index 0 is bare → first main is index 1.
        assert_eq!(first_main_index(&entries), Some(1));
    }

    #[test]
    fn first_main_index_none_when_empty_or_all_bare() {
        assert_eq!(first_main_index(&[]), None);
        let only_bare = parse_worktree_list_porcelain("worktree /x/.git\nHEAD abc\nbare\n");
        assert_eq!(first_main_index(&only_bare), None);
    }
}
