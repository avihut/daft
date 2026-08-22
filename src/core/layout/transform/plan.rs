//! Transform plan builder.
//!
//! Given a source `LayoutState` and a target `LayoutState`, `classify_worktrees`
//! determines which worktrees need to move and `build_plan` sequences the
//! discrete operations to avoid path conflicts.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::state::{ClassifiedWorktree, LayoutState, WorktreeDisposition};
use super::status_snapshot::StatusSnapshot;
use crate::core::fs_volume::MoveStrategy;
use crate::core::worktree::list::ChangedFiles;

// ── TransformOp ──────────────────────────────────────────────────────────

/// A discrete, atomic operation in a layout transform.
#[derive(Debug, Clone)]
pub enum TransformOp {
    /// Move an entire linked-worktree directory from one path to another.
    MoveWorktree {
        branch: Option<String>,
        from: PathBuf,
        to: PathBuf,
        /// How the bytes get there. Decided at plan time so `--dry-run` can
        /// say "copy across volumes" rather than discovering `EXDEV` mid-plan.
        strategy: MoveStrategy,
    },
    /// Rename a *main* working tree's directory.
    ///
    /// `git worktree move` refuses a main working tree — but its `.git` is a
    /// real directory *inside* it, so a plain rename carries the repository,
    /// the index and the working tree together. What goes stale is the linked
    /// worktrees' `.git` pointer files, which name the old
    /// `<main>/.git/worktrees/<n>` path; the executor rewrites exactly that
    /// set (the registrations' own `gitdir` files stay correct — the linked
    /// worktrees did not move).
    MoveMainWorktree {
        branch: Option<String>,
        from: PathBuf,
        to: PathBuf,
    },
    /// Relocate the `.git` directory.
    MoveGitDir { from: PathBuf, to: PathBuf },
    /// Flip `core.bare` in the config of `common_dir`.
    ///
    /// `common_dir` is resolved at plan time, never from the CWD — the ops
    /// around this one move the working tree the CWD may be standing in.
    SetBare { bare: bool, common_dir: PathBuf },
    /// Register `path` as a linked worktree of `common_dir`, **carrying** the
    /// main working tree's private git state (HEAD, index, reflog, …) into
    /// the registration rather than rebuilding it.
    ///
    /// Pivot only: `build_plan` emits this exclusively for the root-role
    /// worktree and `reverse_op` preserves that. The executor refuses unless
    /// `core.bare` is already true in `common_dir` — the role change, in the
    /// one place git records it.
    RegisterWorktree {
        branch: Option<String>,
        path: PathBuf,
        common_dir: PathBuf,
    },
    /// Unregister the linked worktree at `path`, **carrying** its private git
    /// state back into `common_dir`, which is about to become a main working
    /// tree's `.git`. Same pivot-only invariant and `core.bare` guard.
    ///
    /// `path` is where the worktree lives *when this op runs* — the
    /// registration is resolved through it, because git names registration
    /// directories after the path basename, not the branch.
    UnregisterWorktree {
        branch: Option<String>,
        path: PathBuf,
        common_dir: PathBuf,
    },
    /// Move the root worktree from a subdirectory into the project root
    /// (bare/contained -> non-bare/sibling transition).
    CollapseIntoRoot {
        branch: Option<String>,
        worktree_path: PathBuf,
        root_path: PathBuf,
    },
    /// Move the root worktree from the project root into a subdirectory
    /// (non-bare/sibling -> bare/contained transition).
    NestFromRoot {
        branch: Option<String>,
        root_path: PathBuf,
        subdir_path: PathBuf,
    },
    /// Final integrity check — `git fsck`, and every worktree's status
    /// compared against the snapshot taken before execution.
    ValidateIntegrity,
}

// ── TransformPlan ────────────────────────────────────────────────────────

/// What a plan carries across, per worktree — for the plan line and
/// `--dry-run`.
#[derive(Debug, Clone)]
pub struct CarriedState {
    pub branch: Option<String>,
    pub from: PathBuf,
    pub to: PathBuf,
    pub counts: ChangedFiles,
}

/// A sequenced list of operations that transforms one layout into another.
#[derive(Debug)]
pub struct TransformPlan {
    /// Operations to execute, in order.
    pub ops: Vec<TransformOp>,
    /// Worktrees that were skipped (non-conforming, not included).
    pub skipped: Vec<ClassifiedWorktree>,
    /// Human-readable summary of the plan.
    pub description: String,
    /// Per-worktree tree state the plan carries. Empty when no snapshots were
    /// supplied.
    pub carried: Vec<CarriedState>,
}

impl TransformPlan {
    /// Whether any op copies a worktree across volumes.
    pub fn copies_across_volumes(&self) -> bool {
        self.ops.iter().any(|op| {
            matches!(
                op,
                TransformOp::MoveWorktree {
                    strategy: MoveStrategy::CopyThenRemove,
                    ..
                }
            )
        })
    }

    /// The `(from, to)` pairs of every cross-volume copy in the plan.
    pub fn cross_volume_moves(&self) -> Vec<(PathBuf, PathBuf)> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                TransformOp::MoveWorktree {
                    from,
                    to,
                    strategy: MoveStrategy::CopyThenRemove,
                    ..
                } => Some((from.clone(), to.clone())),
                _ => None,
            })
            .collect()
    }
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
                || src_wt.branch.as_ref().is_some_and(|b| include.contains(b))
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
///    worktree directory is back; and so the pivot's git state is carried into
///    the common dir before anything else touches either
/// 4. Root collapse / nest / rename
/// 5. MoveGitDir
/// 6. SetBare, then RegisterWorktree (which carries the pivot's state in)
/// 7. Regular move ops
/// 8. ValidateIntegrity
///
/// `snapshots` — the worktrees' status as captured before execution — only
/// feed the plan's `carried` summary; an empty slice is fine.
pub fn build_plan(
    source: &LayoutState,
    target: &LayoutState,
    classified: &[ClassifiedWorktree],
    snapshots: &[StatusSnapshot],
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
                        strategy: crate::core::fs_volume::strategy_for(
                            &cw.current_path,
                            &cw.target_path,
                        ),
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
            // Subdirectory -> different subdirectory. While the repo is bare
            // the pivot is still an ordinary linked worktree and
            // `git worktree move` handles it.
            (false, false) if source.is_bare => {
                pivot_move = Some(TransformOp::MoveWorktree {
                    branch: cw.branch.clone(),
                    from: cw.current_path.clone(),
                    to: cw.target_path.clone(),
                    strategy: crate::core::fs_volume::strategy_for(
                        &cw.current_path,
                        &cw.target_path,
                    ),
                });
            }
            // A non-bare *main* working tree in a subdirectory — a
            // contained-classic clone whose directory name no longer matches
            // its branch. Its `.git` lives inside it, so the directory renames
            // as a unit; the linked worktrees' pointers are repaired after.
            (false, false) => {
                root_op = Some(TransformOp::MoveMainWorktree {
                    branch: cw.branch.clone(),
                    from: cw.current_path.clone(),
                    to: cw.target_path.clone(),
                });
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

    // A MoveMainWorktree carries `.git` inside the directory it renames, so
    // the git dir the *next* op finds is the rebased one, not `source.git_dir`.
    let effective_source_git_dir = match &root_op {
        Some(TransformOp::MoveMainWorktree { from, to, .. }) => source
            .git_dir
            .strip_prefix(from)
            .map(|rel| to.join(rel))
            .unwrap_or_else(|_| source.git_dir.clone()),
        _ => source.git_dir.clone(),
    };
    let git_dir_changed = !paths_equivalent(&effective_source_git_dir, &target.git_dir);
    let bare_changed = source.is_bare != target.is_bare;
    let has_main_move = matches!(root_op, Some(TransformOp::MoveMainWorktree { .. }));

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
    //     The common dir recorded here is the *source* one: `MoveGitDir` has
    //     not run yet, and under rollback the inverse lands on the other side
    //     of the `MoveGitDir` undo — back at the same dir.
    if bare_changed
        && !target.is_bare
        && let Some(cw) = root_cw
        && let Some(path) = unregister_path
    {
        debug_assert_eq!(cw.disposition, WorktreeDisposition::Root);
        ops.push(TransformOp::UnregisterWorktree {
            branch: cw.branch.clone(),
            path,
            common_dir: source.git_dir.clone(),
        });
    }

    // b. Root collapse/nest/rename
    if let Some(op) = root_op {
        ops.push(op);
    }

    // c. MoveGitDir
    if git_dir_changed {
        ops.push(TransformOp::MoveGitDir {
            from: effective_source_git_dir.clone(),
            to: target.git_dir.clone(),
        });
    }

    // d. SetBare — against the common dir as it exists by now.
    if bare_changed {
        ops.push(TransformOp::SetBare {
            bare: target.is_bare,
            common_dir: if git_dir_changed {
                target.git_dir.clone()
            } else {
                effective_source_git_dir.clone()
            },
        });
    }

    // f. RegisterWorktree (if going bare, register the pivot). The op carries
    //    the main working tree's git state into the registration — HEAD and
    //    index included — so nothing is rebuilt.
    if bare_changed
        && target.is_bare
        && let Some(cw) = root_cw
    {
        debug_assert_eq!(cw.disposition, WorktreeDisposition::Root);
        ops.push(TransformOp::RegisterWorktree {
            branch: cw.branch.clone(),
            path: cw.target_path.clone(),
            common_dir: target.git_dir.clone(),
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
        if has_main_move {
            parts.push("rename the main working tree".to_string());
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

    // ── What the plan carries ────────────────────────────────────────────

    let carried = snapshots
        .iter()
        .map(|s| CarriedState {
            branch: s.branch.clone(),
            from: s.source_path.clone(),
            to: s.target_path.clone(),
            counts: s.counts.clone(),
        })
        .collect();

    Ok(TransformPlan {
        ops,
        skipped,
        description,
        carried,
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
                        branch: Some(branch.to_string()),
                        head: None,
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
        build_plan(source, target, &classified, &[])
    }

    fn is_branch(branch: &Option<String>, name: &str) -> bool {
        branch.as_deref() == Some(name)
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
            index_of(&plan, |op| matches!(
                op,
                TransformOp::SetBare { bare: false, .. }
            ))
            .is_some(),
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
                |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if is_branch(branch, "dev"))
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
            |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if is_branch(branch, "dev")),
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
        assert_eq!(plan.skipped[0].branch.as_deref(), Some("exp"));
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
        let plan = build_plan(&source, &target, &classified, &[]).unwrap();
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
        let plan = build_plan(&source, &target, &classified, &[]).unwrap();
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
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::SetBare { bare: true, .. }
            ))
            .is_some()
        );
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::RegisterWorktree { branch, path, common_dir }
                    if is_branch(branch, "feature/x")
                        && path == Path::new("/repo/feature/x")
                        && common_dir == Path::new("/repo/.git")
            ))
            .is_some(),
            "the pivot becomes a linked worktree, carrying its state"
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
                |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if is_branch(branch, "main"))
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
            |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if is_branch(branch, "dev")),
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
        assert!(index_of(&plan, |op| matches!(op, TransformOp::SetBare { .. })).is_none());
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
                |op| matches!(op, TransformOp::MoveWorktree { branch, .. } if is_branch(branch, "dev"))
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
    fn test_drifted_wrapper_source_renames_the_main_working_tree() {
        // contained-classic clone in `/repo/main` that has since switched to
        // `feature/x`: the target wants `/repo/feature/x`. `.git` lives inside
        // the clone directory, so the directory renames as a unit; `.git` then
        // moves out of it to the bare root — from its *rebased* path.
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
        let plan = plan_for(&source, &target).unwrap();

        let rename_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::MoveMainWorktree { from, to, .. }
                if from == Path::new("/repo/main") && to == Path::new("/repo/feature/x"))
        })
        .expect("the main working tree directory is renamed");
        let git_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::MoveGitDir { from, to }
                if from == Path::new("/repo/feature/x/.git") && to == Path::new("/repo/.git"))
        })
        .expect(".git moves out of the renamed directory, from its new path");
        assert!(rename_idx < git_idx);
        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::MoveWorktree { .. })).is_none(),
            "never `git worktree move` a main working tree"
        );
    }

    #[test]
    fn test_drifted_to_contained_classic_suppresses_a_dead_move_git_dir() {
        // The rename already puts `.git` exactly where the target wants it.
        let source = make_state(
            "/repo/main/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo/main", true)],
        );
        let target = make_state(
            "/repo/feature/x/.git",
            false,
            "master",
            "/repo",
            vec![("feature/x", "/repo/feature/x", true)],
        );
        let plan = plan_for(&source, &target).unwrap();
        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::MoveMainWorktree { .. }
            ))
            .is_some()
        );
        assert!(
            index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. })).is_none(),
            "no .git move: the rename carried it to the target path"
        );
        assert_eq!(plan.ops.len(), 2);
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
            matches!(op, TransformOp::UnregisterWorktree { branch, path, common_dir }
                if is_branch(branch, "feature/x")
                    && path == Path::new("/repo/feature/x")
                    && common_dir == Path::new("/repo/.git"))
        })
        .expect("should unregister the pivot by its current path, carrying its state out");
        let collapse_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::CollapseIntoRoot { .. })
        })
        .expect("should collapse");
        assert!(unregister_idx < collapse_idx);

        assert!(
            index_of(&plan, |op| matches!(
                op,
                TransformOp::SetBare { bare: false, .. }
            ))
            .is_some()
        );
        // No index rebuild anywhere: HEAD and index arrived by relocation.
        assert_eq!(plan.ops.len(), 4, "{:?}", plan.ops);
    }

    #[test]
    fn test_register_and_unregister_carry_the_git_dir_they_run_against() {
        // contained-classic (`.git` inside the pivot dir) -> contained (bare):
        // Unregister never happens (source is non-bare), but Register must
        // name the *target* git dir, which is where `.git` is by then.
        let source = make_state(
            "/repo/main/.git",
            false,
            "main",
            "/repo",
            vec![("main", "/repo/main", true)],
        );
        let target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("main", "/repo/main", true)],
        );
        let plan = plan_for(&source, &target).unwrap();
        let git_idx = index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. })).unwrap();
        let reg_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::RegisterWorktree { common_dir, .. }
                if common_dir == Path::new("/repo/.git"))
        })
        .expect("register against the relocated git dir");
        let bare_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::SetBare { bare: true, common_dir }
                if common_dir == Path::new("/repo/.git"))
        })
        .expect("SetBare against the relocated git dir");
        assert!(git_idx < bare_idx && bare_idx < reg_idx);

        // And the reverse: contained (bare) -> contained-classic. Unregister
        // runs before MoveGitDir, so it names the *source* git dir.
        let plan = plan_for(&target, &source).unwrap();
        let unreg_idx = index_of(&plan, |op| {
            matches!(op, TransformOp::UnregisterWorktree { common_dir, .. }
                if common_dir == Path::new("/repo/.git"))
        })
        .expect("unregister against the source git dir");
        let git_idx = index_of(&plan, |op| matches!(op, TransformOp::MoveGitDir { .. })).unwrap();
        assert!(unreg_idx < git_idx);
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
        assert!(index_of(&plan, |op| matches!(op, TransformOp::SetBare { .. })).is_none());
        assert!(index_of(&plan, |op| matches!(op, TransformOp::MoveWorktree { .. })).is_some());
    }

    #[test]
    fn test_detached_main_nests_under_the_supplied_directory() {
        // The state carries a branchless root; the plan just follows the paths.
        let mut source = make_state(
            "/repo/.git",
            false,
            "main",
            "/repo",
            vec![("x", "/repo", true)],
        );
        source.worktrees[0].branch = None;
        source.worktrees[0].head = Some("abc123".into());
        let mut target = make_state(
            "/repo/.git",
            true,
            "main",
            "/repo",
            vec![("x", "/repo/sandbox", true)],
        );
        target.worktrees[0].branch = None;
        let plan = plan_for(&source, &target).unwrap();
        assert!(
            index_of(
                &plan,
                |op| matches!(op, TransformOp::NestFromRoot { branch: None, subdir_path, .. }
                if subdir_path == Path::new("/repo/sandbox"))
            )
            .is_some()
        );
        assert!(
            index_of(
                &plan,
                |op| matches!(op, TransformOp::RegisterWorktree { branch: None, path, .. }
                if path == Path::new("/repo/sandbox"))
            )
            .is_some()
        );
    }

    #[test]
    fn test_same_volume_moves_are_planned_as_renames() {
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
        assert!(!plan.copies_across_volumes());
        assert!(plan.cross_volume_moves().is_empty());
    }
}
