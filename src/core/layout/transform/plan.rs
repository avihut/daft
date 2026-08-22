//! Transform plan builder.
//!
//! Given a source `LayoutState` and a target `LayoutState`, `classify_worktrees`
//! determines which worktrees need to move and `build_plan` sequences the
//! discrete operations to avoid path conflicts.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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
    ///
    /// `path` is where the worktree lives *when this op runs* — the
    /// registration is resolved through it, because git names registration
    /// directories after the path basename, not the branch.
    UnregisterWorktree { branch: String, path: PathBuf },
    /// Move the root worktree from a subdirectory into the project root
    /// (bare/contained -> non-bare/sibling transition).
    CollapseIntoRoot {
        branch: String,
        worktree_path: PathBuf,
        root_path: PathBuf,
    },
    /// Move the root worktree from the project root into a subdirectory
    /// (non-bare/sibling -> bare/contained transition).
    NestFromRoot {
        branch: String,
        root_path: PathBuf,
        subdir_path: PathBuf,
    },
    /// Point HEAD at `branch` and rebuild the index at `path`.
    ///
    /// Needed whenever a working tree changes role (bare <-> non-bare): the
    /// HEAD it used to resolve through is gone, and the remaining one names a
    /// different branch.
    InitWorktreeIndex { path: PathBuf, branch: String },
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
pub fn paths_equivalent(a: &Path, b: &Path) -> bool {
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
    if let (Some(wt_parent), Some(root_parent)) = (worktree_path.parent(), project_root.parent())
        && wt_parent == root_parent
    {
        return true;
    }
    false
}

/// Classify each worktree by comparing source and target positions.
///
/// - Holds the root role in either state -> `Root`
/// - Current path == target path -> `Conforming` (already in place)
/// - Current path matches source layout (near project root) -> `Conforming`
///   (standard worktree that will be relocated)
/// - Current path differs AND (branch in `include` OR `include_all`)
///   -> `Conforming` (user-opted-in relocation)
/// - Otherwise -> `NonConforming` (skipped)
///
/// The root role is checked in *both* states: a non-bare source hands it to the
/// main working tree, while a bare source has none to give and the target state
/// nominates the worktree that will become one.
pub fn classify_worktrees(
    source: &LayoutState,
    target: &LayoutState,
    include: &[String],
    include_all: bool,
) -> Vec<ClassifiedWorktree> {
    // The two vectors are built from the same source worktree list, in order —
    // `compute_target_state` preserves both length and position. The zip below
    // silently truncates if that ever stops being true.
    debug_assert_eq!(
        source.worktrees.len(),
        target.worktrees.len(),
        "source and target worktree vectors must stay index-aligned"
    );

    source
        .worktrees
        .iter()
        .zip(target.worktrees.iter())
        .map(|(src_wt, tgt_wt)| {
            let disposition = if src_wt.is_root || tgt_wt.is_root {
                WorktreeDisposition::Root
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
/// The sequencing avoids path conflicts, and orders the two registration ops so
/// that a LIFO rollback (see `execute::rollback`) replays them against
/// directories that still exist:
///
/// 1. Vacate ops — move worktrees OUT of soon-to-be-occupied paths first
/// 2. Relocate the pivot while it is still an ordinary linked worktree
/// 3. Unregister the pivot (bare -> non-bare) — early, so its reverse
///    (`RegisterWorktree`) runs *after* the collapse has been undone and the
///    worktree directory is back
/// 4. Root collapse / nest
/// 5. MoveGitDir
/// 6. SetBare, then InitWorktreeIndex / RegisterWorktree
/// 7. Regular move ops
/// 8. ValidateIntegrity
pub fn build_plan(
    source: &LayoutState,
    target: &LayoutState,
    classified: &[ClassifiedWorktree],
    _dry_run: bool,
) -> Result<TransformPlan> {
    let mut vacate_ops: Vec<TransformOp> = Vec::new();
    let mut regular_ops: Vec<TransformOp> = Vec::new();
    let mut skipped: Vec<ClassifiedWorktree> = Vec::new();

    let root_cw = classified
        .iter()
        .find(|cw| cw.disposition == WorktreeDisposition::Root);

    // A NestFromRoot sweeps *everything* sitting at the project root into the
    // subdirectory, so any worktree still inside the root has to leave first.
    let root_at_project_root =
        root_cw.is_some_and(|cw| paths_equivalent(&cw.current_path, &source.project_root));
    let root_will_nest = root_at_project_root
        && root_cw.is_some_and(|cw| !paths_equivalent(&cw.target_path, &source.project_root));

    // ── 1. Collect worktree moves, split into vacate vs regular ──────────

    for cw in classified {
        match cw.disposition {
            WorktreeDisposition::NonConforming => {
                skipped.push(cw.clone());
            }
            WorktreeDisposition::Root => {
                // Handled separately below
            }
            WorktreeDisposition::Conforming => {
                if !paths_equivalent(&cw.current_path, &cw.target_path) {
                    let op = TransformOp::MoveWorktree {
                        branch: cw.branch.clone(),
                        from: cw.current_path.clone(),
                        to: cw.target_path.clone(),
                    };

                    // A worktree needs early vacating if:
                    // 1. It currently lives INSIDE the project root and its
                    //    target is OUTSIDE — handles contained→sibling where
                    //    worktrees must leave the wrapper before collapse.
                    // 2. The root worktree will NestFromRoot (root→subdir),
                    //    which moves ALL root contents — any worktree inside
                    //    the root would get swept into the subdir.
                    let currently_inside = is_inside(&cw.current_path, &source.project_root);
                    let target_outside = !is_inside(&cw.target_path, &source.project_root);

                    if currently_inside && (target_outside || root_will_nest) {
                        vacate_ops.push(op);
                    } else {
                        regular_ops.push(op);
                    }
                }
                // else: already in place, no move needed
            }
        }
    }

    // ── 2. Determine root worktree handling ──────────────────────────────

    let mut root_op: Option<TransformOp> = None;
    let mut pivot_move: Option<TransformOp> = None;

    if let Some(cw) = root_cw
        && !paths_equivalent(&cw.current_path, &cw.target_path)
    {
        let current_is_root = paths_equivalent(&cw.current_path, &source.project_root);
        let target_is_root = paths_equivalent(&cw.target_path, &source.project_root);

        match (current_is_root, target_is_root) {
            // Root -> subdirectory: nest
            (true, false) => {
                root_op = Some(TransformOp::NestFromRoot {
                    branch: cw.branch.clone(),
                    root_path: cw.current_path.clone(),
                    subdir_path: cw.target_path.clone(),
                });
            }
            // Subdirectory -> root: collapse
            (false, true) => {
                root_op = Some(TransformOp::CollapseIntoRoot {
                    branch: cw.branch.clone(),
                    worktree_path: cw.current_path.clone(),
                    root_path: cw.target_path.clone(),
                });
            }
            // Subdirectory -> different subdirectory. Only movable while the
            // repo is bare, where the pivot is still an ordinary linked
            // worktree — `git worktree move` refuses a main working tree.
            (false, false) if source.is_bare => {
                pivot_move = Some(TransformOp::MoveWorktree {
                    branch: cw.branch.clone(),
                    from: cw.current_path.clone(),
                    to: cw.target_path.clone(),
                });
            }
            (false, false) => {
                anyhow::bail!(
                    "The main working tree for '{}' is at {} but the target layout places \
                     it at {}, and git cannot move a main working tree.\n\
                     Transform to 'sibling' first (`daft layout transform sibling`), then \
                     to the layout you want.",
                    cw.branch,
                    cw.current_path.display(),
                    cw.target_path.display()
                );
            }
            // Root -> root (shouldn't happen if paths differ, but be safe)
            (true, true) => {}
        }
    }

    // Where the pivot lives when `UnregisterWorktree` runs — after its own
    // relocation, if any.
    let unregister_path = match (&pivot_move, root_cw) {
        (Some(TransformOp::MoveWorktree { to, .. }), _) => Some(to.clone()),
        (_, Some(cw)) => Some(cw.current_path.clone()),
        _ => None,
    };

    // ── 3. Git dir and bare flag changes ─────────────────────────────────

    let git_dir_changed = !paths_equivalent(&source.git_dir, &target.git_dir);
    let bare_changed = source.is_bare != target.is_bare;

    // ── 4. Sequence everything ───────────────────────────────────────────

    let mut ops: Vec<TransformOp> = Vec::new();

    // a1. Vacate ops first
    ops.extend(vacate_ops);

    // a2. Relocate the pivot while git still allows it
    if let Some(op) = pivot_move {
        ops.push(op);
    }

    // a3. UnregisterWorktree (if going non-bare, unregister the pivot).
    //     Deliberately ahead of the collapse: rollback unwinds strictly LIFO,
    //     so the reverse RegisterWorktree must land after the reverse
    //     NestFromRoot has recreated the directory it writes `.git` into.
    if bare_changed
        && !target.is_bare
        && let Some(cw) = root_cw
        && let Some(path) = unregister_path
    {
        ops.push(TransformOp::UnregisterWorktree {
            branch: cw.branch.clone(),
            path,
        });
    }

    // b. Root collapse/nest
    if let Some(op) = root_op {
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

    // e. InitWorktreeIndex (if going from bare to non-bare).
    //    The pivot's per-worktree HEAD died with its registration, so HEAD has
    //    to be re-pointed at its branch before the index is rebuilt — otherwise
    //    the index is built against whatever the bare repo's HEAD names.
    if bare_changed && !target.is_bare {
        // `select_pivot` either names a pivot or refuses the transform for every
        // bare -> non-bare case, and `classify_worktrees` always marks the
        // target's root entry, so this is `Some` by construction. There is
        // deliberately no default-branch fallback: building the index for the
        // default branch rather than the pivot is the #859 bug itself.
        let cw = root_cw.context(
            "internal error: a bare -> non-bare plan reached index init without a root worktree",
        )?;
        ops.push(TransformOp::InitWorktreeIndex {
            path: cw.target_path.clone(),
            branch: cw.branch.clone(),
        });
    }

    // f. RegisterWorktree (if going bare, register the pivot). The op rebuilds
    //    the worktree's index as part of registering it — it was the main
    //    working tree of a non-bare repo and needs a fresh one in its new role.
    if bare_changed
        && target.is_bare
        && let Some(cw) = root_cw
    {
        ops.push(TransformOp::RegisterWorktree {
            branch: cw.branch.clone(),
            path: cw.target_path.clone(),
        });
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
            parts.push("collapse a worktree into the project root".to_string());
        }
        if has_nest {
            parts.push("nest the main working tree into a subdirectory".to_string());
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

    /// Build a layout state directly; `worktrees` are `(branch, path, is_root)`.
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
                    |(branch, path, is_root)| super::super::state::WorktreeEntry {
                        branch: branch.to_string(),
                        path: PathBuf::from(path),
                        is_root,
                    },
                )
                .collect(),
        }
    }

    fn index_of(plan: &TransformPlan, pred: impl Fn(&TransformOp) -> bool) -> Option<usize> {
        plan.ops.iter().position(pred)
    }

    fn plan_for(source: &LayoutState, target: &LayoutState) -> Result<TransformPlan> {
        let classified = classify_worktrees(source, target, &[], false);
        build_plan(source, target, &classified, false)
    }

    #[test]
    fn test_same_layout_only_validates() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", false), ("dev", "/repo/dev", false)],
        );
        let target = source.clone();
        let plan = plan_for(&source, &target).unwrap();
        assert_eq!(plan.ops.len(), 1);
        assert!(matches!(plan.ops[0], TransformOp::ValidateIntegrity));
    }

    #[test]
    fn test_contained_to_contained_classic() {
        // bare -> non-bare, .git moves into the pivot's subdir
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", false), ("dev", "/repo/dev", false)],
        );
        let target = make_state(
            "/repo/main/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let plan = plan_for(&source, &target).unwrap();

        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. })).is_some(),
            "Should move .git"
        );
        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::SetBare(false))).is_some(),
            "Should flip bare"
        );
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::UnregisterWorktree { .. }
            ))
            .is_some(),
            "Should unregister the pivot"
        );

        // dev should NOT move (already at correct path)
        assert!(
            index_of(
                &plan,
                |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if branch == "dev")
            )
            .is_none(),
            "dev should not move"
        );
    }

    #[test]
    fn test_contained_to_sibling_vacates_first() {
        // Worktrees inside wrapper must vacate before the pivot collapses
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", false), ("dev", "/repo/dev", false)],
        );
        let target = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo", true), ("dev", "/repo.dev", false)],
        );
        let plan = plan_for(&source, &target).unwrap();

        let dev_move_idx = index_of(
            &plan,
            |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if branch == "dev"),
        );
        let unregister_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::UnregisterWorktree { .. })
        });
        let collapse_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::CollapseIntoRoot { .. })
        });
        assert!(dev_move_idx.is_some(), "Should have dev move");
        assert!(collapse_idx.is_some(), "Should have collapse");
        assert!(
            dev_move_idx.unwrap() < collapse_idx.unwrap(),
            "dev should vacate before collapse"
        );
        assert!(
            unregister_idx.unwrap() < collapse_idx.unwrap(),
            "unregister must precede collapse so its rollback lands last"
        );
    }

    #[test]
    fn test_sibling_to_contained_classic() {
        // non-bare -> non-bare but .git moves, root nests
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
        let plan = plan_for(&source, &target).unwrap();

        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::NestFromRoot { .. })).is_some(),
            "Should nest the root worktree"
        );
        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. })).is_some(),
            "Should move .git"
        );
    }

    #[test]
    fn test_non_conforming_worktrees_skipped() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![
                ("main", "/repo/main", false),
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
        let plan = plan_for(&source, &target).unwrap();
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
            vec![("main", "/repo/main", false), ("exp", "/custom/exp", false)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", false), ("exp", "/repo/exp", false)],
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
                ("main", "/repo/main", false),
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
                ("main", "/repo/main", false),
                ("a", "/repo/a", false),
                ("b", "/repo/b", false),
            ],
        );
        let classified = classify_worktrees(&source, &target, &[], true);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        assert_eq!(plan.skipped.len(), 0);
    }

    // ── #859: the root role follows the main working tree ─────────────────

    #[test]
    fn test_nondefault_root_nests_instead_of_moving() {
        // Plain clone whose main working tree is on a feature branch; the
        // default branch has no worktree at all.
        let source = make_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo", true)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "master",
            "/repo",
            vec![("feature/x", "/repo/feature/x", true)],
        );
        let plan = plan_for(&source, &target).unwrap();

        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::NestFromRoot { .. })).is_some(),
            "the main working tree nests, it does not move"
        );
        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::MoveWorktree { .. })).is_none(),
            "git refuses `worktree move` on a main working tree — never plan one"
        );
        assert!(index_of(&plan, |op| matches!(op, TransformOp::SetBare(true))).is_some());
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::RegisterWorktree { branch, path }
                    if branch == "feature/x" && path == Path::new("/repo/feature/x")
            ))
            .is_some(),
            "the pivot becomes a linked worktree (which rebuilds its index)"
        );
    }

    #[test]
    fn test_nondefault_root_default_branch_worktree_is_a_regular_move() {
        // The default branch does have a worktree — just not the main one.
        let source = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("feature/x", "/repo", true), ("main", "/repo.main", false)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![
                ("feature/x", "/repo/feature/x", true),
                ("main", "/repo/main", false),
            ],
        );
        let plan = plan_for(&source, &target).unwrap();

        assert!(index_of(&plan, |op| matches!(op, TransformOp::NestFromRoot { .. })).is_some());
        assert!(
            index_of(
                &plan,
                |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if branch == "main")
            )
            .is_some(),
            "the default branch's worktree is just another linked worktree"
        );
    }

    #[test]
    fn test_nondefault_root_nested_source_vacates_before_nest() {
        let source = make_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![
                ("feature/x", "/repo", true),
                ("dev", "/repo/.worktrees/dev", false),
            ],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "master",
            "/repo",
            vec![
                ("feature/x", "/repo/feature/x", true),
                ("dev", "/repo/dev", false),
            ],
        );
        let plan = plan_for(&source, &target).unwrap();

        let dev_idx = index_of(
            &plan,
            |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if branch == "dev"),
        )
        .expect("dev should move");
        let nest_idx = index_of(&plan, |op| matches!(op, TransformOp::NestFromRoot { .. }))
            .expect("root should nest");
        assert!(
            dev_idx < nest_idx,
            "dev must leave the root before the nest sweeps it into the subdir"
        );
    }

    #[test]
    fn test_nondefault_root_to_regular_nonbare_leaves_the_root_alone() {
        // sibling -> nested: the root worktree stays put whatever branch it is
        // on; only the linked worktrees move.
        let source = make_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo", true), ("dev", "/repo.dev", false)],
        );
        let target = make_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![
                ("feature/x", "/repo", true),
                ("dev", "/repo/.worktrees/dev", false),
            ],
        );
        let plan = plan_for(&source, &target).unwrap();
        assert!(index_of(&plan, |op| matches!(op, TransformOp::SetBare(_))).is_none());
        assert!(index_of(&plan, |op| matches!(op, TransformOp::NestFromRoot { .. })).is_none());
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::CollapseIntoRoot { .. }
            ))
            .is_none()
        );
        assert!(
            index_of(
                &plan,
                |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if branch == "dev")
            )
            .is_some()
        );
    }

    #[test]
    fn test_nondefault_root_to_contained_classic_puts_git_in_its_dir() {
        let source = make_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo", true)],
        );
        let target = make_state(
            "/repo/feature/x/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo/feature/x", true)],
        );
        let plan = plan_for(&source, &target).unwrap();

        assert!(index_of(&plan, |op| matches!(op, TransformOp::NestFromRoot { .. })).is_some());
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::MoveGitDir { to, .. } if to == Path::new("/repo/feature/x/.git")
            ))
            .is_some()
        );
    }

    #[test]
    fn test_wrapper_source_collapses_to_sibling() {
        // contained-classic whose clone dir matches its branch.
        let source = make_state(
            "/repo/feature/x/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo/feature/x", true)],
        );
        let target = make_state(
            "/repo/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo", true)],
        );
        let plan = plan_for(&source, &target).unwrap();
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::CollapseIntoRoot { .. }
            ))
            .is_some()
        );
        assert!(index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. })).is_some());
    }

    #[test]
    fn test_drifted_wrapper_source_is_refused() {
        // contained-classic clone in `/repo/main` that has since switched to
        // `feature/x`: the target wants `/repo/feature/x`, but git will not
        // move a main working tree.
        let source = make_state(
            "/repo/main/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo/main", true)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "master",
            "/repo",
            vec![("feature/x", "/repo/feature/x", true)],
        );
        let classified = classify_worktrees(&source, &target, &[], false);
        let err = build_plan(&source, &target, &classified, false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("cannot move a main working tree"), "{msg}");
        assert!(msg.contains("transform sibling"), "{msg}");
    }

    // ── #859 mirror: bare -> non-bare with no default-branch worktree ──────

    #[test]
    fn test_bare_sole_worktree_collapses_and_unregisters_first() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("feature/x", "/repo/feature/x", false)],
        );
        let target = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("feature/x", "/repo", true)],
        );
        let plan = plan_for(&source, &target).unwrap();

        let unregister_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::UnregisterWorktree { branch, path }
                if branch == "feature/x" && path == Path::new("/repo/feature/x"))
        })
        .expect("should unregister the pivot by its current path");
        let collapse_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::CollapseIntoRoot { .. })
        })
        .expect("should collapse");
        assert!(unregister_idx < collapse_idx);

        assert!(index_of(&plan, |op| matches!(op, TransformOp::SetBare(false))).is_some());
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::InitWorktreeIndex { path, branch }
                    if path == Path::new("/repo") && branch == "feature/x"
            ))
            .is_some(),
            "the root index must be built against the pivot's branch, not the default's"
        );
    }

    #[test]
    fn test_bare_pivot_relocates_before_unregistering() {
        // contained-flat (`/repo/feature-x`) -> contained-classic
        // (`/repo/feature/x`): the pivot is still a linked worktree, so git can
        // move it — but only while its registration still exists.
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("feature/x", "/repo/feature-x", false)],
        );
        let target = make_state(
            "/repo/feature/x/.git",
            false,
            "main",
            "/repo",
            vec![("feature/x", "/repo/feature/x", true)],
        );
        let plan = plan_for(&source, &target).unwrap();

        let move_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::MoveWorktree { from, to, .. }
                if from == Path::new("/repo/feature-x") && to == Path::new("/repo/feature/x"))
        })
        .expect("pivot should move");
        let unregister_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::UnregisterWorktree { path, .. }
                if path == Path::new("/repo/feature/x"))
        })
        .expect("should unregister at the post-move path");
        let git_idx = index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. }))
            .expect("should move .git");
        assert!(move_idx < unregister_idx, "move needs the registration");
        assert!(
            unregister_idx < git_idx,
            "unregister before .git relocates under the pivot"
        );
    }

    #[test]
    fn test_bare_to_bare_has_no_root_ops() {
        let source = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("f/x", "/repo/f/x", false)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("f/x", "/repo/f-x", false)],
        );
        let plan = plan_for(&source, &target).unwrap();
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::UnregisterWorktree { .. }
            ))
            .is_none()
        );
        assert!(index_of(&plan, |op| matches!(op, TransformOp::SetBare(_))).is_none());
        assert!(index_of(&plan, |op| matches!(op, TransformOp::MoveWorktree { .. })).is_some());
    }
}
