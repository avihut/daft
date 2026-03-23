//! Worktree gathering and template matching for layout detection.
//!
//! This module provides two main capabilities:
//! 1. Parsing `git worktree list --porcelain` output into `WorktreeInfo` structs.
//! 2. Matching a set of candidate layouts against existing worktrees to detect
//!    which layout is in use.

use std::path::{Path, PathBuf};

use super::{BuiltinLayout, Layout};
use crate::core::global_config::GlobalConfig;
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

/// Detect the layout structure for a repo that may have only a single worktree
/// (i.e. the template-matching path found no linked worktrees).
///
/// This is the public entry point. It checks whether `project_root/.worktrees/`
/// exists on the filesystem and delegates to [`detect_structure_with_worktrees_dir`].
pub fn detect_structure(
    git_common_dir: &Path,
    project_root: &Path,
    is_bare: bool,
    worktrees: &[WorktreeInfo],
) -> DetectionResult {
    let has_worktrees_dir = project_root.join(".worktrees").is_dir();
    detect_structure_with_worktrees_dir(
        git_common_dir,
        project_root,
        is_bare,
        worktrees,
        has_worktrees_dir,
    )
}

/// Detect the layout structure for a repo that may have only a single worktree.
///
/// Accepts `has_worktrees_dir` explicitly to enable deterministic unit testing
/// without touching the filesystem.
///
/// Detection order:
/// 1. If no main worktree is found → `NoWorktrees`.
/// 2. **ContainedClassic**: non-bare, `git_common_dir`'s parent is a direct
///    child of `project_root`, and that child's name matches the main branch.
/// 3. **Contained** (bare): bare repo, main worktree path ==
///    `project_root.join(branch)`.
/// 4. **Nested**: `project_root/.worktrees/` directory exists.
/// 5. Otherwise → `NoWorktrees` (plain git clone, no layout signal).
pub fn detect_structure_with_worktrees_dir(
    git_common_dir: &Path,
    project_root: &Path,
    is_bare: bool,
    worktrees: &[WorktreeInfo],
    has_worktrees_dir: bool,
) -> DetectionResult {
    // Step 1: find the main worktree.
    let main = match worktrees.iter().find(|w| w.is_main) {
        Some(w) => w,
        None => return DetectionResult::NoWorktrees,
    };

    // Step 2: ContainedClassic — non-bare, .git lives inside a branch subdir.
    if !is_bare {
        if let Some(branch) = &main.branch {
            // git_common_dir is typically <project_root>/<branch>/.git
            // Its parent is the branch subdir.
            if let Some(git_parent) = git_common_dir.parent() {
                // The git parent must be a direct child of project_root.
                if let Ok(relative) = git_parent.strip_prefix(project_root) {
                    let components: Vec<_> = relative.components().collect();
                    if components.len() == 1 {
                        let subdir_name = components[0].as_os_str().to_string_lossy();
                        if subdir_name == branch.as_str() {
                            return DetectionResult::Detected(
                                BuiltinLayout::ContainedClassic.to_layout(),
                            );
                        }
                    }
                }
            }
        }
    }

    // Step 3: Contained (bare) — bare repo, main worktree is a child of project_root
    // named after the branch.
    if is_bare {
        if let Some(branch) = &main.branch {
            let expected = project_root.join(branch.as_str());
            if main.path == expected {
                return DetectionResult::Detected(BuiltinLayout::Contained.to_layout());
            }
        }
    }

    // Step 4: Nested — .worktrees/ directory exists inside project_root.
    if has_worktrees_dir {
        return DetectionResult::Detected(BuiltinLayout::Nested.to_layout());
    }

    // Step 5: No structural signals — plain git clone.
    DetectionResult::NoWorktrees
}

/// Detect the layout for a repository given the raw porcelain output.
///
/// This is the testable entry point — it accepts already-fetched porcelain
/// text instead of running `git` live.
///
/// Detection order:
/// 1. Parse worktrees from porcelain. If no linked worktrees with branches
///    exist → `NoWorktrees`.
/// 2. Build candidate list: builtins + custom layouts from `global_config`.
/// 3. Try template matching against linked worktrees.
/// 4. If template matching doesn't detect → fall back to structural detection.
pub fn detect_layout_from_porcelain(
    porcelain: &str,
    git_common_dir: &Path,
    project_root: &Path,
    is_bare: bool,
    global_config: &GlobalConfig,
) -> DetectionResult {
    let worktrees = parse_worktree_list(porcelain);

    // If there are no linked worktrees with branches, skip template matching.
    let has_linked = worktrees.iter().any(|w| !w.is_main && w.branch.is_some());
    if !has_linked {
        // No linked worktrees — fall back to structural detection.
        return detect_structure(git_common_dir, project_root, is_bare, &worktrees);
    }

    // Build candidate list: builtins first, then custom layouts.
    let mut candidates: Vec<Layout> = BuiltinLayout::all().iter().map(|b| b.to_layout()).collect();
    candidates.extend(global_config.custom_layouts());

    // Try template matching.
    let template_result = match_templates(&worktrees, project_root, is_bare, &candidates);

    match template_result {
        DetectionResult::Detected(_) => template_result,
        _ => detect_structure(git_common_dir, project_root, is_bare, &worktrees),
    }
}

/// Detect the layout for the repository rooted at `git_common_dir` by
/// running live git commands.
///
/// Detection order:
/// 1. Read `core.bare` via git config.
/// 2. Derive `project_root` as `git_common_dir.parent()`.
/// 3. Fetch the worktree list via `git worktree list --porcelain`.
/// 4. Try `detect_layout_from_porcelain` with the direct parent as
///    `project_root`.
/// 5. If no match and non-bare, retry with the grandparent as
///    `project_root` (contained-classic case where `.git` lives at
///    `wrapper/<branch>/.git`).
pub fn detect_layout(git_common_dir: &Path, global_config: &GlobalConfig) -> DetectionResult {
    use crate::git::GitCommand;

    let git = GitCommand::new(true);

    let is_bare = git
        .config_get("core.bare")
        .ok()
        .flatten()
        .map(|v| v == "true")
        .unwrap_or(false);

    let porcelain = match git.worktree_list_porcelain() {
        Ok(p) => p,
        Err(_) => return DetectionResult::NoWorktrees,
    };

    // Primary project_root: direct parent of git_common_dir.
    let project_root = match git_common_dir.parent() {
        Some(p) => p.to_path_buf(),
        None => return DetectionResult::NoWorktrees,
    };

    let result = detect_layout_from_porcelain(
        &porcelain,
        git_common_dir,
        &project_root,
        is_bare,
        global_config,
    );

    // For non-bare repos, retry with the grandparent when no match was found.
    // This handles the contained-classic case where .git is at
    // <wrapper>/<branch>/.git, making the effective project_root <wrapper>.
    if !is_bare
        && matches!(
            result,
            DetectionResult::NoWorktrees | DetectionResult::NoMatch
        )
    {
        if let Some(grandparent) = project_root.parent() {
            let retry = detect_layout_from_porcelain(
                &porcelain,
                git_common_dir,
                grandparent,
                is_bare,
                global_config,
            );
            if matches!(retry, DetectionResult::Detected(_)) {
                return retry;
            }
        }
    }

    result
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

    // ── detect_structure tests ───────────────────────────────────────────────

    #[test]
    fn test_structural_detect_contained_bare_with_child_worktree() {
        // Bare repo at /home/user/myproject, main worktree is at
        // /home/user/myproject/main — matches Contained layout.
        let project_root = PathBuf::from("/home/user/myproject");
        let git_common_dir = project_root.join("main").join(".git"); // irrelevant for bare path
        let worktrees = vec![WorktreeInfo {
            path: project_root.join("main"),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        let result = detect_structure_with_worktrees_dir(
            &git_common_dir,
            &project_root,
            true, // is_bare
            &worktrees,
            false, // no .worktrees/ dir
        );

        match result {
            DetectionResult::Detected(layout) => {
                assert_eq!(layout.name, "contained");
            }
            other => panic!("Expected Detected(contained), got {other:?}"),
        }
    }

    #[test]
    fn test_structural_detect_contained_classic() {
        // Non-bare repo. The .git directory lives at /home/user/myproject/main/.git,
        // so git_common_dir = /home/user/myproject/main/.git.
        // Its parent /home/user/myproject/main is a direct child of project_root,
        // and the subdir name "main" matches the main worktree's branch → ContainedClassic.
        let project_root = PathBuf::from("/home/user/myproject");
        let git_common_dir = project_root.join("main").join(".git");
        let worktrees = vec![WorktreeInfo {
            path: project_root.join("main"),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        let result = detect_structure_with_worktrees_dir(
            &git_common_dir,
            &project_root,
            false, // not bare
            &worktrees,
            false, // no .worktrees/ dir
        );

        match result {
            DetectionResult::Detected(layout) => {
                assert_eq!(layout.name, "contained-classic");
            }
            other => panic!("Expected Detected(contained-classic), got {other:?}"),
        }
    }

    #[test]
    fn test_structural_detect_nested_has_worktrees_dir() {
        // Non-bare repo, main worktree IS the project root (plain clone).
        // .worktrees/ directory exists → Nested layout.
        let project_root = PathBuf::from("/home/user/myproject");
        let git_common_dir = project_root.join(".git");
        let worktrees = vec![WorktreeInfo {
            path: project_root.clone(),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        let result = detect_structure_with_worktrees_dir(
            &git_common_dir,
            &project_root,
            false, // not bare
            &worktrees,
            true, // .worktrees/ dir exists
        );

        match result {
            DetectionResult::Detected(layout) => {
                assert_eq!(layout.name, "nested");
            }
            other => panic!("Expected Detected(nested), got {other:?}"),
        }
    }

    #[test]
    fn test_structural_detect_plain_clone_no_detection() {
        // Non-bare repo, main worktree IS the project root, no .worktrees/ dir.
        // No structural signals → NoWorktrees (plain git clone).
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let project_root = dir.path().to_path_buf();
        let git_common_dir = project_root.join(".git");
        let worktrees = vec![WorktreeInfo {
            path: project_root.clone(),
            branch: Some("main".to_string()),
            is_main: true,
        }];

        // Use detect_structure directly (no .worktrees/ directory on disk)
        let result = detect_structure(&git_common_dir, &project_root, false, &worktrees);

        assert!(
            matches!(result, DetectionResult::NoWorktrees),
            "Expected NoWorktrees for plain git clone, got {result:?}"
        );
    }

    // ── detect_layout_from_porcelain tests ───────────────────────────────────

    #[test]
    fn test_detect_layout_sibling_from_porcelain() {
        let porcelain = "worktree /home/user/myproject\nbranch refs/heads/main\n\nworktree /home/user/myproject.develop\nbranch refs/heads/develop\n\n";
        let result = detect_layout_from_porcelain(
            porcelain,
            Path::new("/home/user/myproject/.git"),
            Path::new("/home/user/myproject"),
            false,
            &GlobalConfig::default(),
        );
        match result {
            DetectionResult::Detected(layout) => assert_eq!(layout.name, "sibling"),
            other => panic!("Expected Detected(sibling), got {:?}", other),
        }
    }

    #[test]
    fn test_detect_layout_plain_clone_no_worktrees() {
        let porcelain = "worktree /home/user/myproject\nbranch refs/heads/main\n\n";
        let result = detect_layout_from_porcelain(
            porcelain,
            Path::new("/home/user/myproject/.git"),
            Path::new("/home/user/myproject"),
            false,
            &GlobalConfig::default(),
        );
        assert!(matches!(result, DetectionResult::NoWorktrees));
    }
}
