//! Transform plan builder.
//!
//! Given a source `LayoutState` and a target `LayoutState`, `classify_worktrees`
//! determines which worktrees need to move and `build_plan` sequences the
//! discrete operations to avoid path conflicts.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::state::{ClassifiedWorktree, LayoutState, WorktreeDisposition};

// ── TransformOp ──────────────────────────────────────────────────────────

/// A discrete, atomic operation in a layout transform.
#[derive(Debug, Clone)]
pub enum TransformOp {
    /// Stash uncommitted changes before moving a worktree.
    StashChanges {
        branch: String,
        worktree_path: PathBuf,
    },
    /// Move an entire worktree directory from one path to another.
    MoveWorktree {
        branch: String,
        from: PathBuf,
        to: PathBuf,
    },
    /// Relocate the `.git` directory.
    MoveGitDir { from: PathBuf, to: PathBuf },
    /// Flip `core.bare` in git config.
    SetBare(bool),
    /// Register a worktree path in bare-mode git internals.
    RegisterWorktree { branch: String, path: PathBuf },
    /// Unregister a worktree that was tracked in bare-mode git internals.
    UnregisterWorktree { branch: String },
    /// Move the default branch working tree from a subdirectory into the project
    /// root (bare/contained -> non-bare/sibling transition).
    CollapseIntoRoot {
        worktree_path: PathBuf,
        root_path: PathBuf,
    },
    /// Move the default branch working tree from the project root into a
    /// subdirectory (non-bare/sibling -> bare/contained transition).
    NestFromRoot {
        root_path: PathBuf,
        subdir_path: PathBuf,
    },
    /// Initialize worktree index tracking (needed after bare -> non-bare).
    InitWorktreeIndex { path: PathBuf },
    /// Create a directory that must exist before subsequent ops.
    CreateDirectory { path: PathBuf },
    /// Re-apply stashed changes after a worktree has been moved.
    PopStash {
        branch: String,
        worktree_path: PathBuf,
    },
    /// Final integrity check — verify all worktree paths are valid.
    ValidateIntegrity,
}

// ── TransformPlan ────────────────────────────────────────────────────────

/// A sequenced list of operations that transforms one layout into another.
#[derive(Debug)]
pub struct TransformPlan {
    /// Operations to execute, in order.
    pub ops: Vec<TransformOp>,
    /// Worktrees that were skipped (non-conforming, not included).
    pub skipped: Vec<ClassifiedWorktree>,
    /// Human-readable summary of the plan.
    pub description: String,
}

// ── Helpers ──────────────────────────────────────────────────────────────

/// Compare two paths for equivalence, handling macOS `/tmp` -> `/private/tmp`
/// symlinks and other canonicalization differences.
fn paths_equivalent(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    // Try canonicalization for existing paths; fall back to plain comparison.
    let canon_a = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let canon_b = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    canon_a == canon_b
}

/// Returns `true` if `child` is inside `parent` (or equal to it).
fn is_inside(child: &Path, parent: &Path) -> bool {
    child.starts_with(parent)
}

// ── classify_worktrees ───────────────────────────────────────────────────

/// Returns `true` if the worktree path looks like it was placed by a layout
/// template relative to the project root — either inside the root or a sibling
/// (shares the same parent directory).
fn is_source_conforming(worktree_path: &Path, project_root: &Path) -> bool {
    // Inside the project root (contained/bare layouts)
    if worktree_path.starts_with(project_root) {
        return true;
    }
    // Sibling of the project root (sibling layout)
    if let (Some(wt_parent), Some(root_parent)) = (worktree_path.parent(), project_root.parent()) {
        if wt_parent == root_parent {
            return true;
        }
    }
    false
}

/// Classify each worktree by comparing source and target positions.
///
/// - Default branch -> `DefaultBranch`
/// - Current path == target path -> `Conforming` (already in place)
/// - Current path matches source layout (near project root) -> `Conforming`
///   (standard worktree that will be relocated)
/// - Current path differs AND (branch in `include` OR `include_all`)
///   -> `Conforming` (user-opted-in relocation)
/// - Otherwise -> `NonConforming` (skipped)
pub fn classify_worktrees(
    source: &LayoutState,
    target: &LayoutState,
    include: &[String],
    include_all: bool,
) -> Vec<ClassifiedWorktree> {
    source
        .worktrees
        .iter()
        .zip(target.worktrees.iter())
        .map(|(src_wt, tgt_wt)| {
            let disposition = if src_wt.is_default {
                WorktreeDisposition::DefaultBranch
            } else if paths_equivalent(&src_wt.path, &tgt_wt.path)
                || include_all
                || include.contains(&src_wt.branch)
                || is_source_conforming(&src_wt.path, &source.project_root)
            {
                WorktreeDisposition::Conforming
            } else {
                WorktreeDisposition::NonConforming
            };

            ClassifiedWorktree {
                branch: src_wt.branch.clone(),
                current_path: src_wt.path.clone(),
                target_path: tgt_wt.path.clone(),
                disposition,
            }
        })
        .collect()
}

// ── build_plan ───────────────────────────────────────────────────────────

/// Build a sequenced transform plan from classified worktrees.
///
/// The sequencing avoids path conflicts:
/// 1. Vacate ops — move worktrees OUT of soon-to-be-occupied paths first
/// 2. Default branch collapse/nest
/// 3. MoveGitDir
/// 4. SetBare / InitWorktreeIndex
/// 5. Register/Unregister worktree for bare transitions
/// 6. Regular move ops
/// 7. ValidateIntegrity
pub fn build_plan(
    source: &LayoutState,
    target: &LayoutState,
    classified: &[ClassifiedWorktree],
    _dry_run: bool,
) -> Result<TransformPlan> {
    let mut vacate_ops: Vec<TransformOp> = Vec::new();
    let mut regular_ops: Vec<TransformOp> = Vec::new();
    let mut skipped: Vec<ClassifiedWorktree> = Vec::new();

    // ── 1. Collect worktree moves, split into vacate vs regular ──────────

    for cw in classified {
        match cw.disposition {
            WorktreeDisposition::NonConforming => {
                skipped.push(cw.clone());
            }
            WorktreeDisposition::DefaultBranch => {
                // Handled separately below
            }
            WorktreeDisposition::Conforming => {
                if !paths_equivalent(&cw.current_path, &cw.target_path) {
                    let op = TransformOp::MoveWorktree {
                        branch: cw.branch.clone(),
                        from: cw.current_path.clone(),
                        to: cw.target_path.clone(),
                    };

                    // A worktree needs early vacating if it currently lives
                    // INSIDE the project root and its target is OUTSIDE (or at
                    // a different location). This handles contained -> sibling
                    // where worktrees must leave the wrapper before the default
                    // branch collapses into the root.
                    let currently_inside = is_inside(&cw.current_path, &source.project_root);
                    let target_outside = !is_inside(&cw.target_path, &source.project_root);

                    if currently_inside && target_outside {
                        vacate_ops.push(op);
                    } else {
                        regular_ops.push(op);
                    }
                }
                // else: already in place, no move needed
            }
        }
    }

    // ── 2. Determine default branch handling ─────────────────────────────

    let default_branch_op = {
        let default_cw = classified
            .iter()
            .find(|cw| cw.disposition == WorktreeDisposition::DefaultBranch);

        default_cw.and_then(|cw| {
            if paths_equivalent(&cw.current_path, &cw.target_path) {
                return None;
            }

            let current_is_root = paths_equivalent(&cw.current_path, &source.project_root);
            let target_is_root = paths_equivalent(&cw.target_path, &source.project_root);

            if current_is_root && !target_is_root {
                // Root -> subdirectory: nest
                Some(TransformOp::NestFromRoot {
                    root_path: cw.current_path.clone(),
                    subdir_path: cw.target_path.clone(),
                })
            } else if !current_is_root && target_is_root {
                // Subdirectory -> root: collapse
                Some(TransformOp::CollapseIntoRoot {
                    worktree_path: cw.current_path.clone(),
                    root_path: cw.target_path.clone(),
                })
            } else if !current_is_root && !target_is_root {
                // Subdirectory -> different subdirectory: treat as a move
                regular_ops.push(TransformOp::MoveWorktree {
                    branch: cw.branch.clone(),
                    from: cw.current_path.clone(),
                    to: cw.target_path.clone(),
                });
                None
            } else {
                // Root -> root (shouldn't happen if paths differ, but be safe)
                None
            }
        })
    };

    // ── 3. Git dir and bare flag changes ─────────────────────────────────

    let git_dir_changed = !paths_equivalent(&source.git_dir, &target.git_dir);
    let bare_changed = source.is_bare != target.is_bare;

    // ── 4. Sequence everything ───────────────────────────────────────────

    let mut ops: Vec<TransformOp> = Vec::new();

    // a. Vacate ops first
    ops.extend(vacate_ops);

    // b. Default branch collapse/nest
    if let Some(op) = default_branch_op {
        ops.push(op);
    }

    // c. MoveGitDir
    if git_dir_changed {
        ops.push(TransformOp::MoveGitDir {
            from: source.git_dir.clone(),
            to: target.git_dir.clone(),
        });
    }

    // d. SetBare
    if bare_changed {
        ops.push(TransformOp::SetBare(target.is_bare));
    }

    // e. InitWorktreeIndex (if going from bare to non-bare)
    if bare_changed && !target.is_bare {
        ops.push(TransformOp::InitWorktreeIndex {
            path: target.git_dir.clone(),
        });
    }

    // f. RegisterWorktree (if going bare, register the default branch)
    if bare_changed && target.is_bare {
        if let Some(cw) = classified
            .iter()
            .find(|cw| cw.disposition == WorktreeDisposition::DefaultBranch)
        {
            ops.push(TransformOp::RegisterWorktree {
                branch: cw.branch.clone(),
                path: cw.target_path.clone(),
            });
        }
    }

    // g. UnregisterWorktree (if going non-bare, unregister the default branch)
    if bare_changed && !target.is_bare {
        if let Some(cw) = classified
            .iter()
            .find(|cw| cw.disposition == WorktreeDisposition::DefaultBranch)
        {
            ops.push(TransformOp::UnregisterWorktree {
                branch: cw.branch.clone(),
            });
        }
    }

    // h. Regular move ops
    ops.extend(regular_ops);

    // i. ValidateIntegrity (always)
    ops.push(TransformOp::ValidateIntegrity);

    // ── Build description ────────────────────────────────────────────────

    let move_count = ops
        .iter()
        .filter(|op| matches!(op, TransformOp::MoveWorktree { .. }))
        .count();
    let has_collapse = ops
        .iter()
        .any(|op| matches!(op, TransformOp::CollapseIntoRoot { .. }));
    let has_nest = ops
        .iter()
        .any(|op| matches!(op, TransformOp::NestFromRoot { .. }));

    let description = if ops.len() == 1 {
        "No changes needed — layout already matches target.".to_string()
    } else {
        let mut parts = Vec::new();
        if has_collapse {
            parts.push("collapse default branch into root".to_string());
        }
        if has_nest {
            parts.push("nest default branch into subdirectory".to_string());
        }
        if git_dir_changed {
            parts.push("relocate .git directory".to_string());
        }
        if bare_changed {
            parts.push(format!(
                "switch bare flag to {}",
                if target.is_bare { "true" } else { "false" }
            ));
        }
        if move_count > 0 {
            parts.push(format!(
                "move {} worktree{}",
                move_count,
                if move_count == 1 { "" } else { "s" }
            ));
        }
        if !skipped.is_empty() {
            parts.push(format!(
                "skip {} non-conforming worktree{}",
                skipped.len(),
                if skipped.len() == 1 { "" } else { "s" }
            ));
        }
        format!("Transform: {}", parts.join(", "))
    };

    Ok(TransformPlan {
        ops,
        skipped,
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(
        git_dir: &str,
        is_bare: bool,
        default_branch: &str,
        project_root: &str,
        worktrees: Vec<(&str, &str, bool)>,
    ) -> LayoutState {
        LayoutState {
            git_dir: PathBuf::from(git_dir),
            is_bare,
            default_branch: default_branch.to_string(),
            project_root: PathBuf::from(project_root),
            worktrees: worktrees
                .into_iter()
                .map(
                    |(branch, path, is_default)| super::super::state::WorktreeEntry {
                        branch: branch.to_string(),
                        path: PathBuf::from(path),
                        is_default,
                    },
                )
                .collect(),
        }
    }

    #[test]
    fn test_same_layout_only_validates() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let target = source.clone();
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        assert_eq!(plan.ops.len(), 1);
        assert!(matches!(plan.ops[0], TransformOp::ValidateIntegrity));
    }

    #[test]
    fn test_contained_to_contained_classic() {
        // bare -> non-bare, .git moves into default branch subdir
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let target = make_state(
            "/repo/main/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();

        let has_move_git = plan
            .ops
            .iter()
            .any(|op| matches!(op, TransformOp::MoveGitDir { .. }));
        let has_set_bare = plan
            .ops
            .iter()
            .any(|op| matches!(op, TransformOp::SetBare(false)));
        let has_unregister = plan
            .ops
            .iter()
            .any(|op| matches!(op, TransformOp::UnregisterWorktree { .. }));
        assert!(has_move_git, "Should move .git");
        assert!(has_set_bare, "Should flip bare");
        assert!(has_unregister, "Should unregister main worktree");

        // dev should NOT move (already at correct path)
        let has_move_dev = plan.ops.iter().any(
            |op| matches!(op, TransformOp::MoveWorktree { ref branch, .. } if branch == "dev"),
        );
        assert!(!has_move_dev, "dev should not move");
    }

    #[test]
    fn test_contained_to_sibling_vacates_first() {
        // Worktrees inside wrapper must vacate before default branch collapses
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let target = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo", true), ("dev", "/repo.dev", false)],
        );
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();

        // dev move should come BEFORE collapse
        let dev_move_idx = plan.ops.iter().position(
            |op| matches!(op, TransformOp::MoveWorktree { ref branch, .. } if branch == "dev"),
        );
        let collapse_idx = plan
            .ops
            .iter()
            .position(|op| matches!(op, TransformOp::CollapseIntoRoot { .. }));
        assert!(dev_move_idx.is_some(), "Should have dev move");
        assert!(collapse_idx.is_some(), "Should have collapse");
        assert!(
            dev_move_idx.unwrap() < collapse_idx.unwrap(),
            "dev should vacate before collapse"
        );
    }

    #[test]
    fn test_sibling_to_contained_classic() {
        // non-bare -> non-bare but .git moves, default branch nests
        let source = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo", true), ("dev", "/repo.dev", false)],
        );
        let target = make_state(
            "/repo/main/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();

        let has_nest = plan
            .ops
            .iter()
            .any(|op| matches!(op, TransformOp::NestFromRoot { .. }));
        let has_move_git = plan
            .ops
            .iter()
            .any(|op| matches!(op, TransformOp::MoveGitDir { .. }));
        assert!(has_nest, "Should nest default branch");
        assert!(has_move_git, "Should move .git");
    }

    #[test]
    fn test_non_conforming_worktrees_skipped() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![
                ("main", "/repo/main", true),
                ("dev", "/repo/dev", false),
                ("exp", "/custom/path/exp", false),
            ],
        );
        let target = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![
                ("main", "/repo", true),
                ("dev", "/repo.dev", false),
                ("exp", "/repo.exp", false),
            ],
        );
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].branch, "exp");
    }

    #[test]
    fn test_include_overrides_non_conforming() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("exp", "/custom/exp", false)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("exp", "/repo/exp", false)],
        );
        let classified = classify_worktrees(&source, &target, &["exp".to_string()], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        assert_eq!(plan.skipped.len(), 0);
    }

    #[test]
    fn test_include_all_overrides_all_non_conforming() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![
                ("main", "/repo/main", true),
                ("a", "/custom/a", false),
                ("b", "/other/b", false),
            ],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![
                ("main", "/repo/main", true),
                ("a", "/repo/a", false),
                ("b", "/repo/b", false),
            ],
        );
        let classified = classify_worktrees(&source, &target, &[], true);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        assert_eq!(plan.skipped.len(), 0);
    }
}
