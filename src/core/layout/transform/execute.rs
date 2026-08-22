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

        TransformOp::MoveGitDir { from, to } => exec_move_git_dir(from, to, &ctx.project_root),

        TransformOp::SetBare(bare) => exec_set_bare(*bare, git),

        TransformOp::RegisterWorktree { branch, path } => {
            exec_register_worktree(branch, path, progress)
        }

        TransformOp::UnregisterWorktree { branch, path } => exec_unregister_worktree(branch, path),

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
            init_worktree_index(path, branch, progress)
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

/// Make `path` a linked worktree of this repository.
///
/// Includes rebuilding its index: a registration without one leaves the
/// worktree reporting every tracked file as both deleted and untracked, and
/// this op is the reverse of `UnregisterWorktree`, which removes the index
/// along with the registration directory.
fn exec_register_worktree(
    branch: &str,
    path: &Path,
    progress: &mut dyn ProgressSink,
) -> Result<()> {
    let git_dir = crate::core::repo::get_git_common_dir()?;
    crate::git::worktree_state::register_worktree(&git_dir, path, branch, progress)?;
    init_worktree_index(path, branch, progress)
}

fn exec_unregister_worktree(branch: &str, path: &Path) -> Result<()> {
    let git_dir = crate::core::repo::get_git_common_dir()?;

    // Fail closed rather than warn-and-continue. The pivot is registered by
    // construction on this path, so a miss means the repository is not in the
    // state the plan was built from — and continuing would arm the LIFO
    // inverse (`RegisterWorktree`) for an unregister that never happened,
    // which on rollback would overwrite a restored main working tree's real
    // `.git` directory with a linked-worktree pointer file. A failing op never
    // pushes its reverse, so bailing here keeps the rollback stack honest.
    let Some(registration) = find_worktree_registration(&git_dir, path) else {
        anyhow::bail!(
            "No worktree registration found for '{branch}' at {}. \
             The repository layout changed since the plan was built — \
             re-run `daft layout transform` to rebuild it.",
            path.display()
        );
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

/// Point HEAD at `branch`, then rebuild the index at `path`.
///
/// Both steps, in that order. A worktree that just changed role has lost the
/// HEAD it used to resolve through: after `UnregisterWorktree` the only one
/// left is the bare repo's, which names the default branch — resetting against
/// that rebuilds the index from the wrong tree and fails `ValidateIntegrity`.
/// `legacy.rs::initialize_index` has always done both.
fn init_worktree_index(path: &Path, branch: &str, _progress: &mut dyn ProgressSink) -> Result<()> {
    // Both commands *write* (HEAD, then the index), so an inherited `GIT_DIR`
    // would retarget them at the enclosing repository — hence `git_command_at`
    // rather than `current_dir`, per the Test Hygiene rule in CLAUDE.md.
    let head_result = crate::utils::git_command_at(path)
        .args(["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")])
        .output()
        .context("Failed to set HEAD")?;

    // A failure here is not cosmetic: HEAD decides which tree the index below
    // is built from. Warning and continuing leaves every tracked file looking
    // deleted and defers the real cause to a confusing `ValidateIntegrity`
    // failure at the very last step.
    if !head_result.status.success() {
        anyhow::bail!(
            "Failed to point HEAD at '{branch}' in {}: {}",
            path.display(),
            String::from_utf8_lossy(&head_result.stderr).trim()
        );
    }

    let reset_result = crate::utils::git_command_at(path)
        .args(["reset", "--mixed", "HEAD"])
        .output()
        .context("Failed to initialize worktree index")?;

    if !reset_result.status.success() {
        anyhow::bail!(
            "Failed to build the index for '{branch}' in {}: {}",
            path.display(),
            String::from_utf8_lossy(&reset_result.stderr).trim()
        );
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
            format!(
                "Register worktree '{branch}' at {} and build its index",
                path.display()
            )
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
    use super::super::state::{LayoutState, compute_target_state, read_source_state};

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

    fn ctx_for(source: &LayoutState, target: &LayoutState) -> ExecutionContext {
        ExecutionContext {
            project_root: source.project_root.clone(),
            git_dir: source.git_dir.clone(),
            target_git_dir: target.git_dir.clone(),
            remote: "origin".to_string(),
            source_worktree: source.project_root.clone(),
        }
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
            branch: "no-such-branch".to_string(),
            from: base.join("does-not-exist"),
            to: base.join("nowhere/at/all"),
        }
    }

    // ── #859 ──────────────────────────────────────────────────────────────

    /// A plain clone whose main working tree is on a feature branch, taken to
    /// `contained` and then failed at the last step: the repository must come
    /// back exactly as it was — in particular *with an index*, which the old
    /// `SetBare(true)` deleted and `SetBare(false)` could not restore.
    #[test]
    #[serial]
    fn failed_transform_of_a_nondefault_root_rolls_back_completely() {
        let (base, repo) = scratch_repo("feature/x");
        let dev = repo.parent().unwrap().join("repo.dev");
        git_ok(
            &repo,
            &["worktree", "add", "-q", dev.to_str().unwrap(), "-b", "dev"],
        );

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);

        let source = read_source_state(&git, "main").unwrap();
        assert!(
            source
                .worktrees
                .iter()
                .any(|wt| wt.branch == "feature/x" && wt.is_root),
            "the main working tree holds the root role even on a feature branch"
        );

        let target = compute_target_state(&BuiltinLayout::Contained.to_layout(), &source).unwrap();
        let classified = classify_worktrees(&source, &target, &[], false);
        let mut plan = build_plan(&source, &target, &classified, false).unwrap();

        let before_list = git_ok(&repo, &["worktree", "list", "--porcelain"]);
        let before_regs = registrations(&source.git_dir);

        // Fail after every real op has run, just before validation.
        let last = plan.ops.len() - 1;
        plan.ops.insert(last, doomed_op(base.path()));

        let ctx = ctx_for(&source, &target);
        execute_plan(&plan, &git, &ctx, &mut NullBridge)
            .expect_err("the doomed op must fail the plan");

        assert!(
            repo.join(".git/index").exists(),
            "rollback must not leave the repo without an index"
        );
        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "false"
        );
        assert_eq!(
            git_ok(&repo, &["status", "--porcelain"]),
            "",
            "the working tree must be clean, not a pile of staged deletions"
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
    /// the feature branch registered as a linked worktree and no stale index.
    #[test]
    #[serial]
    fn successful_transform_of_a_nondefault_root_registers_it_and_drops_the_index() {
        let (_base, repo) = scratch_repo("feature/x");

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);

        let source = read_source_state(&git, "main").unwrap();
        let target = compute_target_state(&BuiltinLayout::Contained.to_layout(), &source).unwrap();
        let classified = classify_worktrees(&source, &target, &[], false);
        let plan = build_plan(&source, &target, &classified, false).unwrap();

        let ctx = ctx_for(&source, &target);
        execute_plan(&plan, &git, &ctx, &mut NullBridge).expect("transform should succeed");

        let wt = repo.join("feature/x");
        assert!(wt.join("README.md").exists());
        assert_eq!(
            git_ok(&repo, &["config", "--get", "core.bare"]).trim(),
            "true"
        );
        assert_eq!(
            git_ok(&wt, &["status", "--porcelain"]),
            "",
            "the relocated worktree must be clean"
        );
        assert_eq!(
            git_ok(&wt, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
            "feature/x",
            "the index must be rebuilt against the pivot's own branch"
        );
        assert!(
            !repo.join(".git/index").exists(),
            "a bare repo keeps no working-tree index"
        );
    }

    /// The bare mirror: no worktree for the default branch, so the sole
    /// worktree takes the root. A failure must put the registration back.
    #[test]
    #[serial]
    fn failed_bare_to_nonbare_transform_restores_the_registration() {
        let (base, repo) = scratch_bare_repo(&[("feature/x", "feature/x")]);

        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);

        let source = read_source_state(&git, "main").unwrap();
        assert!(source.is_bare);
        assert!(source.worktrees.iter().all(|wt| !wt.is_root));

        let target = compute_target_state(&BuiltinLayout::Sibling.to_layout(), &source).unwrap();
        let classified = classify_worktrees(&source, &target, &[], false);
        let mut plan = build_plan(&source, &target, &classified, false).unwrap();

        let before_list = git_ok(&repo, &["worktree", "list", "--porcelain"]);

        let last = plan.ops.len() - 1;
        plan.ops.insert(last, doomed_op(base.path()));

        let ctx = ctx_for(&source, &target);
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
        assert!(repo.join("feature/x/README.md").exists());
        assert_eq!(
            git_ok(&repo.join("feature/x"), &["status", "--porcelain"]),
            ""
        );
        assert!(
            !repo.join(".git/index").exists(),
            "the index built at the root must not survive the rollback"
        );
        assert!(
            !repo.join("README.md").exists(),
            "collapsed files must go back into the worktree"
        );
    }

    // ── Individual ops ────────────────────────────────────────────────────

    /// `git worktree add` names registrations after the *path basename*, so a
    /// slashed branch's registration cannot be guessed from its name.
    #[test]
    #[serial]
    fn unregister_resolves_a_slashed_branch_by_path() {
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
        assert_eq!(
            registrations(&repo.join(".git")),
            vec!["x".to_string()],
            "git names it after the basename, not the branch"
        );

        let _cwd = CwdGuard::enter(&repo);
        exec_unregister_worktree("task/x", &wt).unwrap();

        assert!(
            registrations(&repo.join(".git")).is_empty(),
            "the branch-derived name 'task-x' would have missed entirely"
        );
    }

    #[test]
    #[serial]
    fn failed_move_removes_the_parent_dirs_it_created() {
        let (_base, repo) = scratch_repo("main");
        let _cwd = CwdGuard::enter(&repo);
        let git = GitCommand::new(false);

        let to = repo.join("deep/nested/target");
        exec_move_worktree(&repo.join("missing"), &to, &repo, &git)
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
