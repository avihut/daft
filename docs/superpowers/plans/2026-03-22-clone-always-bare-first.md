# Clone Refactor: Always Bare First — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the clone command to always do a bare clone first, then read
daft.yml from the local data, then decide the final layout — eliminating the
prompt-before-daft.yml bug and the wasteful post-clone reconciliation.

**Architecture:** Split `clone::execute()` into three phases:
`clone_bare_phase()` (always runs), `setup_bare_worktrees()` (bare layouts),
`unbare_and_checkout()` (non-bare layouts). The command layer orchestrates the
phases with layout resolution in between. Remove `execute_regular()` and
`reconcile_layout()`.

**Tech Stack:** Rust, git CLI commands, existing layout resolver and yaml config
loader.

**Spec:** `docs/superpowers/specs/2026-03-22-clone-always-bare-first-design.md`

---

### Task 1: Add `load_config_from_bare` public wrapper

**Files:**

- Modify: `src/hooks/yaml_config_loader.rs:385-394`

- [ ] **Step 1: Add the public function**

Add after `try_load_config_from_ref` (around line 394):

```rust
/// Load daft.yml from a bare repository's HEAD ref.
///
/// Used by the clone command to read the team's layout preference before
/// deciding the final layout. Searches all config file candidates
/// (daft.yml, daft.yaml, .daft.yml, etc.) in priority order.
///
/// Returns `None` if no config file is found on HEAD.
pub fn load_config_from_bare(git_dir: &Path) -> Result<Option<YamlConfig>> {
    try_load_config_from_ref(git_dir, "HEAD")
}
```

- [ ] **Step 2: Build and verify**

```bash
mise run fmt && mise run clippy && mise run test:unit
```

Expected: All pass (no callers yet, but function compiles).

- [ ] **Step 3: Commit**

```bash
git add src/hooks/yaml_config_loader.rs
git commit -m "feat(config): expose load_config_from_bare for pre-layout clone"
```

---

### Task 2: Add `BareCloneParams`, `BareCloneResult`, and `clone_bare_phase()`

**Files:**

- Modify: `src/core/worktree/clone.rs:16-76` (structs and execute fn)

This is the largest task. It extracts the shared bare clone logic from
`execute_bare()` into a standalone phase.

- [ ] **Step 1: Add `BareCloneParams` struct**

Replace the existing `CloneParams` struct (lines 16-40) with `BareCloneParams`.
Keep `CloneParams` temporarily (it's still used by `execute()`) — we'll remove
it in a later task. Add `BareCloneParams` after `CloneParams`:

```rust
/// Input parameters for the bare clone phase.
///
/// Contains everything needed to clone a repository as bare. Layout is NOT
/// included — it's decided after the bare clone, once daft.yml can be read.
pub struct BareCloneParams {
    pub repository_url: String,
    pub branch: Option<String>,
    pub no_checkout: bool,
    pub all_branches: bool,
    pub remote: Option<String>,
    pub remote_name: String,
    pub multi_remote_enabled: bool,
    pub multi_remote_default: String,
    pub checkout_upstream: bool,
    pub use_gitoxide: bool,
}
```

- [ ] **Step 2: Add `BareCloneResult` struct**

Add after `BareCloneParams`:

```rust
/// Result of the bare clone phase.
///
/// Contains all the information needed by subsequent phases to set up
/// worktrees (bare layout) or convert to a regular repo (non-bare layout).
pub struct BareCloneResult {
    pub repo_name: String,
    pub parent_dir: PathBuf,
    pub git_dir: PathBuf,
    pub default_branch: String,
    pub target_branch: String,
    pub branch_exists: bool,
    pub is_empty: bool,
    pub remote_name: String,
    pub repository_url: String,
}
```

- [ ] **Step 3: Implement `clone_bare_phase()`**

Add after the structs. This extracts lines 82-161 from `execute_bare()`:

```rust
/// Phase 1: Clone a repository as bare into `<repo>/.git`.
///
/// Every clone starts here regardless of the final layout. After this
/// phase the caller reads daft.yml and resolves the layout, then calls
/// either `setup_bare_worktrees()` or `unbare_and_checkout()`.
///
/// On return the process cwd is `parent_dir` (the repo directory).
pub fn clone_bare_phase(
    params: &BareCloneParams,
    progress: &mut dyn ProgressSink,
) -> Result<BareCloneResult> {
    let repo_name = crate::extract_repo_name(&params.repository_url)?;
    progress.on_step(&format!("Repository name detected: '{repo_name}'"));

    let (default_branch, target_branch, branch_exists, is_empty) =
        detect_branches_bare(params, progress)?;

    let parent_dir = PathBuf::from(&repo_name);

    if path_exists(&parent_dir) {
        anyhow::bail!("Target path './{} already exists.", parent_dir.display());
    }

    progress.on_step("Creating repository directory...");
    create_directory(&parent_dir)?;

    let git_dir = parent_dir.join(".git");
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    progress.on_step(&format!(
        "Cloning bare repository into './{}'...",
        git_dir.display()
    ));

    if let Err(e) = git.clone_bare(&params.repository_url, &git_dir) {
        remove_directory(&parent_dir).ok();
        return Err(e.context("Git clone failed"));
    }

    let git_dir = git_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize git directory: {}",
            git_dir.display()
        )
    })?;

    progress.on_step(&format!(
        "Changing directory to './{}'",
        parent_dir.display()
    ));
    change_directory(&parent_dir)?;

    // Set up fetch refspec for bare repo
    progress.on_step("Setting up fetch refspec for remote tracking...");
    if let Err(e) = git.setup_fetch_refspec(&params.remote_name) {
        progress.on_warning(&format!("Could not set fetch refspec: {e}"));
    }

    // Set multi-remote config if --remote was provided
    if params.remote.is_some() {
        progress.on_step("Enabling multi-remote mode for this repository...");
        crate::multi_remote::config::set_multi_remote_enabled(&git, true)?;
        let remote_for_path = params
            .remote
            .clone()
            .unwrap_or_else(|| params.multi_remote_default.clone());
        crate::multi_remote::config::set_multi_remote_default(&git, &remote_for_path)?;
    }

    Ok(BareCloneResult {
        repo_name,
        parent_dir,
        git_dir,
        default_branch,
        target_branch,
        branch_exists,
        is_empty,
        remote_name: params.remote_name.clone(),
        repository_url: params.repository_url.clone(),
    })
}
```

- [ ] **Step 4: Add `detect_branches_bare` helper**

The existing `detect_branches()` takes `&CloneParams`. Add a temporary compat
wrapper. This wrapper will be removed in Task 5 when `CloneParams` is deleted
and `detect_branches` is updated to accept `&BareCloneParams` directly:

```rust
fn detect_branches_bare(
    params: &BareCloneParams,
    progress: &mut dyn ProgressSink,
) -> Result<(String, String, bool, bool)> {
    let compat = CloneParams {
        repository_url: params.repository_url.clone(),
        branch: params.branch.clone(),
        no_checkout: params.no_checkout,
        all_branches: params.all_branches,
        remote: params.remote.clone(),
        remote_name: params.remote_name.clone(),
        multi_remote_enabled: params.multi_remote_enabled,
        multi_remote_default: params.multi_remote_default.clone(),
        checkout_upstream: params.checkout_upstream,
        use_gitoxide: params.use_gitoxide,
        layout: crate::core::layout::Layout {
            name: String::new(),
            template: String::new(),
            bare: None,
        },
    };
    detect_branches(&compat, progress)
}
```

This avoids touching `detect_branches` internals. The `layout` field is not used
by `detect_branches` so a dummy value is fine.

- [ ] **Step 5: Build and run tests**

```bash
mise run fmt && mise run clippy && mise run test:unit
```

Expected: All pass (new functions exist but aren't called from the command layer
yet — `execute()` still works as before).

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/clone.rs
git commit -m "feat(clone): add clone_bare_phase with BareCloneParams/Result"
```

---

### Task 3: Add `setup_bare_worktrees()` and `unbare_and_checkout()`

**Files:**

- Modify: `src/core/worktree/clone.rs`

- [ ] **Step 1: Implement `setup_bare_worktrees()`**

Extract from `execute_bare()` lines 160-261 (everything after the clone and
refspec setup). Takes `BareCloneResult`, `BareCloneParams`, and `Layout`:

```rust
/// Phase 4a: Set up worktrees for a bare layout.
///
/// Creates worktrees via `git worktree add`, sets up tracking, and returns
/// the final `CloneResult`. Assumes cwd is `bare_result.parent_dir`.
pub fn setup_bare_worktrees(
    bare_result: &BareCloneResult,
    params: &BareCloneParams,
    layout: &crate::core::layout::Layout,
    progress: &mut dyn ProgressSink,
) -> Result<CloneResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);
    let use_multi_remote = params.remote.is_some() || params.multi_remote_enabled;
    let remote_for_path = params
        .remote
        .clone()
        .unwrap_or_else(|| params.multi_remote_default.clone());

    // Store layout in repos.json
    store_layout(&bare_result.git_dir, layout, progress);

    let should_create_worktree =
        !params.no_checkout && (bare_result.branch_exists || bare_result.is_empty);

    if should_create_worktree {
        let worktree_dir = crate::multi_remote::path::calculate_worktree_path(
            &bare_result.parent_dir,
            &bare_result.target_branch,
            &remote_for_path,
            use_multi_remote,
        );

        let relative_worktree_path = if use_multi_remote {
            PathBuf::from(&remote_for_path).join(&bare_result.target_branch)
        } else {
            PathBuf::from(&bare_result.target_branch)
        };

        if params.all_branches {
            if bare_result.is_empty {
                anyhow::bail!(
                    "Cannot use --all-branches with an empty repository (no branches exist)"
                );
            }
            create_all_worktrees(
                &git,
                &bare_result.remote_name,
                use_multi_remote,
                &remote_for_path,
                params.use_gitoxide,
                progress,
            )?;
        } else if bare_result.is_empty {
            create_orphan_worktree(
                &git,
                &bare_result.target_branch,
                &relative_worktree_path,
                progress,
            )?;
        } else {
            create_single_worktree(
                &git,
                &bare_result.target_branch,
                &relative_worktree_path,
                progress,
            )?;
        }

        progress.on_step(&format!(
            "Changing directory to worktree: './{}'",
            relative_worktree_path.display()
        ));

        if let Err(e) = change_directory(&relative_worktree_path) {
            change_directory(
                bare_result
                    .parent_dir
                    .parent()
                    .unwrap_or(Path::new(".")),
            )
            .ok();
            return Err(e);
        }

        if !bare_result.is_empty {
            setup_tracking(
                &git,
                &bare_result.remote_name,
                &bare_result.target_branch,
                params.checkout_upstream,
                progress,
            );
        }

        let current_dir = get_current_directory()?;

        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir.clone()),
            worktree_dir: Some(current_dir),
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else if !params.no_checkout && !bare_result.branch_exists {
        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: None,
            branch_not_found: true,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else {
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: None,
            worktree_dir: None,
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: true,
        })
    }
}
```

- [ ] **Step 2: Implement `unbare_and_checkout()`**

```rust
/// Phase 4b: Convert a fresh bare clone to a regular (non-bare) repo.
///
/// For a fresh bare clone into `<repo>/.git`, the structure is already
/// correct for a regular repo. Just set `core.bare=false` and check out.
pub fn unbare_and_checkout(
    bare_result: &BareCloneResult,
    params: &BareCloneParams,
    layout: &crate::core::layout::Layout,
    progress: &mut dyn ProgressSink,
) -> Result<CloneResult> {
    let git = GitCommand::new(false).with_gitoxide(params.use_gitoxide);

    // Store layout in repos.json
    store_layout(&bare_result.git_dir, layout, progress);

    progress.on_step("Converting to non-bare repository...");
    git.config_set("core.bare", "false")
        .context("Failed to set core.bare to false")?;

    if !params.no_checkout && (bare_result.branch_exists || bare_result.is_empty) {
        if !bare_result.is_empty {
            progress.on_step("Checking out working tree...");
            git.checkout(&bare_result.target_branch)
                .context("Failed to check out working tree")?;

            // Fetch and set up tracking
            setup_tracking(
                &git,
                &bare_result.remote_name,
                &bare_result.target_branch,
                params.checkout_upstream,
                progress,
            );
        }

        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir.clone()),
            worktree_dir: Some(current_dir),
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else if !params.no_checkout && !bare_result.branch_exists {
        let current_dir = get_current_directory()?;
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: Some(current_dir),
            worktree_dir: None,
            branch_not_found: true,
            is_empty: bare_result.is_empty,
            no_checkout: false,
        })
    } else {
        // --no-checkout: bare→non-bare conversion done, no checkout
        Ok(CloneResult {
            repo_name: bare_result.repo_name.clone(),
            target_branch: bare_result.target_branch.clone(),
            default_branch: bare_result.default_branch.clone(),
            parent_dir: bare_result.parent_dir.clone(),
            git_dir: bare_result.git_dir.clone(),
            remote_name: bare_result.remote_name.clone(),
            repository_url: bare_result.repository_url.clone(),
            cd_target: None,
            worktree_dir: None,
            branch_not_found: false,
            is_empty: bare_result.is_empty,
            no_checkout: true,
        })
    }
}
```

- [ ] **Step 3: Build and run tests**

```bash
mise run fmt && mise run clippy && mise run test:unit
```

Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/clone.rs
git commit -m "feat(clone): add setup_bare_worktrees and unbare_and_checkout phases"
```

---

### Task 4: Rewire `run_clone()` to use the new phases

**Files:**

- Modify: `src/commands/clone.rs:159-286` (the `run_clone` function)

This is the integration task — rewire the command layer to use the new phase
functions and move the layout prompt to post-clone.

- [ ] **Step 1: Rewrite `run_clone()`**

Replace the entire `run_clone` function body (lines 159-286):

```rust
fn run_clone(args: &Args, settings: &DaftSettings, output: &mut dyn Output) -> Result<()> {
    check_dependencies()?;

    let global_config = GlobalConfig::load().unwrap_or_default();
    let original_dir = get_current_directory()?;

    // Phase 1: Always clone bare first
    let bare_params = clone::BareCloneParams {
        repository_url: args.repository_url.clone(),
        branch: args.branch.clone(),
        no_checkout: args.no_checkout,
        all_branches: args.all_branches,
        remote: args.remote.clone(),
        remote_name: settings.remote.clone(),
        multi_remote_enabled: settings.multi_remote_enabled,
        multi_remote_default: settings.multi_remote_default.clone(),
        checkout_upstream: settings.checkout_upstream,
        use_gitoxide: settings.use_gitoxide,
    };

    if should_show_gitoxide_notice(settings.use_gitoxide) {
        output.warning("[experimental] Using gitoxide backend for git operations");
    }

    output.start_spinner("Cloning repository...");
    let bare_result = {
        let mut sink = OutputSink(output);
        clone::clone_bare_phase(&bare_params, &mut sink)
    };
    output.finish_spinner();
    let bare_result = bare_result?;

    // Phase 2: Read daft.yml from the bare repo (if no --layout flag)
    let yaml_layout = if args.layout.is_none() && !bare_result.is_empty {
        match yaml_config_loader::load_config_from_bare(&bare_result.git_dir) {
            Ok(Some(config)) => config.layout,
            Ok(None) => None,
            Err(e) => {
                output.warning(&format!("Could not read daft.yml: {e}"));
                None
            }
        }
    } else {
        None
    };

    // Phase 3: Resolve layout with full context
    let prompted_layout = if args.layout.is_none()
        && yaml_layout.is_none()
        && global_config.defaults.layout.is_none()
    {
        match maybe_prompt_layout_choice(output) {
            LayoutPromptResult::Chosen(layout) => Some(layout),
            LayoutPromptResult::Default => None,
            LayoutPromptResult::Cancelled => {
                // Clean up: we already cloned, so delete it
                change_directory(&original_dir).ok();
                remove_directory(&bare_result.parent_dir).ok();
                return Ok(());
            }
        }
    } else {
        None
    };

    let effective_cli_layout = args.layout.as_deref().or(prompted_layout.as_deref());

    let (layout, _source) = resolve_layout(&LayoutResolutionContext {
        cli_layout: effective_cli_layout,
        repo_store_layout: None,
        yaml_layout: yaml_layout.as_deref(),
        global_config: &global_config,
    });

    // Report layout decision
    if layout.needs_bare() {
        output.step(&format!(
            "Using layout '{}' (worktrees inside repo)",
            layout.name
        ));
    } else {
        output.step(&format!("Using layout '{}'", layout.name));
    }

    // Phase 4: Set up repo in the correct layout
    let result = if layout.needs_bare() {
        output.start_spinner("Setting up worktrees...");
        let r = {
            let mut sink = OutputSink(output);
            clone::setup_bare_worktrees(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r?
    } else {
        output.start_spinner("Setting up repository...");
        let r = {
            let mut sink = OutputSink(output);
            clone::unbare_and_checkout(&bare_result, &bare_params, &layout, &mut sink)
        };
        output.finish_spinner();
        r?
    };

    render_clone_result(&result, &layout, output);

    // Remove stale trust entry if cloning to a path that was previously trusted.
    if !args.trust_hooks {
        let mut trust_db = TrustDatabase::load().unwrap_or_default();
        if trust_db.has_explicit_trust(&result.git_dir) {
            trust_db.remove_trust(&result.git_dir);
            if let Err(e) = trust_db.save() {
                output.warning(&format!("Could not remove stale trust entry: {e}"));
            } else {
                output.step("Removed stale trust entry for previous repository at this path");
            }
        }
    }

    // Run hooks and exec only if a worktree was created
    if result.worktree_dir.is_some() {
        run_post_clone_hook(args, &result, output)?;
        if !layout.needs_bare() {
            run_post_create_hook(args, &result, output)?;
        }

        let exec_result = crate::exec::run_exec_commands(&args.exec, output);

        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;

        exec_result?;
    } else if result.branch_not_found {
        if let Some(ref cd_target) = result.cd_target {
            output.cd_path(cd_target);
        }
        maybe_show_shell_hint(output)?;
    }

    Ok(())
}
```

- [ ] **Step 2: Remove `reconcile_layout()`**

Delete the `reconcile_layout()` function (lines ~409-475 in the original file).
It is no longer needed.

- [ ] **Step 3: Remove unused imports**

Remove `transform` and `Layout` from imports if no longer used directly (check
after removing `reconcile_layout`). The `layout` module imports may change to
just `resolver` and the `Layout` type.

- [ ] **Step 4: Update `run()` function**

The `run()` function at line 120 currently saves `original_dir` and restores on
error. Now `run_clone` itself handles `original_dir` for the cancellation case.
Check that the error path in `run()` still works: `run_clone` errors will cause
`change_directory(&original_dir)` in `run()` — this is still correct since
`original_dir` is captured before `run_clone` is called.

Actually — `run_clone` now tracks `original_dir` internally. The outer `run()`
already does `change_directory(&original_dir).ok()` on error (line 136). This is
fine: both the cancellation path (inside `run_clone`) and the error path (in
`run()`) restore cwd correctly.

- [ ] **Step 5: Build**

```bash
mise run fmt && mise run clippy
```

Expected: Compiles. There may be dead code warnings for the old
`execute()`/`execute_bare()`/`execute_regular()` — that's OK, we'll clean them
up in Task 5.

- [ ] **Step 6: Run unit tests**

```bash
mise run test:unit
```

Expected: All pass.

- [ ] **Step 7: Commit**

```bash
git add src/commands/clone.rs
git commit -m "feat(clone): rewire run_clone to use bare-first phases"
```

---

### Task 5: Clean up dead code in clone modules

**Files:**

- Modify: `src/core/worktree/clone.rs` — remove `execute()`, `execute_bare()`,
  `execute_regular()`, `CloneParams`
- Modify: `src/git/clone.rs` — remove `clone_regular()`,
  `clone_regular_branch()`

- [ ] **Step 1: Remove `execute()`, `execute_bare()`, `execute_regular()`**

In `src/core/worktree/clone.rs`, delete these three functions. They are replaced
by `clone_bare_phase()`, `setup_bare_worktrees()`, and `unbare_and_checkout()`.

- [ ] **Step 2: Remove `CloneParams` and refactor `detect_branches`**

Delete the `CloneParams` struct and the `detect_branches_bare` compat wrapper.
Update `detect_branches` to accept `&BareCloneParams` directly instead of
`&CloneParams`. The function only reads `params.branch`,
`params.repository_url`, and `params.use_gitoxide` — all present on
`BareCloneParams`. Update `clone_bare_phase` to call `detect_branches` directly.

- [ ] **Step 3: Remove `clone_regular()` and `clone_regular_branch()`**

In `src/git/clone.rs`, delete these two functions. Only `clone_bare()` remains.

- [ ] **Step 4: Fix any remaining compilation errors**

Check for any remaining references to removed types/functions. Fix imports.

- [ ] **Step 5: Build, lint, test**

```bash
mise run fmt && mise run clippy && mise run test:unit
```

Expected: All pass, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/clone.rs src/git/clone.rs
git commit -m "refactor(clone): remove old execute/CloneParams/clone_regular"
```

---

### Task 6: Update test scenarios

**Files:**

- Modify: `tests/manual/scenarios/clone/layout-from-daft-yml.yml`
- Modify: `tests/manual/scenarios/clone/layout-prompt-skipped-for-daft-yml.yml`

- [ ] **Step 1: Update `layout-from-daft-yml.yml`**

Replace file content with:

```yaml
name: Clone uses daft.yml layout directly
description:
  When a cloned repo has daft.yml with a layout field and no --layout flag was
  given, daft should clone directly with that layout (no post-clone transform).

repos:
  - name: test-repo
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# test-repo"
        commits:
          - message: "Initial commit"
    daft_yml: |
      layout: contained

steps:
  - name: Clone without --layout flag
    run: git-worktree-clone $REMOTE_TEST_REPO 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "contained"

  - name: Verify repo uses contained (bare) layout
    run: "true"
    expect:
      dirs_exist:
        - "$WORK_DIR/test-repo/main"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/main"
          branch: main

  - name: Verify repos.json stores contained
    run: NO_COLOR=1 daft layout show 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "contained"
```

- [ ] **Step 2: Update `layout-prompt-skipped-for-daft-yml.yml`**

Replace file content with:

```yaml
name: daft.yml layout suppresses first-time prompt
description:
  When the cloned repo has a daft.yml with a layout field, the first-time layout
  prompt should not appear. The daft.yml layout is used directly.

repos:
  - name: test-repo
    default_branch: main
    branches:
      - name: main
        files:
          - path: README.md
            content: "# test-repo"
        commits:
          - message: "Initial commit"
    daft_yml: |
      layout: contained

steps:
  - name: Clone (no prompt should appear, daft.yml takes over)
    run: git-worktree-clone $REMOTE_TEST_REPO 2>&1
    expect:
      exit_code: 0
      output_not_contains:
        - "Use contained?"
        - "set as default"

  - name: Verify contained layout used directly
    run: "true"
    expect:
      dirs_exist:
        - "$WORK_DIR/test-repo/main"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/main"
          branch: main
```

- [ ] **Step 3: Run the updated tests**

```bash
mise run test:manual -- --ci clone:layout-from-daft-yml clone:layout-prompt-skipped-for-daft-yml
```

Expected: Both pass.

- [ ] **Step 4: Run full test suite**

```bash
mise run fmt && mise run clippy && mise run test:unit
mise run test:manual -- --ci
```

Expected: All pass. The previously failing `layout-from-daft-yml` scenario
should now pass.

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/clone/layout-from-daft-yml.yml \
        tests/manual/scenarios/clone/layout-prompt-skipped-for-daft-yml.yml
git commit -m "test(clone): update daft.yml scenarios for bare-first clone flow"
```
