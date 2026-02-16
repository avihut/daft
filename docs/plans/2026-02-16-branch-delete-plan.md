# git-worktree-branch-delete Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Add a `git-worktree-branch-delete` command that fully deletes a
branch, its worktree, and remote tracking branch with safety checks.

**Architecture:** Validate-then-execute two-phase approach. All safety checks
run upfront for every branch; if any fail (without `--force`), the entire
command aborts. Only after validation passes do deletions execute. Follows
existing patterns from `prune.rs`.

**Tech Stack:** Rust, clap (argument parsing), git CLI subprocess wrappers

---

## Task 1: Add new git methods for validation checks

We need several new `GitCommand` methods that don't exist yet:
`merge_base_is_ancestor`, `cherry`, `push_delete`, and `status_porcelain_path`
(to check a specific worktree, not CWD).

**Files:**

- Modify: `src/git.rs` (add new methods after existing ones)

**Step 1: Add `merge_base_is_ancestor` method**

Add after `rev_list_count` (line 460) in `src/git.rs`:

```rust
/// Check if `commit` is an ancestor of `target` using merge-base.
pub fn merge_base_is_ancestor(&self, commit: &str, target: &str) -> Result<bool> {
    let output = Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, target])
        .output()
        .context("Failed to execute git merge-base command")?;

    Ok(output.status.success())
}
```

**Step 2: Add `cherry` method**

```rust
/// Run `git cherry <upstream> <branch>` and return output.
/// Lines prefixed with `-` indicate patches already upstream.
/// Lines prefixed with `+` indicate patches NOT upstream.
pub fn cherry(&self, upstream: &str, branch: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["cherry", upstream, branch])
        .output()
        .context("Failed to execute git cherry command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git cherry failed: {}", stderr);
    }

    String::from_utf8(output.stdout).context("Failed to parse git cherry output")
}
```

**Step 3: Add `push_delete` method**

```rust
/// Delete a remote branch via `git push <remote> --delete <branch>`.
pub fn push_delete(&self, remote: &str, branch: &str) -> Result<()> {
    let mut cmd = Command::new("git");
    cmd.args(["push", remote, "--delete", branch]);

    if self.quiet {
        cmd.arg("--quiet");
    }

    let output = cmd
        .output()
        .context("Failed to execute git push --delete command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git push --delete failed: {}", stderr);
    }

    Ok(())
}
```

**Step 4: Add `has_uncommitted_changes_in` method**

```rust
/// Check if a specific worktree path has uncommitted or untracked changes.
pub fn has_uncommitted_changes_in(&self, worktree_path: &Path) -> Result<bool> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(worktree_path)
        .output()
        .context("Failed to execute git status command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git status failed: {}", stderr);
    }

    let stdout =
        String::from_utf8(output.stdout).context("Failed to parse git status output")?;
    Ok(!stdout.trim().is_empty())
}
```

**Step 5: Add `rev_parse` method**

```rust
/// Resolve a ref to its SHA. Returns the full commit hash.
pub fn rev_parse(&self, rev: &str) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", rev])
        .output()
        .context("Failed to execute git rev-parse command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Git rev-parse failed: {}", stderr);
    }

    let stdout =
        String::from_utf8(output.stdout).context("Failed to parse git rev-parse output")?;
    Ok(stdout.trim().to_string())
}
```

**Step 6: Run unit tests to verify compilation**

Run: `cargo test --lib` Expected: All existing tests pass, new methods compile.

**Step 7: Commit**

```
feat(git): add git methods for branch-delete validation

Add merge_base_is_ancestor, cherry, push_delete,
has_uncommitted_changes_in, and rev_parse methods to GitCommand.
```

---

## Task 2: Create the branch_delete command module (Args + scaffolding)

Set up the command module with clap Args struct, routing, shortcut, and a
minimal `run()` that just parses args and exits.

**Files:**

- Create: `src/commands/branch_delete.rs`
- Modify: `src/commands/mod.rs` (line 5, add `pub mod branch_delete;`)
- Modify: `src/main.rs` (line 46, add match arm; line 91, add daft subcommand)
- Modify: `src/shortcuts.rs` (line 137, add shortcut entry; line 279, add to
  valid_commands)
- Modify: `src/commands/docs.rs` (line 10, add import; line 72, add entry)
- Modify: `xtask/src/main.rs` (line 20, add to COMMANDS; line 55, add match arm;
  line 89, add related_commands)
- Modify: `src/suggest.rs` (line 24, add `"worktree-branch-delete"`)

**Step 1: Create `src/commands/branch_delete.rs` with Args struct**

```rust
use crate::{
    get_git_common_dir, get_project_root,
    git::GitCommand,
    hooks::{HookContext, HookExecutor, HookType, HooksConfig, RemovalReason},
    is_git_repository,
    logging::init_logging,
    output::{CliOutput, Output, OutputConfig},
    remote::get_default_branch_local,
    settings::PruneCdTarget,
    DaftSettings, WorktreeConfig, SHELL_WRAPPER_ENV,
};
use anyhow::{Context, Result};
use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "git-worktree-branch-delete")]
#[command(version = crate::VERSION)]
#[command(about = "Delete branches and their worktrees")]
#[command(long_about = r#"
Deletes one or more local branches along with their associated worktrees and
remote tracking branches in a single operation. This is the inverse of
git-worktree-checkout-branch(1).

Safety checks prevent accidental data loss. The command refuses to delete a
branch that:

  - has uncommitted changes in its worktree
  - has not been merged (or squash-merged) into the default branch
  - is out of sync with its remote tracking branch

Use -D (--force) to override these safety checks. The command always refuses
to delete the repository's default branch (e.g. main), even with --force.

All targeted branches are validated before any deletions begin. If any branch
fails validation without --force, the entire command aborts and no branches
are deleted.

Pre-remove and post-remove lifecycle hooks are executed for each worktree
removal if the repository is trusted. See git-daft(1) for hook management.
"#)]
pub struct Args {
    #[arg(required = true, help = "Branch names to delete")]
    branches: Vec<String>,

    #[arg(short = 'D', long, help = "Force deletion even if not fully merged")]
    force: bool,

    #[arg(short, long, help = "Operate quietly; suppress progress reporting")]
    quiet: bool,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,

    #[arg(long, help = "Do not change directory after deletion")]
    no_cd: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-branch-delete"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let config = OutputConfig::with_autocd(!args.no_cd, args.verbose, settings.autocd);
    let mut output = CliOutput::new(config);

    run_branch_delete(&args, &mut output, &settings)?;
    Ok(())
}

fn run_branch_delete(
    args: &Args,
    output: &mut dyn Output,
    settings: &DaftSettings,
) -> Result<()> {
    // Placeholder - will be implemented in subsequent tasks
    output.info("git-worktree-branch-delete: not yet implemented");
    Ok(())
}
```

**Step 2: Add module declaration**

In `src/commands/mod.rs`, add after `pub mod branch;` (line 5):

```rust
pub mod branch_delete;
```

**Step 3: Add routing in `src/main.rs`**

After line 46 (`"git-worktree-fetch" => ...`), add:

```rust
"git-worktree-branch-delete" => commands::branch_delete::run(),
```

After line 91 (`"worktree-flow-eject" => ...`), add:

```rust
"worktree-branch-delete" => commands::branch_delete::run(),
```

**Step 4: Add shortcut in `src/shortcuts.rs`**

After the `gwtfetch` entry (line 137), add:

```rust
Shortcut {
    alias: "gwtbd",
    command: "git-worktree-branch-delete",
    style: ShortcutStyle::Git,
    extra_args: &[],
},
```

In `test_all_shortcuts_map_to_valid_commands` (line 272-280), add
`"git-worktree-branch-delete"` to the `valid_commands` array.

In `test_shortcuts_for_style` (line 321), update the git shortcuts count from
`8` to `9`.

In `test_resolve_git_style` (line 225-234), add:

```rust
assert_eq!(resolve("gwtbd"), "git-worktree-branch-delete");
```

**Step 5: Add to docs help**

In `src/commands/docs.rs` line 10, add `branch_delete` to the import.

In `get_command_categories()`, add inside the "maintain your worktrees" category
(after the prune entry, line 72):

```rust
CommandEntry {
    display_name: "worktree-branch-delete",
    command: branch_delete::Args::command(),
},
```

**Step 6: Add to xtask**

In `xtask/src/main.rs`:

- Add `"git-worktree-branch-delete"` to `COMMANDS` array (line 20, after
  `"git-worktree-prune"`)
- Add match arm in `get_command_for_name()` (line 55, after prune):
  ```rust
  "git-worktree-branch-delete" => Some(daft::commands::branch_delete::Args::command()),
  ```
- Add `related_commands` entry (line 89, after prune):
  ```rust
  "git-worktree-branch-delete" => vec!["git-worktree-prune", "git-worktree-checkout-branch"],
  ```
- Also add `"git-worktree-branch-delete"` to the related commands for
  `"git-worktree-prune"` and `"git-worktree-checkout-branch"`.

**Step 7: Add to suggest.rs**

In `src/suggest.rs`, add `"worktree-branch-delete"` to `DAFT_SUBCOMMANDS` (after
`"worktree-prune"`, maintaining alphabetical order — it goes before
`"worktree-carry"`):

Actually, alphabetically: `worktree-branch-delete` comes after `worktree-carry`
(`b` < `c`). Wait, no: `branch` < `carry` alphabetically. So insert before
`"worktree-carry"` (line 16).

**Step 8: Add symlink to test framework**

In `tests/integration/test_framework.sh` line 69, add
`"git-worktree-branch-delete"` and `"gwtbd"` to the `symlink_names` array.

**Step 9: Run tests**

Run: `cargo test --lib` Expected: All tests pass including new shortcut
assertions.

Run: `cargo test -p xtask` Expected: xtask tests pass with new command.

**Step 10: Commit**

```
feat: scaffold git-worktree-branch-delete command

Add command module, routing, shortcut (gwtbd), help docs registration,
xtask integration, and suggest support. Implementation is a placeholder.
```

---

## Task 3: Implement Phase 1 — Validation logic

Implement the safety checks that run before any deletion. This is the core logic
that makes the command safe.

**Files:**

- Modify: `src/commands/branch_delete.rs`

**Step 1: Write the validation data structures**

Add to `branch_delete.rs`:

```rust
/// Parsed worktree entry from `git worktree list --porcelain`.
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    is_bare: bool,
}

/// Bundles common parameters used throughout the operation.
struct BranchDeleteContext<'a> {
    git: &'a GitCommand,
    project_root: PathBuf,
    git_dir: PathBuf,
    remote_name: String,
    source_worktree: PathBuf,
    default_branch: String,
}

/// Validated branch ready for deletion.
struct ValidatedBranch {
    name: String,
    worktree_path: Option<PathBuf>,
    remote_name: Option<String>,
    remote_branch_name: Option<String>,
    is_current_worktree: bool,
}

/// A validation error for a single branch.
struct ValidationError {
    branch: String,
    message: String,
}
```

**Step 2: Implement the validation function**

```rust
/// Validate all branches before any deletions.
/// Returns validated branches or a list of errors.
fn validate_branches(
    ctx: &BranchDeleteContext,
    branch_names: &[String],
    worktree_map: &HashMap<String, (PathBuf, bool)>,
    current_wt_path: Option<&Path>,
    force: bool,
    output: &mut dyn Output,
) -> Result<(Vec<ValidatedBranch>, Vec<ValidationError>)> {
    let mut validated = Vec::new();
    let mut errors = Vec::new();

    for branch_name in branch_names {
        output.step(&format!("Validating branch: {branch_name}"));

        // Check 1: Branch exists locally
        if !ctx.git.show_ref_exists(&format!("refs/heads/{branch_name}"))? {
            errors.push(ValidationError {
                branch: branch_name.clone(),
                message: format!("branch '{branch_name}' not found"),
            });
            continue;
        }

        // Check 2: Not the default branch (even with --force)
        if branch_name == &ctx.default_branch {
            errors.push(ValidationError {
                branch: branch_name.clone(),
                message: format!(
                    "refusing to delete the default branch '{branch_name}'"
                ),
            });
            continue;
        }

        // Determine worktree info
        let wt_info = worktree_map.get(branch_name.as_str()).cloned();
        let worktree_path = wt_info.as_ref().map(|(path, _)| path.clone());
        let is_current_worktree = worktree_path.as_ref().is_some_and(|p| {
            current_wt_path.map(|c| c == p).unwrap_or(false)
        });

        if !force {
            // Check 3: No uncommitted changes
            if let Some(ref wt_path) = worktree_path {
                if wt_path.exists() {
                    match ctx.git.has_uncommitted_changes_in(wt_path) {
                        Ok(true) => {
                            errors.push(ValidationError {
                                branch: branch_name.clone(),
                                message: format!(
                                    "has uncommitted changes in worktree (use -D to force)"
                                ),
                            });
                            continue;
                        }
                        Ok(false) => {}
                        Err(e) => {
                            errors.push(ValidationError {
                                branch: branch_name.clone(),
                                message: format!(
                                    "failed to check worktree status: {e}"
                                ),
                            });
                            continue;
                        }
                    }
                }
            }

            // Check 4: Branch is merged into default branch
            match is_branch_merged(ctx, branch_name) {
                Ok(true) => {}
                Ok(false) => {
                    errors.push(ValidationError {
                        branch: branch_name.clone(),
                        message: format!(
                            "not merged into '{}' (use -D to force)",
                            ctx.default_branch
                        ),
                    });
                    continue;
                }
                Err(e) => {
                    errors.push(ValidationError {
                        branch: branch_name.clone(),
                        message: format!(
                            "failed to check merge status: {e}"
                        ),
                    });
                    continue;
                }
            }

            // Check 5: Local and remote in sync
            if let Ok(Some(remote)) =
                ctx.git.get_branch_tracking_remote(branch_name)
            {
                match check_local_remote_sync(ctx, branch_name, &remote) {
                    Ok(true) => {}
                    Ok(false) => {
                        errors.push(ValidationError {
                            branch: branch_name.clone(),
                            message: format!(
                                "local and remote are out of sync (use -D to force)"
                            ),
                        });
                        continue;
                    }
                    Err(e) => {
                        errors.push(ValidationError {
                            branch: branch_name.clone(),
                            message: format!(
                                "failed to check remote sync: {e}"
                            ),
                        });
                        continue;
                    }
                }
            }
        }

        // Determine remote tracking info
        let remote_name = ctx
            .git
            .get_branch_tracking_remote(branch_name)
            .ok()
            .flatten();
        let remote_branch_name = remote_name.as_ref().map(|_| branch_name.clone());

        validated.push(ValidatedBranch {
            name: branch_name.clone(),
            worktree_path,
            remote_name,
            remote_branch_name,
            is_current_worktree,
        });
    }

    Ok((validated, errors))
}
```

**Step 3: Implement merge check (ancestry + squash detection)**

```rust
/// Check if a branch has been merged into the default branch.
/// Handles both regular merges (ancestry) and squash merges (cherry).
fn is_branch_merged(ctx: &BranchDeleteContext, branch_name: &str) -> Result<bool> {
    // First check: is the branch an ancestor of the default branch?
    if ctx
        .git
        .merge_base_is_ancestor(branch_name, &ctx.default_branch)?
    {
        return Ok(true);
    }

    // Second check: squash merge detection via git cherry.
    // If all patches from the branch are already upstream, the branch
    // was squash-merged (or cherry-picked).
    let cherry_output = ctx.git.cherry(&ctx.default_branch, branch_name)?;

    if cherry_output.trim().is_empty() {
        // No commits unique to the branch — it's merged
        return Ok(true);
    }

    // Check if ALL lines start with '-' (meaning all patches are upstream)
    let all_upstream = cherry_output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .all(|line| line.starts_with('-'));

    Ok(all_upstream)
}

/// Check if local branch and remote tracking branch point to the same commit.
fn check_local_remote_sync(
    ctx: &BranchDeleteContext,
    branch_name: &str,
    remote: &str,
) -> Result<bool> {
    let local_ref = format!("refs/heads/{branch_name}");
    let remote_ref = format!("refs/remotes/{remote}/{branch_name}");

    // Check that both refs exist
    if !ctx.git.show_ref_exists(&remote_ref)? {
        // Remote ref doesn't exist — could be already deleted, consider in sync
        return Ok(true);
    }

    let local_sha = ctx.git.rev_parse(&local_ref)?;
    let remote_sha = ctx.git.rev_parse(&remote_ref)?;

    Ok(local_sha == remote_sha)
}
```

**Step 4: Wire validation into `run_branch_delete`**

Replace the placeholder `run_branch_delete` with the full setup + validation
call. The execution phase will be added in the next task.

```rust
fn run_branch_delete(
    args: &Args,
    output: &mut dyn Output,
    settings: &DaftSettings,
) -> Result<()> {
    let wt_config = WorktreeConfig::default();
    let git = GitCommand::new(args.quiet).with_gitoxide(settings.use_gitoxide);

    let default_branch = get_default_branch_local(
        &get_git_common_dir()?,
        &wt_config.remote_name,
        settings.use_gitoxide,
    )?;

    let ctx = BranchDeleteContext {
        git: &git,
        project_root: get_project_root()?,
        git_dir: get_git_common_dir()?,
        remote_name: wt_config.remote_name.clone(),
        source_worktree: std::env::current_dir()?,
        default_branch,
    };

    // Parse worktree list
    let worktree_entries = parse_worktree_list(&git)?;
    let mut worktree_map: HashMap<String, (PathBuf, bool)> = HashMap::new();
    for (i, entry) in worktree_entries.iter().enumerate() {
        if let Some(ref branch) = entry.branch {
            worktree_map.insert(branch.clone(), (entry.path.clone(), i == 0));
        }
    }

    let current_wt_path = git.get_current_worktree_path().ok();

    // Phase 1: Validate all branches
    let (validated, errors) = validate_branches(
        &ctx,
        &args.branches,
        &worktree_map,
        current_wt_path.as_deref(),
        args.force,
        output,
    )?;

    if !errors.is_empty() {
        for error in &errors {
            output.error(&format!(
                "cannot delete '{}': {}",
                error.branch, error.message
            ));
        }
        let total = args.branches.len();
        let failed = errors.len();
        anyhow::bail!(
            "Aborting: {failed} of {total} branches failed validation. No branches were deleted."
        );
    }

    if validated.is_empty() {
        return Ok(());
    }

    // Phase 2: Execute deletions (next task)
    execute_deletions(&ctx, &validated, args.force, settings, output)?;

    Ok(())
}
```

Also copy the `parse_worktree_list` function from `prune.rs` — it's an identical
utility. (We can refactor to share later, but for now keep it local to avoid
touching prune.rs.)

**Step 5: Run tests**

Run: `cargo test --lib` Expected: All tests pass, new code compiles.

**Step 6: Commit**

```
feat: implement branch-delete validation phase

Add safety checks: branch exists, not default branch, no uncommitted
changes, merged into default (with squash detection), local/remote in
sync. Validate-then-execute ensures no partial state.
```

---

## Task 4: Implement Phase 2 — Execution logic

Implement the deletion phase that runs after validation passes.

**Files:**

- Modify: `src/commands/branch_delete.rs`

**Step 1: Implement the execution function**

```rust
/// Execute deletions for all validated branches.
fn execute_deletions(
    ctx: &BranchDeleteContext,
    validated: &[ValidatedBranch],
    force: bool,
    settings: &DaftSettings,
    output: &mut dyn Output,
) -> Result<()> {
    // Separate current worktree branch (process last)
    let (deferred, regular): (Vec<_>, Vec<_>) =
        validated.iter().partition(|b| b.is_current_worktree);

    let mut results: Vec<DeletionResult> = Vec::new();

    // Process non-current branches first
    for branch in &regular {
        let result = delete_single_branch(ctx, branch, force, output);
        results.push(result);
    }

    // Process deferred branch (current worktree) last
    let mut deferred_cd_target: Option<PathBuf> = None;
    for branch in &deferred {
        // Resolve CD target and change directory BEFORE removing worktree
        let cd_target = resolve_prune_cd_target(
            settings.prune_cd_target,
            &ctx.project_root,
            &ctx.git_dir,
            &ctx.remote_name,
            settings.use_gitoxide,
            output,
        );

        if let Err(e) = std::env::set_current_dir(&cd_target) {
            output.error(&format!(
                "Failed to change directory to {}: {e}. \
                 Skipping deletion of branch {}.",
                cd_target.display(),
                branch.name
            ));
            continue;
        }

        let result = delete_single_branch(ctx, branch, force, output);
        if result.worktree_removed {
            deferred_cd_target = Some(cd_target);
        }
        results.push(result);
    }

    // Print summary
    for result in &results {
        if result.has_errors() {
            for error in &result.errors {
                output.error(error);
            }
        }
        let parts = result.deleted_parts();
        if !parts.is_empty() {
            output.info(&format!("Deleted {} ({})", result.branch, parts));
        }
    }

    // Emit CD marker last
    if let Some(ref cd_target) = deferred_cd_target {
        if std::env::var(SHELL_WRAPPER_ENV).is_ok() {
            output.cd_path(cd_target);
        } else {
            output.result(&format!(
                "Run `cd {}` (your previous working directory was removed)",
                cd_target.display()
            ));
        }
    }

    Ok(())
}
```

**Step 2: Implement single branch deletion**

```rust
/// Result of deleting a single branch.
struct DeletionResult {
    branch: String,
    remote_deleted: bool,
    worktree_removed: bool,
    branch_deleted: bool,
    errors: Vec<String>,
}

impl DeletionResult {
    fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    fn deleted_parts(&self) -> String {
        let mut parts = Vec::new();
        if self.worktree_removed {
            parts.push("worktree");
        }
        if self.branch_deleted {
            parts.push("local branch");
        }
        if self.remote_deleted {
            parts.push("remote branch");
        }
        parts.join(", ")
    }
}

/// Delete a single validated branch (remote, worktree, local branch).
fn delete_single_branch(
    ctx: &BranchDeleteContext,
    branch: &ValidatedBranch,
    force: bool,
    output: &mut dyn Output,
) -> DeletionResult {
    let mut result = DeletionResult {
        branch: branch.name.clone(),
        remote_deleted: false,
        worktree_removed: false,
        branch_deleted: false,
        errors: Vec::new(),
    };

    // Step 1: Run pre-remove hook (if worktree exists)
    if let Some(ref wt_path) = branch.worktree_path {
        if let Err(e) = run_hook(
            HookType::PreRemove,
            ctx,
            &wt_path.clone(),
            &branch.name,
            output,
        ) {
            // Check if hook is configured to abort
            output.warning(&format!(
                "Pre-remove hook failed for {}: {e}",
                branch.name
            ));
        }
    }

    // Step 2: Delete remote branch (hardest to recreate)
    if let (Some(ref remote), Some(ref remote_branch)) =
        (&branch.remote_name, &branch.remote_branch_name)
    {
        output.step(&format!(
            "Deleting remote branch {remote}/{remote_branch}..."
        ));
        match ctx.git.push_delete(remote, remote_branch) {
            Ok(()) => {
                result.remote_deleted = true;
                output.step(&format!("Remote branch {remote}/{remote_branch} deleted"));
            }
            Err(e) => {
                result.errors.push(format!(
                    "failed to delete remote branch '{}' on {}: {e}",
                    remote_branch, remote
                ));
            }
        }
    } else {
        output.step(&format!(
            "Branch {} has no remote tracking branch, skipping remote deletion",
            branch.name
        ));
    }

    // Step 3: Remove worktree
    if let Some(ref wt_path) = branch.worktree_path {
        if wt_path.exists() {
            output.step(&format!("Removing worktree at {}...", wt_path.display()));
            match ctx.git.worktree_remove(wt_path, force) {
                Ok(()) => {
                    result.worktree_removed = true;
                    output.step(&format!("Worktree at {} removed", wt_path.display()));

                    // Clean up empty parent directories
                    cleanup_empty_parent_dirs(&ctx.project_root, wt_path, output);
                }
                Err(e) => {
                    result.errors.push(format!(
                        "failed to remove worktree {}: {e}",
                        wt_path.display()
                    ));
                    // Don't delete the branch if worktree removal failed
                    return result;
                }
            }
        } else {
            // Worktree directory doesn't exist but git knows about it
            output.step(&format!(
                "Worktree directory {} not found, removing record...",
                wt_path.display()
            ));
            if let Err(e) = ctx.git.worktree_remove(wt_path, true) {
                output.warning(&format!(
                    "Failed to remove orphaned worktree record: {e}"
                ));
            } else {
                result.worktree_removed = true;
                cleanup_empty_parent_dirs(&ctx.project_root, wt_path, output);
            }
        }
    }

    // Step 4: Delete local branch
    output.step(&format!("Deleting local branch {}...", branch.name));
    match ctx.git.branch_delete(&branch.name, force) {
        Ok(()) => {
            result.branch_deleted = true;
            output.step(&format!("Branch {} deleted", branch.name));
        }
        Err(e) => {
            result.errors.push(format!(
                "failed to delete local branch {}: {e}",
                branch.name
            ));
        }
    }

    // Step 5: Run post-remove hook (if worktree existed)
    if let Some(ref wt_path) = branch.worktree_path {
        if let Err(e) = run_hook(
            HookType::PostRemove,
            ctx,
            &wt_path.clone(),
            &branch.name,
            output,
        ) {
            output.warning(&format!(
                "Post-remove hook failed for {}: {e}",
                branch.name
            ));
        }
    }

    result
}
```

**Step 3: Add helper functions (hooks, cleanup)**

Copy from prune.rs patterns — `run_hook` and `cleanup_empty_parent_dirs` adapted
for `BranchDeleteContext`:

```rust
fn run_hook(
    hook_type: HookType,
    ctx: &BranchDeleteContext,
    worktree_path: &PathBuf,
    branch_name: &str,
    output: &mut dyn Output,
) -> Result<()> {
    let hooks_config = HooksConfig::default();
    let executor = HookExecutor::new(hooks_config)?;

    let hook_ctx = HookContext::new(
        hook_type,
        "branch-delete",
        &ctx.project_root,
        &ctx.git_dir,
        &ctx.remote_name,
        &ctx.source_worktree,
        worktree_path,
        branch_name,
    )
    .with_removal_reason(RemovalReason::Manual);

    executor.execute(&hook_ctx, output)?;
    Ok(())
}

fn cleanup_empty_parent_dirs(
    project_root: &Path,
    worktree_path: &Path,
    output: &mut dyn Output,
) {
    let mut current = worktree_path.parent();
    while let Some(dir) = current {
        if dir == project_root || !dir.starts_with(project_root) {
            break;
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => {
                output.step(&format!("Removed empty directory '{}'", dir.display()));
                current = dir.parent();
            }
            Err(_) => break,
        }
    }
}

/// Resolve where to cd after deleting the user's current worktree.
/// Reuses the same logic and setting as prune (daft.prune.cdTarget).
fn resolve_prune_cd_target(
    cd_target: PruneCdTarget,
    project_root: &Path,
    git_dir: &Path,
    remote_name: &str,
    use_gitoxide: bool,
    output: &mut dyn Output,
) -> PathBuf {
    match cd_target {
        PruneCdTarget::Root => project_root.to_path_buf(),
        PruneCdTarget::DefaultBranch => {
            match get_default_branch_local(git_dir, remote_name, use_gitoxide) {
                Ok(default_branch) => {
                    let branch_dir = project_root.join(&default_branch);
                    if branch_dir.is_dir() {
                        branch_dir
                    } else {
                        output.step(&format!(
                            "Default branch worktree directory '{}' not found, \
                             falling back to project root",
                            branch_dir.display()
                        ));
                        project_root.to_path_buf()
                    }
                }
                Err(e) => {
                    output.warning(&format!(
                        "Cannot determine default branch for cd target: {e}. \
                         Falling back to project root."
                    ));
                    project_root.to_path_buf()
                }
            }
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test --lib` Expected: Compiles and passes.

Run: `cargo clippy -- -D warnings` Expected: No warnings.

**Step 5: Commit**

```
feat: implement branch-delete execution phase

Delete remote branch, remove worktree, delete local branch with hooks
and cleanup. Current worktree deferred to last with CD target resolution.
```

---

## Task 5: Integration tests

Write integration tests covering the main scenarios.

**Files:**

- Create: `tests/integration/test_branch_delete.sh`
- Modify: `tests/integration/test_all.sh` (add source + runner)

**Step 1: Create test file with basic test**

```bash
#!/bin/bash

# Integration tests for git-worktree-branch-delete

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic branch delete after merge
test_branch_delete_basic() {
    local remote_repo=$(create_test_remote "test-repo-bd-basic" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-basic"

    # Create a branch with worktree
    git-worktree-checkout-branch feature/test || return 1
    assert_directory_exists "feature/test" || return 1

    # Make a commit on the feature branch
    cd "feature/test"
    echo "feature work" > feature.txt
    git add feature.txt
    git commit -m "Add feature" >/dev/null 2>&1
    git push origin feature/test >/dev/null 2>&1
    cd ..

    # Merge into main (simulate squash merge by fast-forward for simplicity)
    cd "main"
    git merge feature/test >/dev/null 2>&1
    git push origin main >/dev/null 2>&1
    cd ..

    # Delete the branch
    git-worktree-branch-delete feature/test || return 1

    # Verify worktree was removed
    if [[ -d "feature/test" ]]; then
        log_error "Worktree should have been removed"
        return 1
    fi

    # Verify local branch was deleted
    if git branch | grep -q "feature/test"; then
        log_error "Local branch should have been deleted"
        return 1
    fi

    return 0
}

# Test branch delete refuses unmerged branch
test_branch_delete_refuses_unmerged() {
    local remote_repo=$(create_test_remote "test-repo-bd-unmerged" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-unmerged"

    git-worktree-checkout-branch feature/unmerged || return 1

    cd "feature/unmerged"
    echo "unmerged work" > unmerged.txt
    git add unmerged.txt
    git commit -m "Unmerged work" >/dev/null 2>&1
    git push origin feature/unmerged >/dev/null 2>&1
    cd ..

    # Should fail without --force
    if git-worktree-branch-delete feature/unmerged 2>/dev/null; then
        log_error "Should have refused to delete unmerged branch"
        return 1
    fi

    # Verify branch still exists
    assert_directory_exists "feature/unmerged" || return 1

    return 0
}

# Test branch delete with --force on unmerged branch
test_branch_delete_force_unmerged() {
    local remote_repo=$(create_test_remote "test-repo-bd-force" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-force"

    git-worktree-checkout-branch feature/force-me || return 1

    cd "feature/force-me"
    echo "unmerged work" > work.txt
    git add work.txt
    git commit -m "Some work" >/dev/null 2>&1
    git push origin feature/force-me >/dev/null 2>&1
    cd ..

    # Should succeed with -D
    git-worktree-branch-delete -D feature/force-me || return 1

    # Verify deletion
    if [[ -d "feature/force-me" ]]; then
        log_error "Worktree should have been removed with --force"
        return 1
    fi

    return 0
}

# Test refuses to delete default branch
test_branch_delete_refuses_default() {
    local remote_repo=$(create_test_remote "test-repo-bd-default" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-default"

    # Should fail even with --force
    if git-worktree-branch-delete -D main 2>/dev/null; then
        log_error "Should have refused to delete default branch"
        return 1
    fi

    return 0
}

# Test refuses uncommitted changes
test_branch_delete_refuses_dirty() {
    local remote_repo=$(create_test_remote "test-repo-bd-dirty" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-dirty"

    git-worktree-checkout-branch feature/dirty || return 1

    # Make uncommitted changes
    cd "feature/dirty"
    echo "dirty" > dirty.txt
    cd ..

    # Should fail
    if git-worktree-branch-delete feature/dirty 2>/dev/null; then
        log_error "Should have refused branch with uncommitted changes"
        return 1
    fi

    return 0
}

# Test deleting multiple branches
test_branch_delete_multiple() {
    local remote_repo=$(create_test_remote "test-repo-bd-multi" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-multi"

    # Create two branches
    git-worktree-checkout-branch feature/one || return 1
    git-worktree-checkout-branch feature/two || return 1

    # Merge both into main
    cd "main"
    git merge feature/one >/dev/null 2>&1
    git merge feature/two >/dev/null 2>&1
    git push origin main >/dev/null 2>&1
    cd ..

    # Push both branches so they are in sync
    cd "feature/one" && git push origin feature/one >/dev/null 2>&1 && cd ..
    cd "feature/two" && git push origin feature/two >/dev/null 2>&1 && cd ..

    # Delete both at once
    git-worktree-branch-delete feature/one feature/two || return 1

    # Verify both deleted
    if [[ -d "feature/one" ]] || [[ -d "feature/two" ]]; then
        log_error "Both worktrees should have been removed"
        return 1
    fi

    return 0
}

# Test branch with no worktree (branch-only delete)
test_branch_delete_no_worktree() {
    local remote_repo=$(create_test_remote "test-repo-bd-no-wt" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-no-wt"

    # Create a branch without a worktree (directly via git)
    cd "main"
    git branch no-worktree-branch >/dev/null 2>&1
    cd ..

    # Delete with force (it won't be "merged" since it points at same commit)
    git-worktree-branch-delete no-worktree-branch || return 1

    # Verify branch deleted
    cd "main"
    if git branch | grep -q "no-worktree-branch"; then
        log_error "Branch should have been deleted"
        return 1
    fi
    cd ..

    return 0
}

# Test nonexistent branch
test_branch_delete_nonexistent() {
    local remote_repo=$(create_test_remote "test-repo-bd-noexist" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-bd-noexist"

    if git-worktree-branch-delete nonexistent-branch 2>/dev/null; then
        log_error "Should have failed for nonexistent branch"
        return 1
    fi

    return 0
}

run_branch_delete_tests() {
    run_test "branch_delete_basic" "test_branch_delete_basic"
    run_test "branch_delete_refuses_unmerged" "test_branch_delete_refuses_unmerged"
    run_test "branch_delete_force_unmerged" "test_branch_delete_force_unmerged"
    run_test "branch_delete_refuses_default" "test_branch_delete_refuses_default"
    run_test "branch_delete_refuses_dirty" "test_branch_delete_refuses_dirty"
    run_test "branch_delete_multiple" "test_branch_delete_multiple"
    run_test "branch_delete_no_worktree" "test_branch_delete_no_worktree"
    run_test "branch_delete_nonexistent" "test_branch_delete_nonexistent"
}
```

**Step 2: Register in test_all.sh**

Add source line (after test_prune.sh):

```bash
source "$(dirname "${BASH_SOURCE[0]}")/test_branch_delete.sh"
```

Add `run_branch_delete_tests` in `run_all_integration_tests()` (after
`run_prune_tests`).

**Step 3: Build and run integration tests**

Run: `mise run dev` (to build + create symlinks) Run:
`mise run test-integration` Expected: All new tests pass.

**Step 4: Commit**

```
test: add integration tests for git-worktree-branch-delete

Cover basic delete, unmerged refusal, force override, default branch
protection, dirty worktree refusal, multiple branches, no-worktree
branch, and nonexistent branch.
```

---

## Task 6: Generate man page and CLI docs

Generate and commit the man page and CLI reference documentation.

**Files:**

- Generated: `man/git-worktree-branch-delete.1`
- Generated: `docs/cli/git-worktree-branch-delete.md`

**Step 1: Generate man page and CLI docs**

Run: `mise run gen-man` Run: `mise run gen-cli-docs` (or equivalent xtask
command)

**Step 2: Verify**

Run: `mise run verify-man` Expected: PASS

**Step 3: Commit**

```
docs: add man page and CLI reference for git-worktree-branch-delete
```

---

## Task 7: Update documentation site and SKILL.md

Update the VitePress docs site sidebar config and SKILL.md for agent awareness.

**Files:**

- Modify: `docs/.vitepress/config.ts` (add sidebar entry for new CLI page)
- Modify: `SKILL.md` (add command to the skill documentation)
- Modify: `docs/guide/shortcuts.md` (add gwtbd to shortcuts table)

**Step 1: Add to VitePress sidebar**

In `docs/.vitepress/config.ts`, find the CLI Reference sidebar section and add
the new command entry.

**Step 2: Update SKILL.md**

Add `git-worktree-branch-delete` / `gwtbd` to the command reference section,
describing what it does and when to use it.

**Step 3: Update shortcuts guide**

Add `gwtbd` to the shortcuts table in `docs/guide/shortcuts.md`.

**Step 4: Verify docs build**

Run: `mise run docs:site-build` Expected: Build succeeds.

**Step 5: Commit**

```
docs: add branch-delete to docs site, SKILL.md, and shortcuts guide
```

---

## Task 8: Final verification

Run the full CI suite locally to ensure everything passes.

**Files:** None (verification only)

**Step 1: Format**

Run: `mise run fmt`

**Step 2: Lint**

Run: `mise run clippy` Expected: Zero warnings.

**Step 3: Unit tests**

Run: `mise run test-unit` Expected: All pass.

**Step 4: Integration tests**

Run: `mise run test-integration` Expected: All pass including new branch-delete
tests.

**Step 5: Verify man pages**

Run: `mise run verify-man` Expected: PASS

**Step 6: Full CI simulation**

Run: `mise run ci` Expected: All checks pass.

**Step 7: Commit any formatting fixes**

If `mise run fmt` made changes, commit them:

```
style: format branch-delete code
```
