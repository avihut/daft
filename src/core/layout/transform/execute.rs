//! Plan executor with rollback support.
//!
//! Iterates through a `TransformPlan`'s operations, executing each one and
//! pushing a reverse operation onto a rollback stack. On failure the stack is
//! unwound in reverse order to restore the repository to its pre-transform
//! state (best-effort).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::plan::{TransformOp, TransformPlan};
use super::status_snapshot::{Artifacts, StatusSnapshot, drift, normalize, porcelain_status};
use crate::core::fs_volume::MoveStrategy;
use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::git::worktree_state::{attach_worktree, detach_worktree};
use crate::hooks::move_hooks::{MoveHookParams, run_setup_hooks, run_teardown_hooks};
use crate::hooks::tracking::TrackedAttribute;

// ── Execution context ────────────────────────────────────────────────────

/// Context needed by move hooks and the integrity check during plan execution.
///
/// Callers populate this from the repository state the plan was built from,
/// so nothing here is re-derived from a CWD the transform may have moved out
/// from under itself.
pub struct ExecutionContext {
    pub project_root: PathBuf,
    /// The `.git` directory the transform starts from.
    pub git_dir: PathBuf,
    /// Where `.git` ends up.
    pub target_git_dir: PathBuf,
    pub remote: String,
    pub source_worktree: PathBuf,
    /// The repository's default branch — what the bare repository's own HEAD
    /// names once its working tree has moved out.
    pub default_branch: String,
    /// Per-worktree `git status --porcelain`, captured before execution.
    /// `ValidateIntegrity` compares against these.
    pub status_snapshots: Vec<StatusSnapshot>,
    /// The normalizer's artifact set, shared with the capture side.
    pub artifacts: Artifacts,
}

// ── Public result type ─────────────────────────────────────────────────────

/// Outcome of executing a transform plan.
#[derive(Debug)]
pub struct ExecuteResult {
    /// Number of operations that completed successfully.
    pub ops_completed: usize,
    /// Total number of operations in the plan.
    pub ops_total: usize,
}

// ── Top-level executor ─────────────────────────────────────────────────────

/// The branch name move hooks see for a worktree; a detached one has none,
/// so hooks get a stable placeholder rather than an empty variable.
fn hook_branch(branch: &Option<String>) -> String {
    branch.clone().unwrap_or_else(|| "detached".to_string())
}

/// Execute every operation in the plan, maintaining a rollback stack.
///
/// On failure the executor attempts to undo completed operations in reverse
/// order, then propagates the original error. Progress messages are emitted
/// via `sink` for each step. Move hooks are fired around every op that changes
/// a worktree's path, using the provided `ExecutionContext`.
pub fn execute_plan(
    plan: &TransformPlan,
    git: &GitCommand,
    ctx: &ExecutionContext,
    sink: &mut (impl ProgressSink + HookRunner),
) -> Result<ExecuteResult> {
    let total = plan.ops.len();
    let mut rollback_stack: Vec<TransformOp> = Vec::new();

    for (i, op) in plan.ops.iter().enumerate() {
        sink.on_step(&format!("[{}/{}] {}", i + 1, total, describe_op(op)));

        // Build move hook params for any op that changes a worktree's path.
        let moved = match op {
            TransformOp::MoveWorktree {
                branch, from, to, ..
            }
            | TransformOp::MoveMainWorktree { branch, from, to } => Some((branch, from, to)),
            TransformOp::CollapseIntoRoot {
                branch,
                worktree_path,
                root_path,
            } => Some((branch, worktree_path, root_path)),
            TransformOp::NestFromRoot {
                branch,
                root_path,
                subdir_path,
            } => Some((branch, root_path, subdir_path)),
            _ => None,
        };
        let move_params = moved.map(|(branch, from, to)| MoveHookParams {
            old_worktree_path: from.clone(),
            new_worktree_path: to.clone(),
            old_branch_name: hook_branch(branch),
            new_branch_name: hook_branch(branch),
            project_root: ctx.project_root.clone(),
            git_dir: ctx.git_dir.clone(),
            remote: ctx.remote.clone(),
            source_worktree: ctx.source_worktree.clone(),
            command: "layout-transform".to_string(),
            changed_attributes: HashSet::from([TrackedAttribute::Path]),
        });

        // Fire teardown hooks before a worktree move
        if let Some(ref params) = move_params {
            run_teardown_hooks(params, sink);
        }

        if let Err(e) = execute_op(op, git, ctx, sink) {
            sink.on_warning(&format!("Operation failed: {e:#}"));
            // Note: rollback does not fire inverse move hooks. Hook-managed
            // state (e.g., direnv, mise trust) may be inconsistent after
            // rollback. This is intentional: hooks are best-effort and
            // non-transactional, consistent with how daft handles hook
            // failures elsewhere.
            sink.on_warning("Attempting rollback of completed operations...");

            if let Err(rb_err) = rollback(&rollback_stack, git, ctx, sink) {
                sink.on_warning(&format!("Rollback encountered errors: {rb_err:#}"));
            }

            return Err(e.context(format!(
                "Failed at step {}/{}: {}",
                i + 1,
                total,
                describe_op(op)
            )));
        }

        // Fire setup hooks after a successful worktree move
        if let Some(ref params) = move_params {
            run_setup_hooks(params, sink);
        }

        if let Some(rev) = reverse_op(op) {
            rollback_stack.push(rev);
        }
    }

    Ok(ExecuteResult {
        ops_completed: total,
        ops_total: total,
    })
}

// ── Per-op dispatch ────────────────────────────────────────────────────────

/// Execute a single transform operation.
fn execute_op(
    op: &TransformOp,
    git: &GitCommand,
    ctx: &ExecutionContext,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    match op {
        TransformOp::MoveWorktree {
            from, to, strategy, ..
        } => exec_move_worktree(from, to, *strategy, ctx, git, progress),

        TransformOp::MoveMainWorktree { from, to, .. } => {
            exec_move_main_worktree(from, to, &ctx.project_root)
        }

        TransformOp::MoveGitDir { from, to } => exec_move_git_dir(from, to, &ctx.project_root),

        TransformOp::SetBare { bare, common_dir } => exec_set_bare(*bare, common_dir),

        TransformOp::RegisterWorktree {
            branch,
            path,
            common_dir,
        } => exec_register_worktree(
            branch.as_deref(),
            path,
            common_dir,
            &ctx.default_branch,
            progress,
        ),

        TransformOp::UnregisterWorktree {
            path, common_dir, ..
        } => exec_unregister_worktree(path, common_dir, progress),

        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
            ..
        } => exec_collapse_into_root(worktree_path, root_path, &ctx.project_root),

        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
            ..
        } => exec_nest_from_root(root_path, subdir_path),

        TransformOp::ValidateIntegrity => exec_validate_integrity(ctx, progress),
    }
}

// ── Individual op implementations ──────────────────────────────────────────

fn exec_move_worktree(
    from: &Path,
    to: &Path,
    strategy: MoveStrategy,
    ctx: &ExecutionContext,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    match strategy {
        MoveStrategy::Rename => {
            let created = ensure_parent_dirs(to)?;

            if let Err(e) = git.worktree_move(from, to) {
                // Leave no scaffolding behind for a move that never happened —
                // the empty `repo/task/` left by the #859 failure came from
                // here.
                for dir in created.iter().rev() {
                    let _ = fs::remove_dir(dir);
                }
                return Err(e);
            }
        }
        MoveStrategy::CopyThenRemove => copy_then_remove(from, to, ctx, progress)?,
    }

    if let Some(parent) = from.parent() {
        prune_empty_dirs_within(parent, &ctx.project_root);
    }

    Ok(())
}

/// A worktree move across volumes: copy → repair git's records → verify →
/// remove the source. The source is untouched until the copy is verified, so
/// an interruption leaves at worst a partial copy to delete.
fn copy_then_remove(
    from: &Path,
    to: &Path,
    ctx: &ExecutionContext,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let created = ensure_parent_dirs(to)?;
    let unwind_dirs = |created: &[PathBuf]| {
        for dir in created.iter().rev() {
            let _ = fs::remove_dir(dir);
        }
    };
    if to.exists() {
        unwind_dirs(&created);
        anyhow::bail!("{} already exists", to.display());
    }

    // The common dir for `worktree repair` — where `.git` is right now.
    // Cross-volume moves are regular linked-worktree moves, emitted after
    // every root-role op, so the target git dir is the one that exists.
    let common_dir = if ctx.target_git_dir.is_dir() {
        ctx.target_git_dir.clone()
    } else {
        ctx.git_dir.clone()
    };
    let repair = |path: &Path| -> Result<()> {
        let out = crate::utils::git_command_at(&common_dir)
            .arg("--git-dir")
            .arg(&common_dir)
            .args(["worktree", "repair"])
            .arg(path)
            .output()
            .context("running git worktree repair")?;
        if !out.status.success() {
            anyhow::bail!(
                "git worktree repair {} failed: {}",
                path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    };

    progress.on_step(&format!(
        "Copying {} to {} (different volume)",
        from.display(),
        to.display()
    ));
    if let Err(e) = crate::cow_copy::copy_dir(from, to) {
        let _ = fs::remove_dir_all(to);
        unwind_dirs(&created);
        return Err(e.context("copying the worktree across volumes"));
    }

    // Point git's registration at the copy, then make sure the copy is a
    // worktree git recognises and that it carries the same state.
    let verified = (|| -> Result<()> {
        repair(to)?;
        let top = crate::utils::git_command_at(to)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("verifying the copied worktree")?;
        if !top.status.success() {
            anyhow::bail!(
                "the copy at {} is not a working tree git recognises",
                to.display()
            );
        }
        if let Some(snapshot) = ctx
            .status_snapshots
            .iter()
            .find(|s| s.source_path == canonical_or_owned(from))
        {
            let after = normalize(&porcelain_status(to)?, &ctx.artifacts);
            let d = drift(&snapshot.lines, &after);
            if !d.missing.is_empty() {
                anyhow::bail!(
                    "the copy at {} is missing {} status entr{} the source has (e.g. {})",
                    to.display(),
                    d.missing.len(),
                    if d.missing.len() == 1 { "y" } else { "ies" },
                    d.missing[0]
                );
            }
        }
        Ok(())
    })();
    if let Err(e) = verified {
        // Repair may have half-rewritten the registration: point it back at
        // the source first, then drop the copy.
        let _ = repair(from);
        let _ = fs::remove_dir_all(to);
        unwind_dirs(&created);
        return Err(e);
    }

    if let Err(e) = fs::remove_dir_all(from) {
        // The move succeeded; a leftover source is worth naming loudly, not
        // worth undoing a verified copy over.
        progress.on_warning(&format!(
            "Copied {} to {} but could not remove the source: {e}. Remove it by hand.",
            from.display(),
            to.display()
        ));
    }
    Ok(())
}

/// Create `to`'s parent chain, returning the directories that did not exist
/// beforehand (shallowest first) so a failed move can remove exactly those.
fn ensure_parent_dirs(to: &Path) -> Result<Vec<PathBuf>> {
    let Some(parent) = to.parent() else {
        return Ok(Vec::new());
    };

    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cursor = Some(parent);
    while let Some(dir) = cursor {
        if dir.exists() {
            break;
        }
        missing.push(dir.to_path_buf());
        cursor = dir.parent();
    }
    missing.reverse();
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create parent directory: {}", parent.display()))?;

    Ok(missing)
}

/// Rename a main working tree's directory, carrying its `.git` with it.
fn exec_move_main_worktree(from: &Path, to: &Path, project_root: &Path) -> Result<()> {
    if to.exists() {
        anyhow::bail!(
            "{} already exists; the main working tree cannot be renamed over it",
            to.display()
        );
    }
    let created = ensure_parent_dirs(to)?;
    if let Err(e) = fs::rename(from, to) {
        for dir in created.iter().rev() {
            let _ = fs::remove_dir(dir);
        }
        return Err(e).with_context(|| {
            format!(
                "Failed to rename the main working tree {} -> {}",
                from.display(),
                to.display()
            )
        });
    }
    // The registrations' own `gitdir` files still name the linked worktrees'
    // unchanged `.git` files and stay correct; what went stale is each linked
    // worktree's `.git` pointer, which named `<from>/.git/worktrees/<n>`.
    fixup_gitdir_references(&to.join(".git"))?;
    // CWD may have been inside `from`.
    crate::utils::change_directory(to)?;
    if let Some(parent) = from.parent() {
        prune_empty_dirs_within(parent, project_root);
    }
    Ok(())
}

fn exec_move_git_dir(from: &Path, to: &Path, project_root: &Path) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create parent directory for .git: {}",
                parent.display()
            )
        })?;
    }

    // If the target path exists as a file (e.g., a worktree's .git pointer
    // file), remove it first — fs::rename can't overwrite a file with a dir.
    if to.is_file() {
        fs::remove_file(to)
            .with_context(|| format!("Failed to remove existing .git file at {}", to.display()))?;
    }

    fs::rename(from, to).with_context(|| {
        format!(
            "Failed to move .git directory from {} to {}",
            from.display(),
            to.display()
        )
    })?;

    fixup_gitdir_references(to)?;

    // CD to the new .git's parent so subsequent git commands can find the repo.
    // After NestFromRoot + MoveGitDir, the old CWD may no longer contain .git.
    if let Some(parent) = to.parent() {
        crate::utils::change_directory(parent)?;
    }

    // Reap the directory `.git` just vacated. `CollapseIntoRoot` cannot do it:
    // when the collapsing worktree is a *main* working tree its `.git` is a
    // real directory, so the `remove_dir` there always fails and the emptied
    // clone directory survives — and the next `NestFromRoot` sweeps that stray
    // into the new worktree (`repo/main` -> `repo/<branch>/main`). Pruning here
    // is a no-op for every other case: the directory is only removed when it is
    // empty and strictly inside the project root.
    if let Some(vacated) = from.parent() {
        prune_empty_dirs_within(vacated, project_root);
    }

    Ok(())
}

/// Flip `core.bare` in `<common_dir>/config` — and nothing else.
///
/// Written through the file rather than through the CWD's repository: the ops
/// around this one move the directory the CWD may be standing in. Trivially
/// reversible, which is what lets a failed plan roll the flag back.
fn exec_set_bare(bare: bool, common_dir: &Path) -> Result<()> {
    let config = common_dir.join("config");
    let value = if bare { "true" } else { "false" };
    let out = crate::utils::git_command_at(common_dir)
        .args(["config", "--file"])
        .arg(&config)
        .args(["core.bare", value])
        .output()
        .with_context(|| format!("setting core.bare in {}", config.display()))?;
    if !out.status.success() {
        anyhow::bail!(
            "Failed to set core.bare = {value} in {}: {}",
            config.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Make `path` a linked worktree of `common_dir`, carrying the main working
/// tree's private git state in with it.
///
/// `common_dir` comes from the op, not `get_git_common_dir()`: that resolver
/// reads CWD, and `exec_collapse_into_root` and `exec_move_git_dir` both change
/// CWD mid-plan — as does a vacate op that moves the directory the user
/// invoked daft from.
fn exec_register_worktree(
    branch: Option<&str>,
    path: &Path,
    common_dir: &Path,
    default_branch: &str,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let attached = attach_worktree(common_dir, path, branch, default_branch)?;
    progress.on_step(&format!(
        "Carried {} git state entr{} into {}",
        attached.relocation.carried(),
        if attached.relocation.carried() == 1 {
            "y"
        } else {
            "ies"
        },
        attached.registration.display()
    ));
    Ok(())
}

/// Dissolve `path`'s registration, carrying its private git state back into
/// `common_dir`.
///
/// Fails closed rather than warn-and-continue when the registration is
/// missing: the pivot is registered by construction on this path, so a miss
/// means the repository is not in the state the plan was built from — and
/// continuing would arm the LIFO inverse (`RegisterWorktree`) for an
/// unregister that never happened. A failing op never pushes its reverse, so
/// bailing keeps the rollback stack honest.
fn exec_unregister_worktree(
    path: &Path,
    common_dir: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let detached = detach_worktree(common_dir, path)?;
    progress.on_step(&format!(
        "Carried {} git state entr{} out of {}",
        detached.relocation.carried(),
        if detached.relocation.carried() == 1 {
            "y"
        } else {
            "ies"
        },
        detached.registration.display()
    ));
    Ok(())
}

/// Canonicalize when the path exists, otherwise keep it verbatim — enough to
/// see through macOS's `/tmp` -> `/private/tmp` symlink without failing on
/// paths a transform has already moved.
fn canonical_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn exec_collapse_into_root(
    worktree_path: &Path,
    root_path: &Path,
    project_root: &Path,
) -> Result<()> {
    let staging = root_path.join(".daft-transform-staging");
    fs::create_dir_all(&staging)
        .with_context(|| format!("Failed to create staging dir: {}", staging.display()))?;

    // Move each file/dir from worktree_path to staging (skip .git)
    for entry in fs::read_dir(worktree_path)
        .with_context(|| format!("Failed to read worktree dir: {}", worktree_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        fs::rename(entry.path(), staging.join(&name))
            .with_context(|| format!("Failed to move {} to staging", entry.path().display()))?;
    }

    // Remove the worktree's .git pointer file (it's a text file, not a
    // directory) if `UnregisterWorktree` has not already taken it away. A main
    // working tree's real `.git` *directory* is left for `MoveGitDir`.
    let wt_git_file = worktree_path.join(".git");
    if wt_git_file.is_file() {
        fs::remove_file(&wt_git_file).ok();
    }

    // Remove the now-empty worktree dir
    fs::remove_dir(worktree_path).ok();

    // CD to root_path — the old worktree dir may have been the CWD, and it's
    // now deleted. Subsequent ops (SetBare, etc.) need a valid CWD.
    crate::utils::change_directory(root_path)?;

    // Move from staging to root
    for entry in fs::read_dir(&staging)
        .with_context(|| format!("Failed to read staging dir: {}", staging.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        fs::rename(entry.path(), root_path.join(&name)).with_context(|| {
            format!(
                "Failed to move {} from staging to root",
                entry.path().display()
            )
        })?;
    }

    fs::remove_dir(&staging).ok();

    // `/repo/task/x` -> `/repo` leaves `/repo/task` behind.
    if let Some(parent) = worktree_path.parent() {
        prune_empty_dirs_within(parent, project_root);
    }

    Ok(())
}

fn exec_nest_from_root(root_path: &Path, subdir_path: &Path) -> Result<()> {
    let staging = root_path.join(".daft-transform-staging");
    fs::create_dir_all(&staging)
        .with_context(|| format!("Failed to create staging dir: {}", staging.display()))?;

    let staging_name = staging
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();

    // Move each file/dir from root into staging, skipping .git, the staging
    // dir itself, and linked worktree directories (directories containing a
    // .git file, which indicates they are linked worktrees).
    for entry in fs::read_dir(root_path)
        .with_context(|| format!("Failed to read root dir: {}", root_path.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();

        if name == ".git" || name == staging_name {
            continue;
        }

        // Skip linked worktree directories (they have a .git *file* inside)
        if entry.file_type()?.is_dir() {
            let dotgit = entry.path().join(".git");
            if dotgit.exists() && dotgit.is_file() {
                continue;
            }
        }

        fs::rename(entry.path(), staging.join(&name))
            .with_context(|| format!("Failed to move {} to staging", entry.path().display()))?;
    }

    // Create target subdir and move files there
    fs::create_dir_all(subdir_path)
        .with_context(|| format!("Failed to create subdir: {}", subdir_path.display()))?;

    for entry in fs::read_dir(&staging)
        .with_context(|| format!("Failed to read staging dir: {}", staging.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        fs::rename(entry.path(), subdir_path.join(&name)).with_context(|| {
            format!(
                "Failed to move {} from staging to subdir",
                entry.path().display()
            )
        })?;
    }

    fs::remove_dir(&staging).ok();

    Ok(())
}

/// `git fsck`, plus every worktree's status compared against the snapshot
/// taken before execution.
///
/// The previous version failed the plan on **any** dirty worktree — a dirty
/// gate dressed as an integrity check. Dirt is expected now; what must not
/// change is *which* dirt. A status entry that vanished means something the
/// transform was supposed to carry did not arrive, and fails the plan (which
/// rolls it back). An entry that appeared is a warning: a move hook writing
/// into the tree is not a loss.
fn exec_validate_integrity(ctx: &ExecutionContext, progress: &mut dyn ProgressSink) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    // 1. Run git fsck to check repository integrity
    progress.on_step("Running git fsck...");
    match crate::utils::git_command_at(&ctx.project_root)
        .arg("--git-dir")
        .arg(&ctx.target_git_dir)
        .args(["fsck", "--no-dangling"])
        .output()
    {
        Ok(result) if result.status.success() => {}
        Ok(result) => {
            let stderr = String::from_utf8_lossy(&result.stderr);
            let msg = stderr.trim();
            if !msg.is_empty() {
                errors.push(format!("git fsck: {msg}"));
            }
        }
        Err(e) => {
            progress.on_warning(&format!("Could not run git fsck: {e}"));
        }
    }

    // 2. Every snapshotted worktree must be where the plan put it, with the
    //    same status.
    progress.on_step("Verifying worktree state...");
    for snapshot in &ctx.status_snapshots {
        if !snapshot.target_path.is_dir() {
            errors.push(format!(
                "worktree expected at {} is missing",
                snapshot.target_path.display()
            ));
            continue;
        }
        let after = match porcelain_status(&snapshot.target_path) {
            Ok(raw) => normalize(&raw, &ctx.artifacts),
            Err(e) => {
                errors.push(format!("{e:#}"));
                continue;
            }
        };
        let d = drift(&snapshot.lines, &after);
        if !d.missing.is_empty() {
            let sample: Vec<&str> = d.missing.iter().take(3).map(String::as_str).collect();
            errors.push(format!(
                "worktree at {} lost {} status entr{} across the transform ({}{})",
                snapshot.target_path.display(),
                d.missing.len(),
                if d.missing.len() == 1 { "y" } else { "ies" },
                sample.join("; "),
                if d.missing.len() > 3 { "; …" } else { "" }
            ));
        }
        if !d.extra.is_empty() {
            progress.on_warning(&format!(
                "worktree at {} gained {} status entr{} during the transform (a hook wrote into it?): {}",
                snapshot.target_path.display(),
                d.extra.len(),
                if d.extra.len() == 1 { "y" } else { "ies" },
                d.extra.join("; ")
            ));
        }
    }

    if errors.is_empty() {
        progress.on_step("Integrity check passed");
        Ok(())
    } else {
        for err in &errors {
            progress.on_warning(&format!("Integrity issue: {err}"));
        }
        anyhow::bail!(
            "Transform completed but integrity check found {} issue{}: {}",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" },
            errors.join("; ")
        )
    }
}

// ── Gitdir fixup ───────────────────────────────────────────────────────────

/// After moving `.git`, update worktree `.git` files to point to the new
/// worktrees registration paths.
///
/// Each file at `<new_git_dir>/worktrees/<name>/gitdir` contains the absolute
/// path to a worktree's `.git` file. We read that path and then overwrite the
/// worktree's `.git` file so it points back to `<new_git_dir>/worktrees/<name>`.
fn fixup_gitdir_references(new_git_dir: &Path) -> Result<()> {
    let worktrees_dir = new_git_dir.join("worktrees");
    if !worktrees_dir.exists() {
        return Ok(());
    }

    let entries = fs::read_dir(&worktrees_dir).with_context(|| {
        format!(
            "Failed to read worktrees directory: {}",
            worktrees_dir.display()
        )
    })?;

    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let gitdir_file = entry.path().join("gitdir");
        if !gitdir_file.exists() {
            continue;
        }

        // The gitdir file contains the absolute path to the worktree's .git file
        let worktree_git_path = fs::read_to_string(&gitdir_file)
            .with_context(|| format!("Failed to read gitdir file: {}", gitdir_file.display()))?;
        let worktree_git_path = PathBuf::from(worktree_git_path.trim());

        if !worktree_git_path.exists() {
            continue;
        }

        // Skip if the path is a directory — this means .git moved INTO this
        // worktree's location (e.g., contained-classic where the default
        // branch directory IS where .git lives). No pointer file to update.
        if worktree_git_path.is_dir() {
            continue;
        }

        // Update the worktree's .git file to point to the new registration path
        let new_registration_path = entry.path();
        fs::write(
            &worktree_git_path,
            format!("gitdir: {}\n", new_registration_path.display()),
        )
        .with_context(|| {
            format!(
                "Failed to update .git file at {}",
                worktree_git_path.display()
            )
        })?;
    }

    Ok(())
}

// ── Rollback ───────────────────────────────────────────────────────────────

/// Compute the reverse operation for rollback purposes.
///
/// Every op but `ValidateIntegrity` has an exact inverse: moves swap their
/// ends, the bare flag flips back, and `RegisterWorktree` ⇄
/// `UnregisterWorktree` carry the same state the other way — against the
/// `common_dir` recorded at plan time, which LIFO unwinding puts back on the
/// right side of any `MoveGitDir` undo.
fn reverse_op(op: &TransformOp) -> Option<TransformOp> {
    match op {
        TransformOp::MoveWorktree {
            branch,
            from,
            to,
            strategy,
        } => Some(TransformOp::MoveWorktree {
            branch: branch.clone(),
            from: to.clone(),
            to: from.clone(),
            strategy: *strategy,
        }),

        TransformOp::MoveMainWorktree { branch, from, to } => Some(TransformOp::MoveMainWorktree {
            branch: branch.clone(),
            from: to.clone(),
            to: from.clone(),
        }),

        TransformOp::MoveGitDir { from, to } => Some(TransformOp::MoveGitDir {
            from: to.clone(),
            to: from.clone(),
        }),

        TransformOp::SetBare { bare, common_dir } => Some(TransformOp::SetBare {
            bare: !bare,
            common_dir: common_dir.clone(),
        }),

        TransformOp::RegisterWorktree {
            branch,
            path,
            common_dir,
        } => Some(TransformOp::UnregisterWorktree {
            branch: branch.clone(),
            path: path.clone(),
            common_dir: common_dir.clone(),
        }),

        TransformOp::UnregisterWorktree {
            branch,
            path,
            common_dir,
        } => Some(TransformOp::RegisterWorktree {
            branch: branch.clone(),
            path: path.clone(),
            common_dir: common_dir.clone(),
        }),

        TransformOp::CollapseIntoRoot {
            branch,
            worktree_path,
            root_path,
        } => Some(TransformOp::NestFromRoot {
            branch: branch.clone(),
            root_path: root_path.clone(),
            subdir_path: worktree_path.clone(),
        }),

        TransformOp::NestFromRoot {
            branch,
            root_path,
            subdir_path,
        } => Some(TransformOp::CollapseIntoRoot {
            branch: branch.clone(),
            worktree_path: subdir_path.clone(),
            root_path: root_path.clone(),
        }),

        TransformOp::ValidateIntegrity => None,
    }
}

/// Execute reverse operations in reverse order to undo a partial transform.
fn rollback(
    stack: &[TransformOp],
    git: &GitCommand,
    ctx: &ExecutionContext,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let mut first_error: Option<anyhow::Error> = None;

    for op in stack.iter().rev() {
        progress.on_step(&format!("Rollback: {}", describe_op(op)));
        if let Err(e) = execute_op(op, git, ctx, progress) {
            progress.on_warning(&format!("Rollback step failed: {e:#}"));
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    match first_error {
        Some(e) => Err(e.context("Rollback completed with errors")),
        None => {
            progress.on_step("Rollback completed successfully");
            Ok(())
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Remove `start` and its ancestors while they are empty, stopping at — and
/// never removing — `project_root`.
///
/// Only acts on directories strictly inside `project_root`, so the parents of
/// sibling and centralized worktrees (which daft does not own) are left alone.
/// The predecessor of this helper started at the *moved* path, which no longer
/// exists, so it broke on its first `remove_dir` and never climbed at all.
fn prune_empty_dirs_within(start: &Path, project_root: &Path) {
    let root = canonical_or_owned(project_root);
    let mut current = canonical_or_owned(start);

    while current != root && current.starts_with(&root) {
        if fs::remove_dir(&current).is_err() {
            break;
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => break,
        }
    }
}

fn label(branch: &Option<String>) -> &str {
    branch.as_deref().unwrap_or("(detached)")
}

/// Human-readable description of a transform operation, suitable for progress
/// output.
pub fn describe_op(op: &TransformOp) -> String {
    match op {
        TransformOp::MoveWorktree {
            branch,
            from,
            to,
            strategy,
        } => {
            format!(
                "Move worktree '{}': {} -> {}{}",
                label(branch),
                from.display(),
                to.display(),
                match strategy {
                    MoveStrategy::Rename => "",
                    MoveStrategy::CopyThenRemove => " (copy across volumes)",
                }
            )
        }
        TransformOp::MoveMainWorktree { branch, from, to } => {
            format!(
                "Rename the main working tree '{}': {} -> {}",
                label(branch),
                from.display(),
                to.display()
            )
        }
        TransformOp::MoveGitDir { from, to } => {
            format!("Move .git: {} -> {}", from.display(), to.display())
        }
        TransformOp::SetBare { bare, .. } => {
            format!("Set core.bare = {bare}")
        }
        TransformOp::RegisterWorktree { branch, path, .. } => {
            format!(
                "Register worktree '{}' at {}, carrying its git state",
                label(branch),
                path.display()
            )
        }
        TransformOp::UnregisterWorktree { branch, path, .. } => {
            format!(
                "Unregister worktree '{}' at {}, carrying its git state out",
                label(branch),
                path.display()
            )
        }
        TransformOp::CollapseIntoRoot {
            worktree_path,
            root_path,
            ..
        } => {
            format!(
                "Collapse {} into {}",
                worktree_path.display(),
                root_path.display()
            )
        }
        TransformOp::NestFromRoot {
            root_path,
            subdir_path,
            ..
        } => {
            format!(
                "Nest {} into {}",
                root_path.display(),
                subdir_path.display()
            )
        }
        TransformOp::ValidateIntegrity => "Validate repository integrity".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::NullBridge;
    use crate::core::layout::BuiltinLayout;
    use crate::test_support::CwdGuard;
    use serial_test::serial;
    use std::path::Path;
    use tempfile::TempDir;

    use super::super::plan::{build_plan, classify_worktrees};
    use super::super::state::{
        ClassifiedWorktree, LayoutState, PivotOverride, compute_target_state, read_source_state,
    };
    use super::super::status_snapshot::capture;

    /// Run git in `dir` with a fixed test identity — never global config
    /// (CLAUDE.md Critical Rule #1).
    fn git_ok(dir: &Path, args: &[&str]) -> String {
        let out = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {args:?} failed in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A throwaway non-bare repo at `<tmp>/repo` whose main working tree is on
    /// `branch`. Paths are canonicalized so they compare against the state the
    /// engine reads back.
    fn scratch_repo(branch: &str) -> (TempDir, PathBuf) {
        let base = tempfile::tempdir().expect("tempdir");
        let repo = base.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        fs::write(repo.join("main.py"), "print(1)\n").unwrap();
        git_ok(&repo, &["add", "."]);
        git_ok(&repo, &["commit", "-q", "-m", "init"]);
        if branch != "main" {
            git_ok(&repo, &["switch", "-q", "-c", branch]);
        }
        let repo = repo.canonicalize().unwrap();
        (base, repo)
    }

    /// A throwaway repo in daft's bare shape: a bare git dir at `<repo>/.git`
    /// with linked worktrees beside it. `worktrees` are `(branch, subdir)`.
    fn scratch_bare_repo(worktrees: &[(&str, &str)]) -> (TempDir, PathBuf) {
        let base = tempfile::tempdir().expect("tempdir");
        let src = base.path().join("src");
        fs::create_dir_all(&src).unwrap();
        git_ok(&src, &["init", "-q", "-b", "main"]);
        fs::write(src.join("README.md"), "hello\n").unwrap();
        git_ok(&src, &["add", "."]);
        git_ok(&src, &["commit", "-q", "-m", "init"]);

        let repo = base.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_ok(
            base.path(),
            &[
                "clone",
                "-q",
                "--bare",
                src.to_str().unwrap(),
                repo.join(".git").to_str().unwrap(),
            ],
        );
        let repo = repo.canonicalize().unwrap();

        for (branch, subdir) in worktrees {
            let path = repo.join(subdir);
            git_ok(
                &repo.join(".git"),
                &[
                    "worktree",
                    "add",
                    "-q",
                    path.to_str().unwrap(),
                    "-b",
                    branch,
                ],
            );
        }
        (base, repo)
    }

    /// Feed `input` to `git <args>` in `dir`.
    fn git_stdin(dir: &Path, args: &[&str], input: &str) -> String {
        use std::io::Write;
        let mut child = crate::utils::git_command_at(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn git");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// Dirty `wt` in every way a tree can be dirty without an in-progress
    /// operation: modified, staged, staged-then-modified, untracked,
    /// intent-to-add, and an unmerged entry built directly in the index (a
    /// real merge conflict would leave `MERGE_HEAD`, which is a blocker).
    fn dirty_every_way(wt: &Path) {
        fs::write(wt.join("README.md"), "hello\nlocal\n").unwrap(); // ' M'
        fs::write(wt.join("main.py"), "print(2)\n").unwrap();
        git_ok(wt, &["add", "main.py"]); // 'M '
        fs::write(wt.join("both.txt"), "a\n").unwrap();
        git_ok(wt, &["add", "both.txt"]);
        fs::write(wt.join("both.txt"), "a\nb\n").unwrap(); // 'AM'
        fs::write(wt.join("NOTES.md"), "scratch\n").unwrap(); // '??'
        fs::write(wt.join("planned.txt"), "soon\n").unwrap();
        git_ok(wt, &["add", "-N", "planned.txt"]); // intent-to-add
        let b = git_stdin(wt, &["hash-object", "-w", "--stdin"], "base\n");
        let o = git_stdin(wt, &["hash-object", "-w", "--stdin"], "ours\n");
        let t = git_stdin(wt, &["hash-object", "-w", "--stdin"], "theirs\n");
        fs::write(
            wt.join("conflicted.txt"),
            "<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n",
        )
        .unwrap();
        let info = format!(
            "100644 {b} 1\tconflicted.txt\n100644 {o} 2\tconflicted.txt\n100644 {t} 3\tconflicted.txt\n"
        );
        git_stdin(wt, &["update-index", "--index-info"], &info);
    }

    fn ctx_for(source: &LayoutState, target: &LayoutState) -> ExecutionContext {
        ExecutionContext {
            project_root: source.project_root.clone(),
            git_dir: source.git_dir.clone(),
            target_git_dir: target.git_dir.clone(),
            remote: "origin".to_string(),
            source_worktree: source.project_root.clone(),
            default_branch: source.default_branch.clone(),
            status_snapshots: Vec::new(),
            artifacts: Artifacts::default(),
        }
    }

    /// Read the source state, compute the target, classify, snapshot, and
    /// build the plan — the way `cmd_transform` does.
    #[allow(clippy::type_complexity)]
    fn prepare(
        git: &GitCommand,
        layout: BuiltinLayout,
    ) -> (
        LayoutState,
        LayoutState,
        Vec<ClassifiedWorktree>,
        TransformPlan,
        ExecutionContext,
    ) {
        let source = read_source_state(git, "main").unwrap();
        let target =
            compute_target_state(&layout.to_layout(), &source, &PivotOverride::default()).unwrap();
        let classified = classify_worktrees(&source, &target, &[], false);
        let artifacts = Artifacts::for_transform(&source, &target);
        let snapshots = capture(&classified, &artifacts).unwrap();
        let plan = build_plan(&source, &target, &classified, &snapshots).unwrap();
        let mut ctx = ctx_for(&source, &target);
        ctx.status_snapshots = snapshots;
        ctx.artifacts = artifacts;
        (source, target, classified, plan, ctx)
    }

    /// Names of the `worktrees/<n>` registration dirs, sorted.
    fn registrations(git_dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(git_dir.join("worktrees"))
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// An op guaranteed to fail without having touched anything.
    fn doomed_op(base: &Path) -> TransformOp {
        TransformOp::MoveWorktree {
            branch: Some("no-such-branch".to_string()),
            from: base.join("does-not-exist"),
            to: base.join("nowhere/at/all"),
            strategy: MoveStrategy::Rename,
        }
    }

    fn status(wt: &Path) -> String {
        git_ok(wt, &["status", "--porcelain"])
    }

    fn diff_cached(wt: &Path) -> String {
        git_ok(wt, &["diff", "--cached"])
    }

    // ── #859 / #875 ──────────────────────────────────────────────────────

    /// A plain clone whose main working tree is on a feature branch, dirty
    /// in every way, taken to `contained` and then failed at the last step:
    /// the repository must come back exactly as it was — index bytes included,
    /// because rollback *moves* it back rather than rebuilding it.
    #[test]
    #[serial]
    fn failed_transform_of_a_nondefault_root_rolls_back_completely() {
        let (base, repo) = scratch_repo("feature/x");
        let dev = repo.parent().unwrap().join("repo.dev");
        git_ok(
            &repo,
            &["worktree", "add", "-q", dev.to_str().unwrap(), "-b", "dev"],
        );
        dirty_every_way(&repo);
        let status_before = status(&repo);
        let index_before = fs::read(repo.join(".git/index")).unwrap();

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);

        let (source, _target, _classified, mut plan, ctx) = prepare(&git, BuiltinLayout::Contained);
        assert!(
            source
                .worktrees
                .iter()
                .any(|wt| wt.branch.as_deref() == Some("feature/x") && wt.is_root),
            "the main working tree holds the root role even on a feature branch"
        );

        let before_list = git_ok(&repo, &["worktree", "list", "--porcelain"]);
        let before_regs = registrations(&source.git_dir);

        // Fail after every real op has run, just before validation.
        let last = plan.ops.len() - 1;
        plan.ops.insert(last, doomed_op(base.path()));

        execute_plan(&plan, &git, &ctx, &mut NullBridge)
            .expect_err("the doomed op must fail the plan");

        assert!(
            repo.join(".git/index").exists(),
            "rollback must not leave the repo without an index"
        );
        assert_eq!(
            fs::read(repo.join(".git/index")).unwrap(),
            index_before,
            "the index must come back byte for byte — moved, not rebuilt"
        );
        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "false"
        );
        assert_eq!(
            status(&repo),
            status_before,
            "every status entry must be back"
        );
        assert_eq!(
            git_ok(&repo, &["worktree", "list", "--porcelain"]),
            before_list
        );
        assert_eq!(registrations(&source.git_dir), before_regs);
        assert!(
            repo.join("README.md").exists(),
            "the nested files must be back at the root"
        );
        assert!(
            !repo.join("feature").exists(),
            "the abandoned nest directory must not survive"
        );
    }

    /// The same transform, allowed to finish: the end state is a bare repo with
    /// the feature branch registered as a linked worktree whose index and HEAD
    /// were *carried*, not rebuilt — so every kind of dirt survives.
    #[test]
    #[serial]
    fn successful_transform_of_a_nondefault_root_registers_it_and_carries_its_index() {
        let (_base, repo) = scratch_repo("feature/x");
        dirty_every_way(&repo);
        let status_before = status(&repo);
        let cached_before = diff_cached(&repo);
        assert!(
            status_before.contains("UU conflicted.txt"),
            "{status_before}"
        );
        assert!(status_before.contains("M  main.py"), "{status_before}");
        assert!(status_before.contains("AM both.txt"), "{status_before}");
        let index_before = fs::read(repo.join(".git/index")).unwrap();

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let (_source, _target, _classified, plan, ctx) = prepare(&git, BuiltinLayout::Contained);

        execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("transform should succeed");

        let wt = repo.join("feature/x");
        assert!(wt.join("README.md").exists());
        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "true"
        );
        assert_eq!(status(&wt), status_before, "status must be byte-identical");
        assert_eq!(
            diff_cached(&wt),
            cached_before,
            "the staged diff must be identical"
        );
        assert_eq!(
            git_ok(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
            "feature/x"
        );
        assert!(
            !repo.join(".git/index").exists(),
            "a bare repo keeps no working-tree index"
        );
        let reg = repo.join(".git/worktrees/x");
        assert!(
            reg.join("index").exists(),
            "the index lives in the registration"
        );
        assert_eq!(
            fs::read(reg.join("index")).unwrap(),
            index_before,
            "the index was moved, byte for byte"
        );
        assert_eq!(
            fs::read_to_string(repo.join(".git/HEAD")).unwrap(),
            "ref: refs/heads/main\n",
            "the bare repository's HEAD names the default branch"
        );
        assert!(
            git_ok(&wt, &["stash", "list"]).trim().is_empty(),
            "nothing was stashed"
        );
    }

    /// The bare mirror: no worktree for the default branch, so the sole
    /// worktree takes the root. A failure must put the registration — and the
    /// dirty state it carried — back.
    #[test]
    #[serial]
    fn failed_bare_to_nonbare_transform_restores_the_registration() {
        let (base, repo) = scratch_bare_repo(&[("feature/x", "feature/x")]);
        let wt = repo.join("feature/x");
        fs::write(wt.join("README.md"), "hello\nmore\n").unwrap();
        fs::write(wt.join("NOTES.md"), "x\n").unwrap();
        git_ok(&wt, &["add", "NOTES.md"]);
        let status_before = status(&wt);
        let index_before = fs::read(repo.join(".git/worktrees/x/index")).unwrap();

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);

        let (source, _target, _classified, mut plan, ctx) = prepare(&git, BuiltinLayout::Sibling);
        assert!(source.is_bare);
        assert!(source.worktrees.iter().all(|wt| !wt.is_root));

        let before_list = git_ok(&repo, &["worktree", "list", "--porcelain"]);

        let last = plan.ops.len() - 1;
        plan.ops.insert(last, doomed_op(base.path()));

        execute_plan(&plan, &git, &ctx, &mut NullBridge)
            .expect_err("the doomed op must fail the plan");

        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "true"
        );
        assert_eq!(
            git_ok(&repo, &["worktree", "list", "--porcelain"]),
            before_list,
            "the worktree must be registered again at its original path"
        );
        assert!(wt.join("README.md").exists());
        assert_eq!(status(&wt), status_before);
        assert_eq!(
            fs::read(repo.join(".git/worktrees/x/index")).unwrap(),
            index_before,
            "the registration's index is back, byte for byte"
        );
        assert!(
            !repo.join(".git/index").exists(),
            "the index carried to the root must not survive the rollback"
        );
        assert!(
            !repo.join("README.md").exists(),
            "collapsed files must go back into the worktree"
        );
    }

    /// The bare mirror, allowed to finish: a dirty pivot becomes the main
    /// working tree with its state intact.
    #[test]
    #[serial]
    fn bare_to_nonbare_with_a_dirty_pivot_carries_it_through() {
        let (_base, repo) = scratch_bare_repo(&[("feature/x", "feature/x")]);
        let wt = repo.join("feature/x");
        fs::write(wt.join("main.py"), "print(1)\n").unwrap();
        git_ok(&wt, &["add", "main.py"]);
        git_ok(&wt, &["commit", "-q", "-m", "add main.py"]);
        dirty_every_way(&wt);
        let status_before = status(&wt);
        let cached_before = diff_cached(&wt);

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let (_source, _target, _classified, plan, ctx) = prepare(&git, BuiltinLayout::Sibling);
        execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("transform should succeed");

        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "false"
        );
        assert_eq!(
            git_ok(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
            "feature/x"
        );
        assert_eq!(status(&repo), status_before);
        assert_eq!(diff_cached(&repo), cached_before);
        assert!(registrations(&repo.join(".git")).is_empty());
        assert!(!wt.exists());
    }

    #[test]
    #[serial]
    fn carried_state_survives_a_full_round_trip() {
        let (_base, repo) = scratch_repo("feature/x");
        dirty_every_way(&repo);
        git_ok(&repo, &["update-ref", "ORIG_HEAD", "HEAD"]);
        let status_before = status(&repo);
        let reflog_lines = fs::read_to_string(repo.join(".git/logs/HEAD"))
            .unwrap()
            .lines()
            .count();

        // One `GitCommand` per leg: its gitoxide handle is discovered from
        // the CWD once and cached, exactly as a fresh process would see it.
        {
            let _cwd = CwdGuard::enter(&repo);
            let git = GitCommand::new(false);
            let (_s, _t, _c, plan, ctx) = prepare(&git, BuiltinLayout::Contained);
            execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("out");
        }
        let wt = repo.join("feature/x");
        assert_eq!(status(&wt), status_before);
        assert!(repo.join(".git/worktrees/x/ORIG_HEAD").exists());
        {
            let _cwd = CwdGuard::enter(&wt);
            let git = GitCommand::new(false);
            let (_s, _t, _c, plan, ctx) = prepare(&git, BuiltinLayout::Sibling);
            execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("back");
        }
        assert_eq!(status(&repo), status_before);
        assert!(repo.join(".git/ORIG_HEAD").exists(), "ORIG_HEAD came back");
        assert_eq!(
            fs::read_to_string(repo.join(".git/logs/HEAD"))
                .unwrap()
                .lines()
                .count(),
            reflog_lines,
            "the HEAD reflog was carried both ways"
        );
        assert!(!wt.exists());
    }

    #[test]
    #[serial]
    fn transform_preserves_a_detached_main_working_trees_head() {
        let (_base, repo) = scratch_repo("main");
        let oid = git_ok(&repo, &["rev-parse", "HEAD"]).trim().to_string();
        git_ok(&repo, &["switch", "-q", "--detach", "HEAD"]);
        fs::write(repo.join("README.md"), "hello\nlocal\n").unwrap();

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let source = read_source_state(&git, "main").unwrap();
        assert_eq!(source.worktrees[0].branch, None);
        let over = PivotOverride {
            branch: None,
            dirname: Some("sandbox".into()),
        };
        let target =
            compute_target_state(&BuiltinLayout::Contained.to_layout(), &source, &over).unwrap();
        let classified = classify_worktrees(&source, &target, &[], false);
        let artifacts = Artifacts::for_transform(&source, &target);
        let snapshots = capture(&classified, &artifacts).unwrap();
        let plan = build_plan(&source, &target, &classified, &snapshots).unwrap();
        let mut ctx = ctx_for(&source, &target);
        ctx.status_snapshots = snapshots;
        execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("transform should succeed");

        let wt = repo.join("sandbox");
        assert_eq!(git_ok(&wt, &["rev-parse", "HEAD"]).trim(), oid);
        assert_eq!(
            git_ok(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
            "HEAD"
        );
        assert_eq!(status(&wt), " M README.md\n");
    }

    #[test]
    #[serial]
    fn drifted_contained_classic_renames_the_main_working_tree_and_repairs_linked_pointers() {
        // contained-classic: the clone is `/repo/main`, switched to feature/x,
        // with a linked worktree beside it.
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("repo");
        let main = root.join("main");
        fs::create_dir_all(&main).unwrap();
        git_ok(&main, &["init", "-q", "-b", "main"]);
        fs::write(main.join("README.md"), "hello\n").unwrap();
        git_ok(&main, &["add", "."]);
        git_ok(&main, &["commit", "-q", "-m", "init"]);
        let dev = root.join("dev");
        git_ok(
            &main,
            &["worktree", "add", "-q", dev.to_str().unwrap(), "-b", "dev"],
        );
        git_ok(&main, &["switch", "-q", "-c", "feature/x"]);
        fs::write(main.join("README.md"), "hello\nlocal\n").unwrap();
        let root = root.canonicalize().unwrap();
        let main = root.join("main");
        let dev = root.join("dev");

        let _cwd = CwdGuard::enter(&main);
        let git = GitCommand::new(false);
        // `cmd_transform` widens a wrapped layout's project root to the
        // wrapper directory (from the layout recorded in repos.json); do the
        // same by hand here.
        let mut source = read_source_state(&git, "main").unwrap();
        source.project_root = root.clone();
        let target = compute_target_state(
            &BuiltinLayout::ContainedClassic.to_layout(),
            &source,
            &PivotOverride::default(),
        )
        .unwrap();
        let classified = classify_worktrees(&source, &target, &[], false);
        let artifacts = Artifacts::for_transform(&source, &target);
        let snapshots = capture(&classified, &artifacts).unwrap();
        let plan = build_plan(&source, &target, &classified, &snapshots).unwrap();
        let mut ctx = ctx_for(&source, &target);
        ctx.status_snapshots = snapshots;
        assert!(
            plan.ops
                .iter()
                .any(|op| matches!(op, TransformOp::MoveMainWorktree { .. })),
            "{:?}",
            plan.ops
        );
        execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("transform should succeed");

        let moved = root.join("feature/x");
        assert!(
            moved.join(".git").is_dir(),
            ".git travelled inside the renamed directory"
        );
        assert!(!main.exists());
        assert_eq!(status(&moved), " M README.md\n");
        // The linked worktree still works: its pointer was repaired.
        assert_eq!(status(&dev), "");
        assert!(
            fs::read_to_string(dev.join(".git"))
                .unwrap()
                .contains("feature/x/.git/worktrees/"),
            "the pointer names the new main working tree's git dir"
        );
    }

    #[test]
    #[serial]
    fn cross_volume_strategy_copies_repairs_and_removes_the_source() {
        // CI has one volume, so the copy branch is driven by constructing
        // the op directly rather than by a real `EXDEV`.
        let (_base, repo) = scratch_repo("main");
        let dev = repo.parent().unwrap().join("repo.dev");
        git_ok(
            &repo,
            &["worktree", "add", "-q", dev.to_str().unwrap(), "-b", "dev"],
        );
        fs::write(dev.join("README.md"), "hello\ndev\n").unwrap();
        fs::write(dev.join("scratch.txt"), "x\n").unwrap();
        let status_before = status(&dev);
        let to = repo.parent().unwrap().join("elsewhere/dev");

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let source = read_source_state(&git, "main").unwrap();
        let mut ctx = ctx_for(&source, &source);
        let artifacts = Artifacts::default();
        let classified = vec![ClassifiedWorktree {
            branch: Some("dev".into()),
            current_path: dev.clone(),
            target_path: to.clone(),
            disposition: super::super::state::WorktreeDisposition::Conforming,
        }];
        ctx.status_snapshots = capture(&classified, &artifacts).unwrap();
        let plan = TransformPlan {
            ops: vec![
                TransformOp::MoveWorktree {
                    branch: Some("dev".into()),
                    from: dev.clone(),
                    to: to.clone(),
                    strategy: MoveStrategy::CopyThenRemove,
                },
                TransformOp::ValidateIntegrity,
            ],
            skipped: vec![],
            description: String::new(),
            carried: vec![],
        };
        execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("copy move should succeed");

        assert!(!dev.exists(), "the source is removed after verification");
        assert_eq!(status(&to), status_before);
        assert!(
            git_ok(&repo, &["worktree", "list", "--porcelain"]).contains(to.to_str().unwrap()),
            "git's registration points at the copy"
        );
    }

    #[test]
    #[serial]
    fn validate_integrity_bails_when_a_worktree_loses_a_change() {
        let (_base, repo) = scratch_repo("feature/x");
        fs::write(repo.join("README.md"), "hello\nlocal\n").unwrap();
        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let (_s, _t, _c, plan, mut ctx) = prepare(&git, BuiltinLayout::Contained);
        // Claim the tree had an entry it never had: the post-transform status
        // will be "missing" it, which must fail the plan and roll it back.
        ctx.status_snapshots[0]
            .lines
            .push("?? never-existed.txt".to_string());
        let err = execute_plan(&plan, &git, &ctx, &mut NullBridge).expect_err("must fail");
        assert!(format!("{err:#}").contains("never-existed.txt"), "{err:#}");
        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "false",
            "rolled back"
        );
        assert_eq!(status(&repo), " M README.md\n");
    }

    /// Mid-rebase HEAD is detached, but `rebase-merge/head-name` still names
    /// the branch: the source state must carry it, so a paused rebase reads
    /// as exactly that and not also as a nameless detached tree.
    #[test]
    #[serial]
    fn source_state_recovers_the_branch_of_a_main_worktree_mid_rebase() {
        let (_base, repo) = scratch_repo("feature/x");
        git_ok(&repo, &["switch", "-q", "--detach", "HEAD"]);
        let rebase = repo.join(".git/rebase-merge");
        fs::create_dir_all(&rebase).unwrap();
        fs::write(rebase.join("head-name"), "refs/heads/feature/x\n").unwrap();
        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let source = read_source_state(&git, "main").unwrap();
        assert_eq!(source.worktrees[0].branch.as_deref(), Some("feature/x"));
        assert!(source.worktrees[0].is_root);
        assert_eq!(
            super::super::state::root_situation(&BuiltinLayout::Contained.to_layout(), &source),
            super::super::state::RootSituation::Settled
        );
    }

    // ── Individual ops ────────────────────────────────────────────────────

    /// Unregistering a worktree of a *non-bare* repository would move its
    /// HEAD and index over the main working tree's — the guard makes that
    /// misuse impossible.
    #[test]
    #[serial]
    fn unregister_refuses_a_non_bare_common_dir() {
        let (_base, repo) = scratch_repo("main");
        let wt = repo.join("task/x");
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                wt.to_str().unwrap(),
                "-b",
                "task/x",
            ],
        );
        let _cwd = CwdGuard::enter(&repo);
        let err = exec_unregister_worktree(&wt, &repo.join(".git"), &mut NullBridge).unwrap_err();
        assert!(err.to_string().contains("core.bare"), "{err}");
        assert_eq!(registrations(&repo.join(".git")), vec!["x".to_string()]);
        assert!(repo.join(".git/index").exists());
    }

    #[test]
    #[serial]
    fn failed_move_removes_the_parent_dirs_it_created() {
        let (_base, repo) = scratch_repo("main");
        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);
        let source = read_source_state(&git, "main").unwrap();
        let ctx = ctx_for(&source, &source);

        let to = repo.join("deep/nested/target");
        exec_move_worktree(
            &repo.join("missing"),
            &to,
            MoveStrategy::Rename,
            &ctx,
            &git,
            &mut NullBridge,
        )
        .expect_err("moving a nonexistent worktree must fail");

        assert!(
            !repo.join("deep").exists(),
            "a move that never happened must leave no scaffolding behind"
        );
    }

    #[test]
    fn prune_stops_at_the_project_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().canonicalize().unwrap().join("repo");
        let deep = root.join("a/b/c");
        fs::create_dir_all(&deep).unwrap();

        prune_empty_dirs_within(&deep, &root);

        assert!(!root.join("a").exists(), "empty chain should be removed");
        assert!(root.exists(), "the project root itself is never removed");
    }

    #[test]
    fn prune_leaves_dirs_outside_the_project_root_alone() {
        let base = tempfile::tempdir().unwrap();
        let outside = base.path().canonicalize().unwrap().join("elsewhere");
        let root = base.path().canonicalize().unwrap().join("repo");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&root).unwrap();

        prune_empty_dirs_within(&outside, &root);

        assert!(
            outside.exists(),
            "sibling/centralized parents are not daft's to remove"
        );
    }
}
