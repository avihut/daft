# daft merge Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `daft merge` — a cross-worktree merge command with full git flag
parity, layered git-config-driven defaults, optional post-merge cleanup, and
explicit handling of worktree-specific edge cases (ephemeral worktree for
targets without one, promote-on-conflict, cross-worktree `--abort` /
`--continue` / `--quit`).

**Architecture:** Two new Rust modules: `src/commands/merge.rs` (clap `Args`,
mode dispatch, prompts, verbose reporting) and `src/core/worktree/merge.rs`
(plumbing fast-forward, worktree-delegated merge execution, ephemeral-worktree
lifecycle, cross-worktree merge-state scan). Settings extend `DaftSettings` with
a `merge.*` group read from `daft.merge.*` git-config keys. `daft list` gains a
`--merging` filter flag for status surfacing. Post-merge cleanup delegates to
existing worktree-remove and `git branch -d` code paths.

**Tech Stack:** Rust, clap v4 (command-line parsing), anyhow (error handling),
daft's existing `GitCommand` wrapper, existing
`src/core/worktree/temp_worktree.rs` for temp worktrees, existing `src/hooks/`
for hook dispatch, existing YAML manual-test scenarios at
`tests/manual/scenarios/`.

---

## File Structure

**New files:**

- `src/commands/merge.rs` — clap `Args` struct, CLI flag validation, mode
  dispatch (start / abort / continue / quit), prompt handling, user-facing
  output. Entry point `pub fn run() -> Result<()>`.
- `src/core/worktree/merge.rs` — core logic: target resolution, pre-flight
  checks, pure-fast-forward plumbing, worktree-delegated merge, ephemeral
  worktree lifecycle, cross-worktree merge-state scan, cleanup orchestration.
  Public API: `execute_start()`, `execute_finish()`,
  `list_in_progress_merges()`.

**Modified files:**

- `src/commands/mod.rs` — add `pub mod merge;`.
- `src/main.rs` — add `"git-worktree-merge" => commands::merge::run(),` symlink
  route, add `"merge" => commands::merge::run(),` and
  `"worktree-merge" => commands::merge::run(),` daft-subcommand routes.
- `src/shortcuts.rs` — add `gwtm` git-style shortcut, update the
  `valid_commands` list in tests, bump the expected-git-style count from 10
  to 11.
- `src/suggest.rs` — add `"merge"` and `"worktree-merge"` to `DAFT_SUBCOMMANDS`
  (preserving alphabetical order).
- `src/doctor/installation.rs` — add `"git-worktree-merge"` to
  `EXPECTED_SYMLINKS`.
- `src/commands/list.rs` — add `--merging` flag to `Args`, thread through to the
  list filter, add `merging` / `since` virtual columns.
- `src/core/settings.rs` — add merge fields to `DaftSettings`, defaults,
  `load()` plumbing, add new config keys to the keys module.
- `xtask/src/main.rs` — add `"git-worktree-merge"` to `COMMANDS`, `"daft-merge"`
  to `DAFT_VERBS`, and add arm to `get_command_for_name()`.
- `src/commands/docs.rs` — add merge to the relevant help category.
- `src/commands/completions/bash.rs` — add `merge` subcommand completion,
  `list --merging` flag.
- `src/commands/completions/zsh.rs` — same.
- `src/commands/completions/fish.rs` — same.
- `src/commands/completions/fig.rs` — same.
- `daft.yml` — (optional) illustrate a `worktree-pre-remove` hook example
  showing it still fires for `-r` cleanup. Not required for this plan.

**Regenerated files (by tooling):**

- `man/git-worktree-merge.1`, `man/daft-merge.1` — via `mise run man:gen`.

**New documentation:**

- `docs/cli/daft-merge.md` — reference page, following `docs/cli/daft-doctor.md`
  template.
- `SKILL.md` — update with the new command.

**New tests:**

- Unit tests co-located in `src/core/worktree/merge.rs` and
  `src/commands/merge.rs`.
- YAML scenarios in `tests/manual/scenarios/merge/` (see Testing tasks below for
  complete list).

---

## Conventions used throughout the plan

- **Test-first discipline.** Every logic change has a test written before the
  code. Verify fail → implement → verify pass → commit.
- **Commit cadence.** Each task ends with one commit. Commit messages use
  Conventional Commits (`feat`, `fix`, `refactor`, `docs`, `test`, `chore`).
- **Run verification after every change.**
  - Unit tests: `cargo test --lib -- <test_pattern>`.
  - YAML scenarios: `mise run test:manual -- --ci merge:<scenario>`.
  - Lint: `mise run clippy`. Format: `mise run fmt`.
- **Rust edition and imports.** Follow the patterns in `src/commands/carry.rs`
  (clap derive, `use crate::...`, `anyhow::Result`).
- **Clippy zero warnings.** The `clippy` hook is required in CI.

---

## Slice 1 — Command scaffolding

Goal: `daft merge --help` runs and displays help. No merge logic yet; the
command prints "not yet implemented" and exits non-zero on any invocation that
isn't `--help` / `-h`.

### Task 1.1: Create `src/commands/merge.rs` skeleton

**Files:**

- Create: `src/commands/merge.rs`
- Modify: `src/commands/mod.rs`

- [ ] **Step 1: Create the merge command file with clap scaffold**

Write `src/commands/merge.rs`:

```rust
//! git-worktree-merge - Merge branches across worktrees
//!
//! Mirrors git merge semantics when --into is omitted; enables
//! cross-worktree merges (merge <source>... into <target> from any
//! worktree) when --into is supplied. Finish commands (--abort,
//! --continue, --quit) take an optional positional <worktree|branch>
//! argument, default to CWD.

use anyhow::Result;
use clap::Parser;

#[derive(Parser)]
#[command(name = "git-worktree-merge")]
#[command(version = crate::VERSION)]
#[command(about = "Merge branches across worktrees")]
#[command(long_about = r#"
Merges one or more source branches into a target worktree's branch.

When --into is omitted, the target is the current worktree's branch,
mirroring `git merge`. When --into <target> is supplied, the merge is
performed against that worktree's branch from wherever you are.

Multiple sources invoke git's octopus strategy, announced explicitly.

Finish commands (--abort, --continue, --quit) take an optional positional
<worktree|branch>; default to the current worktree's branch.
"#)]
pub struct Args {
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,
}

pub fn run() -> Result<()> {
    let _args = Args::parse_from(crate::get_clap_args("git-worktree-merge"));
    anyhow::bail!("daft merge: not yet implemented");
}
```

- [ ] **Step 2: Register the module in `src/commands/mod.rs`**

Insert alphabetically between `list` and `multi_remote`:

```rust
pub mod merge;
```

- [ ] **Step 3: Run `cargo build` to verify compilation**

Run: `cargo build --bin daft` Expected: builds successfully with no errors.

- [ ] **Step 4: Commit**

```bash
git add src/commands/merge.rs src/commands/mod.rs
git commit -m "feat: scaffold daft merge command module"
```

### Task 1.2: Register the command in main dispatch

**Files:**

- Modify: `src/main.rs`

- [ ] **Step 1: Add the symlink route and two subcommand routes**

In `src/main.rs`, add `"git-worktree-merge" => commands::merge::run(),` in the
symlink dispatch block (after `"git-worktree-list"`). Then in the daft
subcommand dispatch, add `"merge" => commands::merge::run(),` in the
alphabetical verb aliases section (after `"list"`) and
`"worktree-merge" => commands::merge::run(),` in the worktree-prefixed section
(after `"worktree-list"`).

Example location for the first: after line 81
(`"git-worktree-list" => commands::list::run(),`). Example location for the verb
alias: after line 138 (`"list" => commands::list::run(),`). Example location for
worktree-prefixed: after line 154 (`"worktree-list" => commands::list::run(),`).

- [ ] **Step 2: Build and invoke `--help`**

Run: `cargo build --bin daft && ./target/debug/daft merge --help` Expected: help
text appears, exit 0.

- [ ] **Step 3: Invoke without args**

Run: `./target/debug/daft merge` Expected: error "daft merge: not yet
implemented", exit non-zero.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: route daft merge through main dispatch"
```

### Task 1.3: Register the shortcut, subcommand list, and doctor symlink

**Files:**

- Modify: `src/shortcuts.rs`
- Modify: `src/suggest.rs`
- Modify: `src/doctor/installation.rs`

- [ ] **Step 1: Write failing tests for the new shortcut and subcommand**

In `src/shortcuts.rs`, inside the `tests` module, extend
`test_resolve_git_style`:

```rust
assert_eq!(resolve("gwtm"), "git-worktree-merge");
```

And update `test_all_shortcuts_map_to_valid_commands` to include
`"git-worktree-merge"` in the `valid_commands` array.

And update `test_shortcuts_for_style` to expect `git_shortcuts.len() == 11`
instead of `10`.

In `src/suggest.rs`, inside the test that verifies alphabetical order, no change
needed (that test doesn't hard-code a length).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -p daft shortcuts::tests` Expected: failures in
`test_resolve_git_style` (gwtm unresolved) and `test_shortcuts_for_style`
(length mismatch).

- [ ] **Step 3: Add `gwtm` to the `SHORTCUTS` array**

In `src/shortcuts.rs`, add (alphabetically correct position, but within the
Git-style block — after `gwtls`):

```rust
Shortcut {
    alias: "gwtm",
    command: "git-worktree-merge",
    style: ShortcutStyle::Git,
},
```

- [ ] **Step 4: Add `merge` and `worktree-merge` to `DAFT_SUBCOMMANDS`**

In `src/suggest.rs`, preserve alphabetical order (insert between `"list"` and
`"multi-remote"` for `"merge"`, and between `"worktree-list"` and
`"worktree-prune"` for `"worktree-merge"`):

```rust
"merge",
...
"worktree-merge",
```

- [ ] **Step 5: Add to `EXPECTED_SYMLINKS`**

In `src/doctor/installation.rs`, add `"git-worktree-merge"` to the
`EXPECTED_SYMLINKS` array (alphabetical placement after `"git-worktree-list"`,
or keep grouped with other `git-worktree-*` by insertion order).

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib -p daft shortcuts::tests suggest::` Expected: all tests
pass.

- [ ] **Step 7: Commit**

```bash
git add src/shortcuts.rs src/suggest.rs src/doctor/installation.rs
git commit -m "feat: register gwtm shortcut and merge subcommand suggestions"
```

### Task 1.4: Register the command for man page generation

**Files:**

- Modify: `xtask/src/main.rs`

- [ ] **Step 1: Add `"git-worktree-merge"` to `COMMANDS`**

Insert in the `COMMANDS` array in `xtask/src/main.rs` (alphabetical, after
`"git-worktree-list"` and before `"git-worktree-sync"`):

```rust
"git-worktree-list",
"git-worktree-merge",
"git-worktree-sync",
```

- [ ] **Step 2: Add daft verb entry**

In the `DAFT_VERBS` array, add after `"daft-list"`:

```rust
DaftVerbEntry {
    daft_name: "daft-merge",
    source_command: "git-worktree-merge",
    about_override: None,
},
```

- [ ] **Step 3: Add arm to `get_command_for_name()`**

In `xtask/src/main.rs` `get_command_for_name()`:

```rust
"git-worktree-merge" => Some(daft::commands::merge::Args::command()),
```

- [ ] **Step 4: Verify xtask compiles**

Run: `cargo build -p xtask` Expected: builds successfully.

- [ ] **Step 5: Verify xtask consistency tests still pass**

Run: `cargo test -p xtask` Expected: all existing xtask tests (which iterate
over `COMMANDS`) pass with the new entry.

- [ ] **Step 6: Commit**

```bash
git add xtask/src/main.rs
git commit -m "feat: register git-worktree-merge for man page generation"
```

---

## Slice 2 — Basic CWD merge (smoke path)

Goal: `daft merge <source>` in a worktree dispatches to
`git -C <cwd> merge <source>`. Single source, no flags, no target resolution.
Lays the foundation for real merge logic before we add cross-worktree behavior.

### Task 2.1: Core module skeleton with `execute_start`

**Files:**

- Create: `src/core/worktree/merge.rs`
- Modify: `src/core/worktree/mod.rs`

- [ ] **Step 1: Create `src/core/worktree/merge.rs`**

```rust
//! Core merge logic for daft merge.
//!
//! Handles target resolution, pre-flight checks, pure-fast-forward
//! advancement via plumbing, worktree-delegated merge execution, the
//! ephemeral worktree lifecycle, cross-worktree in-progress-merge scan,
//! and post-merge cleanup orchestration.

use anyhow::{Context, Result};
use std::path::Path;

use crate::git::GitCommand;

/// Parameters for starting a merge.
pub struct StartParams {
    /// One or more source refs to merge in.
    pub sources: Vec<String>,
}

/// Outcome of a start-form merge.
pub struct StartOutcome {
    pub already_up_to_date: bool,
    pub conflicted: bool,
}

/// Execute a merge in the current working tree.
///
/// This minimal implementation delegates to `git merge` unchanged.
/// Later slices extend it with --into, safety rails, flag passthrough,
/// and cross-worktree behavior.
pub fn execute_start(
    target_worktree: &Path,
    params: &StartParams,
    git: &GitCommand,
) -> Result<StartOutcome> {
    let status = git
        .run_in(target_worktree, &["merge".to_string()].into_iter()
            .chain(params.sources.iter().cloned())
            .collect::<Vec<_>>())
        .context("failed to invoke git merge")?;

    Ok(StartOutcome {
        already_up_to_date: false,
        conflicted: !status.success(),
    })
}

#[cfg(test)]
mod tests {
    // Tests added in later tasks.
}
```

(Note: `git.run_in(path, args)` is the method name in daft's existing
`GitCommand` wrapper. If the actual method is named differently, match the name
from `src/git.rs`. Inspect with
`grep -n "pub fn run\|pub fn run_in" src/git.rs`.)

- [ ] **Step 2: Register module in `src/core/worktree/mod.rs`**

Add `pub mod merge;` alphabetically (between `list` and `prune` or the
appropriate location per the file's existing ordering).

- [ ] **Step 3: Build to verify**

Run: `cargo build --bin daft` Expected: builds. Adjust `git.run_in(...)` call
site to the real method if it didn't compile.

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/merge.rs src/core/worktree/mod.rs
git commit -m "feat: scaffold core merge module"
```

### Task 2.2: Wire start-form command to core

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Extend `Args` with a `sources` positional**

Replace the `Args` struct in `src/commands/merge.rs`:

```rust
#[derive(Parser)]
#[command(name = "git-worktree-merge")]
#[command(version = crate::VERSION)]
#[command(about = "Merge branches across worktrees")]
#[command(long_about = r#"
(existing long_about unchanged)
"#)]
pub struct Args {
    /// Source branches/commits to merge. Two or more invoke octopus.
    #[arg(value_name = "SOURCE", num_args = 1..)]
    pub sources: Vec<String>,

    #[arg(short, long, help = "Be verbose; show detailed progress")]
    pub verbose: bool,
}
```

- [ ] **Step 2: Replace `run()` body to dispatch through core**

```rust
pub fn run() -> Result<()> {
    use crate::{
        core::worktree::merge as core,
        git::GitCommand,
        is_git_repository,
        logging::init_logging,
        settings::DaftSettings,
    };

    let args = Args::parse_from(crate::get_clap_args("git-worktree-merge"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    if args.sources.is_empty() {
        anyhow::bail!("specify at least one source to merge");
    }

    let settings = DaftSettings::load()?;
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);

    let cwd = std::env::current_dir()?;
    let params = core::StartParams { sources: args.sources };

    let outcome = core::execute_start(&cwd, &params, &git)?;

    if outcome.already_up_to_date {
        println!("Already up to date.");
    } else if outcome.conflicted {
        anyhow::bail!("merge conflicted — resolve then run `daft merge --continue`");
    } else {
        println!("Merge complete.");
    }
    Ok(())
}
```

- [ ] **Step 3: Build and verify compilation**

Run: `cargo build --bin daft` Expected: builds. Fix import paths if any don't
resolve.

- [ ] **Step 4: Commit**

```bash
git add src/commands/merge.rs
git commit -m "feat: dispatch daft merge start form to core"
```

### Task 2.3: First end-to-end YAML scenario — basic CWD merge

**Files:**

- Create: `tests/manual/scenarios/merge/basic.yml`

- [ ] **Step 1: Write the failing scenario**

Create `tests/manual/scenarios/merge/basic.yml`:

```yaml
name: Merge basic
description: "daft merge <source> in CWD performs git merge <source>"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Checkout feature branch
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Merge feature into main (from main worktree)
    run: git-worktree-merge feature/test-feature
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Merge complete."

  - name: Verify main contains feature commits
    run: git log --oneline main
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run the scenario**

Run: `mise run test:manual -- --ci merge:basic` Expected: the scenario runs and
passes (main's branch now contains feature/test-feature's tip as a merge commit
or fast-forward advance, depending on the fixture's topology).

- [ ] **Step 3: If it fails**, read the output carefully. Most likely failure
      modes are:

  - Fixture branch name differs — adjust `feature/test-feature`.
  - CWD directory naming — `$WORK_DIR/test-repo/main` differs in this fixture;
    inspect `tests/manual/scenarios/list/basic.yml` for the conventional path
    naming.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/merge/basic.yml
git commit -m "test: basic daft merge scenario in CWD"
```

---

## Slice 3 — `--into` target resolution

Goal: `daft merge feat --into main` performs the merge in `main`'s worktree
regardless of where the command is invoked from. Target accepts either a
worktree directory name or a branch name, with worktree winning on conflict
(matching `carry`'s convention).

### Task 3.1: Add `--into` to clap `Args`

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Extend `Args` with `into`**

Add field to `Args`:

```rust
/// Target worktree/branch. Defaults to the current worktree's branch.
#[arg(long = "into", value_name = "TARGET")]
pub into: Option<String>,
```

- [ ] **Step 2: Thread `args.into` into `StartParams`**

Extend `src/core/worktree/merge.rs` `StartParams`:

```rust
pub struct StartParams {
    pub sources: Vec<String>,
    /// Optional target; None → current worktree's branch.
    pub target: Option<String>,
}
```

Update `run()` in `src/commands/merge.rs` to set `target: args.into`.

- [ ] **Step 3: Build**

Run: `cargo build --bin daft` Expected: compiles.

- [ ] **Step 4: Commit (no behavior change yet — plumbing only)**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs
git commit -m "feat: add --into flag to merge Args and StartParams"
```

### Task 3.2: Target-resolution helper

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write failing tests for target resolution**

Append to the `tests` module in `src/core/worktree/merge.rs`:

```rust
#[cfg(test)]
mod target_resolution_tests {
    use super::*;
    use crate::core::worktree::list::WorktreeInfo;
    use std::path::PathBuf;

    fn wt(name: &str, branch: Option<&str>, path: &str) -> WorktreeInfo {
        let mut info = WorktreeInfo::empty(name);
        info.branch = branch.map(|s| s.to_string());
        info.path = Some(PathBuf::from(path));
        info
    }

    #[test]
    fn resolves_worktree_by_directory_name() {
        let worktrees = vec![
            wt("main", Some("main"), "/repo/main"),
            wt("feat", Some("feature/x"), "/repo/feat"),
        ];
        let resolved = resolve_target(Some("feat"), &worktrees, None).unwrap();
        assert_eq!(resolved.path, PathBuf::from("/repo/feat"));
        assert_eq!(resolved.branch, "feature/x");
    }

    #[test]
    fn resolves_worktree_by_branch_name() {
        let worktrees = vec![
            wt("main", Some("main"), "/repo/main"),
            wt("feat-x", Some("feature/x"), "/repo/feat-x"),
        ];
        let resolved = resolve_target(Some("feature/x"), &worktrees, None).unwrap();
        assert_eq!(resolved.branch, "feature/x");
    }

    #[test]
    fn worktree_name_wins_on_collision() {
        let worktrees = vec![
            wt("shared", Some("branch-a"), "/repo/shared"),
            wt("other", Some("shared"), "/repo/other"),
        ];
        let resolved = resolve_target(Some("shared"), &worktrees, None).unwrap();
        assert_eq!(resolved.branch, "branch-a");
    }

    #[test]
    fn defaults_to_cwd_branch() {
        let worktrees = vec![
            wt("main", Some("main"), "/repo/main"),
            wt("feat", Some("feature/x"), "/repo/feat"),
        ];
        let resolved = resolve_target(None, &worktrees, Some("main")).unwrap();
        assert_eq!(resolved.branch, "main");
    }

    #[test]
    fn no_match_is_error() {
        let worktrees = vec![wt("main", Some("main"), "/repo/main")];
        assert!(resolve_target(Some("bogus"), &worktrees, None).is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify failure**

Run:
`cargo test --lib --package daft -- core::worktree::merge::target_resolution_tests`
Expected: all five tests fail (function `resolve_target` undefined).

- [ ] **Step 3: Implement `resolve_target`**

Add to `src/core/worktree/merge.rs`:

```rust
use crate::core::worktree::list::WorktreeInfo;
use std::path::PathBuf;

/// A resolved merge target.
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    /// Branch being merged into.
    pub branch: String,
    /// Path to the target worktree on disk, if one exists.
    pub path: PathBuf,
}

/// Resolve a target identifier to a worktree.
///
/// If `target` is `Some`, match by worktree directory name first, then by
/// branch name. If `target` is `None`, use `cwd_branch` (the current
/// worktree's branch).
pub fn resolve_target(
    target: Option<&str>,
    worktrees: &[WorktreeInfo],
    cwd_branch: Option<&str>,
) -> Result<ResolvedTarget> {
    match target {
        Some(name) => {
            // worktree directory name wins
            if let Some(w) = worktrees.iter().find(|w| w.name == name) {
                return Ok(ResolvedTarget {
                    branch: w.branch.clone().unwrap_or_else(|| name.to_string()),
                    path: w.path.clone().unwrap_or_default(),
                });
            }
            // then branch name
            if let Some(w) = worktrees.iter().find(|w| w.branch.as_deref() == Some(name)) {
                return Ok(ResolvedTarget {
                    branch: name.to_string(),
                    path: w.path.clone().unwrap_or_default(),
                });
            }
            anyhow::bail!("no worktree or branch named '{}'", name)
        }
        None => {
            let branch = cwd_branch
                .ok_or_else(|| anyhow::anyhow!("cannot determine current branch"))?;
            if let Some(w) = worktrees.iter().find(|w| w.branch.as_deref() == Some(branch)) {
                Ok(ResolvedTarget {
                    branch: branch.to_string(),
                    path: w.path.clone().unwrap_or_default(),
                })
            } else {
                anyhow::bail!("current branch '{}' has no known worktree", branch)
            }
        }
    }
}
```

(Adjust to whatever fields `WorktreeInfo` actually has — inspect
`src/core/worktree/list.rs` for the struct definition. Adapt `w.name` /
`w.branch` / `w.path` accordingly.)

- [ ] **Step 4: Run tests to verify pass**

Run:
`cargo test --lib --package daft -- core::worktree::merge::target_resolution_tests`
Expected: all five pass.

- [ ] **Step 5: Commit**

```bash
git add src/core/worktree/merge.rs
git commit -m "feat: resolve --into target by worktree dir or branch name"
```

### Task 3.3: Wire target resolution into `execute_start`

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Refactor `execute_start` to accept resolved target**

Replace the current `execute_start` body:

```rust
pub fn execute_start(
    params: &StartParams,
    git: &GitCommand,
    project_root: &Path,
) -> Result<StartOutcome> {
    let worktrees = crate::core::worktree::list::collect_worktree_info(
        project_root,
        /* ... defaults per list::collect_worktree_info signature ... */
    )?;
    let cwd_branch = git.current_branch_in(&std::env::current_dir()?).ok();
    let target = resolve_target(params.target.as_deref(), &worktrees, cwd_branch.as_deref())?;

    let mut args = vec!["merge".to_string()];
    args.extend(params.sources.iter().cloned());

    let status = git.run_in(&target.path, &args)
        .context("failed to invoke git merge")?;

    Ok(StartOutcome {
        already_up_to_date: false,
        conflicted: !status.success(),
    })
}
```

(Adapt to real `collect_worktree_info` signature — inspect
`src/core/worktree/list.rs:746`. Adapt `git.current_branch_in()` to the real API
by grepping `grep -n "current_branch\|branch_of" src/git.rs`.)

- [ ] **Step 2: Update the command entry point in `src/commands/merge.rs`**

```rust
let project_root = crate::get_project_root()?;
let outcome = core::execute_start(&params, &git, &project_root)?;
```

- [ ] **Step 3: Build**

Run: `cargo build --bin daft`

- [ ] **Step 4: Commit**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs
git commit -m "feat: dispatch merge against resolved --into target"
```

### Task 3.4: YAML scenario — cross-worktree merge via `--into`

**Files:**

- Create: `tests/manual/scenarios/merge/cross-worktree.yml`

- [ ] **Step 1: Write the scenario**

```yaml
name: Merge cross-worktree
description: "daft merge <source> --into <target> works from any worktree"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Check out feature
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Merge feature into main *from the feature worktree*
    run: git-worktree-merge --into main feature/test-feature
    cwd: "$WORK_DIR/test-repo/feature/test-feature"
    expect:
      exit_code: 0
      output_contains:
        - "Merge complete."

  - name: Verify main advanced without being CWD
    run: git -C $WORK_DIR/test-repo/main log --oneline -1
    cwd: "$WORK_DIR/test-repo/feature/test-feature"
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run**

Run: `mise run test:manual -- --ci merge:cross-worktree` Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/merge/cross-worktree.yml
git commit -m "test: cross-worktree merge via --into"
```

---

## Slice 4 — Safety rails

Goal: Refuse source == target, target mid-operation, and dirty target (per
`merge.require_clean_target`).

### Task 4.1: Pre-flight check — source equals target

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write failing test**

In the `tests` module:

```rust
#[test]
fn refuses_when_source_equals_target() {
    let worktrees = vec![
        wt("main", Some("main"), "/repo/main"),
    ];
    let result = validate_distinct(
        &["main".to_string()],
        &ResolvedTarget { branch: "main".into(), path: "/repo/main".into() },
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("same branch"));
}

#[test]
fn allows_distinct_source_and_target() {
    let result = validate_distinct(
        &["feat".to_string()],
        &ResolvedTarget { branch: "main".into(), path: "/repo/main".into() },
    );
    assert!(result.is_ok());
}
```

- [ ] **Step 2: Verify fail**

Run: `cargo test --lib --package daft -- core::worktree::merge::` Expected: fail
(function undefined).

- [ ] **Step 3: Implement `validate_distinct`**

```rust
pub fn validate_distinct(sources: &[String], target: &ResolvedTarget) -> Result<()> {
    for src in sources {
        if src == &target.branch {
            anyhow::bail!("cannot merge branch '{}' into the same branch", src);
        }
    }
    Ok(())
}
```

Call from `execute_start` before the actual merge invocation.

- [ ] **Step 4: Verify pass**

Run: `cargo test --lib --package daft -- core::worktree::merge::` Expected:
pass.

- [ ] **Step 5: Add YAML scenario
      `tests/manual/scenarios/merge/same-source-target.yml`**

```yaml
name: Merge same source and target
description: "daft merge X --into X is refused"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Attempt same-branch merge
    run: git-worktree-merge main --into main
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "same branch"
```

- [ ] **Step 6: Run scenario**

Run: `mise run test:manual -- --ci merge:same-source-target` Expected: pass.

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/same-source-target.yml
git commit -m "feat: refuse merging a branch into itself"
```

### Task 4.2: Pre-flight check — in-progress operations

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn detects_in_progress_merge() {
    // Use a tempdir fixture with MERGE_HEAD present.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".git").join("MERGE_HEAD"), "abc").ok();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    std::fs::write(tmp.path().join(".git").join("MERGE_HEAD"), "abc").unwrap();
    let state = detect_in_progress(tmp.path()).unwrap();
    assert_eq!(state, Some(InProgressOp::Merge));
}

#[test]
fn detects_in_progress_rebase() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git").join("rebase-merge")).unwrap();
    let state = detect_in_progress(tmp.path()).unwrap();
    assert_eq!(state, Some(InProgressOp::Rebase));
}

#[test]
fn clean_worktree_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let state = detect_in_progress(tmp.path()).unwrap();
    assert_eq!(state, None);
}
```

- [ ] **Step 2: Verify fail**

Run: `cargo test --lib --package daft -- core::worktree::merge::`

- [ ] **Step 3: Implement**

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum InProgressOp {
    Merge,
    Rebase,
    CherryPick,
    Bisect,
}

impl InProgressOp {
    pub fn description(&self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Bisect => "bisect",
        }
    }
}

pub fn detect_in_progress(worktree: &Path) -> Result<Option<InProgressOp>> {
    let git_dir = worktree.join(".git");
    // Worktree link-files have a gitdir pointer; handle both cases.
    let resolved = if git_dir.is_file() {
        let content = std::fs::read_to_string(&git_dir)?;
        let prefix = "gitdir: ";
        let line = content.lines().next().unwrap_or_default();
        let rel = line.strip_prefix(prefix).unwrap_or(line);
        worktree.join(rel)
    } else {
        git_dir
    };

    if resolved.join("MERGE_HEAD").exists() {
        return Ok(Some(InProgressOp::Merge));
    }
    if resolved.join("rebase-merge").exists() || resolved.join("rebase-apply").exists() {
        return Ok(Some(InProgressOp::Rebase));
    }
    if resolved.join("CHERRY_PICK_HEAD").exists() {
        return Ok(Some(InProgressOp::CherryPick));
    }
    if resolved.join("BISECT_LOG").exists() {
        return Ok(Some(InProgressOp::Bisect));
    }
    Ok(None)
}
```

Call in `execute_start` after target resolution; if `Some(op)`, bail with a
message that surfaces git's state:

```rust
if let Some(op) = detect_in_progress(&target.path)? {
    anyhow::bail!(
        "target worktree '{}' is mid-{}; finish or abort it first",
        target.branch,
        op.description()
    );
}
```

- [ ] **Step 4: Verify pass**

- [ ] **Step 5: Add YAML scenario
      `tests/manual/scenarios/merge/target-in-operation.yml`**

```yaml
name: Merge refuses mid-operation target
description: "daft merge refuses if target worktree has an in-progress op"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Induce an in-progress rebase in main
    run: |
      cd $WORK_DIR/test-repo/main
      mkdir -p .git/rebase-merge
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Attempt merge into main — must refuse
    run: git-worktree-merge feature/test-feature --into main
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 1
      output_contains:
        - "mid-rebase"
```

(Inducing state this way is a test-harness shortcut; real git would leave richer
files. The presence of the dir is enough for `detect_in_progress`.)

- [ ] **Step 6: Run and commit**

```bash
mise run test:manual -- --ci merge:target-in-operation
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/target-in-operation.yml
git commit -m "feat: refuse merge when target worktree is mid-operation"
```

### Task 4.3: Pre-flight check — dirty target

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write test**

```rust
#[test]
fn dirty_target_refused_when_required() {
    // Uses fake_status to simulate `git status --porcelain` output.
    assert!(check_clean(&target_path, "M  file.txt\n", true).is_err());
}

#[test]
fn dirty_target_allowed_when_not_required() {
    assert!(check_clean(&target_path, "M  file.txt\n", false).is_ok());
}

#[test]
fn clean_target_ok() {
    assert!(check_clean(&target_path, "", true).is_ok());
}
```

(Adapt to a design that accepts status output as a parameter so tests don't
require invoking git.)

- [ ] **Step 2: Implement**

```rust
pub fn check_clean(target: &Path, porcelain: &str, require_clean: bool) -> Result<()> {
    if !require_clean || porcelain.trim().is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "target worktree '{}' has uncommitted changes; commit, stash, or pass \
         --no-require-clean-target",
        target.display()
    );
}
```

In `execute_start`, call:

```rust
let porcelain = git.status_porcelain_in(&target.path)?;
check_clean(&target.path, &porcelain, settings.merge_require_clean_target)?;
```

(Grep `grep -n "porcelain\|status_porcelain" src/git.rs` to find or add the
helper; if missing, invoke `git -C <path> status --porcelain` via `run_in` and
capture stdout.)

- [ ] **Step 3: Add scenario `tests/manual/scenarios/merge/dirty-target.yml`**

```yaml
name: Merge refuses dirty target
description: "daft merge refuses when target has uncommitted changes"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Check out feature
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Dirty the main worktree
    run: |
      echo "dirty" > $WORK_DIR/test-repo/main/new-file.txt
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Attempt merge — must refuse
    run: git-worktree-merge feature/test-feature --into main
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 1
      output_contains:
        - "uncommitted changes"
```

- [ ] **Step 4: Run and commit**

```bash
mise run test:manual -- --ci merge:dirty-target
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/dirty-target.yml
git commit -m "feat: refuse merge when target worktree is dirty"
```

---

## Slice 5 — Multi-source (octopus) announcement

Goal: When `sources.len() >= 2`, daft prints an explicit announcement and lets
git's natural octopus strategy engage. git errors when octopus can't proceed
(conflict) are surfaced verbatim.

### Task 5.1: Announcement and execution

**Files:**

- Modify: `src/core/worktree/merge.rs`
- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn announces_octopus_for_multi_source() {
    let params = StartParams {
        sources: vec!["a".into(), "b".into(), "c".into()],
        target: None,
    };
    let msg = announcement(&params, "main");
    assert!(msg.is_some());
    assert!(msg.unwrap().contains("3 sources"));
    assert!(msg.unwrap().contains("octopus"));
}

#[test]
fn no_announcement_for_single_source() {
    let params = StartParams {
        sources: vec!["a".into()],
        target: None,
    };
    assert!(announcement(&params, "main").is_none());
}
```

- [ ] **Step 2: Implement**

```rust
pub fn announcement(params: &StartParams, target_branch: &str) -> Option<String> {
    if params.sources.len() >= 2 {
        Some(format!(
            "Merging {} sources into {} via octopus strategy",
            params.sources.len(),
            target_branch
        ))
    } else {
        None
    }
}
```

In `execute_start`, print the announcement to stderr via the output helper
before invoking `git merge`. Accept an `&mut dyn Output` parameter or a callback
for testability (match the pattern used in `carry::execute` which takes
`&mut OutputSink`).

- [ ] **Step 3: YAML scenario — `tests/manual/scenarios/merge/octopus.yml`**

```yaml
name: Merge octopus success
description: "daft merge of multiple sources invokes octopus and announces it"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Checkout two feature branches from main
    run: |
      cd $WORK_DIR/test-repo/main
      git checkout -b feat-a
      echo "a" > file-a.txt && git add file-a.txt && git commit -m "feat a"
      git checkout main
      git checkout -b feat-b
      echo "b" > file-b.txt && git add file-b.txt && git commit -m "feat b"
      git checkout main
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Octopus merge
    run: git-worktree-merge feat-a feat-b
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "2 sources"
        - "octopus"
        - "Merge complete."
```

- [ ] **Step 4: Scenario for octopus conflict —
      `tests/manual/scenarios/merge/octopus-conflict.yml`**

Same setup, but feat-a and feat-b both touch the same file with conflicting
content. git octopus refuses when any pair conflicts:

```yaml
- name: Attempt octopus of conflicting branches
  run: git-worktree-merge feat-a feat-b
  cwd: "$WORK_DIR/test-repo/main"
  expect:
    exit_code: 1
    output_contains:
      - "octopus"
```

- [ ] **Step 5: Run and commit**

```bash
mise run test:manual -- --ci merge:octopus merge:octopus-conflict
git add src/core/worktree/merge.rs src/commands/merge.rs tests/manual/scenarios/merge/
git commit -m "feat: announce octopus strategy for multi-source merges"
```

---

## Slice 6 — Passthrough flags (git parity)

Goal: daft accepts every common `git merge` flag and forwards it to `git merge`.
Config layering (Slice 13) will build on this; for now, flags come strictly from
the CLI.

### Task 6.1: Extend `Args` with all passthrough flags

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Add flags to `Args`**

```rust
// Commit message and editor
#[arg(short = 'm', value_name = "MSG")]
pub message: Option<String>,
#[arg(short = 'F', long = "file", value_name = "FILE")]
pub file: Option<std::path::PathBuf>,
#[arg(long = "edit", conflicts_with = "no_edit")]
pub edit: bool,
#[arg(long = "no-edit", conflicts_with = "edit")]
pub no_edit: bool,
#[arg(long = "cleanup", value_name = "MODE")]
pub cleanup: Option<String>,

// Fast-forward control
#[arg(long = "ff", conflicts_with_all = ["no_ff", "ff_only"])]
pub ff: bool,
#[arg(long = "no-ff", conflicts_with_all = ["ff", "ff_only"])]
pub no_ff: bool,
#[arg(long = "ff-only", conflicts_with_all = ["ff", "no_ff"])]
pub ff_only: bool,

// Squash
#[arg(long = "squash", conflicts_with = "no_squash")]
pub squash: bool,
#[arg(long = "no-squash", conflicts_with = "squash")]
pub no_squash: bool,

// Commit control
#[arg(long = "commit", conflicts_with = "no_commit")]
pub commit: bool,
#[arg(long = "no-commit", conflicts_with = "commit")]
pub no_commit: bool,

// Signoff
#[arg(long = "signoff", conflicts_with = "no_signoff")]
pub signoff: bool,
#[arg(long = "no-signoff", conflicts_with = "signoff")]
pub no_signoff: bool,

// Strategy
#[arg(short = 's', long = "strategy", value_name = "STRAT")]
pub strategy: Option<String>,
#[arg(short = 'X', long = "strategy-option", value_name = "OPT")]
pub strategy_options: Vec<String>,

// GPG
#[arg(short = 'S', long = "gpg-sign", value_name = "KEYID", num_args = 0..=1, default_missing_value = "")]
pub gpg_sign: Option<String>,
#[arg(long = "no-gpg-sign", conflicts_with = "gpg_sign")]
pub no_gpg_sign: bool,

// Verification
#[arg(long = "verify-signatures", conflicts_with = "no_verify_signatures")]
pub verify_signatures: bool,
#[arg(long = "no-verify-signatures", conflicts_with = "verify_signatures")]
pub no_verify_signatures: bool,

// History
#[arg(long = "allow-unrelated-histories")]
pub allow_unrelated_histories: bool,

// Stat control
#[arg(long = "stat", conflicts_with = "no_stat")]
pub stat: bool,
#[arg(long = "no-stat", conflicts_with = "stat", short = 'n')]
pub no_stat: bool,
```

- [ ] **Step 2: Add serializer to core**

In `src/core/worktree/merge.rs`, add a helper that converts an `EffectiveFlags`
struct into `Vec<String>` suitable for `git merge`:

```rust
#[derive(Debug, Default, Clone)]
pub struct EffectiveFlags {
    pub message: Option<String>,
    pub file: Option<PathBuf>,
    pub edit: Option<bool>,
    pub cleanup: Option<String>,
    pub ff: Option<FfMode>,
    pub squash: Option<bool>,
    pub commit: Option<bool>,
    pub signoff: Option<bool>,
    pub strategy: Option<String>,
    pub strategy_options: Vec<String>,
    pub gpg_sign: Option<GpgSign>,
    pub verify_signatures: Option<bool>,
    pub allow_unrelated_histories: bool,
    pub stat: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfMode { Auto, Only, Never }

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpgSign { Default, KeyId(String), Disabled }

pub fn render_flags(flags: &EffectiveFlags) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(m) = &flags.message { out.extend(["-m".into(), m.clone()]); }
    if let Some(f) = &flags.file { out.extend(["-F".into(), f.display().to_string()]); }
    match flags.edit {
        Some(true) => out.push("--edit".into()),
        Some(false) => out.push("--no-edit".into()),
        None => {}
    }
    if let Some(c) = &flags.cleanup { out.extend(["--cleanup".into(), c.clone()]); }
    match flags.ff {
        Some(FfMode::Auto) => out.push("--ff".into()),
        Some(FfMode::Only) => out.push("--ff-only".into()),
        Some(FfMode::Never) => out.push("--no-ff".into()),
        None => {}
    }
    match flags.squash {
        Some(true) => out.push("--squash".into()),
        Some(false) => out.push("--no-squash".into()),
        None => {}
    }
    match flags.commit {
        Some(true) => out.push("--commit".into()),
        Some(false) => out.push("--no-commit".into()),
        None => {}
    }
    match flags.signoff {
        Some(true) => out.push("--signoff".into()),
        Some(false) => out.push("--no-signoff".into()),
        None => {}
    }
    if let Some(s) = &flags.strategy { out.extend(["-s".into(), s.clone()]); }
    for x in &flags.strategy_options { out.extend(["-X".into(), x.clone()]); }
    match &flags.gpg_sign {
        Some(GpgSign::Default) => out.push("-S".into()),
        Some(GpgSign::KeyId(k)) => out.push(format!("-S{k}")),
        Some(GpgSign::Disabled) => out.push("--no-gpg-sign".into()),
        None => {}
    }
    match flags.verify_signatures {
        Some(true) => out.push("--verify-signatures".into()),
        Some(false) => out.push("--no-verify-signatures".into()),
        None => {}
    }
    if flags.allow_unrelated_histories { out.push("--allow-unrelated-histories".into()); }
    match flags.stat {
        Some(true) => out.push("--stat".into()),
        Some(false) => out.push("--no-stat".into()),
        None => {}
    }
    out
}
```

- [ ] **Step 3: Write unit tests for the serializer**

```rust
#[test]
fn renders_empty_flags() {
    assert!(render_flags(&EffectiveFlags::default()).is_empty());
}

#[test]
fn renders_ff_modes() {
    let mut f = EffectiveFlags::default();
    f.ff = Some(FfMode::Only);
    assert_eq!(render_flags(&f), vec!["--ff-only"]);
    f.ff = Some(FfMode::Never);
    assert_eq!(render_flags(&f), vec!["--no-ff"]);
}

#[test]
fn renders_multiple_strategy_options() {
    let mut f = EffectiveFlags::default();
    f.strategy_options = vec!["theirs".into(), "ignore-space-change".into()];
    assert_eq!(
        render_flags(&f),
        vec!["-X", "theirs", "-X", "ignore-space-change"]
    );
}

#[test]
fn renders_gpg_sign_variants() {
    let mut f = EffectiveFlags::default();
    f.gpg_sign = Some(GpgSign::Default);
    assert_eq!(render_flags(&f), vec!["-S"]);
    f.gpg_sign = Some(GpgSign::KeyId("ABCD1234".into()));
    assert_eq!(render_flags(&f), vec!["-SABCD1234"]);
}
```

- [ ] **Step 4: Build, run tests, verify**

Run: `cargo test --lib --package daft -- core::worktree::merge::` Expected: all
pass.

- [ ] **Step 5: Wire CLI → `EffectiveFlags` in `run()`**

Build `EffectiveFlags` from `args` (mapping `args.ff/args.no_ff/args.ff_only` to
`FfMode`, etc.). Pass to `StartParams` (extend the struct with a
`flags: EffectiveFlags` field). `execute_start` passes them through
`render_flags()` and appends to its `git merge` argv.

- [ ] **Step 6: YAML scenarios for common flag paths**

Create each with a minimal setup that exercises the flag:

- `tests/manual/scenarios/merge/ff.yml` — default behavior produces fast-forward
  when possible; assert `git log --oneline main | head -1` matches the source
  tip (no new merge commit created).
- `tests/manual/scenarios/merge/ff-only.yml` — non-FF case with `--ff-only` →
  exit non-zero, output contains `"not possible to fast-forward"`.
- `tests/manual/scenarios/merge/no-ff.yml` — FF-eligible case with `--no-ff` →
  produces a merge commit (`git log --oneline` shows two parents).
- `tests/manual/scenarios/merge/squash.yml` — `--squash` produces a staged but
  uncommitted state in target; assert `git status` shows staged changes and
  `MERGE_HEAD` is absent.
- `tests/manual/scenarios/merge/signoff.yml` — `--signoff` → commit message
  contains `Signed-off-by:`.
- `tests/manual/scenarios/merge/strategy-ours.yml` — `-s ours` picks target's
  version when conflicting.
- `tests/manual/scenarios/merge/strategy-option.yml` — `-X theirs` resolves
  conflicts in favor of source.
- `tests/manual/scenarios/merge/already-up-to-date.yml` — merge of an ancestor →
  exit 0, output contains `"Already up to date"`.

Each scenario follows the same template as `basic.yml` with appropriate setup
and `output_contains` assertions.

- [ ] **Step 7: Run all**

Run: `mise run test:manual -- --ci merge` Expected: all scenarios pass.

- [ ] **Step 8: Commit**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/
git commit -m "feat: full git flag parity for daft merge"
```

---

## Slice 7 — Finish commands (`--abort` / `--continue` / `--quit`)

Goal: Finish commands operate on a worktree's in-progress merge. They take an
optional positional `<worktree|branch>` argument; omitted ⇒ CWD. If the resolved
worktree has no in-progress merge, error and list candidates.

### Task 7.1: Reshape `Args` to support modes

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Add mutually-exclusive mode flags**

```rust
#[arg(long = "abort", conflicts_with_all = ["continue_merge", "quit"])]
pub abort: bool,
#[arg(long = "continue", conflicts_with_all = ["abort", "quit"])]
pub continue_merge: bool,
#[arg(long = "quit", conflicts_with_all = ["abort", "continue_merge"])]
pub quit: bool,

/// Worktree/branch to operate on for --abort/--continue/--quit.
/// When a mode flag is present, `sources` is repurposed: the first entry
/// (if any) becomes the worktree argument. Validated in `run()`.
```

(Note: we reuse `sources` as the positional slot for both start and finish
forms; validation in `run()` ensures exactly one positional arg in finish mode,
at least one in start mode.)

- [ ] **Step 2: Mode dispatch in `run()`**

```rust
pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-merge"));
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let settings = DaftSettings::load()?;
    let git = GitCommand::new(false).with_gitoxide(settings.use_gitoxide);
    let project_root = crate::get_project_root()?;

    if args.abort || args.continue_merge || args.quit {
        let worktree_arg = match args.sources.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            _ => anyhow::bail!(
                "finish commands (--abort/--continue/--quit) take at most one positional \
                 <worktree|branch>"
            ),
        };
        let mode = if args.abort { core::FinishMode::Abort }
                   else if args.continue_merge { core::FinishMode::Continue }
                   else { core::FinishMode::Quit };
        core::execute_finish(&core::FinishParams { worktree: worktree_arg, mode }, &git, &project_root)?;
        return Ok(());
    }

    // Existing start-mode dispatch (unchanged)
    ...
}
```

### Task 7.2: Implement `execute_finish` in core

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn finish_errors_when_no_merge_in_progress() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    let err = ensure_merge_in_progress(tmp.path()).unwrap_err();
    assert!(err.to_string().contains("no in-progress merge"));
}

#[test]
fn finish_ok_when_merge_head_present() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    std::fs::write(tmp.path().join(".git").join("MERGE_HEAD"), "abc").unwrap();
    assert!(ensure_merge_in_progress(tmp.path()).is_ok());
}
```

- [ ] **Step 2: Implement**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishMode { Abort, Continue, Quit }

pub struct FinishParams {
    pub worktree: Option<String>,
    pub mode: FinishMode,
}

pub fn ensure_merge_in_progress(worktree: &Path) -> Result<()> {
    if detect_in_progress(worktree)? == Some(InProgressOp::Merge) {
        Ok(())
    } else {
        anyhow::bail!(
            "no in-progress merge in worktree '{}'",
            worktree.display()
        )
    }
}

pub fn execute_finish(
    params: &FinishParams,
    git: &GitCommand,
    project_root: &Path,
) -> Result<()> {
    let worktrees = crate::core::worktree::list::collect_worktree_info(
        project_root, /* ... */
    )?;
    let cwd_branch = git.current_branch_in(&std::env::current_dir()?).ok();
    let target = resolve_target(params.worktree.as_deref(), &worktrees, cwd_branch.as_deref())?;

    if ensure_merge_in_progress(&target.path).is_err() {
        // Enumerate candidates for the helpful error
        let candidates: Vec<_> = worktrees.iter()
            .filter(|w| {
                let p = w.path.as_ref().map(|p| p.as_path()).unwrap_or(Path::new(""));
                detect_in_progress(p).ok().flatten() == Some(InProgressOp::Merge)
            })
            .collect();
        let mut msg = format!(
            "no in-progress merge in worktree '{}'",
            target.path.display()
        );
        if !candidates.is_empty() {
            msg.push_str("\n\nmerges in progress elsewhere:");
            for c in &candidates {
                if let Some(branch) = &c.branch {
                    msg.push_str(&format!("\n  {}", branch));
                }
            }
            msg.push_str("\n\nretry with: daft merge --");
            msg.push_str(match params.mode {
                FinishMode::Abort => "abort",
                FinishMode::Continue => "continue",
                FinishMode::Quit => "quit",
            });
            msg.push_str(" <branch>");
        }
        anyhow::bail!(msg);
    }

    let flag = match params.mode {
        FinishMode::Abort => "--abort",
        FinishMode::Continue => "--continue",
        FinishMode::Quit => "--quit",
    };

    let status = git.run_in(&target.path, &["merge".into(), flag.into()])?;
    if !status.success() {
        anyhow::bail!("git merge {} failed in {}", flag, target.path.display());
    }
    Ok(())
}
```

- [ ] **Step 3: Run unit tests**

- [ ] **Step 4: YAML scenarios**

Create the following, each with a setup that produces a conflict and then runs
the finish form:

- `tests/manual/scenarios/merge/abort.yml` — CWD target, `--abort`.
- `tests/manual/scenarios/merge/abort-cross-worktree.yml` — from a different
  worktree, `--abort main`.
- `tests/manual/scenarios/merge/continue.yml` — conflict, resolve files,
  `--continue`.
- `tests/manual/scenarios/merge/quit.yml` — conflict, `--quit` leaves state
  intact.
- `tests/manual/scenarios/merge/abort-no-merge-in-progress.yml` — `--abort` on a
  clean worktree lists candidates from elsewhere or errors with "no in-progress
  merge".

Template for the abort scenario (others follow the same pattern):

```yaml
name: Merge abort CWD
description: "daft merge --abort aborts an in-progress merge in CWD"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and set up a conflict
    run: |
      cd $WORK_DIR/test-repo/main
      git checkout -b conflict-src
      echo "a" > conflict.txt && git add . && git commit -m "a"
      git checkout main
      echo "b" > conflict.txt && git add . && git commit -m "b"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Trigger conflict
    run: git-worktree-merge conflict-src
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1

  - name: Abort
    run: git-worktree-merge --abort
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify no in-progress merge
    run: |
      test ! -e $WORK_DIR/test-repo/main/.git/MERGE_HEAD && echo "clean"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["clean"]
```

- [ ] **Step 5: Run and commit**

```bash
mise run test:manual -- --ci merge
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/
git commit -m "feat: finish commands --abort/--continue/--quit"
```

---

## Slice 8 — Conflict: report-and-stay

Goal: When the merge conflicts in the target worktree, daft prints the target
path and conflicted files, exits non-zero, and leaves the user where they are
(no auto-cd).

### Task 8.1: Capture conflict state

**Files:**

- Modify: `src/core/worktree/merge.rs`
- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Add conflicted-files enumeration**

```rust
pub fn conflicted_files(worktree: &Path, git: &GitCommand) -> Result<Vec<String>> {
    let porcelain = git.status_porcelain_in(worktree)?;
    Ok(porcelain
        .lines()
        .filter(|l| l.starts_with("UU ") || l.starts_with("AA ") || l.starts_with("DD "))
        .map(|l| l[3..].to_string())
        .collect())
}
```

- [ ] **Step 2: Populate `StartOutcome` with conflict details**

```rust
pub struct StartOutcome {
    pub already_up_to_date: bool,
    pub conflicted: bool,
    pub target_path: PathBuf,
    pub conflicted_files: Vec<String>,
}
```

After `git merge` returns non-zero, call `conflicted_files()` and populate the
outcome. Command layer (`run()`) formats the message:

```rust
if outcome.conflicted {
    eprintln!("merge conflicted in {}", outcome.target_path.display());
    if !outcome.conflicted_files.is_empty() {
        eprintln!("conflicted files:");
        for f in &outcome.conflicted_files {
            eprintln!("  {}", f);
        }
    }
    eprintln!(
        "\nresolve in the target worktree, then run:\n  \
         daft merge --continue{branch_arg}\n  \
         daft merge --abort{branch_arg}",
        branch_arg = "  # add <branch> if running from a different worktree"
    );
    std::process::exit(1);
}
```

- [ ] **Step 3: YAML scenario `tests/manual/scenarios/merge/conflict.yml`**

```yaml
name: Merge conflict report-and-stay
description: "daft merge reports conflict target and exits non-zero; no auto-cd"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set up conflicting branches
    run: |
      cd $WORK_DIR/test-repo/main
      git checkout -b conflict-src
      echo "a" > c.txt && git add . && git commit -m "a"
      git checkout main
      echo "b" > c.txt && git add . && git commit -m "b"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Attempt merge from a different worktree
    run: git-worktree-checkout conflict-src
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Merge cross-worktree — should conflict
    run: git-worktree-merge conflict-src --into main
    cwd: "$WORK_DIR/test-repo/conflict-src"
    expect:
      exit_code: 1
      output_contains:
        - "merge conflicted in"
        - "c.txt"
        - "daft merge --continue"
        - "daft merge --abort"
```

- [ ] **Step 4: Run and commit**

```bash
mise run test:manual -- --ci merge:conflict
git add src/core/worktree/merge.rs src/commands/merge.rs tests/manual/scenarios/merge/conflict.yml
git commit -m "feat: report-and-stay on merge conflict with target path"
```

---

## Slice 9 — Target has no worktree: pure-FF plumbing

Goal: When the target branch exists as a ref but has no worktree, and the merge
is a pure fast-forward (single source, target is ancestor of source, no
`--squash`, no `--no-ff`), advance the ref via `git update-ref`. No worktree
needed, no prompt.

### Task 9.1: Detect "target has no worktree"

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Adjust `ResolvedTarget` to carry presence info**

```rust
#[derive(Debug, Clone)]
pub struct ResolvedTarget {
    pub branch: String,
    /// Path to the worktree, if one exists. None → ref-only target.
    pub path: Option<PathBuf>,
}
```

Update callers and tests accordingly. Branch-name resolution that finds no
worktree should verify the branch exists as a ref
(`git show-ref --verify refs/heads/<branch>`); if so, return `path: None`.

### Task 9.2: FF detection and plumbing advancement

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write tests**

```rust
#[test]
fn is_pure_ff_requires_single_source() {
    let mut f = EffectiveFlags::default();
    assert!(is_pure_ff_eligible(2, &f, None /* already_ff: unknown, see impl */));
    // multi-source ⇒ not FF
}

#[test]
fn is_pure_ff_rejects_squash_and_no_ff() {
    let mut f = EffectiveFlags::default();
    f.squash = Some(true);
    assert!(!is_pure_ff_eligible(1, &f, Some(true)));
    f.squash = None;
    f.ff = Some(FfMode::Never);
    assert!(!is_pure_ff_eligible(1, &f, Some(true)));
}
```

(The exact function signature should accept whatever the design lands on; if you
prefer, make `is_pure_ff_eligible(sources_len, flags, is_ancestor)` return a
bool.)

- [ ] **Step 2: Implement**

```rust
pub fn is_pure_ff_eligible(
    sources_len: usize,
    flags: &EffectiveFlags,
    is_ancestor: Option<bool>,
) -> bool {
    if sources_len != 1 { return false; }
    if flags.squash == Some(true) { return false; }
    if flags.ff == Some(FfMode::Never) { return false; }
    is_ancestor == Some(true)
}

pub fn advance_ref_via_plumbing(
    git: &GitCommand,
    target_branch: &str,
    source_sha: &str,
) -> Result<()> {
    let target_ref = format!("refs/heads/{target_branch}");
    git.run(&[
        "update-ref".into(),
        target_ref,
        source_sha.to_string(),
    ]).context("failed to update target ref")?;
    Ok(())
}
```

- [ ] **Step 3: In `execute_start`, branch on target presence**

```rust
match target.path.as_ref() {
    Some(path) => {
        // existing worktree-delegated merge
    }
    None => {
        let source_sha = git.rev_parse(&params.sources[0])?;
        let target_sha = git.rev_parse(&target.branch)?;
        let is_ancestor = git.is_ancestor(&target_sha, &source_sha)?;
        if is_pure_ff_eligible(params.sources.len(), &params.flags, Some(is_ancestor)) {
            advance_ref_via_plumbing(git, &target.branch, &source_sha)?;
            return Ok(StartOutcome { already_up_to_date: false, conflicted: false, ..Default::default() });
        }
        // Non-FF + no worktree: defer to Slice 10 (ephemeral prompt). For now, bail.
        anyhow::bail!(
            "target branch '{}' has no worktree and merge is not pure fast-forward; \
             run `daft checkout {}` first or use --adopt-target (coming in slice 10)",
            target.branch, target.branch,
        );
    }
}
```

- [ ] **Step 4: YAML scenario
      `tests/manual/scenarios/merge/no-target-worktree-ff.yml`**

```yaml
name: Merge FF when target has no worktree
description:
  "Pure FF advances target ref via plumbing without requiring a worktree"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Create a feature branch without a worktree
    run: |
      cd $WORK_DIR/test-repo/main
      git branch feat-no-wt
      # Advance feat-no-wt without creating a worktree
      git update-ref refs/heads/feat-no-wt $(git rev-parse HEAD)
      echo "x" > x.txt && git add . && git commit -m "x on main"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: FF-merge main into feat-no-wt from main
    run: git-worktree-merge main --into feat-no-wt
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Merge complete."

  - name: Verify feat-no-wt advanced
    run: |
      test "$(git rev-parse main)" = "$(git rev-parse feat-no-wt)" && echo "same"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["same"]
```

- [ ] **Step 5: Run and commit**

```bash
mise run test:manual -- --ci merge:no-target-worktree-ff
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/no-target-worktree-ff.yml
git commit -m "feat: fast-forward advance target ref when target has no worktree"
```

---

## Slice 10 — Ephemeral worktree + prompt flow

Goal: For non-FF merges where the target has no worktree, prompt the user (TTY),
or honor `--adopt-target` / `--no-adopt-target` /
`daft.merge.adoptTargetOnDemand`. If yes, create an ephemeral worktree in daft's
temp area, perform the merge there, remove on success, promote on conflict
(Slice 11).

### Task 10.1: Add adopt flags + settings field

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Add CLI flags**

```rust
#[arg(long = "adopt-target", conflicts_with = "no_adopt_target")]
pub adopt_target: bool,

#[arg(long = "no-adopt-target", conflicts_with = "adopt_target")]
pub no_adopt_target: bool,
```

(Settings field `merge_adopt_target_on_demand` added in Slice 13.)

### Task 10.2: Prompt helper

**Files:**

- Modify: `src/commands/merge.rs` (new helper function)

- [ ] **Step 1: Write tests**

For a `decide_adopt` that takes flags + tty status + config:

```rust
#[test]
fn adopt_flag_wins() {
    assert_eq!(decide_adopt(true, false, false, Preset::Prompt), Decision::Yes);
}

#[test]
fn no_adopt_flag_wins() {
    assert_eq!(decide_adopt(false, true, true, Preset::Yes), Decision::No);
}

#[test]
fn preset_yes_no_flag_no_tty() {
    assert_eq!(decide_adopt(false, false, false, Preset::Yes), Decision::Yes);
}

#[test]
fn preset_no_without_flag_without_tty() {
    assert_eq!(decide_adopt(false, false, false, Preset::No), Decision::No);
}

#[test]
fn preset_prompt_no_tty_is_no() {
    assert_eq!(decide_adopt(false, false, false, Preset::Prompt), Decision::No);
}

#[test]
fn preset_prompt_with_tty_is_ask() {
    assert_eq!(decide_adopt(false, false, true, Preset::Prompt), Decision::Ask);
}
```

- [ ] **Step 2: Implement**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset { Prompt, Yes, No }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision { Yes, No, Ask }

pub fn decide_adopt(flag_yes: bool, flag_no: bool, is_tty: bool, preset: Preset) -> Decision {
    if flag_yes { return Decision::Yes; }
    if flag_no { return Decision::No; }
    match (preset, is_tty) {
        (Preset::Yes, _) => Decision::Yes,
        (Preset::No, _) => Decision::No,
        (Preset::Prompt, true) => Decision::Ask,
        (Preset::Prompt, false) => Decision::No,
    }
}

fn ask_user_prompt(target_branch: &str) -> Result<bool> {
    use std::io::{self, Write};
    eprint!(
        "target '{}' has no worktree and this merge cannot fast-forward.\n\
         create an ephemeral worktree to perform the merge? [y/N] ",
        target_branch
    );
    io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    Ok(matches!(buf.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}
```

### Task 10.3: Ephemeral worktree lifecycle

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Use `temp_worktree::create/remove`**

```rust
pub fn execute_ephemeral_merge(
    target_branch: &str,
    params: &StartParams,
    git: &GitCommand,
    bare_root: &Path,
) -> Result<StartOutcome> {
    let temp_path = crate::core::worktree::temp_worktree::create(bare_root, target_branch)?;

    let mut merge_args = vec!["merge".into()];
    merge_args.extend(render_flags(&params.flags));
    merge_args.extend(params.sources.iter().cloned());

    let status = git.run_in(&temp_path, &merge_args)?;

    if status.success() {
        crate::core::worktree::temp_worktree::remove(&temp_path)?;
        return Ok(StartOutcome {
            already_up_to_date: false,
            conflicted: false,
            target_path: temp_path, // removed already; not used downstream on success
            conflicted_files: vec![],
        });
    }

    // Slice 11 handles the conflict-promotion path.
    // For now, leave the temp worktree in place and report.
    let files = conflicted_files(&temp_path, git)?;
    Ok(StartOutcome {
        already_up_to_date: false,
        conflicted: true,
        target_path: temp_path,
        conflicted_files: files,
    })
}
```

### Task 10.4: Plumb into `execute_start`

**Files:**

- Modify: `src/core/worktree/merge.rs`
- Modify: `src/commands/merge.rs`

- [ ] **Step 1: In `execute_start` no-worktree branch, dispatch to adopt
      decision**

Pass `decide_adopt` result as parameter or compute it in the command layer and
set a `StartParams::ephemeral_ok: bool`. Refuse with a clear error when decision
is `No`:

```rust
anyhow::bail!(
    "target '{}' has no worktree and this merge cannot fast-forward; \
     run `daft checkout {}` first, or pass --adopt-target",
    target.branch, target.branch
);
```

When `Yes`: call `execute_ephemeral_merge`.

### Task 10.5: YAML scenarios

**Files:**

- `tests/manual/scenarios/merge/no-target-worktree-prompt-accept.yml`
- `tests/manual/scenarios/merge/no-target-worktree-prompt-decline.yml`
- `tests/manual/scenarios/merge/no-target-worktree-no-tty.yml`
- `tests/manual/scenarios/merge/no-target-worktree-flag.yml`

Accept scenario (pipe "y" into stdin):

```yaml
- name: Non-FF merge with prompt accept
  run: echo "y" | git-worktree-merge feat-x --into target-no-wt
  cwd: "$WORK_DIR/test-repo/main"
  expect:
    exit_code: 0
    output_contains:
      - "create an ephemeral worktree"
      - "Merge complete."
```

Decline scenario pipes "n" and expects exit 1. Flag scenario uses
`--adopt-target` and expects no prompt text. No-TTY scenario uses
`run: sh -c 'git-worktree-merge ... </dev/null'` to force non-TTY; expects
refusal.

- [ ] **Step 1: Write and run all four**

Run:
`mise run test:manual -- --ci merge:no-target-worktree-prompt-accept merge:no-target-worktree-prompt-decline merge:no-target-worktree-no-tty merge:no-target-worktree-flag`

- [ ] **Step 2: Commit**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/
git commit -m "feat: ephemeral worktree with prompt for non-FF no-worktree targets"
```

---

## Slice 11 — Promote-on-conflict

Goal: When an ephemeral worktree merge conflicts, move the worktree to its
layout-resolved sibling path, update daft's internal worktree bookkeeping, fire
`worktree-post-create` hook, and report the layout-resolved path (not the temp
path) in the conflict message.

### Task 11.1: Layout resolver + move-worktree primitive

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Locate the layout resolver**

Run:
`grep -rn "resolve_worktree_path\|layout::resolve" src/core/layout/ 2>/dev/null | head`

Use the existing resolver — do not duplicate it. Likely named along the lines of
`crate::core::layout::resolve_path_for_branch(project_root, branch)`.

- [ ] **Step 2: Move logic**

```rust
pub fn promote_ephemeral_to_layout(
    temp_path: &Path,
    target_branch: &str,
    project_root: &Path,
    git: &GitCommand,
) -> Result<PathBuf> {
    let layout_path = crate::core::layout::resolve_path_for_branch(
        project_root, target_branch
    )?;

    if layout_path.exists() {
        anyhow::bail!(
            "cannot promote ephemeral worktree: destination '{}' already exists",
            layout_path.display()
        );
    }

    // Rename the worktree (filesystem move)
    std::fs::create_dir_all(layout_path.parent().ok_or_else(|| {
        anyhow::anyhow!("invalid layout path: no parent")
    })?)?;
    std::fs::rename(temp_path, &layout_path)
        .context("failed to move ephemeral worktree to layout path")?;

    // Update git's worktree metadata so git knows the new location.
    git.run(&["worktree".into(), "repair".into(), layout_path.display().to_string()])?;

    Ok(layout_path)
}
```

(`git worktree repair` rewrites gitdir pointers to match the current path.
Verify this is sufficient for daft's layout model; if not, use
`git worktree remove --force <temp>` + `git worktree add <layout> <branch>` as
an alternative — but that would lose the conflicted index state. Prefer
`repair`.)

- [ ] **Step 3: Fire `worktree-post-create` hook**

Inspect `src/hooks/` for the hook-execution helper. It likely lives in
`src/hooks/executor.rs` or similar. Typical call:

```rust
crate::hooks::executor::run_hook(
    project_root,
    "worktree-post-create",
    /* env vars: DAFT_WORKTREE_PATH, DAFT_BRANCH, etc. */,
)?;
```

- [ ] **Step 4: Call promotion from `execute_ephemeral_merge` on conflict**

```rust
if !status.success() {
    let layout_path = promote_ephemeral_to_layout(
        &temp_path, target_branch, project_root, git
    )?;
    let files = conflicted_files(&layout_path, git)?;
    return Ok(StartOutcome {
        already_up_to_date: false,
        conflicted: true,
        target_path: layout_path,
        conflicted_files: files,
    });
}
```

### Task 11.2: YAML scenario

**Files:**

- Create: `tests/manual/scenarios/merge/ephemeral-conflict-promote.yml`

```yaml
name: Ephemeral merge conflict promotes to layout path
description:
  "When an ephemeral merge conflicts, worktree is moved to layout path"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set up conflicting content where target has no worktree
    run: |
      cd $WORK_DIR/test-repo/main
      git checkout -b base-branch
      echo "a" > c.txt && git add . && git commit -m "a"
      git checkout -b other-branch
      echo "b" > c.txt && git add . && git commit -m "b"
      git checkout main
      # Remove base-branch worktree so it's ref-only
      # (fixture may or may not have one; adjust as needed)
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Merge other-branch into base-branch with --adopt-target
    run: git-worktree-merge other-branch --into base-branch --adopt-target
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "merge conflicted"

  - name: Verify promoted worktree exists at layout path
    run: |
      test -d $WORK_DIR/test-repo/base-branch && echo "promoted"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["promoted"]
```

- [ ] **Step 1: Run and commit**

```bash
mise run test:manual -- --ci merge:ephemeral-conflict-promote
git add src/core/worktree/merge.rs tests/manual/scenarios/merge/ephemeral-conflict-promote.yml
git commit -m "feat: promote ephemeral worktree to layout path on conflict"
```

---

## Slice 12 — Post-merge cleanup (`-r` / `-rb`)

Goal: `-r` removes the source worktree after a successful merge. `-b` (requires
`-r`) additionally deletes the source branch via `git branch -d`.

### Task 12.1: Add flags and pair-validation

**Files:**

- Modify: `src/commands/merge.rs`

- [ ] **Step 1: Add flags**

```rust
#[arg(short = 'r', long = "remove", help = "Remove the source worktree after successful merge")]
pub remove: bool,

#[arg(
    short = 'b',
    long = "and-branch",
    help = "Also delete the source branch (requires --remove)",
    requires = "remove"
)]
pub and_branch: bool,
```

(`requires = "remove"` enforces the `-b` without `-r` ⇒ error at parse time.)

### Task 12.2: Cleanup orchestration

**Files:**

- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Write tests for classification**

```rust
#[test]
fn classifies_branch_with_worktree() {
    let worktrees = vec![wt("feat-x", Some("feature/x"), "/repo/feat-x")];
    let c = classify_source("feat-x", &worktrees, |_| true);
    assert_eq!(c, SourceClass::BranchWithWorktree {
        worktree_path: "/repo/feat-x".into(),
        branch: "feature/x".into(),
    });
}

#[test]
fn classifies_branch_without_worktree() {
    let worktrees = vec![];
    let c = classify_source("feature/x", &worktrees, |name| name == "feature/x");
    assert_eq!(c, SourceClass::BranchNoWorktree { branch: "feature/x".into() });
}

#[test]
fn classifies_commit_sha() {
    let worktrees = vec![];
    let c = classify_source("abc123", &worktrees, |_| false);
    assert_eq!(c, SourceClass::CommitOrDetached);
}
```

- [ ] **Step 2: Implement**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceClass {
    BranchWithWorktree { worktree_path: PathBuf, branch: String },
    BranchNoWorktree { branch: String },
    CommitOrDetached,
}

pub fn classify_source(
    source: &str,
    worktrees: &[WorktreeInfo],
    branch_exists: impl Fn(&str) -> bool,
) -> SourceClass {
    if let Some(w) = worktrees.iter().find(|w| w.name == source) {
        return SourceClass::BranchWithWorktree {
            worktree_path: w.path.clone().unwrap_or_default(),
            branch: w.branch.clone().unwrap_or_else(|| source.to_string()),
        };
    }
    if let Some(w) = worktrees.iter().find(|w| w.branch.as_deref() == Some(source)) {
        return SourceClass::BranchWithWorktree {
            worktree_path: w.path.clone().unwrap_or_default(),
            branch: source.to_string(),
        };
    }
    if branch_exists(source) {
        return SourceClass::BranchNoWorktree { branch: source.to_string() };
    }
    SourceClass::CommitOrDetached
}

pub struct CleanupOptions {
    pub remove_worktree: bool,
    pub also_branch: bool,
}

pub fn execute_cleanup(
    sources: &[String],
    worktrees: &[WorktreeInfo],
    options: &CleanupOptions,
    git: &GitCommand,
    project_root: &Path,
) -> Result<()> {
    for src in sources {
        let class = classify_source(src, worktrees, |b| {
            git.rev_parse(&format!("refs/heads/{b}")).is_ok()
        });
        match class {
            SourceClass::BranchWithWorktree { worktree_path, branch } => {
                if options.remove_worktree {
                    // Use existing daft worktree-remove code path; fires hooks.
                    crate::core::worktree::branch_delete::remove_worktree(
                        project_root, &worktree_path, /* fire hooks */ true, git
                    )?;
                }
                if options.also_branch {
                    git.run(&["branch".into(), "-d".into(), branch.clone()])
                        .with_context(|| format!("failed to delete branch '{branch}'"))?;
                }
            }
            SourceClass::BranchNoWorktree { branch } => {
                if options.also_branch {
                    git.run(&["branch".into(), "-d".into(), branch.clone()])
                        .with_context(|| format!("failed to delete branch '{branch}'"))?;
                }
            }
            SourceClass::CommitOrDetached => { /* nothing to clean up */ }
        }
    }
    Ok(())
}
```

(Adapt `branch_delete::remove_worktree` to whatever existing helper is actually
used by `daft remove` — inspect `src/commands/worktree_branch.rs::run_remove`
and find the underlying helper.)

- [ ] **Step 3: Call from command layer after a successful merge**

```rust
if !outcome.conflicted && !outcome.already_up_to_date && (args.remove || args.and_branch) {
    core::execute_cleanup(
        &args.sources,
        &worktrees,
        &core::CleanupOptions {
            remove_worktree: args.remove,
            also_branch: args.and_branch,
        },
        &git,
        &project_root,
    )?;
}
```

### Task 12.3: YAML scenarios

**Files:**

- `tests/manual/scenarios/merge/remove-source.yml`
- `tests/manual/scenarios/merge/remove-source-and-branch.yml`
- `tests/manual/scenarios/merge/remove-unmerged-branch.yml`

Template for `remove-source.yml`:

```yaml
name: Merge with -r removes source worktree
description: "daft merge -r removes source worktree after success"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and checkout
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Create feature worktree
    run: git-worktree-checkout feature/test-feature
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Merge with -r
    run: git-worktree-merge feature/test-feature -r
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify feature worktree removed
    run: |
      test ! -d $WORK_DIR/test-repo/feature/test-feature && echo "removed"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains: ["removed"]
```

For `remove-unmerged-branch.yml`, set up a squash merge then `-rb`; git refuses
branch-d; daft surfaces the error with exit non-zero.

- [ ] **Step 1: Run all three**

Run:
`mise run test:manual -- --ci merge:remove-source merge:remove-source-and-branch merge:remove-unmerged-branch`

- [ ] **Step 2: Commit**

```bash
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/remove-source.yml tests/manual/scenarios/merge/remove-source-and-branch.yml tests/manual/scenarios/merge/remove-unmerged-branch.yml
git commit -m "feat: post-merge cleanup with -r and -rb"
```

---

## Slice 13 — Layered config via `daft.merge.*`

Goal: Read merge defaults from git config (global + local); CLI flags override.
Verbose mode reports which layer supplied each non-default value.

### Task 13.1: Extend `DaftSettings` with merge fields

**Files:**

- Modify: `src/core/settings.rs`

- [ ] **Step 1: Add fields to the `DaftSettings` struct**

After existing fields:

```rust
// Merge defaults
pub merge_ff: FfMode,
pub merge_squash: bool,
pub merge_commit: bool,
pub merge_edit: Option<bool>, // None = TTY default
pub merge_signoff: bool,
pub merge_gpg_sign: Option<String>, // None = unset, Some("") = default key
pub merge_verify_signatures: bool,
pub merge_allow_unrelated_histories: bool,
pub merge_strategy: Option<String>,
pub merge_strategy_options: Vec<String>,
pub merge_adopt_target_on_demand: AdoptPreset,
pub merge_require_clean_target: bool,
pub merge_post_merge_remove_source_worktree: bool,
pub merge_post_merge_also_remove_source_branch: bool,
```

Auxiliary enums — **do not duplicate**. `FfMode` already lives in
`src/core/worktree/merge.rs` (from Slice 6); re-use it in settings via:

```rust
use crate::core::worktree::merge::FfMode;
```

Add `AdoptPreset` for the first time here (it's settings-only):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptPreset { Prompt, Yes, No }
```

- [ ] **Step 2: Defaults**

In the `defaults` module, add:

```rust
pub const MERGE_FF: FfMode = FfMode::Auto;
pub const MERGE_SQUASH: bool = false;
pub const MERGE_COMMIT: bool = true;
pub const MERGE_SIGNOFF: bool = false;
pub const MERGE_VERIFY_SIGNATURES: bool = false;
pub const MERGE_ALLOW_UNRELATED_HISTORIES: bool = false;
pub const MERGE_ADOPT_TARGET_ON_DEMAND: AdoptPreset = AdoptPreset::Prompt;
pub const MERGE_REQUIRE_CLEAN_TARGET: bool = true;
pub const MERGE_POST_MERGE_REMOVE_SOURCE_WORKTREE: bool = false;
pub const MERGE_POST_MERGE_ALSO_REMOVE_SOURCE_BRANCH: bool = false;
```

- [ ] **Step 3: Config keys**

Add to the keys module (likely `src/core/settings/keys.rs` or similar):

```rust
pub const MERGE_FF: &str = "daft.merge.ff";
pub const MERGE_SQUASH: &str = "daft.merge.squash";
pub const MERGE_COMMIT: &str = "daft.merge.commit";
pub const MERGE_EDIT: &str = "daft.merge.edit";
pub const MERGE_SIGNOFF: &str = "daft.merge.signoff";
pub const MERGE_GPG_SIGN: &str = "daft.merge.gpgSign";
pub const MERGE_VERIFY_SIGNATURES: &str = "daft.merge.verifySignatures";
pub const MERGE_ALLOW_UNRELATED_HISTORIES: &str = "daft.merge.allowUnrelatedHistories";
pub const MERGE_STRATEGY: &str = "daft.merge.strategy";
pub const MERGE_STRATEGY_OPTION: &str = "daft.merge.strategyOption";
pub const MERGE_ADOPT_TARGET_ON_DEMAND: &str = "daft.merge.adoptTargetOnDemand";
pub const MERGE_REQUIRE_CLEAN_TARGET: &str = "daft.merge.requireCleanTarget";
pub const MERGE_POST_MERGE_REMOVE_SOURCE_WORKTREE: &str = "daft.merge.postMerge.removeSourceWorktree";
pub const MERGE_POST_MERGE_ALSO_REMOVE_SOURCE_BRANCH: &str = "daft.merge.postMerge.alsoRemoveSourceBranch";
```

- [ ] **Step 4: Extend `DaftSettings::load()`**

Add per-key parsing following the existing pattern in the file (likely
`git.config_get(keys::X)?` then `parse_*`):

```rust
if let Some(v) = git.config_get(keys::MERGE_FF)? {
    settings.merge_ff = match v.as_str() {
        "auto" => FfMode::Auto,
        "only" => FfMode::Only,
        "never" => FfMode::Never,
        _ => defaults::MERGE_FF,
    };
}
if let Some(v) = git.config_get(keys::MERGE_SQUASH)? {
    settings.merge_squash = parse_bool(&v, defaults::MERGE_SQUASH);
}
// ... and so on for each key
```

- [ ] **Step 5: Unit tests**

In `src/core/settings.rs` `#[cfg(test)] mod tests`, add:

```rust
#[test]
fn defaults_for_merge() {
    let s = DaftSettings::default();
    assert_eq!(s.merge_ff, FfMode::Auto);
    assert!(!s.merge_squash);
    assert!(s.merge_require_clean_target);
}
```

- [ ] **Step 6: Build + run tests + commit**

```bash
cargo test --lib --package daft -- core::settings::
git add src/core/settings.rs
git commit -m "feat: add daft.merge.* settings fields and config keys"
```

### Task 13.2: Plumb settings into merge command

**Files:**

- Modify: `src/commands/merge.rs`
- Modify: `src/core/worktree/merge.rs`

- [ ] **Step 1: Merge settings into `EffectiveFlags`**

Build `EffectiveFlags` by starting with settings defaults and overriding with
CLI flags:

```rust
fn compute_effective_flags(args: &Args, settings: &DaftSettings) -> EffectiveFlags {
    let mut f = EffectiveFlags::default();

    // FF mode — CLI wins
    f.ff = if args.ff_only { Some(FfMode::Only) }
        else if args.no_ff { Some(FfMode::Never) }
        else if args.ff { Some(FfMode::Auto) }
        else { Some(settings.merge_ff) };

    // Squash
    f.squash = if args.squash { Some(true) }
        else if args.no_squash { Some(false) }
        else if settings.merge_squash { Some(true) }
        else { None };

    // ... all other flags similarly ...

    f
}
```

- [ ] **Step 2: Verbose deviation reporting**

Track which layer supplied each non-default value. Simple approach: alongside
`compute_effective_flags`, return a `Vec<(String, String, Layer)>` of overrides,
then if `args.verbose`, print:

```
merge: squash=true (from global config)
merge: ff=never (from local config)
```

- [ ] **Step 3: YAML scenarios**

- `tests/manual/scenarios/merge/config-layered-defaults.yml` — set global
  `daft.merge.squash=true` and local `daft.merge.ff=never`; invoke
  `daft merge ...`; assert the effective behavior matches (squash merge +
  `--no-ff` forced). Verify CLI `--no-squash` overrides.
- `tests/manual/scenarios/merge/config-verbose-reports-source.yml` — same setup,
  run with `-v`; output contains `(from global config)` and
  `(from local config)` lines.

- [ ] **Step 4: Run and commit**

```bash
mise run test:manual -- --ci merge:config-layered-defaults merge:config-verbose-reports-source
git add src/commands/merge.rs src/core/worktree/merge.rs tests/manual/scenarios/merge/
git commit -m "feat: layered config for merge defaults with verbose layer reporting"
```

---

## Slice 14 — `daft list --merging` companion

Goal: `daft list --merging` filters to worktrees with `MERGE_HEAD` present, and
shows the branches being merged in plus time since.

### Task 14.1: Add `--merging` flag

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Add flag to `Args`**

In list's `Args` (likely around line 92 per earlier grep), add:

```rust
#[arg(long = "merging", help = "Only show worktrees with an in-progress merge")]
pub merging: bool,
```

- [ ] **Step 2: Thread into the filter pipeline**

Inspect the list command's filtering logic (around where stats/columns are
composed) and add a filter that retains only entries where
`detect_in_progress(&info.path) == Some(InProgressOp::Merge)`.

- [ ] **Step 3: Add virtual columns**

Extend the column registry (look for `COLUMNS` / `ColumnDef` in
`src/core/worktree/list.rs`) with two merging-specific columns:

- `merging` — joined source branches (from `MERGE_MSG` parse or `MERGE_HEAD` +
  git cat-file --pretty).
- `since` — relative time from `MERGE_HEAD` mtime to now.

When `--merging` is passed, these columns appear in the default column set.

- [ ] **Step 4: Unit tests**

```rust
#[test]
fn merging_filter_retains_only_in_progress() {
    // construct two WorktreeInfo with paths pointing to tempdirs —
    // one with MERGE_HEAD written, one without — and assert the
    // filter output.
}
```

- [ ] **Step 5: YAML scenario
      `tests/manual/scenarios/merge/status-list-merging.yml`**

```yaml
name: daft list --merging shows in-progress merges
description: "list --merging filters to worktrees with MERGE_HEAD present"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Set up conflict and leave it in place
    run: |
      cd $WORK_DIR/test-repo/main
      git checkout -b conflict-src
      echo "a" > c.txt && git add . && git commit -m "a"
      git checkout main
      echo "b" > c.txt && git add . && git commit -m "b"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Trigger conflict in main
    run: git-worktree-merge conflict-src
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1

  - name: List --merging shows main
    run: NO_COLOR=1 git-worktree-list --merging
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "main"
        - "conflict-src"
```

- [ ] **Step 6: Run and commit**

```bash
mise run test:manual -- --ci merge:status-list-merging
git add src/commands/list.rs src/core/worktree/list.rs tests/manual/scenarios/merge/status-list-merging.yml
git commit -m "feat: daft list --merging filter for in-progress merges"
```

---

## Slice 15 — Shell completions

Goal: `daft merge` completes subcommand correctly, with flags and source/target
branches suggested across all four shells. `daft list --merging` also completes.

### Task 15.1: Bash, zsh, fish, fig

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `src/commands/completions/fig.rs`

- [ ] **Step 1: Update the bash completion string**

In `src/commands/completions/bash.rs`, find `DAFT_BASH_COMPLETIONS` and:

1. Add `"merge"` and `"worktree-merge"` to the top-level subcommand list.
2. Add a completion case for `merge` with the full flag set and branch
   completion for the `<source>` positional and `--into` value.
3. Add `--merging` to the `list` flag completion.

- [ ] **Step 2: Repeat for zsh, fish, fig**

Each shell module has an analogous hardcoded completion string. Mirror the same
additions.

- [ ] **Step 3: Generate the completion outputs and inspect**

Run:
`./target/debug/daft completions bash > /tmp/bash.comp && grep -n "merge" /tmp/bash.comp | head`
Expected: merge entries appear.

- [ ] **Step 4: YAML scenario
      `tests/manual/scenarios/completions/merge-completions.yml`**

A smoke test that invokes `daft completions <shell>` and asserts the output
contains `merge` and `--merging`:

```yaml
name: Merge subcommand completions
description: "shell completions include merge subcommand and --merging flag"

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Bash completions include merge
    run: daft completions bash | grep -E "merge|--merging"
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0
      output_contains: ["merge", "--merging"]
  # Repeat for zsh, fish
```

- [ ] **Step 5: Run and commit**

```bash
mise run test:manual -- --ci completions:merge-completions
git add src/commands/completions/ tests/manual/scenarios/completions/merge-completions.yml
git commit -m "feat: shell completions for daft merge and list --merging"
```

---

## Slice 16 — Docs, man pages, SKILL

Goal: Regenerate man pages; write `docs/cli/daft-merge.md`; update `SKILL.md`
with the new command so AI agents learn it.

### Task 16.1: Regenerate man pages

**Files:**

- Modify: `man/` (auto-generated)

- [ ] **Step 1: Regenerate**

Run: `mise run man:gen` Expected: new files `man/git-worktree-merge.1` and
`man/daft-merge.1` appear; existing files are updated if `--help` text changed.

- [ ] **Step 2: Verify**

Run: `mise run man:verify` Expected: pass.

- [ ] **Step 3: Commit**

```bash
git add man/
git commit -m "docs: regenerate man pages for daft merge"
```

### Task 16.2: Reference page

**Files:**

- Create: `docs/cli/daft-merge.md`

- [ ] **Step 1: Copy the template and fill in**

Use `docs/cli/daft-doctor.md` as the structural template. Required sections:

- Frontmatter with `title` and `description`.
- Synopsis with full CLI surface (start form, finish forms, flags).
- Configuration keys table (mirror spec).
- Examples: CWD merge, cross-worktree, octopus, squash, abort, continue,
  cleanup, ephemeral.
- Error modes.
- See also (list, carry, sync, hooks).

No emoji (per CLAUDE.md rule).

- [ ] **Step 2: Commit**

```bash
git add docs/cli/daft-merge.md
git commit -m "docs: add daft-merge reference page"
```

### Task 16.3: Update SKILL.md

**Files:**

- Modify: `SKILL.md`

- [ ] **Step 1: Add `daft merge` section**

Following the existing SKILL.md structure, add a section teaching AI agents:

- What the command does.
- When to use it (cross-worktree merges, octopus, cleanup).
- Key flags with short examples.
- Common pitfalls (conflict ⇒ report-and-stay; target must be clean).

- [ ] **Step 2: Commit**

```bash
git add SKILL.md
git commit -m "docs: teach SKILL.md about daft merge"
```

### Task 16.4: Update help output categories

**Files:**

- Modify: `src/commands/docs.rs`

- [ ] **Step 1: Find `get_command_categories()`**

Grep: `grep -n "get_command_categories" src/commands/docs.rs`

- [ ] **Step 2: Add `merge` to an appropriate category** (likely the "Worktree
      commands" category with `checkout`, `sync`, `carry`, etc.).

- [ ] **Step 3: Run `daft --help` and verify merge appears**

Run: `./target/debug/daft --help 2>&1 | grep -A1 merge`

- [ ] **Step 4: Commit**

```bash
git add src/commands/docs.rs
git commit -m "docs: list daft merge in help output"
```

---

## Slice 17 — Final verification

Goal: All tests pass, lints clean, format checked, full scenario matrix passes,
CI equivalent succeeds locally.

### Task 17.1: Full local CI

- [ ] **Step 1: Run full test and lint suite**

Run each in turn:

```bash
mise run fmt:check
mise run clippy
mise run test:unit
mise run test:integration
mise run test:manual -- --ci merge
```

Expected: every command exits 0.

- [ ] **Step 2: Simulate full CI**

Run: `mise run ci` Expected: exit 0.

- [ ] **Step 3: If anything fails**, fix the underlying issue (not the test),
      re-run, and commit a targeted fix. Never skip lints or tests to force
      completion.

- [ ] **Step 4: Final squash-merge readiness check**

Run: `git log --oneline master..HEAD` Expected: a clean series of
conventional-commit messages, each tied to one logical unit of work.

### Task 17.2: PR readiness

- [ ] **Step 1: Verify per CLAUDE.md requirements**

- All lint checks green.
- Every bug-fix or behavior change has a regression test (YAML scenario or unit
  test).
- Man pages regenerated and committed.
- Completions cover the new surface across all four shells.
- SKILL.md updated.
- Shortcut added across all five completions files.

- [ ] **Step 2: PR title + body**

PR title: `feat: daft merge command with cross-worktree support` PR body
references the issue (`Fixes #330`), labels: `feat`, milestone: `Public Launch`,
assignee: `avihut`.

---

## Notes for the executor

- **Do not skip the refactoring between slices.** When Slice 3 changes
  `ResolvedTarget` to hold `Option<PathBuf>`, every consumer from earlier slices
  needs to update. Run tests after each slice finishes.
- **Don't guess at daft APIs.** When a task says "adapt to the real signature",
  inspect the relevant file with `grep -n "pub fn"` or read the module directly.
  The plan calls out every file to check.
- **Fixture branch names may differ.** The `standard-remote` fixture is
  referenced throughout; if a scenario's branch name doesn't resolve, the first
  fix is to inspect the fixture definition and adapt the scenario's branch names
  — not to change the plan's intent.
- **Prefer small commits.** Each task ends with a commit. If a slice gets long,
  commit intermediate "refactor" / "test" steps and keep the feat commit for the
  behavior change itself.
- **When a test is flaky**, investigate rather than retrying. The
  worktree-per-branch fixtures occasionally have timing issues that reveal real
  bugs.

## Deferred work (not in this plan)

From the spec's "Deferred / future work" section — these are explicitly out of
scope:

- `merge-pre` / `merge-post` daft-level hooks.
- `--finish` composite flag.
- Auto-fetch / auto-push.
- `DAFT_MERGE_TARGET` session hint env var.
- Branch protection list (`daft.merge.protectedBranches`).
- Force-delete variants (`-D`, `--force`).
- Squash-reachability detection.
- `merge-conflict` hook.
- Dedicated `daft merge list` / `daft merge status` command.
