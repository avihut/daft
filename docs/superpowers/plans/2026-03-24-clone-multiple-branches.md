# Clone Multiple Branches Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `-b` repeatable in the clone command, unify `-b`/`--all-branches`
under a `BranchSource` abstraction, extract a shared `OperationTable` TUI
component, and add a TUI table for multi-branch clone progress.

**Architecture:** Introduce `BranchSource` enum that unifies single-branch,
multi-branch, and all-branches modes. Extract the TUI table infrastructure from
sync/prune into a shared `OperationTable` component. Refactor the clone
command's Phase 4 (worktree setup) to use the shared TUI when multiple branches
are involved, with per-worktree hook presentation.

**Tech Stack:** Rust, clap (CLI), ratatui (TUI), gix (gitoxide), mpsc channels
(event-driven TUI)

**Spec:** `docs/superpowers/specs/2026-03-24-clone-multiple-branches-design.md`

---

## File Structure

### New files

| File                                                       | Responsibility                                                           |
| ---------------------------------------------------------- | ------------------------------------------------------------------------ |
| `src/core/worktree/branch_source.rs`                       | `BranchSource` enum, `BranchPlan` struct, resolution logic               |
| `src/output/tui/operation_table.rs`                        | `OperationTable`, `TableConfig`, `CompletedTable` — shared TUI component |
| `tests/manual/scenarios/clone/multi-branch-contained.yml`  | YAML test: multi `-b` with contained layout                              |
| `tests/manual/scenarios/clone/multi-branch-sibling.yml`    | YAML test: multi `-b` with sibling layout (default branch injection)     |
| `tests/manual/scenarios/clone/multi-branch-head-token.yml` | YAML test: `HEAD`/`@` token resolution                                   |
| `tests/manual/scenarios/clone/multi-branch-missing.yml`    | YAML test: warning for nonexistent branches                              |
| `tests/manual/scenarios/clone/multi-branch-hooks.yml`      | YAML test: per-worktree hooks fire during multi-branch clone             |

### Modified files

| File                               | Change                                                                                                            |
| ---------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `src/commands/clone.rs`            | Change `-b` from `Option<String>` to `Vec<String>`, construct `BranchSource`, wire TUI for multi-branch           |
| `src/core/worktree/clone.rs`       | Accept `BranchPlan`, add satellite worktree creation loop, fire per-worktree hooks                                |
| `src/core/worktree/sync_dag.rs`    | Add `Setup(String)` to `TaskId`, `Setup` to `OperationPhase`, `Created`/`BaseCreated`/`NotFound` to `TaskMessage` |
| `src/core/worktree/mod.rs`         | Add `pub mod branch_source;`                                                                                      |
| `src/output/tui/mod.rs`            | Add `pub mod operation_table;`, export `OperationTable`                                                           |
| `src/output/tui/state.rs`          | Add `Setup => "setting up"` match arm, add `FinalStatus::Created`                                                 |
| `src/output/tui/driver.rs`         | Extract render loop into `OperationTable::run()`                                                                  |
| `src/commands/sync.rs`             | Refactor to use `OperationTable` instead of inline `TuiRenderer` wiring                                           |
| `src/commands/prune.rs`            | Refactor to use `OperationTable` instead of inline `TuiRenderer` wiring                                           |
| `src/commands/completions/bash.rs` | Add `HEAD @` to `-b` value completions                                                                            |
| `src/commands/completions/zsh.rs`  | Add `HEAD @` to `-b` value completions                                                                            |
| `src/commands/completions/fish.rs` | Add `HEAD @` to `-b` value completions                                                                            |
| `src/git/oxide.rs`                 | Add `validate_branch_in_remotes()` and `list_remote_branches_local()`                                             |
| `src/git/remote.rs`                | Add `validate_branches_exist()` with gitoxide fast path                                                           |

---

## Task 1: BranchSource and BranchPlan types

**Files:**

- Create: `src/core/worktree/branch_source.rs`
- Modify: `src/core/worktree/mod.rs`

- [ ] **Step 1: Write unit tests for BranchSource resolution**

In `src/core/worktree/branch_source.rs`, add the module with types and tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_non_bare() {
        let plan = BranchSource::Default
            .resolve("main", false, &["main", "develop"]);
        assert_eq!(plan.base, Some("main".into()));
        assert!(plan.satellites.is_empty());
        assert_eq!(plan.cd_target, Some("main".into()));
        assert!(plan.not_found.is_empty());
    }

    #[test]
    fn single_source_non_bare() {
        let plan = BranchSource::Single("develop".into())
            .resolve("main", false, &["main", "develop"]);
        assert_eq!(plan.base, Some("develop".into()));
        assert!(plan.satellites.is_empty());
        assert_eq!(plan.cd_target, Some("develop".into()));
    }

    #[test]
    fn multiple_source_non_bare_injects_default() {
        let plan = BranchSource::Multiple(vec!["feat-a".into(), "feat-b".into()])
            .resolve("main", false, &["main", "feat-a", "feat-b"]);
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.satellites, vec!["feat-a", "feat-b"]);
        assert_eq!(plan.cd_target, Some("feat-a".into()));
    }

    #[test]
    fn multiple_source_non_bare_default_already_listed() {
        let plan = BranchSource::Multiple(vec!["main".into(), "feat-a".into()])
            .resolve("main", false, &["main", "feat-a"]);
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.satellites, vec!["feat-a"]);
        assert_eq!(plan.cd_target, Some("main".into()));
    }

    #[test]
    fn multiple_source_bare_no_injection() {
        let plan = BranchSource::Multiple(vec!["feat-a".into(), "feat-b".into()])
            .resolve("main", true, &["main", "feat-a", "feat-b"]);
        assert_eq!(plan.base, None);
        assert_eq!(plan.satellites, vec!["feat-a", "feat-b"]);
        assert_eq!(plan.cd_target, Some("feat-a".into()));
    }

    #[test]
    fn head_and_at_tokens_resolved() {
        let expanded = expand_default_tokens(
            &["HEAD".into(), "feat-a".into(), "@".into()],
            "main",
        );
        // HEAD and @ both resolve to main, deduplicated, order preserved
        assert_eq!(expanded, vec!["main", "feat-a"]);
    }

    #[test]
    fn missing_branches_collected() {
        let plan = BranchSource::Multiple(vec!["feat-a".into(), "typo".into()])
            .resolve("main", true, &["main", "feat-a"]);
        assert_eq!(plan.satellites, vec!["feat-a"]);
        assert_eq!(plan.not_found, vec!["typo"]);
    }

    #[test]
    fn cd_target_skips_missing_branches() {
        let plan = BranchSource::Multiple(vec!["typo".into(), "feat-a".into()])
            .resolve("main", true, &["main", "feat-a"]);
        assert_eq!(plan.cd_target, Some("feat-a".into()));
    }

    #[test]
    fn all_source_non_bare() {
        let plan = BranchSource::All
            .resolve("main", false, &["main", "develop", "feat-a"]);
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.satellites, vec!["develop", "feat-a"]);
        assert_eq!(plan.cd_target, Some("main".into()));
    }

    #[test]
    fn all_source_bare() {
        let plan = BranchSource::All
            .resolve("main", true, &["main", "develop", "feat-a"]);
        assert_eq!(plan.base, None);
        assert_eq!(plan.satellites, vec!["main", "develop", "feat-a"]);
    }

    #[test]
    fn all_branches_missing_non_bare_cd_target() {
        let plan = BranchSource::Multiple(vec!["typo-a".into(), "typo-b".into()])
            .resolve("main", false, &["main"]);
        // Default injected as base, but no valid cd targets from original list
        assert_eq!(plan.base, Some("main".into()));
        assert_eq!(plan.cd_target, Some("main".into())); // falls back to base
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib branch_source` Expected: compilation error — module and
types don't exist yet

- [ ] **Step 3: Implement BranchSource, BranchPlan, and resolution logic**

In `src/core/worktree/branch_source.rs`:

```rust
/// Unified branch selection for the clone command.
///
/// Replaces the separate `-b` / `--all-branches` handling with a single
/// abstraction that resolves to a concrete `BranchPlan`.
#[derive(Debug, Clone)]
pub enum BranchSource {
    /// No -b, no --all-branches: just the default branch.
    Default,
    /// Single -b <branch>: one explicit branch (today's behavior).
    Single(String),
    /// Multiple -b flags: explicit list of branches.
    Multiple(Vec<String>),
    /// --all-branches: discover all remote branches.
    All,
}

/// Resolved plan for which worktrees to create.
#[derive(Debug, Clone)]
pub struct BranchPlan {
    /// Branch for the base worktree (non-bare layouts only).
    pub base: Option<String>,
    /// Branches for satellite worktrees.
    pub satellites: Vec<String>,
    /// Which worktree to cd into after clone.
    pub cd_target: Option<String>,
    /// Branches that weren't found on remote.
    pub not_found: Vec<String>,
}
```

Implement
`expand_default_tokens(branches: &[String], default_branch: &str) -> Vec<String>`
that replaces `HEAD`/`@` with the actual default branch name, then deduplicates
while preserving first-occurrence order.

Implement
`BranchSource::resolve(&self, default_branch: &str, is_bare: bool, remote_branches: &[&str]) -> BranchPlan`
following the resolution rules from the spec:

- `Default`: base = default (non-bare) or satellite (bare), cd = default
- `Single(b)`: base = b (non-bare) or satellite (bare), cd = b, not_found if
  missing
- `Multiple(list)`: expand tokens, deduplicate, validate against
  remote_branches, inject default to base for non-bare, cd = first valid from
  original order (fall back to base if none valid)
- `All`: base = default (non-bare), satellites = all others, cd = base

Implement
`BranchSource::from_args(branches: &[String], all_branches: bool) -> Self`
constructor that maps from clap args.

- [ ] **Step 4: Register module**

In `src/core/worktree/mod.rs`, add:

```rust
pub mod branch_source;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib branch_source` Expected: all tests pass

- [ ] **Step 6: Run clippy and fmt**

Run: `mise run fmt && mise run clippy` Expected: no warnings

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/branch_source.rs src/core/worktree/mod.rs
git commit -m "feat: add BranchSource and BranchPlan types with resolution logic"
```

---

## Task 2: Extend DAG event types

**Files:**

- Modify: `src/core/worktree/sync_dag.rs:23-112` — `TaskId`, `OperationPhase`,
  `TaskMessage`
- Modify: `src/output/tui/state.rs:36-50,199-205` — `FinalStatus`, `apply_event`
  match

- [ ] **Step 1: Add new variants to TaskId**

In `src/core/worktree/sync_dag.rs`, add to the `TaskId` enum (after `Push`):

```rust
/// Set up a worktree during clone.
Setup(String),
```

- [ ] **Step 2: Add Setup to OperationPhase**

In the `OperationPhase` enum, add:

```rust
Setup,
```

Update the `label()` method to include:

```rust
OperationPhase::Setup => "Setting up worktrees".to_string(),
```

- [ ] **Step 3: Add new TaskMessage variants**

In the `TaskMessage` enum, add:

```rust
Created,
BaseCreated,
NotFound,
```

- [ ] **Step 4: Update FinalStatus**

In `src/output/tui/state.rs`, add to `FinalStatus`:

```rust
Created,
```

- [ ] **Step 5: Update apply_event active_label match**

In `src/output/tui/state.rs`, in `apply_event()`, add to the `active_label`
match:

```rust
OperationPhase::Setup => "setting up",
```

- [ ] **Step 6: Update TaskMessage → FinalStatus mapping**

In `apply_event()`, in the `TaskCompleted` handler where `TaskMessage` is
matched to `FinalStatus`, add:

```rust
TaskMessage::Created => FinalStatus::Created,
TaskMessage::BaseCreated => FinalStatus::Created,
TaskMessage::NotFound => FinalStatus::Skipped,
```

- [ ] **Step 7: Update any exhaustive match sites in sync.rs and prune.rs**

Search for matches on `TaskId` in `src/commands/sync.rs` (lines 705-810) and
`src/commands/prune.rs` (lines 393-420). Add a `TaskId::Setup(_)` arm that
matches the pattern used by other per-branch tasks. For now, these can be
unreachable:

```rust
TaskId::Setup(_) => unreachable!("Setup is only used by clone"),
```

- [ ] **Step 8: Verify compilation and run all existing tests**

Run: `mise run fmt && mise run clippy && mise run test:unit` Expected: all pass
— no behavioral changes, only new variants

- [ ] **Step 9: Commit**

```bash
git add src/core/worktree/sync_dag.rs src/output/tui/state.rs \
        src/commands/sync.rs src/commands/prune.rs
git commit -m "feat: add Setup/Created/NotFound variants to DAG event types"
```

---

## Task 3: Gitoxide local ref validation

**Files:**

- Modify: `src/git/oxide.rs` — add `validate_branch_in_remotes()`,
  `list_remote_branches_local()`
- Modify: `src/git/remote.rs` — add `validate_branches_exist()` with gitoxide
  fast path

- [ ] **Step 1: Write unit tests for local ref validation**

In `src/git/oxide.rs`, add tests to the existing `#[cfg(test)] mod tests`:

```rust
#[test]
#[serial]
fn test_validate_branch_in_remotes() {
    let (dir, _repo) = create_test_repo();
    let path = dir.path().canonicalize().unwrap();

    // Add a remote and create remote-tracking refs
    git_cmd()
        .args(["remote", "add", "origin", "https://example.com/repo.git"])
        .current_dir(&path)
        .output()
        .unwrap();
    git_cmd()
        .args(["update-ref", "refs/remotes/origin/main", "refs/heads/main"])
        .current_dir(&path)
        .output()
        .unwrap();
    git_cmd()
        .args(["update-ref", "refs/remotes/origin/develop", "refs/heads/main"])
        .current_dir(&path)
        .output()
        .unwrap();

    let saved_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&path).unwrap();
    let repo = gix::open(&path).unwrap();
    if let Some(cwd) = saved_cwd {
        let _ = std::env::set_current_dir(cwd);
    }

    assert!(validate_branch_in_remotes(&repo, "origin", "main").unwrap());
    assert!(validate_branch_in_remotes(&repo, "origin", "develop").unwrap());
    assert!(!validate_branch_in_remotes(&repo, "origin", "nonexistent").unwrap());
}

#[test]
#[serial]
fn test_list_remote_branches_local() {
    let (dir, _repo) = create_test_repo();
    let path = dir.path().canonicalize().unwrap();

    git_cmd()
        .args(["remote", "add", "origin", "https://example.com/repo.git"])
        .current_dir(&path)
        .output()
        .unwrap();
    git_cmd()
        .args(["update-ref", "refs/remotes/origin/main", "refs/heads/main"])
        .current_dir(&path)
        .output()
        .unwrap();
    git_cmd()
        .args(["update-ref", "refs/remotes/origin/develop", "refs/heads/main"])
        .current_dir(&path)
        .output()
        .unwrap();

    let saved_cwd = std::env::current_dir().ok();
    std::env::set_current_dir(&path).unwrap();
    let repo = gix::open(&path).unwrap();
    if let Some(cwd) = saved_cwd {
        let _ = std::env::set_current_dir(cwd);
    }

    let branches = list_remote_branches_local(&repo, "origin").unwrap();
    assert!(branches.contains(&"main".to_string()));
    assert!(branches.contains(&"develop".to_string()));
    assert_eq!(branches.len(), 2);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib oxide::tests::test_validate_branch_in_remotes` Expected:
compilation error — functions don't exist yet

- [ ] **Step 3: Implement gitoxide local ref functions**

In `src/git/oxide.rs`, add:

```rust
/// Check if a branch exists in the already-fetched remote refs (no network).
///
/// After `git clone --bare`, remote refs are available locally under
/// `refs/remotes/<remote>/`. This avoids a network round-trip compared to
/// `ls_remote_branch_exists`.
pub fn validate_branch_in_remotes(
    repo: &Repository,
    remote_name: &str,
    branch: &str,
) -> Result<bool> {
    let ref_name = format!("refs/remotes/{remote_name}/{branch}");
    Ok(repo.try_find_reference(&ref_name)?.is_some())
}

/// List all branches available in the local remote-tracking refs (no network).
///
/// Returns branch names without the `refs/remotes/<remote>/` prefix.
pub fn list_remote_branches_local(
    repo: &Repository,
    remote_name: &str,
) -> Result<Vec<String>> {
    let prefix = format!("refs/remotes/{remote_name}/");
    let platform = repo.references()?;
    let references = platform.prefixed(&prefix)?;
    let mut branches = Vec::new();

    for reference_result in references {
        let reference = reference_result
            .map_err(|e| anyhow::anyhow!("Failed to read reference: {e}"))?;
        let full_name = reference.name().as_bstr().to_string();
        if let Some(branch_name) = full_name.strip_prefix(&prefix) {
            // Skip HEAD ref
            if branch_name != "HEAD" {
                branches.push(branch_name.to_string());
            }
        }
    }

    Ok(branches)
}
```

- [ ] **Step 4: Add GitCommand wrapper with fallback**

In `src/git/remote.rs`, add:

```rust
/// Validate which branches from a list exist on the remote.
///
/// Returns a vec of (branch_name, exists) pairs.
/// Uses local refs when gitoxide is enabled (zero network), falls back
/// to git CLI ls-remote otherwise.
pub fn validate_branches_exist(
    &self,
    remote_name: &str,
    branches: &[String],
) -> Result<Vec<(String, bool)>> {
    if self.use_gitoxide {
        if let Ok(repo) = self.gix_repo() {
            return branches
                .iter()
                .map(|b| {
                    oxide::validate_branch_in_remotes(&repo, remote_name, b)
                        .map(|exists| (b.clone(), exists))
                })
                .collect();
        }
    }
    // CLI fallback: one ls-remote per branch
    branches
        .iter()
        .map(|b| {
            self.ls_remote_branch_exists(remote_name, b)
                .map(|exists| (b.clone(), exists))
        })
        .collect()
}

/// List all branches on a remote using local refs.
///
/// Uses local refs when gitoxide is enabled (zero network), falls back
/// to git CLI ls-remote otherwise.
pub fn list_remote_branches(&self, remote_name: &str) -> Result<Vec<String>> {
    if self.use_gitoxide {
        if let Ok(repo) = self.gix_repo() {
            return oxide::list_remote_branches_local(&repo, remote_name);
        }
    }
    // CLI fallback: parse ls-remote --heads output
    let output = self.ls_remote_heads(remote_name, None)?;
    Ok(output
        .lines()
        .filter_map(|line| {
            line.split('\t')
                .nth(1)
                .and_then(|r| r.strip_prefix("refs/heads/"))
                .map(|s| s.to_string())
        })
        .collect())
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run:
`cargo test --lib oxide::tests::test_validate_branch_in_remotes && cargo test --lib oxide::tests::test_list_remote_branches_local`
Expected: all pass

- [ ] **Step 6: Run clippy and fmt**

Run: `mise run fmt && mise run clippy` Expected: no warnings

- [ ] **Step 7: Commit**

```bash
git add src/git/oxide.rs src/git/remote.rs
git commit -m "feat: add gitoxide local ref validation for multi-branch clone"
```

---

## Task 4: Extract OperationTable shared TUI component

**Files:**

- Create: `src/output/tui/operation_table.rs`
- Modify: `src/output/tui/mod.rs` — add module and exports
- Modify: `src/output/tui/driver.rs` — extract render loop

This task extracts the TUI infrastructure into a reusable component without
changing any command behavior.

- [ ] **Step 1: Create OperationTable with TableConfig and CompletedTable**

In `src/output/tui/operation_table.rs`:

```rust
use super::state::{PhaseState, TuiState, WorktreeRow, HookSummaryEntry};
use super::columns::{Column, SortSpec};
use crate::core::worktree::sync_dag::DagEvent;
use std::path::PathBuf;
use std::sync::mpsc;

/// Configuration for a TUI table operation.
pub struct TableConfig {
    pub columns: Option<Vec<Column>>,
    pub columns_explicit: bool,
    pub sort_spec: Option<SortSpec>,
    pub extra_rows: u16,
    pub show_hook_sub_rows: bool,
}

/// Result returned after the TUI completes.
pub struct CompletedTable {
    pub rows: Vec<WorktreeRow>,
    pub hook_summaries: Vec<HookSummaryEntry>,
}

/// Reusable TUI table for any command that processes worktrees in parallel.
///
/// Used by sync, prune, and clone commands. Each command provides its own
/// phases, initial rows, and worker threads that send DagEvent messages.
pub struct OperationTable {
    phases: Vec<PhaseState>,
    initial_rows: Vec<WorktreeRow>,
    receiver: mpsc::Receiver<DagEvent>,
    config: TableConfig,
    project_root: PathBuf,
    cwd: PathBuf,
    unowned_start_index: Option<usize>,
}
```

Implement `OperationTable::new(...)` constructor and
`OperationTable::run(self) -> Result<CompletedTable>` that:

1. Constructs `TuiState` from the provided phases, rows, and config
2. Constructs `TuiRenderer` with the state and receiver
3. Calls `TuiRenderer::run()`
4. Converts the returned `TuiState` into `CompletedTable`

- [ ] **Step 2: Register module and exports**

In `src/output/tui/mod.rs`, add:

```rust
pub mod operation_table;
pub use operation_table::{CompletedTable, OperationTable, TableConfig};
```

- [ ] **Step 3: Verify compilation**

Run: `cargo build` Expected: compiles — `OperationTable` wraps existing
`TuiRenderer` without changing it

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/operation_table.rs src/output/tui/mod.rs
git commit -m "feat: add OperationTable shared TUI component"
```

---

## Task 5: Refactor sync to use OperationTable

**Files:**

- Modify: `src/commands/sync.rs` — replace inline `TuiRenderer` wiring with
  `OperationTable`

- [ ] **Step 1: Identify the TUI wiring in sync**

In `src/commands/sync.rs`, the TUI path (after the TTY check at line 215)
currently:

1. Collects worktree info and creates `WorktreeRow`s
2. Defines `PhaseState`s
3. Creates `mpsc::channel()`
4. Spawns worker thread
5. Creates `TuiState` manually
6. Creates `TuiRenderer` with state + receiver
7. Calls `renderer.run()`
8. Uses returned `TuiState` for post-TUI summary

Replace steps 5-7 with `OperationTable::new(...).run()`.

- [ ] **Step 2: Replace TuiRenderer wiring with OperationTable**

The sync command should construct an `OperationTable` with its phases, rows,
receiver, and config, then call `.run()`. The returned `CompletedTable` replaces
the `TuiState` used for post-TUI processing.

Update the post-TUI code to read from `CompletedTable.rows` and
`CompletedTable.hook_summaries` instead of `TuiState` fields.

- [ ] **Step 3: Run all sync tests**

Run: `mise run test:manual -- --ci sync` Expected: all sync scenarios pass —
behavior unchanged

- [ ] **Step 4: Run full test suite**

Run: `mise run test:unit && mise run test:manual -- --ci` Expected: all pass

- [ ] **Step 5: Commit**

```bash
git add src/commands/sync.rs
git commit -m "refactor: sync command uses shared OperationTable"
```

---

## Task 6: Refactor prune to use OperationTable

**Files:**

- Modify: `src/commands/prune.rs` — replace inline `TuiRenderer` wiring with
  `OperationTable`

- [ ] **Step 1: Replace TuiRenderer wiring with OperationTable**

Same pattern as sync. Prune's TUI path creates phases `[Fetch, Prune]`, seeds
rows for gone branches, spawns a worker thread, and creates `TuiRenderer`.
Replace with `OperationTable::new(...).run()`.

- [ ] **Step 2: Run all prune tests**

Run: `mise run test:manual -- --ci prune` Expected: all prune scenarios pass —
behavior unchanged

- [ ] **Step 3: Run full test suite**

Run: `mise run test:unit && mise run test:manual -- --ci` Expected: all pass

- [ ] **Step 4: Commit**

```bash
git add src/commands/prune.rs
git commit -m "refactor: prune command uses shared OperationTable"
```

---

## Task 7: Change -b to Vec<String> and construct BranchSource

**Files:**

- Modify: `src/commands/clone.rs:43-154` — Args struct and validation

- [ ] **Step 1: Change the clap field type**

In `src/commands/clone.rs`, change the `branch` field in `Args`:

From:

```rust
#[arg(
    short = 'b',
    long = "branch",
    value_name = "BRANCH",
    help = "Branch to check out instead of the remote's default branch"
)]
pub branch: Option<String>,
```

To:

```rust
#[arg(
    short = 'b',
    long = "branch",
    value_name = "BRANCH",
    action = clap::ArgAction::Append,
    help = "Branch to check out (repeatable; use HEAD or @ for default branch)"
)]
pub branch: Vec<String>,
```

- [ ] **Step 2: Update validation**

In `validate_arg_combinations()`, update the conflicts:

- `--all-branches && !branch.is_empty()` (line 144) — unchanged logic, new
  syntax
- `--no-checkout && !branch.is_empty()` (line 147) — unchanged logic
- Add: `--remote` conflicts with multiple `-b` (but not single):

  ```rust
  if args.remote.is_some() && args.branch.len() > 1 {
      anyhow::bail!("--remote cannot be used with multiple -b flags");
  }
  ```

- [ ] **Step 3: Construct BranchSource from args**

In `run_clone()`, after validation, construct `BranchSource`:

```rust
let branch_source = BranchSource::from_args(&args.branch, args.all_branches);
```

For now, pass only the first branch (or None) to `BareCloneParams.branch` to
keep Phase 1 unchanged:

```rust
let bare_branch = match &branch_source {
    BranchSource::Single(b) => Some(b.clone()),
    BranchSource::Multiple(list) => {
        // For Phase 1, we don't need a specific branch — bare clone
        // fetches all refs. We'll resolve in Phase 2.
        None
    }
    _ => None,
};
```

- [ ] **Step 4: Verify compilation and existing single-branch tests**

Run: `mise run fmt && mise run clippy && mise run test:manual -- --ci clone`
Expected: all existing clone tests pass

- [ ] **Step 5: Commit**

```bash
git add src/commands/clone.rs
git commit -m "feat: make -b flag repeatable with Vec<String> and BranchSource"
```

---

## Task 8: Integrate BranchPlan into clone core logic

**Files:**

- Modify: `src/core/worktree/clone.rs` — add branch resolution phase, satellite
  worktree creation
- Modify: `src/commands/clone.rs` — wire Phase 2 branch resolution

- [ ] **Step 1: Add branch resolution after bare clone**

In `src/commands/clone.rs`, after `clone_bare_phase()` returns and layout is
resolved, add Phase 2 — branch resolution:

```rust
// Phase 2: resolve BranchSource to BranchPlan
let remote_branches = git.list_remote_branches(&remote_name)?;
let remote_branch_refs: Vec<&str> = remote_branches.iter().map(|s| s.as_str()).collect();
let is_bare = layout.is_bare();
let branch_plan = branch_source.resolve(
    &bare_result.default_branch,
    is_bare,
    &remote_branch_refs,
);

// Warn about missing branches
for branch in &branch_plan.not_found {
    output.warn(&format!("Branch '{}' not found on remote", branch));
}
```

- [ ] **Step 2: Add satellite worktree creation function**

In `src/core/worktree/clone.rs`, add a function for creating satellite
worktrees:

```rust
/// Create satellite worktrees for additional branches after the base is set up.
pub fn create_satellite_worktrees(
    repo_path: &Path,
    branches: &[String],
    layout: &Layout,
    progress: &mut dyn ProgressSink,
) -> Result<Vec<(String, PathBuf)>> {
    let git = GitCommand::new();
    let mut created = Vec::new();

    for branch in branches {
        let worktree_path = layout.resolve_path(branch)?;
        progress.on_step(&format!("Creating worktree for '{}'", branch));
        git.worktree_add(&worktree_path, branch)?;
        git.set_upstream_from("origin", branch, &worktree_path)?;
        created.push((branch.clone(), worktree_path));
    }

    Ok(created)
}
```

- [ ] **Step 3: Wire BranchPlan into the existing Phase 4 dispatch**

In `run_clone()`, modify the Phase 4 dispatch to use `branch_plan`:

For single-branch mode (`BranchSource::Default` or `Single`): call existing
functions unchanged.

For multi-branch mode (`Multiple` or `All`): set up base worktree first, then
call `create_satellite_worktrees()` for satellites. (TUI integration comes in
Task 9.)

- [ ] **Step 4: Update cd target**

Replace the current cd target logic with `branch_plan.cd_target`. The cd file
path should resolve to the worktree path of the cd_target branch.

- [ ] **Step 5: Verify compilation**

Run: `mise run fmt && mise run clippy` Expected: compiles clean

- [ ] **Step 6: Commit**

```bash
git add src/core/worktree/clone.rs src/commands/clone.rs
git commit -m "feat: integrate BranchPlan into clone core logic"
```

---

## Task 9: Wire TUI table for multi-branch clone

**Files:**

- Modify: `src/commands/clone.rs` — TUI path for multi-branch clone
- Modify: `src/core/worktree/clone.rs` — event-driven satellite creation

- [ ] **Step 1: Add TUI dispatch for multi-branch**

In `src/commands/clone.rs`, after Phase 3 (layout resolution), check if
multi-branch TUI is needed:

```rust
let use_tui = matches!(branch_source, BranchSource::Multiple(_) | BranchSource::All)
    && std::io::IsTerminal::is_terminal(&std::io::stderr())
    && args.verbose < 2;
```

If `use_tui` is true, use the `OperationTable` path. Otherwise, use the existing
sequential path.

- [ ] **Step 2: Build initial rows and phases for clone TUI**

```rust
let phases = vec![
    PhaseState::completed("Cloning repository (bare)"),
    PhaseState::pending("Setting up worktrees"),
];

let initial_rows: Vec<WorktreeRow> = branch_plan
    .satellites
    .iter()
    .map(|branch| WorktreeRow::pending(branch, &layout.resolve_path(branch)?))
    .collect();
```

If there's a base worktree, add it as the first row (already completed or
in-progress).

- [ ] **Step 3: Spawn worker thread with DagEvent emission**

```rust
let (tx, rx) = mpsc::channel();

thread::spawn(move || {
    // Base worktree first (sequential)
    if let Some(base_branch) = &branch_plan.base {
        tx.send(DagEvent::TaskStarted {
            phase: OperationPhase::Setup,
            branch_name: base_branch.clone(),
        }).ok();
        // ... setup base worktree ...
        tx.send(DagEvent::TaskCompleted {
            phase: OperationPhase::Setup,
            branch_name: base_branch.clone(),
            status: TaskStatus::Succeeded,
            message: TaskMessage::BaseCreated,
            updated_info: None,
        }).ok();
    }

    // Satellites (can be parallelized later, sequential for now)
    for branch in &branch_plan.satellites {
        tx.send(DagEvent::TaskStarted { ... }).ok();
        // ... run worktree-pre-create hook via TuiBridge ...
        // ... git worktree add ...
        // ... run worktree-post-create hook via TuiBridge ...
        tx.send(DagEvent::TaskCompleted { ... }).ok();
    }

    tx.send(DagEvent::AllDone).ok();
});
```

- [ ] **Step 4: Create OperationTable and run**

```rust
let table = OperationTable::new(
    phases,
    initial_rows,
    rx,
    TableConfig {
        columns: /* from args */,
        columns_explicit: /* from args */,
        sort_spec: None,
        extra_rows: 0,
        show_hook_sub_rows: args.verbose >= 1,
    },
    project_root,
    cwd,
    None, // no unowned_start_index for clone
);

let completed = table.run()?;
```

- [ ] **Step 5: Add per-worktree hook execution via TuiBridge**

In the worker thread, for each satellite worktree, use `TuiBridge` to run
`worktree-pre-create` and `worktree-post-create` hooks, exactly as prune uses it
for `worktree-pre-remove` / `worktree-post-remove`:

```rust
let mut bridge = TuiBridge::new(executor.clone(), tx.clone(), branch.clone());
let pre_outcome = bridge.run_hook(&HookContext {
    hook_type: HookType::WorktreePreCreate,
    // ... context fields ...
})?;

if pre_outcome == HookOutcome::Abort {
    // Skip this worktree, send TaskCompleted with Failed
    continue;
}

// git worktree add ...

bridge.run_hook(&HookContext {
    hook_type: HookType::WorktreePostCreate,
    // ... context fields ...
})?;
```

- [ ] **Step 6: Post-TUI handling**

After `table.run()` returns:

1. Run `post-clone` hook (once, not per-worktree)
2. Print partial failure summary if any worktrees failed
3. Write cd target to `DAFT_CD_FILE`

- [ ] **Step 7: Verify compilation**

Run: `mise run fmt && mise run clippy` Expected: compiles clean

- [ ] **Step 8: Commit**

```bash
git add src/commands/clone.rs src/core/worktree/clone.rs
git commit -m "feat: wire TUI table for multi-branch clone with per-worktree hooks"
```

---

## Task 10: Shell completions and man page

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

- [ ] **Step 1: Add HEAD and @ to bash completions**

In `src/commands/completions/bash.rs`, find the `-b` value completion for the
clone command and add `HEAD @` as static completions.

- [ ] **Step 2: Add HEAD and @ to zsh completions**

In `src/commands/completions/zsh.rs`, same change.

- [ ] **Step 3: Add HEAD and @ to fish completions**

In `src/commands/completions/fish.rs`, same change.

- [ ] **Step 4: Regenerate man page**

Run: `mise run man:gen`

- [ ] **Step 5: Verify completions**

Run: `mise run test:manual -- --ci completions` (if completions tests exist) or
manually verify with `mise run dev && git-worktree-clone --help`.

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/ man/
git commit -m "feat: add HEAD/@ completions for -b flag, regenerate man pages"
```

---

## Task 11: YAML test scenarios for multi-branch clone

**Files:**

- Create: `tests/manual/scenarios/clone/multi-branch-contained.yml`
- Create: `tests/manual/scenarios/clone/multi-branch-sibling.yml`
- Create: `tests/manual/scenarios/clone/multi-branch-head-token.yml`
- Create: `tests/manual/scenarios/clone/multi-branch-missing.yml`
- Create: `tests/manual/scenarios/clone/multi-branch-hooks.yml`

- [ ] **Step 1: Write multi-branch contained layout test**

```yaml
name: Clone with multiple -b flags (contained layout)
description:
  Multiple -b flags create one worktree per branch in a bare repo layout.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with two branches
    run:
      git-worktree-clone --layout contained -b develop -b feature/test-feature
      $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo"
        - "$WORK_DIR/test-repo/.git"
        - "$WORK_DIR/test-repo/develop"
        - "$WORK_DIR/test-repo/feature/test-feature"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/develop"
          branch: develop
        - dir: "$WORK_DIR/test-repo/feature/test-feature"
          branch: feature/test-feature
```

- [ ] **Step 2: Write multi-branch sibling layout test (default injection)**

```yaml
name: Clone with multiple -b flags (sibling layout)
description:
  With sibling layout and multiple -b, default branch is implicitly checked out
  in the base worktree.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with two branches (sibling layout)
    run:
      git-worktree-clone --layout sibling -b develop -b feature/test-feature
      $REMOTE_TEST_REPO
    expect:
      exit_code: 0
      # Base worktree has default branch (main)
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo"
          branch: main
      # Satellites created
      dirs_exist:
        - "$WORK_DIR/test-repo.develop"
        - "$WORK_DIR/test-repo.feature-test-feature"
```

- [ ] **Step 3: Write HEAD/@ token test**

```yaml
name: Clone with HEAD and @ tokens
description: HEAD and @ resolve to the remote default branch in -b values.

repos:
  - name: test-repo-head
    use_fixture: standard-remote

steps:
  - name: Clone with @ token
    run:
      git-worktree-clone --layout contained -b @ -b develop
      $REMOTE_TEST_REPO_HEAD
    expect:
      exit_code: 0
      dirs_exist:
        - "$WORK_DIR/test-repo-head/main"
        - "$WORK_DIR/test-repo-head/develop"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo-head/main"
          branch: main
        - dir: "$WORK_DIR/test-repo-head/develop"
          branch: develop
```

- [ ] **Step 4: Write missing branch warning test**

```yaml
name: Clone with nonexistent branch
description:
  Nonexistent branches produce a warning but clone succeeds for valid ones.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone with one valid and one invalid branch
    run:
      git-worktree-clone --layout contained -b develop -b nonexistent-branch
      $REMOTE_TEST_REPO 2>&1
    expect:
      exit_code: 0
      output_contains:
        - "not found on remote"
      dirs_exist:
        - "$WORK_DIR/test-repo/develop"
      is_git_worktree:
        - dir: "$WORK_DIR/test-repo/develop"
          branch: develop
```

- [ ] **Step 5: Write per-worktree hooks test**

```yaml
name: Multi-branch clone fires per-worktree hooks
description:
  worktree-pre-create and worktree-post-create fire once per satellite worktree.

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
      - name: develop
        from: main
        files:
          - path: dev.txt
            content: "dev"
        commits:
          - message: "Add dev"
      - name: feature
        from: main
        files:
          - path: feat.txt
            content: "feat"
        commits:
          - message: "Add feat"
    daft_yml: |
      hooks:
        worktree-post-create:
          jobs:
            - name: create-marker
              run: touch "$DAFT_WORKTREE_PATH/.create-hook-ran"

steps:
  - name: Clone with multiple branches and trust hooks
    run:
      git-worktree-clone --layout contained --trust-hooks -b develop -b feature
      $REMOTE_TEST_REPO 2>&1
    expect:
      exit_code: 0

  - name: Verify hook ran for develop
    run: "true"
    expect:
      files_exist:
        - "$WORK_DIR/test-repo/develop/.create-hook-ran"

  - name: Verify hook ran for feature
    run: "true"
    expect:
      files_exist:
        - "$WORK_DIR/test-repo/feature/.create-hook-ran"
```

- [ ] **Step 6: Run all new scenarios**

Run: `mise run test:manual -- --ci clone:multi-branch` Expected: all pass

- [ ] **Step 7: Run full test suite**

Run: `mise run test:unit && mise run test:manual -- --ci` Expected: all pass
(new + existing)

- [ ] **Step 8: Commit**

```bash
git add tests/manual/scenarios/clone/multi-branch-*.yml
git commit -m "test: add YAML scenarios for multi-branch clone"
```

---

## Task 12: Final integration pass

- [ ] **Step 1: Run full CI locally**

Run: `mise run ci` Expected: all checks pass (fmt, clippy, unit tests,
integration tests, man page verification)

- [ ] **Step 2: Run manual smoke test**

Create a temp dir and test the real flow against a GitHub repo:

```bash
cd /tmp
GIT_AUTHOR_NAME="Test" GIT_AUTHOR_EMAIL="test@test.com" \
git-worktree-clone --layout contained \
    -b main -b develop \
    https://github.com/<some-public-repo>
```

Verify: two worktrees created, TUI table shown, cd into first branch.

- [ ] **Step 3: Test backward compatibility**

Verify single `-b` still works identically:

```bash
git-worktree-clone --layout contained -b main https://github.com/<some-public-repo>
```

Verify: spinner (not TUI table), single worktree, same output as before.

- [ ] **Step 4: Verify --all-branches still works**

```bash
git-worktree-clone --layout contained --all-branches https://github.com/<some-public-repo>
```

- [ ] **Step 5: Commit any final fixes**

```bash
git add -A
git commit -m "fix: integration fixes for multi-branch clone"
```
