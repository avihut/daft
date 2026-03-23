//! Worktree gathering and template matching for layout detection.
//!
//! This module provides two main capabilities:
//! 1. Parsing `git worktree list --porcelain` output into `WorktreeInfo` structs.
//! 2. Matching a set of candidate layouts against existing worktrees to detect
//!    which layout is in use.

use std::path::{Path, PathBuf};

use super::Layout;
use crate::core::layout::resolver::DetectionResult;
use crate::core::layout::template::{render, resolve_path};
use crate::core::multi_remote::path::build_template_context;

/// Information about a single worktree from `git worktree list --porcelain`.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>, // None for detached HEAD
    pub is_main: bool,          // first non-bare entry = main worktree
}

/// Parse the output of `git worktree list --porcelain` into a list of `WorktreeInfo`.
///
/// Rules:
/// - Skip bare entries (line "bare" appears after a worktree line).
/// - Parse branch from "branch refs/heads/..." lines.
/// - Handle "detached" entries (branch = None).
/// - The first non-bare entry has `is_main = true`.
pub fn parse_worktree_list(porcelain: &str) -> Vec<WorktreeInfo> {
    let mut result = Vec::new();
    let mut found_main = false;

    // Split into stanzas (blank-line delimited)
    let stanzas: Vec<&str> = porcelain
        .split("\n\n")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    for stanza in stanzas {
        let mut path: Option<PathBuf> = None;
        let mut branch: Option<String> = None;
        let mut is_bare = false;
        let mut is_detached = false;

        for line in stanza.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p.trim()));
            } else if line.trim() == "bare" {
                is_bare = true;
            } else if line.trim() == "detached" {
                is_detached = true;
            } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = Some(b.trim().to_string());
            }
        }

        // Skip bare entries
        if is_bare {
            continue;
        }

        let Some(path) = path else {
            continue;
        };

        // Detached HEAD → no branch
        if is_detached {
            branch = None;
        }

        let is_main = !found_main;
        found_main = true;

        result.push(WorktreeInfo {
            path,
            branch,
            is_main,
        });
    }

    result
}

/// Count the number of `|` filter operators inside `{{ }}` expressions in a template.
pub fn filter_count(template: &str) -> usize {
    let mut count = 0;
    let mut rest = template;

    while let Some(start) = rest.find("{{") {
        let after_open = &rest[start + 2..];
        let Some(end) = after_open.find("}}") else {
            break;
        };
        let expr = &after_open[..end];
        count += expr.chars().filter(|&c| c == '|').count();
        rest = &after_open[end + 2..];
    }

    count
}

/// Match candidate layouts against existing worktrees to determine which layout is in use.
///
/// - Filters candidates by bare compatibility (`layout.needs_bare()` must match `is_bare`).
/// - Only considers non-main worktrees with branches for matching.
/// - Renders each template with each worktree's branch and compares paths.
/// - Returns `Detected` if exactly one layout has matches.
/// - Uses specificity tiebreaker (fewer `|` filter operators) when multiple layouts
///   produce identical paths.
/// - Returns `Ambiguous` if different layouts match different worktrees.
/// - Returns `NoMatch` if nothing matched.
pub fn match_templates(
    worktrees: &[WorktreeInfo],
    project_root: &Path,
    is_bare: bool,
    candidates: &[Layout],
) -> DetectionResult {
    // Filter candidates by bare compatibility
    let compatible_candidates: Vec<&Layout> = candidates
        .iter()
        .filter(|l| l.needs_bare() == is_bare)
        .collect();

    if compatible_candidates.is_empty() {
        return DetectionResult::NoMatch;
    }

    // Only consider non-main worktrees with branches
    let linked_worktrees: Vec<&WorktreeInfo> = worktrees
        .iter()
        .filter(|w| !w.is_main && w.branch.is_some())
        .collect();

    if linked_worktrees.is_empty() {
        return DetectionResult::NoWorktrees;
    }

    // For each candidate, collect which worktrees it matches
    let mut layout_matches: Vec<(&Layout, Vec<&WorktreeInfo>)> = Vec::new();

    for layout in &compatible_candidates {
        let mut matched = Vec::new();
        for wt in &linked_worktrees {
            let branch = wt.branch.as_deref().unwrap(); // safe: filtered above
            let ctx = build_template_context(project_root, branch);
            if let Ok(rendered) = render(&layout.template, &ctx) {
                if let Ok(expected_path) = resolve_path(&rendered, project_root) {
                    if expected_path == wt.path {
                        matched.push(*wt);
                    }
                }
            }
        }
        if !matched.is_empty() {
            layout_matches.push((layout, matched));
        }
    }

    if layout_matches.is_empty() {
        return DetectionResult::NoMatch;
    }

    if layout_matches.len() == 1 {
        return DetectionResult::Detected((*layout_matches[0].0).clone());
    }

    // Multiple layouts matched — check if they all matched the same worktrees
    let first_paths: Vec<PathBuf> = layout_matches[0].1.iter().map(|w| w.path.clone()).collect();

    let all_same = layout_matches.iter().all(|(_, wts)| {
        let paths: Vec<PathBuf> = wts.iter().map(|w| w.path.clone()).collect();
        paths == first_paths
    });

    if all_same {
        // Tiebreaker: prefer the layout with fewer filter operators (more specific)
        layout_matches.sort_by_key(|(layout, _)| filter_count(&layout.template));
        return DetectionResult::Detected((*layout_matches[0].0).clone());
    }

    // Different layouts match different worktrees
    DetectionResult::Ambiguous
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::layout::BuiltinLayout;

    // ── parse_worktree_list tests ────────────────────────────────────────────

    #[test]
    fn test_parse_worktree_list_basic() {
        let input = "\
worktree /home/user/myproject
HEAD abc123
branch refs/heads/main

worktree /home/user/myproject.develop
HEAD def456
branch refs/heads/develop
";
        let worktrees = parse_worktree_list(input);
        assert_eq!(worktrees.len(), 2);

        let main = &worktrees[0];
        assert_eq!(main.path, PathBuf::from("/home/user/myproject"));
        assert_eq!(main.branch, Some("main".to_string()));
        assert!(main.is_main);

        let dev = &worktrees[1];
        assert_eq!(dev.path, PathBuf::from("/home/user/myproject.develop"));
        assert_eq!(dev.branch, Some("develop".to_string()));
        assert!(!dev.is_main);
    }

    #[test]
    fn test_parse_worktree_list_detached_head() {
        let input = "\
worktree /home/user/myproject
HEAD abc123
branch refs/heads/main

worktree /home/user/myproject.detached
HEAD deadbeef
detached
";
        let worktrees = parse_worktree_list(input);
        assert_eq!(worktrees.len(), 2);

        let detached = &worktrees[1];
        assert_eq!(
            detached.path,
            PathBuf::from("/home/user/myproject.detached")
        );
        assert_eq!(detached.branch, None);
        assert!(!detached.is_main);
    }

    #[test]
    fn test_parse_worktree_list_bare_entry_skipped() {
        let input = "\
worktree /home/user/myproject.git
HEAD abc123
bare

worktree /home/user/myproject.develop
HEAD def456
branch refs/heads/develop
";
        let worktrees = parse_worktree_list(input);
        // Bare entry is skipped; develop is the first non-bare → is_main = true
        assert_eq!(worktrees.len(), 1);
        assert_eq!(
            worktrees[0].path,
            PathBuf::from("/home/user/myproject.develop")
        );
        assert!(worktrees[0].is_main);
    }

    // ── filter_count tests ───────────────────────────────────────────────────

    #[test]
    fn test_filter_count() {
        assert_eq!(filter_count("{{ branch }}"), 0);
        assert_eq!(filter_count("{{ branch | sanitize }}"), 1);
        assert_eq!(filter_count("{{ branch | repo | sanitize }}"), 2);
        assert_eq!(
            filter_count("{{ repo_path }}/{{ branch | sanitize }}"),
            1 // only inside {{ }}, not counting the outer /
        );
        assert_eq!(filter_count("no templates here"), 0);
    }

    // ── match_templates tests ────────────────────────────────────────────────

    #[test]
    fn test_match_templates_sibling_detected() {
        // project_root = /home/user/myproject (not bare)
        // linked worktree at /home/user/myproject.develop (sibling layout)
        let project_root = PathBuf::from("/home/user/myproject");
        let worktrees = vec![
            WorktreeInfo {
                path: project_root.clone(),
                branch: Some("main".to_string()),
                is_main: true,
            },
            WorktreeInfo {
                path: PathBuf::from("/home/user/myproject.develop"),
                branch: Some("develop".to_string()),
                is_main: false,
            },
        ];

        let candidates: Vec<Layout> = BuiltinLayout::all().iter().map(|b| b.to_layout()).collect();

        let result = match_templates(&worktrees, &project_root, false, &candidates);
        match result {
            DetectionResult::Detected(layout) => {
                assert_eq!(layout.name, "sibling");
            }
            other => panic!("Expected Detected(sibling), got {other:?}"),
        }
    }

    #[test]
    fn test_match_templates_contained_detected() {
        // project_root = /home/user/myproject (bare)
        // linked worktree at /home/user/myproject/develop (contained layout)
        let project_root = PathBuf::from("/home/user/myproject");
        let worktrees = vec![
            WorktreeInfo {
                path: project_root.clone(),
                branch: Some("main".to_string()),
                is_main: true,
            },
            WorktreeInfo {
                path: PathBuf::from("/home/user/myproject/develop"),
                branch: Some("develop".to_string()),
                is_main: false,
            },
        ];

        let candidates: Vec<Layout> = BuiltinLayout::all().iter().map(|b| b.to_layout()).collect();

        let result = match_templates(&worktrees, &project_root, true, &candidates);
        match result {
            DetectionResult::Detected(layout) => {
                assert_eq!(layout.name, "contained");
            }
            other => panic!("Expected Detected(contained), got {other:?}"),
        }
    }

    #[test]
    fn test_match_templates_no_linked_worktrees_returns_no_match() {
        // Only main worktree — nothing to match against
        let project_root = PathBuf::from("/home/user/myproject");
        let worktrees = vec![WorktreeInfo {
            path: project_root.clone(),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        let candidates: Vec<Layout> = BuiltinLayout::all().iter().map(|b| b.to_layout()).collect();

        let result = match_templates(&worktrees, &project_root, false, &candidates);
        assert!(
            matches!(result, DetectionResult::NoWorktrees),
            "Expected NoWorktrees, got {result:?}"
        );
    }

    #[test]
    fn test_match_templates_contained_tiebreaker_prefers_fewer_filters() {
        // Both `contained` ({{ repo_path }}/{{ branch }}, 0 filters) and
        // `contained-flat` ({{ repo_path }}/{{ branch | sanitize }}, 1 filter)
        // would match a worktree at /home/user/myproject/develop because
        // "develop" has no slashes so sanitize is a no-op.
        // The tiebreaker should pick contained (fewer filters = 0).
        let project_root = PathBuf::from("/home/user/myproject");
        let worktrees = vec![
            WorktreeInfo {
                path: project_root.clone(),
                branch: Some("main".to_string()),
                is_main: true,
            },
            WorktreeInfo {
                path: PathBuf::from("/home/user/myproject/develop"),
                branch: Some("develop".to_string()),
                is_main: false,
            },
        ];

        // Only test the two bare-compatible layouts that could conflict
        let candidates = vec![
            BuiltinLayout::Contained.to_layout(),
            BuiltinLayout::ContainedFlat.to_layout(),
        ];

        let result = match_templates(&worktrees, &project_root, true, &candidates);
        match result {
            DetectionResult::Detected(layout) => {
                assert_eq!(
                    layout.name, "contained",
                    "Tiebreaker should prefer layout with fewer filters"
                );
            }
            other => panic!("Expected Detected(contained), got {other:?}"),
        }
    }
}
