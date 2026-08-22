//! Plan executor with rollback support.
//!
//! Iterates through a `TransformPlan`'s operations, executing each one and
//! pushing a reverse operation onto a rollback stack. On failure the stack is
//! unwound in reverse order to restore the repository to its pre-transform
//! state (best-effort).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use super::plan::{TransformOp, TransformPlan};
use crate::core::{HookRunner, ProgressSink};
use crate::git::GitCommand;
use crate::hooks::move_hooks::{MoveHookParams, run_setup_hooks, run_teardown_hooks};
use crate::hooks::tracking::TrackedAttribute;

// ── Execution context ────────────────────────────────────────────────────

/// Context needed by move hooks during plan execution.
///
/// Callers must populate this from the available repository state so that
/// `execute_plan` can build `MoveHookParams` for each `MoveWorktree` op.
pub struct ExecutionContext {
    pub project_root: PathBuf,
    /// The `.git` directory the transform starts from.
    pub git_dir: PathBuf,
    /// Where `.git` ends up. Used by the index epilogues, which must name the
    /// file explicitly rather than re-derive it from a cwd that the transform
    /// may have moved out from under itself.
    pub target_git_dir: PathBuf,
    pub remote: String,
    pub source_worktree: PathBuf,
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

/// Execute every operation in the plan, maintaining a rollback stack.
///
/// On failure the executor attempts to undo completed operations in reverse
/// order, then propagates the original error. Progress messages are emitted
/// via `sink` for each step. Move hooks are fired around `MoveWorktree`
/// operations using the provided `ExecutionContext`.
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
        // This covers MoveWorktree, CollapseIntoRoot, and NestFromRoot.
        let move_params = match op {
            TransformOp::MoveWorktree { branch, from, to } => Some(MoveHookParams {
                old_worktree_path: from.clone(),
                new_worktree_path: to.clone(),
                old_branch_name: branch.clone(),
                new_branch_name: branch.clone(),
                project_root: ctx.project_root.clone(),
                git_dir: ctx.git_dir.clone(),
                remote: ctx.remote.clone(),
                source_worktree: ctx.source_worktree.clone(),
                command: "layout-transform".to_string(),
                changed_attributes: HashSet::from([TrackedAttribute::Path]),
            }),
            TransformOp::CollapseIntoRoot {
                branch,
                worktree_path,
                root_path,
            } => Some(MoveHookParams {
                old_worktree_path: worktree_path.clone(),
                new_worktree_path: root_path.clone(),
                old_branch_name: branch.clone(),
                new_branch_name: branch.clone(),
                project_root: ctx.project_root.clone(),
                git_dir: ctx.git_dir.clone(),
                remote: ctx.remote.clone(),
                source_worktree: ctx.source_worktree.clone(),
                command: "layout-transform".to_string(),
                changed_attributes: HashSet::from([TrackedAttribute::Path]),
            }),
            TransformOp::NestFromRoot {
                branch,
                root_path,
                subdir_path,
            } => Some(MoveHookParams {
                old_worktree_path: root_path.clone(),
                new_worktree_path: subdir_path.clone(),
                old_branch_name: branch.clone(),
                new_branch_name: branch.clone(),
                project_root: ctx.project_root.clone(),
                git_dir: ctx.git_dir.clone(),
                remote: ctx.remote.clone(),
                source_worktree: ctx.source_worktree.clone(),
                command: "layout-transform".to_string(),
                changed_attributes: HashSet::from([TrackedAttribute::Path]),
            }),
            _ => None,
        };

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

            match rollback(&rollback_stack, git, ctx, sink) {
                Ok(()) => drop_index_after_rollback(plan, ctx),
                Err(rb_err) => {
                    sink.on_warning(&format!("Rollback encountered errors: {rb_err:#}"));
                }
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

    drop_index_after_success(plan, ctx);

    Ok(ExecuteResult {
        ops_completed: total,
        ops_total: total,
    })
}

// ── Index epilogues ────────────────────────────────────────────────────────
//
// A bare repository keeps no working-tree index: leaving one behind makes
// `git status` in the repo dir report every tracked file as deleted. Dropping
// it is therefore part of *finishing* a bare transform — never part of
// `SetBare(true)` itself, which has to stay trivially reversible so a failed
// plan can roll `core.bare` back without having destroyed the index the
// working tree still needs (#859).

/// After a fully successful plan that ends bare, remove the now-meaningless
/// working-tree index. Best-effort: a missing file is the normal case.
fn drop_index_after_success(plan: &TransformPlan, ctx: &ExecutionContext) {
    if plan
        .ops
        .iter()
        .any(|op| matches!(op, TransformOp::SetBare(true)))
    {
        let _ = fs::remove_file(ctx.target_git_dir.join("index"));
    }
}

/// After a *complete* rollback of a plan that started bare, remove the index
/// `InitWorktreeIndex` built at the root, so the restored repository is bare
/// again in the same sense it was before.
fn drop_index_after_rollback(plan: &TransformPlan, ctx: &ExecutionContext) {
    if plan
        .ops
        .iter()
        .any(|op| matches!(op, TransformOp::SetBare(false)))
    {
        let _ = fs::remove_file(ctx.git_dir.join("index"));
    }
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
        TransformOp::StashChanges {
            branch,
            worktree_path,
        } => exec_stash_changes(branch, worktree_path, git),

        TransformOp::PopStash {
            branch,
            worktree_path,
        } => exec_pop_stash(branch, worktree_path, git, progress),

        TransformOp::MoveWorktree {
            branch: _,
            from,
            to,
        } => exec_move_worktree(from, to, &ctx.project_root, git),

        TransformOp::MoveGitDir { from, to } => exec_move_git_dir(from, to),

        TransformOp::SetBare(bare) => exec_set_bare(*bare, git),

        TransformOp::RegisterWorktree { branch, path } => {
            exec_register_worktree(branch, path, progress)
        }

        TransformOp::UnregisterWorktree { branch, path } => {
            exec_unregister_worktree(branch, path, progress)
        }

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

        TransformOp::InitWorktreeIndex { path, branch } => {
            exec_init_worktree_index(path, branch, progress)
        }

        TransformOp::CreateDirectory { path } => {
            fs::create_dir_all(path)
                .with_context(|| format!("Failed to create directory: {}", path.display()))?;
            Ok(())
        }

        TransformOp::ValidateIntegrity => exec_validate_integrity(progress),
    }
}

// ── Individual op implementations ──────────────────────────────────────────

fn exec_stash_changes(_branch: &str, worktree_path: &Path, git: &GitCommand) -> Result<()> {
    let prev = crate::utils::get_current_directory()?;
    crate::utils::change_directory(worktree_path)?;

    let result = git.stash_push_with_untracked("daft-transform: temporary stash before move");

    crate::utils::change_directory(&prev)?;
    result
}

fn exec_pop_stash(
    _branch: &str,
    worktree_path: &Path,
    git: &GitCommand,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let prev = crate::utils::get_current_directory()?;
    crate::utils::change_directory(worktree_path)?;

    if let Err(e) = git.stash_pop() {
        progress.on_warning(&format!(
            "Could not restore stashed changes: {e}. Run 'git stash pop' manually."
        ));
    }

    crate::utils::change_directory(&prev)?;
    Ok(())
}

fn exec_move_worktree(from: &Path, to: &Path, project_root: &Path, git: &GitCommand) -> Result<()> {
    let created = ensure_parent_dirs(to)?;

    if let Err(e) = git.worktree_move(from, to) {
        // Leave no scaffolding behind for a move that never happened — the
        // empty `repo/task/` left by the #859 failure came from here.
        for dir in created.iter().rev() {
            let _ = fs::remove_dir(dir);
        }
        return Err(e);
    }

    if let Some(parent) = from.parent() {
        prune_empty_dirs_within(parent, project_root);
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

fn exec_move_git_dir(from: &Path, to: &Path) -> Result<()> {
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

    Ok(())
}

/// Flip `core.bare` — and nothing else.
///
/// The stale index a bare repo is left holding is cleaned up by
/// `drop_index_after_success`, not here: this op's reverse is `SetBare(!bare)`,
/// so anything it destroys is destroyed for good the moment a later op fails.
fn exec_set_bare(bare: bool, git: &GitCommand) -> Result<()> {
    let value = if bare { "true" } else { "false" };
    git.config_set("core.bare", value)?;
    Ok(())
}

fn exec_register_worktree(
    branch: &str,
    path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    super::legacy::register_worktree(&git_dir, path, branch, progress)
}

fn exec_unregister_worktree(
    branch: &str,
    path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let git_dir = crate::core::repo::get_git_common_dir()?;

    let Some(registration) = find_worktree_registration(&git_dir, path) else {
        progress.on_warning(&format!(
            "No worktree registration found for '{branch}' at {} — nothing to unregister.",
            path.display()
        ));
        return Ok(());
    };

    fs::remove_dir_all(&registration).with_context(|| {
        format!(
            "Failed to remove worktree registration: {}",
            registration.display()
        )
    })?;

    Ok(())
}

/// Find the `<common>/worktrees/<name>` registration that belongs to
/// `worktree_path`.
///
/// The name cannot be derived from the branch: `git worktree add` names the
/// registration after the *path basename* (`R/task/x` -> `x`) and disambiguates
/// collisions with a numeric suffix. Its `gitdir` file — the same link
/// `fixup_gitdir_references` rewrites — is the authoritative back-pointer.
fn find_worktree_registration(git_dir: &Path, worktree_path: &Path) -> Option<PathBuf> {
    let worktrees_dir = git_dir.join("worktrees");
    let wanted_dir = canonical_or_owned(worktree_path);
    let wanted_file = canonical_or_owned(&worktree_path.join(".git"));

    for entry in fs::read_dir(&worktrees_dir).ok()? {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let Ok(recorded) = fs::read_to_string(entry.path().join("gitdir")) else {
            continue;
        };
        let recorded = PathBuf::from(recorded.trim());
        if recorded.as_os_str().is_empty() {
            continue;
        }

        if canonical_or_owned(&recorded) == wanted_file {
            return Some(entry.path());
        }
        // The `.git` pointer file may already be gone (a collapse removes it);
        // fall back to the directory that holds it.
        if let Some(parent) = recorded.parent()
            && canonical_or_owned(parent) == wanted_dir
        {
            return Some(entry.path());
        }
    }

    None
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
    // directory) that linked back to the bare repo's worktree registration.
    // This file is orphaned after collapse — the UnregisterWorktree op will
    // clean up the registration side.
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

fn exec_init_worktree_index(
    path: &Path,
    branch: &str,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    // Point HEAD at the branch before rebuilding the index. A worktree that
    // just changed role has lost the HEAD it used to resolve through: after
    // `UnregisterWorktree` the only one left is the bare repo's, which names
    // the default branch — reset against that would rebuild the index from the
    // wrong tree and fail `ValidateIntegrity` (legacy.rs::initialize_index has
    // always done both steps).
    let head_result = Command::new("git")
        .args(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])
        .current_dir(path)
        .output()
        .context("Failed to set HEAD")?;

    if !head_result.status.success() {
        let stderr = String::from_utf8_lossy(&head_result.stderr);
        progress.on_warning(&format!("git symbolic-ref warning: {}", stderr.trim()));
    }

    let reset_result = Command::new("git")
        .args(["reset", "--mixed", "HEAD"])
        .current_dir(path)
        .output()
        .context("Failed to initialize worktree index")?;

    if !reset_result.status.success() {
        let stderr = String::from_utf8_lossy(&reset_result.stderr);
        progress.on_warning(&format!("git reset warning: {}", stderr.trim()));
    }

    Ok(())
}

fn exec_validate_integrity(progress: &mut dyn ProgressSink) -> Result<()> {
    let mut errors: Vec<String> = Vec::new();

    // 1. Run git fsck to check repository integrity
    progress.on_step("Running git fsck...");
    match Command::new("git").args(["fsck", "--no-dangling"]).output() {
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

    // 2. Check each worktree for unexpected dirty state
    progress.on_step("Verifying worktree state...");
    match Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .output()
    {
        Ok(result) if result.status.success() => {
            let porcelain = String::from_utf8_lossy(&result.stdout);
            // Delegate the porcelain parse to the shared core parser; check each
            // non-bare worktree for unexpected dirty state.
            for entry in crate::core::worktree::porcelain::parse_worktree_list_porcelain(&porcelain)
            {
                if entry.is_bare {
                    continue;
                }
                if let Ok(status) = Command::new("git")
                    .args(["status", "--porcelain"])
                    .current_dir(&entry.path)
                    .output()
                    && status.status.success()
                {
                    let out = String::from_utf8_lossy(&status.stdout);
                    // Filter out layout artifacts (.gitignore,
                    // .worktrees/) that are cleaned up after the
                    // transform completes.
                    let real_changes = out
                        .lines()
                        .filter(|l| !l.is_empty())
                        .filter(|l| {
                            let path_part = if l.len() > 3 { &l[3..] } else { l };
                            !path_part.starts_with(".gitignore")
                                && !path_part.starts_with(".worktrees/")
                                && !path_part.starts_with(".worktrees")
                        })
                        .count();
                    if real_changes > 0 {
                        errors.push(format!(
                            "Worktree at {} has unexpected dirty state",
                            entry.path.display()
                        ));
                    }
                }
            }
        }
        _ => {
            progress.on_warning("Could not verify worktree states");
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
            "Transform completed but integrity check found {} issue{}. \
             The repository may need manual inspection.",
            errors.len(),
            if errors.len() == 1 { "" } else { "s" }
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
            format!("gitdir: {}", new_registration_path.display()),
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
/// Returns `None` for operations that cannot be meaningfully reversed
/// (stash, index init, validation, directory creation).
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

        TransformOp::RegisterWorktree { branch, path } => Some(TransformOp::UnregisterWorktree {
            branch: branch.clone(),
            path: path.clone(),
        }),

        TransformOp::UnregisterWorktree { branch, path } => Some(TransformOp::RegisterWorktree {
            branch: branch.clone(),
            path: path.clone(),
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

        // These operations are not easily reversible
        TransformOp::StashChanges { .. }
        | TransformOp::PopStash { .. }
        | TransformOp::InitWorktreeIndex { .. }
        | TransformOp::CreateDirectory { .. }
        | TransformOp::ValidateIntegrity => None,
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

/// Human-readable description of a transform operation, suitable for progress
/// output.
pub fn describe_op(op: &TransformOp) -> String {
    match op {
        TransformOp::StashChanges { branch, .. } => {
            format!("Stash changes in '{branch}'")
        }
        TransformOp::PopStash { branch, .. } => {
            format!("Restore stashed changes in '{branch}'")
        }
        TransformOp::MoveWorktree { branch, from, to } => {
            format!(
                "Move worktree '{branch}': {} -> {}",
                from.display(),
                to.display()
            )
        }
        TransformOp::MoveGitDir { from, to } => {
            format!("Move .git: {} -> {}", from.display(), to.display())
        }
        TransformOp::SetBare(bare) => {
            format!("Set core.bare = {bare}")
        }
        TransformOp::RegisterWorktree { branch, path } => {
            format!("Register worktree '{branch}' at {}", path.display())
        }
        TransformOp::UnregisterWorktree { branch, path } => {
            format!("Unregister worktree '{branch}' at {}", path.display())
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
        TransformOp::InitWorktreeIndex { path, branch } => {
            format!("Initialize index for '{branch}' at {}", path.display())
        }
        TransformOp::CreateDirectory { path } => {
            format!("Create directory {}", path.display())
        }
        TransformOp::ValidateIntegrity => "Validate repository integrity".to_string(),
    }
}
