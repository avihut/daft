# Template-Driven Layout Transform Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the hardcoded 2x2 bare/non-bare transform matrix with a
template-driven engine that computes an ordered plan of discrete operations from
layout state diffs.

**Architecture:** The engine reads the current repo state (worktree locations,
git_dir, bare flag), computes target state by evaluating the target layout
template for each branch, diffs the two states into a plan of discrete
operations, sequences them via path-conflict analysis, then executes with
rollback support. Dry-run prints the plan without executing.

**Tech Stack:** Rust, git CLI (worktree list/move), existing
Layout/TemplateContext infrastructure.

**Spec:**
`docs/superpowers/specs/2026-03-23-template-driven-layout-transform-design.md`

---

## File Structure

| Action | File                                                                   | Responsibility                                                                  |
| ------ | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| Create | `src/core/layout/transform/mod.rs`                                     | Module root, re-exports, `TransformOp` enum, `TransformPlan` struct             |
| Create | `src/core/layout/transform/state.rs`                                   | `LayoutState`, `WorktreeEntry`, `read_source_state()`, `compute_target_state()` |
| Create | `src/core/layout/transform/plan.rs`                                    | `build_plan()`, conflict detection, dependency graph, topological sort          |
| Create | `src/core/layout/transform/execute.rs`                                 | `execute_plan()`, per-op executors, rollback stack                              |
| Create | `src/core/layout/transform/print.rs`                                   | `format_plan()` for dry-run and verbose output                                  |
| Rename | `src/core/layout/transform.rs` → `src/core/layout/transform/legacy.rs` | Existing functions kept for adopt/eject until migrated                          |
| Modify | `src/core/layout/mod.rs`                                               | Update `mod transform` to point to directory module                             |
| Modify | `src/commands/layout.rs:386-525`                                       | Replace `cmd_transform` 2x2 matrix with plan builder + executor                 |
| Modify | `src/commands/layout.rs:68-75`                                         | Add `--dry-run`, `--include`, `--include-all` to `TransformArgs`                |
| Create | `tests/manual/scenarios/layout/transform-contained-to-classic.yml`     | YAML integration test                                                           |
| Create | `tests/manual/scenarios/layout/transform-classic-to-sibling.yml`       | YAML integration test                                                           |
| Create | `tests/manual/scenarios/layout/transform-dry-run.yml`                  | YAML integration test                                                           |

---

### Task 1: Module Restructure and Data Model

Convert the monolithic `transform.rs` into a module directory and define the
core data types.

**Files:**

- Rename: `src/core/layout/transform.rs` → `src/core/layout/transform/legacy.rs`
- Create: `src/core/layout/transform/mod.rs`
- Create: `src/core/layout/transform/state.rs`
- Modify: `src/core/layout/mod.rs` (module declaration stays the same — Rust
  resolves `transform/mod.rs` automatically)

- [ ] **Step 1: Convert transform.rs to a module directory**

Move the existing file and create the module root:

```bash
mkdir -p src/core/layout/transform
mv src/core/layout/transform.rs src/core/layout/transform/legacy.rs
```

Create `src/core/layout/transform/mod.rs`:

```rust
//! Layout transformation engine.
//!
//! The transform engine computes a plan of discrete operations by diffing the
//! current repository layout state against a target layout. Operations are
//! sequenced via path-conflict analysis and executed with rollback support.

pub mod legacy;
pub mod state;

// Re-export legacy items that are still used by adopt/eject and other callers
pub use legacy::{
    collapse_bare_to_non_bare, convert_to_bare, convert_to_non_bare,
    is_bare_worktree_layout, parse_worktrees, CollapseBareParams, CollapseBareResult,
    ConvertToBareParams, ConvertToBareResult, ConvertToNonBareParams, ConvertToNonBareResult,
    WorktreeInfo,
};
```

- [ ] **Step 2: Verify compilation**

Run: `mise run clippy` Expected: 0 warnings — the module split should be
transparent to callers.

- [ ] **Step 3: Run unit tests to confirm no regressions**

Run: `mise run test:unit` Expected: All tests pass (same count as before).

- [ ] **Step 4: Create the state data model**

Create `src/core/layout/transform/state.rs`:

```rust
//! Layout state representation for transform planning.
//!
//! `LayoutState` captures where everything is (source) or should be (target):
//! git_dir location, bare flag, and all worktree positions.

use std::path::PathBuf;

/// Snapshot of a repository's layout state.
#[derive(Debug, Clone)]
pub struct LayoutState {
    /// Absolute path to the `.git` directory.
    pub git_dir: PathBuf,
    /// Whether `core.bare` is true.
    pub is_bare: bool,
    /// The default branch name (the branch co-located with `.git` for non-bare,
    /// or the first/primary worktree for bare).
    pub default_branch: String,
    /// The project root / wrapper directory. For bare and wrapped non-bare
    /// layouts this is the parent of worktrees. For regular non-bare layouts
    /// this is the repo root itself.
    pub project_root: PathBuf,
    /// All worktree entries (including the default branch for bare layouts).
    pub worktrees: Vec<WorktreeEntry>,
}

/// A single worktree's position in a layout state.
#[derive(Debug, Clone)]
pub struct WorktreeEntry {
    /// Branch name (e.g., "main", "feature/auth").
    pub branch: String,
    /// Absolute path to the worktree directory.
    pub path: PathBuf,
    /// Whether this is the default branch.
    pub is_default: bool,
}

/// Classification of a worktree during transform planning.
#[derive(Debug, Clone, PartialEq)]
pub enum WorktreeDisposition {
    /// Worktree conforms to the target template — will be relocated if needed.
    Conforming,
    /// Worktree does not match the target template — skipped by default.
    NonConforming,
    /// Worktree is the default branch and needs special handling (collapse/nest).
    DefaultBranch,
}

/// A classified worktree entry in the transform plan.
#[derive(Debug, Clone)]
pub struct ClassifiedWorktree {
    pub branch: String,
    pub current_path: PathBuf,
    pub target_path: PathBuf,
    pub disposition: WorktreeDisposition,
}
```

- [ ] **Step 5: Verify compilation**

Run: `mise run clippy` Expected: 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add src/core/layout/transform/
git commit -m "refactor(transform): convert to module directory and add state data model"
```

---

### Task 2: Source and Target State Readers

Implement functions to read the current repo state and compute the target state
from a layout template.

**Files:**

- Modify: `src/core/layout/transform/state.rs`
- Modify: `src/core/layout/transform/mod.rs` (add re-exports)

- [ ] **Step 1: Write tests for read_source_state**

Add to the bottom of `state.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source_worktrees() {
        // Test parsing porcelain output into WorktreeEntry list
        let porcelain = "\
worktree /home/user/myproject
bare

worktree /home/user/myproject/main
branch refs/heads/main

worktree /home/user/myproject/develop
branch refs/heads/develop

";
        let entries = parse_porcelain_to_entries(porcelain);
        assert_eq!(entries.len(), 2); // bare entry excluded
        assert_eq!(entries[0].branch, "main");
        assert_eq!(entries[1].branch, "develop");
    }

    #[test]
    fn test_parse_source_skips_detached() {
        let porcelain = "\
worktree /home/user/myproject
bare

worktree /home/user/myproject/main
branch refs/heads/main

worktree /home/user/myproject/sandbox
HEAD abc123
detached

";
        let entries = parse_porcelain_to_entries(porcelain);
        assert_eq!(entries.len(), 1); // only main, detached skipped
        assert_eq!(entries[0].branch, "main");
    }

    #[test]
    fn test_compute_target_worktree_path_contained() {
        use crate::core::layout::BuiltinLayout;
        let layout = BuiltinLayout::Contained.to_layout();
        let project_root = PathBuf::from("/home/user/myproject");
        let path = compute_target_worktree_path(&layout, &project_root, "feature/auth").unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject/feature/auth"));
    }

    #[test]
    fn test_compute_target_worktree_path_sibling() {
        use crate::core::layout::BuiltinLayout;
        let layout = BuiltinLayout::Sibling.to_layout();
        let project_root = PathBuf::from("/home/user/myproject");
        let path = compute_target_worktree_path(&layout, &project_root, "feature/auth").unwrap();
        assert_eq!(path, PathBuf::from("/home/user/myproject.feature-auth"));
    }

    #[test]
    fn test_compute_target_git_dir_bare() {
        use crate::core::layout::BuiltinLayout;
        let layout = BuiltinLayout::Contained.to_layout();
        let project_root = PathBuf::from("/home/user/myproject");
        let git_dir = compute_target_git_dir(&layout, &project_root, "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/.git"));
    }

    #[test]
    fn test_compute_target_git_dir_wrapped_nonbare() {
        use crate::core::layout::BuiltinLayout;
        let layout = BuiltinLayout::ContainedClassic.to_layout();
        let project_root = PathBuf::from("/home/user/myproject");
        let git_dir = compute_target_git_dir(&layout, &project_root, "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/main/.git"));
    }

    #[test]
    fn test_compute_target_git_dir_regular_nonbare() {
        use crate::core::layout::BuiltinLayout;
        let layout = BuiltinLayout::Sibling.to_layout();
        let project_root = PathBuf::from("/home/user/myproject");
        let git_dir = compute_target_git_dir(&layout, &project_root, "main").unwrap();
        assert_eq!(git_dir, PathBuf::from("/home/user/myproject/.git"));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -- transform::state::tests -v 2>&1 | tail -20` Expected:
FAIL — functions not defined yet.

- [ ] **Step 3: Implement the state reader functions**

Add to `state.rs` above the tests module:

```rust
use crate::core::layout::Layout;
use crate::core::multi_remote::path::build_template_context;
use anyhow::{Context, Result};

/// Parse `git worktree list --porcelain` output into worktree entries.
///
/// Skips the bare root entry and detached HEAD worktrees.
pub fn parse_porcelain_to_entries(porcelain: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut is_bare = false;
    let mut is_detached = false;

    for line in porcelain.lines() {
        if let Some(wt_path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(wt_path));
            is_bare = false;
            is_detached = false;
        } else if line == "bare" {
            is_bare = true;
        } else if line.starts_with("detached") {
            is_detached = true;
        } else if let Some(branch_ref) = line.strip_prefix("branch ") {
            if !is_bare && !is_detached {
                if let Some(branch) = branch_ref.strip_prefix("refs/heads/") {
                    if let Some(path) = current_path.take() {
                        entries.push(WorktreeEntry {
                            branch: branch.to_string(),
                            path,
                            is_default: false, // Caller sets this
                        });
                    }
                }
            }
        } else if line.is_empty() {
            current_path = None;
            is_bare = false;
            is_detached = false;
        }
    }

    entries
}

/// Read the current layout state from the repository.
///
/// Requires being inside a git repository. Uses `git worktree list --porcelain`
/// for worktree positions and `git config core.bare` for bare detection.
pub fn read_source_state(
    git: &crate::git::GitCommand,
    default_branch: &str,
) -> Result<LayoutState> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    let git_dir = std::fs::canonicalize(&git_dir)
        .with_context(|| format!("Could not canonicalize git dir: {}", git_dir.display()))?;

    let is_bare = git
        .config_get("core.bare")
        .ok()
        .flatten()
        .is_some_and(|v| v.to_lowercase() == "true");

    let project_root = git_dir
        .parent()
        .context("Could not determine project root")?
        .to_path_buf();

    let porcelain = git.worktree_list_porcelain()?;
    let mut worktrees = parse_porcelain_to_entries(&porcelain);

    // Mark the default branch
    for wt in &mut worktrees {
        if wt.branch == default_branch {
            wt.is_default = true;
        }
    }

    Ok(LayoutState {
        git_dir,
        is_bare,
        default_branch: default_branch.to_string(),
        project_root,
        worktrees,
    })
}

/// Compute where a worktree should be for a given layout and branch.
///
/// Applies the `needs_wrapper()` adjustment: for wrapped non-bare layouts,
/// `project_root` is used directly (it's the wrapper). For all others,
/// `project_root` is the repo root.
pub fn compute_target_worktree_path(
    layout: &Layout,
    project_root: &std::path::Path,
    branch: &str,
) -> Result<PathBuf> {
    let ctx = build_template_context(project_root, branch);
    layout.worktree_path(&ctx)
}

/// Compute where `.git` should live for a target layout.
///
/// - Bare layouts: `project_root/.git`
/// - Wrapped non-bare: `project_root/<default_branch>/.git`
///   (evaluate template with default branch to find the clone subdir)
/// - Regular non-bare: `project_root/.git`
pub fn compute_target_git_dir(
    layout: &Layout,
    project_root: &std::path::Path,
    default_branch: &str,
) -> Result<PathBuf> {
    if layout.needs_bare() {
        Ok(project_root.join(".git"))
    } else if layout.needs_wrapper() {
        let default_path = compute_target_worktree_path(layout, project_root, default_branch)?;
        Ok(default_path.join(".git"))
    } else {
        Ok(project_root.join(".git"))
    }
}

/// Compute the full target layout state from a layout template.
pub fn compute_target_state(
    layout: &Layout,
    project_root: &std::path::Path,
    default_branch: &str,
    source_worktrees: &[WorktreeEntry],
) -> Result<LayoutState> {
    let git_dir = compute_target_git_dir(layout, project_root, default_branch)?;

    let mut worktrees = Vec::new();
    for wt in source_worktrees {
        let target_path = compute_target_worktree_path(layout, project_root, &wt.branch)?;
        worktrees.push(WorktreeEntry {
            branch: wt.branch.clone(),
            path: target_path,
            is_default: wt.is_default,
        });
    }

    Ok(LayoutState {
        git_dir,
        is_bare: layout.needs_bare(),
        default_branch: default_branch.to_string(),
        project_root: project_root.to_path_buf(),
        worktrees,
    })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib -- transform::state::tests -v 2>&1 | tail -20` Expected:
All PASS.

- [ ] **Step 5: Run clippy and full unit tests**

Run: `mise run clippy && mise run test:unit` Expected: 0 warnings, all tests
pass.

- [ ] **Step 6: Commit**

```bash
git add src/core/layout/transform/
git commit -m "feat(transform): add source and target state readers"
```

---

### Task 3: Plan Builder with Conflict-Driven Sequencing

The core engine: diff source and target states into an ordered plan of
operations.

**Files:**

- Create: `src/core/layout/transform/plan.rs`
- Modify: `src/core/layout/transform/mod.rs` (add `pub mod plan;` and
  re-exports)

- [ ] **Step 1: Define TransformOp and TransformPlan**

Create `src/core/layout/transform/plan.rs`:

```rust
//! Transform plan builder.
//!
//! Diffs source and target layout states to produce an ordered list of
//! operations. Sequencing is determined by path-conflict analysis — no
//! hardcoded per-layout-pair logic.

use super::state::{ClassifiedWorktree, LayoutState, WorktreeDisposition, WorktreeEntry};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// A discrete, reversible transform operation.
#[derive(Debug, Clone)]
pub enum TransformOp {
    /// Stash uncommitted changes in a worktree before moving it.
    StashChanges {
        branch: String,
        worktree_path: PathBuf,
    },
    /// Move a linked worktree via `git worktree move`.
    MoveWorktree {
        branch: String,
        from: PathBuf,
        to: PathBuf,
    },
    /// Move the `.git` directory to a new location and fix up references.
    MoveGitDir { from: PathBuf, to: PathBuf },
    /// Set `core.bare` to the given value.
    SetBare(bool),
    /// Register a worktree with git (create .git file, gitdir, HEAD, commondir).
    RegisterWorktree {
        branch: String,
        path: PathBuf,
    },
    /// Remove a worktree registration from `.git/worktrees/`.
    UnregisterWorktree { branch: String },
    /// Move checkout files from a subdirectory into its parent (eject default branch).
    CollapseIntoRoot {
        worktree_path: PathBuf,
        root_path: PathBuf,
    },
    /// Move checkout files from a directory into a new subdirectory (adopt default branch).
    NestFromRoot {
        root_path: PathBuf,
        subdir_path: PathBuf,
    },
    /// Rebuild the git index after bare/non-bare conversion.
    InitWorktreeIndex { path: PathBuf },
    /// Create a directory (and parents).
    CreateDirectory { path: PathBuf },
    /// Restore stashed changes after a worktree has been moved.
    PopStash {
        branch: String,
        worktree_path: PathBuf,
    },
    /// Run `git fsck` or equivalent integrity check.
    ValidateIntegrity,
}

/// The complete transform plan.
#[derive(Debug)]
pub struct TransformPlan {
    /// Ordered operations to execute.
    pub ops: Vec<TransformOp>,
    /// Worktrees that won't be relocated (non-conforming, not --included).
    pub skipped: Vec<ClassifiedWorktree>,
    /// Summary description for output.
    pub description: String,
}
```

- [ ] **Step 2: Implement classify_worktrees**

Add to `plan.rs`:

```rust
/// Classify each worktree as conforming, non-conforming, or default branch.
pub fn classify_worktrees(
    source: &LayoutState,
    target: &LayoutState,
    include_branches: &[String],
    include_all: bool,
) -> Vec<ClassifiedWorktree> {
    source
        .worktrees
        .iter()
        .map(|src_wt| {
            let target_wt = target
                .worktrees
                .iter()
                .find(|t| t.branch == src_wt.branch);

            let target_path = target_wt
                .map(|t| t.path.clone())
                .unwrap_or_else(|| src_wt.path.clone());

            let disposition = if src_wt.is_default {
                WorktreeDisposition::DefaultBranch
            } else {
                let paths_match = paths_equivalent(&src_wt.path, &target_path);
                let is_included = include_all
                    || include_branches.iter().any(|b| b == &src_wt.branch);

                if paths_match {
                    // Already in the right place
                    WorktreeDisposition::Conforming
                } else if is_included {
                    // Not in right place but user opted in to relocating
                    WorktreeDisposition::Conforming
                } else {
                    WorktreeDisposition::NonConforming
                }
            };

            ClassifiedWorktree {
                branch: src_wt.branch.clone(),
                current_path: src_wt.path.clone(),
                target_path,
                disposition,
            }
        })
        .collect()
}

/// Compare two paths accounting for symlinks and /tmp vs /private/tmp.
fn paths_equivalent(a: &std::path::Path, b: &std::path::Path) -> bool {
    let a_canon = a.canonicalize().unwrap_or_else(|_| a.to_path_buf());
    let b_canon = b.canonicalize().unwrap_or_else(|_| b.to_path_buf());
    a_canon == b_canon
}
```

- [ ] **Step 3: Implement build_plan**

Add to `plan.rs`:

```rust
/// Build a transform plan by diffing source and target states.
///
/// The plan is fully computed before any mutations. Operations are ordered
/// by path-conflict analysis to ensure safe execution.
pub fn build_plan(
    source: &LayoutState,
    target: &LayoutState,
    classified: &[ClassifiedWorktree],
    force: bool,
) -> Result<TransformPlan> {
    let mut ops: Vec<TransformOp> = Vec::new();
    let mut skipped: Vec<ClassifiedWorktree> = Vec::new();

    let git_dir_moves = !paths_equivalent(&source.git_dir, &target.git_dir);
    let bare_changes = source.is_bare != target.is_bare;

    // ── Phase 1: Collect worktree moves ────────────────────────────────

    // Worktrees that need to vacate paths (move OUT first)
    let mut vacate_ops: Vec<TransformOp> = Vec::new();
    // Worktrees that move to new positions (after vacating)
    let mut move_ops: Vec<TransformOp> = Vec::new();

    for cw in classified {
        match cw.disposition {
            WorktreeDisposition::NonConforming => {
                skipped.push(cw.clone());
            }
            WorktreeDisposition::DefaultBranch => {
                // Default branch is handled separately (collapse/nest/move)
            }
            WorktreeDisposition::Conforming => {
                if !paths_equivalent(&cw.current_path, &cw.target_path) {
                    // Check if this worktree's current path is inside a directory
                    // that will become something else (vacate-before-occupy).
                    let needs_early_move =
                        is_path_inside(&cw.current_path, &target.project_root)
                            && !is_path_inside(&cw.target_path, &target.project_root);

                    let op = TransformOp::MoveWorktree {
                        branch: cw.branch.clone(),
                        from: cw.current_path.clone(),
                        to: cw.target_path.clone(),
                    };

                    if needs_early_move {
                        vacate_ops.push(op);
                    } else {
                        move_ops.push(op);
                    }
                }
            }
        }
    }

    // ── Phase 2: Determine default branch handling ─────────────────────

    let default_wt = classified
        .iter()
        .find(|cw| cw.disposition == WorktreeDisposition::DefaultBranch);

    let default_branch_op = if let Some(dw) = default_wt {
        determine_default_branch_op(source, target, dw)
    } else {
        None
    };

    // ── Phase 3: Sequence operations ───────────────────────────────────

    // 1. Vacate worktrees that are in the way
    ops.extend(vacate_ops);

    // 2. Handle default branch (collapse, nest, or nothing)
    if let Some(ref db_op) = default_branch_op {
        match db_op {
            TransformOp::CollapseIntoRoot { .. } => {
                // Collapse after vacating (files move from subdir to root)
                ops.push(db_op.clone());
                // Unregister the default branch worktree (it becomes the main tree)
                ops.push(TransformOp::UnregisterWorktree {
                    branch: source.default_branch.clone(),
                });
            }
            TransformOp::NestFromRoot { .. } => {
                // Nest before other ops that might conflict with root
                ops.push(db_op.clone());
            }
            _ => {
                ops.push(db_op.clone());
            }
        }
    }

    // 3. Move git_dir if needed
    if git_dir_moves {
        ops.push(TransformOp::MoveGitDir {
            from: source.git_dir.clone(),
            to: target.git_dir.clone(),
        });
    }

    // 4. Flip bare if needed
    if bare_changes {
        ops.push(TransformOp::SetBare(target.is_bare));
    }

    // 5. Init worktree index if bare changed
    if bare_changes {
        // Determine where the working tree will be
        let index_path = if target.is_bare {
            // Going bare: index is in the default branch worktree
            default_wt
                .map(|dw| dw.target_path.clone())
                .unwrap_or_else(|| target.project_root.clone())
        } else if target.git_dir.parent() != Some(target.project_root.as_path()) {
            // Wrapped non-bare: index is in the clone subdir
            target
                .git_dir
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| target.project_root.clone())
        } else {
            // Regular non-bare: index is at project root
            target.project_root.clone()
        };
        ops.push(TransformOp::InitWorktreeIndex { path: index_path });
    }

    // 6. Register default branch as worktree if going bare
    if !source.is_bare && target.is_bare {
        if let Some(dw) = default_wt {
            ops.push(TransformOp::RegisterWorktree {
                branch: dw.branch.clone(),
                path: dw.target_path.clone(),
            });
        }
    }

    // 7. Move remaining conforming worktrees
    ops.extend(move_ops);

    // 8. Validate integrity
    ops.push(TransformOp::ValidateIntegrity);

    let description = format!(
        "{} → {} ({}bare → {}bare)",
        "current",
        "target",
        if source.is_bare { "" } else { "non-" },
        if target.is_bare { "" } else { "non-" },
    );

    Ok(TransformPlan {
        ops,
        skipped,
        description,
    })
}

/// Determine what operation the default branch needs.
fn determine_default_branch_op(
    source: &LayoutState,
    target: &LayoutState,
    default_wt: &ClassifiedWorktree,
) -> Option<TransformOp> {
    let source_at_root = paths_equivalent(&default_wt.current_path, &source.project_root);
    let target_at_root = paths_equivalent(&default_wt.target_path, &target.project_root);

    if source_at_root && !target_at_root {
        // Default branch needs to move from root into a subdirectory
        Some(TransformOp::NestFromRoot {
            root_path: default_wt.current_path.clone(),
            subdir_path: default_wt.target_path.clone(),
        })
    } else if !source_at_root && target_at_root {
        // Default branch needs to collapse from subdirectory into root
        Some(TransformOp::CollapseIntoRoot {
            worktree_path: default_wt.current_path.clone(),
            root_path: default_wt.target_path.clone(),
        })
    } else if !paths_equivalent(&default_wt.current_path, &default_wt.target_path) {
        // Default branch is in a subdir and stays in a subdir, but path changed
        // (e.g., contained-classic → contained: worktree stays but .git moves)
        // This is handled by MoveGitDir, not a worktree move.
        None
    } else {
        None // Already in the right place
    }
}

/// Check if `child` is a subdirectory of `parent`.
fn is_path_inside(child: &std::path::Path, parent: &std::path::Path) -> bool {
    let child_canon = child.canonicalize().unwrap_or_else(|_| child.to_path_buf());
    let parent_canon = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    child_canon.starts_with(&parent_canon) && child_canon != parent_canon
}
```

- [ ] **Step 4: Write tests for plan builder**

Add to `plan.rs`:

```rust
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
                .map(|(branch, path, is_default)| WorktreeEntry {
                    branch: branch.to_string(),
                    path: PathBuf::from(path),
                    is_default,
                })
                .collect(),
        }
    }

    #[test]
    fn test_same_layout_no_ops() {
        let source = make_state(
            "/repo/.git", true, "main", "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let target = source.clone();
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        // Only ValidateIntegrity (no moves, no bare change)
        assert_eq!(plan.ops.len(), 1);
        assert!(matches!(plan.ops[0], TransformOp::ValidateIntegrity));
    }

    #[test]
    fn test_contained_to_contained_classic_plan() {
        let source = make_state(
            "/repo/.git", true, "main", "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let target = make_state(
            "/repo/main/.git", false, "main", "/repo",
            vec![("main", "/repo/main", true), ("dev", "/repo/dev", false)],
        );
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();

        // Should have: MoveGitDir, SetBare(false), InitWorktreeIndex, ValidateIntegrity
        let has_move_git = plan.ops.iter().any(|op| matches!(op, TransformOp::MoveGitDir { .. }));
        let has_set_bare = plan.ops.iter().any(|op| matches!(op, TransformOp::SetBare(false)));
        assert!(has_move_git);
        assert!(has_set_bare);
        // dev should NOT move (already at correct path)
        let has_move_dev = plan.ops.iter().any(|op| matches!(op, TransformOp::MoveWorktree { branch, .. } if branch == "dev"));
        assert!(!has_move_dev);
    }

    #[test]
    fn test_non_conforming_worktrees_skipped() {
        let source = make_state(
            "/repo/.git", true, "main", "/repo",
            vec![
                ("main", "/repo/main", true),
                ("dev", "/repo/dev", false),
                ("exp", "/custom/path/exp", false),
            ],
        );
        let target = make_state(
            "/repo/.git", false, "main", "/repo",
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
            "/repo/.git", true, "main", "/repo",
            vec![
                ("main", "/repo/main", true),
                ("exp", "/custom/path/exp", false),
            ],
        );
        let target = make_state(
            "/repo/.git", true, "main", "/repo",
            vec![
                ("main", "/repo/main", true),
                ("exp", "/repo/exp", false),
            ],
        );
        let classified = classify_worktrees(&source, &target, &["exp".to_string()], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();
        assert_eq!(plan.skipped.len(), 0);
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --lib -- transform::plan::tests -v 2>&1 | tail -20` Expected:
All PASS.

- [ ] **Step 6: Update mod.rs re-exports**

Add to `src/core/layout/transform/mod.rs`:

```rust
pub mod plan;

pub use plan::{build_plan, classify_worktrees, TransformOp, TransformPlan};
pub use state::{
    compute_target_git_dir, compute_target_state, compute_target_worktree_path,
    read_source_state, ClassifiedWorktree, LayoutState, WorktreeDisposition, WorktreeEntry,
};
```

- [ ] **Step 7: Clippy + full test suite**

Run: `mise run clippy && mise run test:unit` Expected: 0 warnings, all pass.

- [ ] **Step 8: Commit**

```bash
git add src/core/layout/transform/
git commit -m "feat(transform): implement plan builder with conflict-driven sequencing"
```

---

### Task 4: Plan Executor with Rollback

Execute each operation in the plan, with a rollback stack for failure recovery.

**Files:**

- Create: `src/core/layout/transform/execute.rs`
- Modify: `src/core/layout/transform/mod.rs`

- [ ] **Step 1: Create the executor skeleton**

Create `src/core/layout/transform/execute.rs`:

```rust
//! Plan executor with rollback support.
//!
//! Executes each `TransformOp` in order, pushing completed ops onto a rollback
//! stack. On failure, attempts to reverse all completed operations.

use super::plan::{TransformOp, TransformPlan};
use crate::core::ProgressSink;
use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Result of executing a transform plan.
pub struct ExecuteResult {
    pub ops_completed: usize,
    pub ops_total: usize,
}

/// Execute all operations in a transform plan.
pub fn execute_plan(
    plan: &TransformPlan,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<ExecuteResult> {
    let mut rollback_stack: Vec<TransformOp> = Vec::new();
    let total = plan.ops.len();

    for (i, op) in plan.ops.iter().enumerate() {
        progress.on_step(&format!("[{}/{}] {}", i + 1, total, describe_op(op)));

        if let Err(e) = execute_op(op, git, progress) {
            progress.on_warning(&format!("Operation failed: {e}"));
            progress.on_step("Attempting rollback...");

            if let Err(rollback_err) = rollback(&rollback_stack, git, progress) {
                progress.on_warning(&format!("Rollback failed: {rollback_err}"));
                progress.on_warning(
                    "Manual recovery may be needed. Check `git worktree list` and `git status`.",
                );
            }

            return Err(e.context(format!("Transform failed at step {}/{}", i + 1, total)));
        }

        // Push the reverse operation for rollback
        if let Some(reverse) = reverse_op(op) {
            rollback_stack.push(reverse);
        }
    }

    Ok(ExecuteResult {
        ops_completed: total,
        ops_total: total,
    })
}

/// Execute a single transform operation.
fn execute_op(
    op: &TransformOp,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    match op {
        TransformOp::StashChanges {
            worktree_path, ..
        } => {
            let prev = crate::utils::get_current_directory()?;
            crate::utils::change_directory(worktree_path)?;
            git.stash_push_with_untracked("daft-layout-transform: preserving changes")?;
            crate::utils::change_directory(&prev)?;
            Ok(())
        }

        TransformOp::PopStash {
            worktree_path, ..
        } => {
            let prev = crate::utils::get_current_directory()?;
            crate::utils::change_directory(worktree_path)?;
            if let Err(e) = git.stash_pop() {
                progress.on_warning(&format!(
                    "Could not restore stashed changes: {e}. Run `git stash pop` manually."
                ));
            }
            crate::utils::change_directory(&prev)?;
            Ok(())
        }

        TransformOp::MoveWorktree { from, to, .. } => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            git.worktree_move(from, to)
                .with_context(|| format!("Failed to move worktree {} → {}", from.display(), to.display()))?;
            // Clean up empty parents
            if let Some(parent) = from.parent() {
                cleanup_empty_parents(parent);
            }
            Ok(())
        }

        TransformOp::MoveGitDir { from, to } => {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            fs::rename(from, to)
                .with_context(|| format!("Failed to move .git {} → {}", from.display(), to.display()))?;
            // Fix up worktree gitdir references
            fixup_gitdir_references(to)?;
            Ok(())
        }

        TransformOp::SetBare(bare) => {
            git.config_set("core.bare", if *bare { "true" } else { "false" })?;
            if *bare {
                // Remove index when going bare
                let git_dir = crate::core::repo::get_git_common_dir()?;
                let index = git_dir.join("index");
                if index.exists() {
                    fs::remove_file(&index).ok();
                }
            }
            Ok(())
        }

        TransformOp::RegisterWorktree { branch, path } => {
            let git_dir = crate::core::repo::get_git_common_dir()?;
            super::legacy::register_worktree(&git_dir, path, branch, progress)?;
            Ok(())
        }

        TransformOp::UnregisterWorktree { branch } => {
            let git_dir = crate::core::repo::get_git_common_dir()?;
            let wt_reg = git_dir.join("worktrees").join(branch);
            if wt_reg.exists() {
                fs::remove_dir_all(&wt_reg).ok();
            }
            Ok(())
        }

        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
        } => {
            // Move all files from worktree subdir into root (via staging)
            let staging = root_path.join(".daft-transform-staging");
            fs::create_dir_all(&staging)?;

            // Move worktree contents to staging
            for entry in fs::read_dir(worktree_path)? {
                let entry = entry?;
                let name = entry.file_name();
                if name == ".git" {
                    continue; // .git handled separately
                }
                fs::rename(entry.path(), staging.join(&name))?;
            }

            // Remove empty worktree dir
            fs::remove_dir(worktree_path).ok();

            // Move from staging to root
            for entry in fs::read_dir(&staging)? {
                let entry = entry?;
                fs::rename(entry.path(), root_path.join(entry.file_name()))?;
            }
            fs::remove_dir(&staging)?;

            Ok(())
        }

        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
        } => {
            // Move all files from root into a subdirectory (via staging)
            let staging = root_path.join(".daft-transform-staging");
            fs::create_dir_all(&staging)?;

            for entry in fs::read_dir(root_path)? {
                let entry = entry?;
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                // Skip .git, staging dir itself, and any existing worktree dirs
                if name_str == ".git"
                    || name_str == ".daft-transform-staging"
                    || entry.path().is_dir()
                        && entry.path() != *root_path
                        && is_likely_worktree(&entry.path())
                {
                    continue;
                }
                fs::rename(entry.path(), staging.join(&name))?;
            }

            // Create subdir and move from staging
            fs::create_dir_all(subdir_path)?;
            for entry in fs::read_dir(&staging)? {
                let entry = entry?;
                fs::rename(entry.path(), subdir_path.join(entry.file_name()))?;
            }
            fs::remove_dir(&staging)?;

            Ok(())
        }

        TransformOp::InitWorktreeIndex { path } => {
            let prev = crate::utils::get_current_directory()?;
            crate::utils::change_directory(path)?;
            let output = std::process::Command::new("git")
                .args(["reset", "--mixed", "HEAD"])
                .current_dir(path)
                .output()
                .context("Failed to initialize worktree index")?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                progress.on_warning(&format!("git reset warning: {}", stderr.trim()));
            }
            crate::utils::change_directory(&prev)?;
            Ok(())
        }

        TransformOp::CreateDirectory { path } => {
            fs::create_dir_all(path)
                .with_context(|| format!("Failed to create {}", path.display()))?;
            Ok(())
        }

        TransformOp::ValidateIntegrity => {
            // Light validation: check that git status works
            let output = std::process::Command::new("git")
                .args(["status", "--porcelain"])
                .output();
            match output {
                Ok(o) if o.status.success() => Ok(()),
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    progress.on_warning(&format!("Integrity check warning: {}", stderr.trim()));
                    Ok(()) // Warning, not failure
                }
                Err(e) => {
                    progress.on_warning(&format!("Could not run integrity check: {e}"));
                    Ok(())
                }
            }
        }
    }
}

/// Fix up `.git/worktrees/*/gitdir` and worktree `.git` files after moving .git.
fn fixup_gitdir_references(new_git_dir: &Path) -> Result<()> {
    let worktrees_dir = new_git_dir.join("worktrees");
    if !worktrees_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(&worktrees_dir)? {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }

        let gitdir_file = entry.path().join("gitdir");
        if gitdir_file.exists() {
            let old_path = fs::read_to_string(&gitdir_file)?.trim().to_string();
            if let Some(old_parent) = PathBuf::from(&old_path).parent() {
                // The gitdir file points to the worktree's .git file.
                // Update the worktree's .git file to point back to the new location.
                let wt_git_file = PathBuf::from(&old_path);
                if wt_git_file.exists() {
                    let new_gitdir_path = entry.path();
                    fs::write(
                        &wt_git_file,
                        format!("gitdir: {}\n", new_gitdir_path.display()),
                    )?;
                }
            }
        }
    }

    Ok(())
}

/// Compute the reverse of an operation for rollback.
fn reverse_op(op: &TransformOp) -> Option<TransformOp> {
    match op {
        TransformOp::MoveWorktree { branch, from, to } => Some(TransformOp::MoveWorktree {
            branch: branch.clone(),
            from: to.clone(),
            to: from.clone(),
        }),
        TransformOp::MoveGitDir { from, to } => Some(TransformOp::MoveGitDir {
            from: to.clone(),
            to: from.clone(),
        }),
        TransformOp::SetBare(bare) => Some(TransformOp::SetBare(!bare)),
        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
        } => Some(TransformOp::NestFromRoot {
            root_path: root_path.clone(),
            subdir_path: worktree_path.clone(),
        }),
        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
        } => Some(TransformOp::CollapseIntoRoot {
            worktree_path: subdir_path.clone(),
            root_path: root_path.clone(),
        }),
        // These operations are not easily reversible or don't need rollback
        TransformOp::StashChanges { .. }
        | TransformOp::PopStash { .. }
        | TransformOp::RegisterWorktree { .. }
        | TransformOp::UnregisterWorktree { .. }
        | TransformOp::InitWorktreeIndex { .. }
        | TransformOp::CreateDirectory { .. }
        | TransformOp::ValidateIntegrity => None,
    }
}

/// Attempt to roll back completed operations in reverse order.
fn rollback(
    stack: &[TransformOp],
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    for op in stack.iter().rev() {
        progress.on_step(&format!("Rollback: {}", describe_op(op)));
        execute_op(op, git, progress)?;
    }
    Ok(())
}

/// Human-readable description of an operation for progress output.
pub fn describe_op(op: &TransformOp) -> String {
    match op {
        TransformOp::StashChanges { branch, .. } => format!("Stash changes in '{branch}'"),
        TransformOp::PopStash { branch, .. } => format!("Restore changes in '{branch}'"),
        TransformOp::MoveWorktree { branch, from, to } => {
            format!("Move '{}': {} → {}", branch, from.display(), to.display())
        }
        TransformOp::MoveGitDir { from, to } => {
            format!("Move .git: {} → {}", from.display(), to.display())
        }
        TransformOp::SetBare(bare) => {
            format!("Set core.bare = {bare}")
        }
        TransformOp::RegisterWorktree { branch, path } => {
            format!("Register worktree '{branch}' at {}", path.display())
        }
        TransformOp::UnregisterWorktree { branch } => {
            format!("Unregister worktree '{branch}'")
        }
        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
        } => format!(
            "Collapse {} → {}",
            worktree_path.display(),
            root_path.display()
        ),
        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
        } => format!(
            "Nest {} → {}",
            root_path.display(),
            subdir_path.display()
        ),
        TransformOp::InitWorktreeIndex { path } => {
            format!("Initialize index at {}", path.display())
        }
        TransformOp::CreateDirectory { path } => format!("Create {}", path.display()),
        TransformOp::ValidateIntegrity => "Validate repository integrity".to_string(),
    }
}

/// Check if a directory looks like a git worktree (has a .git file).
fn is_likely_worktree(path: &Path) -> bool {
    path.join(".git").exists()
}

/// Remove empty parent directories up to a reasonable depth.
fn cleanup_empty_parents(dir: &Path) {
    let mut current = dir;
    for _ in 0..5 {
        if current
            .read_dir()
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            fs::remove_dir(current).ok();
        } else {
            break;
        }
        match current.parent() {
            Some(p) => current = p,
            None => break,
        }
    }
}
```

- [ ] **Step 2: Add to mod.rs**

Add to `src/core/layout/transform/mod.rs`:

```rust
pub mod execute;

pub use execute::{describe_op, execute_plan, ExecuteResult};
```

- [ ] **Step 3: Verify compilation**

Run: `mise run clippy` Expected: 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add src/core/layout/transform/
git commit -m "feat(transform): implement plan executor with rollback support"
```

---

### Task 5: Plan Printer (Dry Run)

Format the transform plan for human-readable output.

**Files:**

- Create: `src/core/layout/transform/print.rs`
- Modify: `src/core/layout/transform/mod.rs`

- [ ] **Step 1: Create the printer**

Create `src/core/layout/transform/print.rs`:

```rust
//! Human-readable plan formatting for dry-run output and verbose mode.

use super::execute::describe_op;
use super::plan::TransformPlan;
use super::state::WorktreeDisposition;
use crate::output::Output;

/// Print the full transform plan to the output.
pub fn print_plan(plan: &TransformPlan, output: &mut dyn Output) {
    output.step(&format!("Transform plan ({} operations):", plan.ops.len()));

    for (i, op) in plan.ops.iter().enumerate() {
        output.step(&format!("  {}. {}", i + 1, describe_op(op)));
    }

    if !plan.skipped.is_empty() {
        output.step(&format!(
            "\nSkipped ({} non-conforming):",
            plan.skipped.len()
        ));
        for cw in &plan.skipped {
            output.step(&format!(
                "  '{}': {} (use --include to relocate)",
                cw.branch,
                cw.current_path.display()
            ));
        }
    }
}
```

- [ ] **Step 2: Add to mod.rs**

Add to `src/core/layout/transform/mod.rs`:

```rust
pub mod print;

pub use print::print_plan;
```

- [ ] **Step 3: Verify compilation**

Run: `mise run clippy` Expected: 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add src/core/layout/transform/
git commit -m "feat(transform): add plan printer for dry-run output"
```

---

### Task 6: CLI Integration — Replace the 2x2 Matrix

Wire the new engine into `commands/layout.rs`, replacing the hardcoded dispatch
matrix.

**Files:**

- Modify: `src/commands/layout.rs:68-75` (TransformArgs)
- Modify: `src/commands/layout.rs:386-525` (cmd_transform)

- [ ] **Step 1: Add CLI flags to TransformArgs**

In `src/commands/layout.rs`, update the `TransformArgs` struct:

```rust
#[derive(Args)]
struct TransformArgs {
    /// Target layout name or template
    layout: String,

    /// Force transform even with uncommitted changes
    #[arg(short, long)]
    force: bool,

    /// Show plan without executing
    #[arg(long)]
    dry_run: bool,

    /// Also relocate this non-conforming worktree (repeatable)
    #[arg(long = "include", value_name = "BRANCH")]
    include: Vec<String>,

    /// Relocate all non-conforming worktrees
    #[arg(long)]
    include_all: bool,
}
```

- [ ] **Step 2: Replace cmd_transform with plan-based implementation**

Replace the body of `cmd_transform` (after layout resolution, around line 412)
with the new engine. The function should:

1. Detect default branch
2. Build GitCommand
3. Call `read_source_state()`
4. Compute effective project root (adjust for `needs_wrapper()` on current
   layout)
5. Call `compute_target_state()`
6. Call `classify_worktrees()` with `--include` / `--include-all` flags
7. Check for dirty worktrees (abort unless `--force`)
8. Insert `StashChanges` / `PopStash` ops for dirty worktrees if `--force`
9. Call `build_plan()`
10. If `--dry-run`: call `print_plan()` and return
11. Call `execute_plan()`
12. Update repos.json with new layout
13. CD to user's original branch worktree

The key change is replacing the `match (is_currently_bare, target_needs_bare)`
block (lines 433-489) with:

```rust
// Read current state
let default_branch = crate::remote::get_default_branch_local(
    &get_git_common_dir()?,
    &settings.remote,
    settings.use_gitoxide,
)
.unwrap_or_else(|_| "main".to_string());

let source = transform::read_source_state(&git, &default_branch)?;

// Compute effective project root for the TARGET layout
let effective_root = if target_layout.needs_wrapper() || target_layout.needs_bare() {
    // For bare and wrapped layouts, project root is the wrapper
    source.project_root.clone()
} else {
    // For regular non-bare, project root is the repo root
    source.project_root.clone()
};

let target = transform::compute_target_state(
    &target_layout,
    &effective_root,
    &default_branch,
    &source.worktrees,
)?;

let classified = transform::classify_worktrees(
    &source,
    &target,
    &args.include,
    args.include_all,
);

// Check for dirty worktrees
if !args.force {
    let prev_dir = crate::utils::get_current_directory()?;
    for cw in &classified {
        if cw.disposition == transform::WorktreeDisposition::NonConforming {
            continue;
        }
        crate::utils::change_directory(&cw.current_path)?;
        if git.has_uncommitted_changes()? {
            crate::utils::change_directory(&prev_dir)?;
            anyhow::bail!(
                "Worktree '{}' has uncommitted changes. Commit, stash, or use --force.",
                cw.branch
            );
        }
    }
    crate::utils::change_directory(&prev_dir)?;
}

let mut plan = transform::build_plan(&source, &target, &classified, args.force)?;

// Insert stash/pop ops for dirty worktrees when --force
if args.force {
    insert_stash_ops(&mut plan, &classified, &git)?;
}

if args.dry_run {
    transform::print_plan(&plan, output);
    return Ok(());
}

output.start_spinner("Transforming layout...");
let exec_result = {
    let mut sink = OutputSink(output);
    transform::execute_plan(&plan, &git, &mut sink)
};
output.finish_spinner();
exec_result?;
```

- [ ] **Step 3: Add completions for new flags**

Update bash/zsh/fish completions in the layout transform case to include
`--dry-run`, `--include`, `--include-all`.

In `src/commands/completions/bash.rs`, update the transform flags:

```
COMPREPLY=( $(compgen -W "--force -f --dry-run --include --include-all -h --help" -- "$cur") )
```

In `src/commands/completions/zsh.rs`, same update.

In `src/commands/completions/fish.rs`, add:

```
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l dry-run -d 'Show plan without executing'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l include -r -d 'Also relocate this non-conforming worktree'
complete -c daft -n '__fish_seen_subcommand_from layout; and __fish_seen_subcommand_from transform' -l include-all -d 'Relocate all non-conforming worktrees'
```

- [ ] **Step 4: Verify compilation**

Run: `mise run clippy` Expected: 0 warnings.

- [ ] **Step 5: Run unit tests**

Run: `mise run test:unit` Expected: All pass.

- [ ] **Step 6: Build and test manually**

Run: `mise run dev`

Test dry-run:

```bash
cd /path/to/contained-repo
daft layout transform sibling --dry-run
```

Expected: prints the plan without making changes.

- [ ] **Step 7: Regenerate CLI docs and man pages**

Run: `mise run docs:cli:gen && mise run man:gen`

- [ ] **Step 8: Commit**

```bash
git add src/commands/layout.rs src/commands/completions/ docs/cli/ man/
git commit -m "feat(transform): wire plan-based engine into CLI, add --dry-run/--include flags"
```

---

### Task 7: YAML Integration Tests

Add integration test scenarios for key transform paths, especially the new
layouts.

**Files:**

- Create: `tests/manual/scenarios/layout/transform-contained-to-classic.yml`
- Create: `tests/manual/scenarios/layout/transform-classic-to-sibling.yml`
- Create: `tests/manual/scenarios/layout/transform-dry-run.yml`

- [ ] **Step 1: Create contained → contained-classic test**

```yaml
name: Transform contained to contained-classic
description:
  Transform from contained (bare) to contained-classic (wrapped non-bare).
  Verifies .git moves into the default branch subdir and bare flag flips.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with contained layout
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/.git"
        - "$WORK_DIR/test-repo/main"

  - name: Checkout develop
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Transform to contained-classic
    run: daft layout transform contained-classic
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/main/.git"
        - "$WORK_DIR/test-repo/develop"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/main"
          branch: main
        - dir: "$WORK_DIR/test-repo/develop"
          branch: develop
```

- [ ] **Step 2: Create contained-classic → sibling test**

```yaml
name: Transform contained-classic to sibling
description:
  Transform from contained-classic (wrapped non-bare) to sibling. Verifies
  default branch collapses to root and worktrees move to siblings.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with contained-classic layout
    run: git-worktree-clone --layout contained-classic $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Checkout develop
    run: git-worktree-checkout develop
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Transform to sibling
    run: daft layout transform sibling
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo/.git"
      files_exist:
        - "$WORK_DIR/test-repo/README.md"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo"
          branch: main
```

- [ ] **Step 3: Create dry-run test**

```yaml
name: Transform dry run
description: Verify --dry-run shows plan without making changes.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with contained layout
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Dry run transform
    run: daft layout transform sibling --dry-run 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "plan"
      dirs_exist:
        - "$WORK_DIR/test-repo/.git"
        - "$WORK_DIR/test-repo/main"
```

- [ ] **Step 4: Run the existing transform tests to verify no regressions**

Run:
`DAFT_NO_UPDATE_CHECK=1 DAFT_NO_TRUST_PRUNE=1 mise run test:manual -- --ci layout:transform-contained-to-sibling layout:transform-to-contained layout:transform-same-layout-noop`
Expected: All pass (existing tests still work with the new engine).

- [ ] **Step 5: Run the new tests**

Run:
`DAFT_NO_UPDATE_CHECK=1 DAFT_NO_TRUST_PRUNE=1 mise run test:manual -- --ci layout:transform-contained-to-classic layout:transform-dry-run`
Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add tests/manual/scenarios/layout/
git commit -m "test(transform): add YAML scenarios for new transform engine"
```

---

### Task 8: Cleanup and Final Verification

Remove dead code paths and verify the full test suite.

**Files:**

- Modify: `src/commands/layout.rs` (remove old `relocate_worktrees`,
  `transform_to_bare`, `collapse_bare_to_non_bare` if unused)
- Modify: `src/core/layout/transform/mod.rs` (reduce legacy re-exports)

- [ ] **Step 1: Check what legacy functions are still used**

Search for callers of `convert_to_bare`, `convert_to_non_bare`,
`collapse_bare_to_non_bare` outside of `commands/layout.rs`:

```bash
rg 'convert_to_bare|convert_to_non_bare|collapse_bare_to_non_bare' src/ --type rust
```

If only `commands/layout.rs` uses them (and we've replaced that), they can be
removed from re-exports. Keep them in `legacy.rs` for now in case
`adopt`/`eject` still call them.

- [ ] **Step 2: Remove unused local functions from commands/layout.rs**

Remove `relocate_worktrees()`, `transform_to_bare()`,
`collapse_bare_to_non_bare()` from `commands/layout.rs` if they are no longer
called. Keep `relocate_worktrees_public()` if still used by post-clone
reconciliation.

- [ ] **Step 3: Run the full CI suite**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: All
clean.

- [ ] **Step 4: Run the full YAML test suite for layout**

Run:
`DAFT_NO_UPDATE_CHECK=1 DAFT_NO_TRUST_PRUNE=1 mise run test:manual -- --ci layout:transform-contained-to-sibling layout:transform-to-contained layout:transform-same-layout-noop layout:transform-contained-to-classic layout:transform-dry-run`
Expected: All pass.

- [ ] **Step 5: Run the full integration test suite**

Run: `mise run test:integration` Expected: All pass.

- [ ] **Step 6: Commit**

```bash
git add src/
git commit -m "refactor(transform): remove dead code paths replaced by plan-based engine"
```
