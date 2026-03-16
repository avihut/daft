# Branch Ownership & Local Branch Sync Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development
> (if subagents available) or superpowers:executing-plans to implement this
> plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add branch ownership detection, show local-only branches in prune/sync
tables, gate rebase/push by ownership, and support temp worktrees for local
branch rebase.

**Architecture:** Ownership is determined by comparing the branch tip author
email (`git log -1 --format=%ae`) against `git config user.email`, matching
GitHub's heuristic. The sync DAG receives separate owned/unowned worktree lists;
only owned branches get Rebase/Push task nodes. Local-only branches are seeded
into the TUI table before construction and use temp worktrees (in `.daft-tmp/`)
for rebase operations.

**Tech Stack:** Rust, clap, ratatui, git CLI, YAML test framework

**Spec:**
`docs/superpowers/specs/2026-03-15-branch-ownership-and-local-branch-sync-design.md`

---

## File Structure

### New files

| File                                                    | Responsibility                            |
| ------------------------------------------------------- | ----------------------------------------- |
| `src/core/worktree/temp_worktree.rs`                    | Temp worktree create/remove/cleanup guard |
| `tests/manual/scenarios/sync/ownership-rebase-push.yml` | Ownership gates rebase/push               |
| `tests/manual/scenarios/sync/include-unowned.yml`       | `--include unowned` merges sections       |
| `tests/manual/scenarios/sync/include-email.yml`         | `--include alice@example.com`             |
| `tests/manual/scenarios/sync/include-branch.yml`        | `--include feat/x`                        |
| `tests/manual/scenarios/prune/local-branch-visible.yml` | Local-only branches in prune              |
| `tests/manual/scenarios/list/owner-column.yml`          | Owner column in list output               |
| `tests/manual/scenarios/sync/temp-worktree-rebase.yml`  | Rebase via temp worktree                  |

### Modified files

| File                            | What changes                                                              |
| ------------------------------- | ------------------------------------------------------------------------- |
| `src/core/worktree/list.rs`     | Add `owner_email` field to `WorktreeInfo`, helper to fetch author email   |
| `src/core/worktree/mod.rs`      | Add `pub mod temp_worktree;`                                              |
| `src/core/columns.rs`           | Add `Owner` variant to `ListColumn`, update positions                     |
| `src/output/tui/columns.rs`     | Add `Owner` variant to `Column`, update priorities                        |
| `src/output/format.rs`          | Add `owner` to `ColumnValues`, compute it                                 |
| `src/commands/list.rs`          | Add owner to `TableRow`, table rendering, JSON output, sorting            |
| `src/commands/sync.rs`          | Ownership split, `--include` flag, two-section TUI, local branch handling |
| `src/commands/prune.rs`         | Seed local-only gone branches into TUI table                              |
| `src/commands/sync_shared.rs`   | Pass ownership context to prune task execution                            |
| `src/core/worktree/sync_dag.rs` | Accept owned/unowned lists in `build_sync`                                |
| `src/output/tui/state.rs`       | Section tracking, ordering logic                                          |
| `src/output/tui/render.rs`      | Section divider rendering, owner cell rendering                           |
| `src/settings.rs`               | No changes (uses `git config user.email` directly)                        |

---

## Chunk 1: Owner Data & Column Infrastructure

### Task 1: Add `owner_email` field to `WorktreeInfo`

**Files:**

- Modify: `src/core/worktree/list.rs`

- [ ] **Step 1: Add helper function `get_author_email_for_ref`**

In `src/core/worktree/list.rs`, add after the existing
`get_last_commit_info_for_ref` function (around line 317):

```rust
/// Get the author email of the tip commit on a given branch ref.
fn get_author_email_for_ref(branch_ref: &str, cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ae", branch_ref])
        .current_dir(cwd)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let email = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if email.is_empty() {
        None
    } else {
        Some(email)
    }
}
```

- [ ] **Step 2: Add `owner_email` field to `WorktreeInfo` struct**

In the `WorktreeInfo` struct (line ~92, before closing brace), add:

```rust
    /// Author email of the branch tip commit (for ownership detection).
    pub owner_email: Option<String>,
```

- [ ] **Step 3: Initialize `owner_email: None` in `WorktreeInfo::empty()`**

In the `empty()` method, add `owner_email: None,` to the struct literal.

- [ ] **Step 4: Populate `owner_email` in `collect_worktree_info()`**

After the `get_last_commit_info_for_ref` call (around line 577), add:

```rust
let owner_email = if !entry.is_detached {
    get_author_email_for_ref(&branch_display, &entry.path)
} else {
    None
};
```

Add `owner_email,` to the `WorktreeInfo` construction (line ~633).

- [ ] **Step 5: Populate `owner_email` in `collect_branch_info()` for local
      branches**

In the local branch loop (around line 706), after
`get_last_commit_info_for_ref`, add:

```rust
let owner_email = get_author_email_for_ref(branch, cwd);
```

Add `owner_email,` to the `WorktreeInfo` construction (line ~732).

- [ ] **Step 6: Populate `owner_email` in `collect_branch_info()` for remote
      branches**

In the remote branch loop (around line 791), after
`get_last_commit_info_for_ref`, add:

```rust
let owner_email = get_author_email_for_ref(remote_branch, cwd);
```

Add `owner_email,` to the `WorktreeInfo` construction (line ~805).

- [ ] **Step 7: Run `mise run clippy` and `mise run test:unit` to verify
      compilation**

Run: `mise run clippy && mise run test:unit` Expected: PASS (no warnings, all
tests pass)

- [ ] **Step 8: Commit**

```bash
git add src/core/worktree/list.rs
git commit -m "feat(list): add owner_email field to WorktreeInfo"
```

---

### Task 2: Add `Owner` to user-facing column system

**Files:**

- Modify: `src/core/columns.rs`

- [ ] **Step 1: Add `Owner` variant to `ListColumn` enum**

Between `Age` and `LastCommit`:

```rust
    /// Branch tip author email (for ownership detection).
    Owner,
```

- [ ] **Step 2: Update `ListColumn::all()`**

Insert `ListColumn::Owner` between `Age` and `LastCommit`:

```rust
&[
    ListColumn::Annotation,
    ListColumn::Branch,
    ListColumn::Path,
    ListColumn::Base,
    ListColumn::Changes,
    ListColumn::Remote,
    ListColumn::Age,
    ListColumn::Owner,
    ListColumn::LastCommit,
]
```

- [ ] **Step 3: Update `canonical_position()`**

```rust
Self::Owner => 8,
Self::LastCommit => 9,  // was 8
```

- [ ] **Step 4: Update `cli_name()`**

Add: `Self::Owner => "owner",`

- [ ] **Step 5: Update `FromStr` impl**

Add to the match: `"owner" => Ok(Self::Owner),`

- [ ] **Step 6: Run `mise run test:unit` — fix any failing column tests**

Run: `mise run test:unit` Expected: Tests may fail due to new column count.
Update assertions:

- `test_list_column_canonical_position` should pass (auto-checks ordering)
- Tests comparing against `ListColumn::list_defaults()` will include Owner

- [ ] **Step 7: Commit**

```bash
git add src/core/columns.rs
git commit -m "feat(columns): add Owner column to user-facing column system"
```

---

### Task 3: Add `Owner` to TUI column system

**Files:**

- Modify: `src/output/tui/columns.rs`

- [ ] **Step 1: Add `Owner` variant to `Column` enum**

Between `Age` and `LastCommit`:

```rust
    /// Branch tip author email. Priority 8.
    Owner,
```

- [ ] **Step 2: Update `Column::priority()`**

```rust
Self::Owner => 8,
Self::LastCommit => 9,  // was 8
```

- [ ] **Step 3: Update `Column::label()`**

Add: `Self::Owner => "Owner",`

- [ ] **Step 4: Update `Column::from_list_column()`**

Add: `ListColumn::Owner => Column::Owner,`

- [ ] **Step 5: Update `ALL_COLUMNS` constant**

Insert `Column::Owner` between `Column::Age` and `Column::LastCommit`.

- [ ] **Step 6: Update `column_content_width()` match**

Add arm: `Column::Owner => 0,`

Note: This uses `0` as a temporary placeholder because the `ColumnValues` struct
does not have the `owner` field yet (added in Task 4). Update to
`v.owner.len() as u16` in Task 4 Step 2 after the field exists.

- [ ] **Step 7: Run `mise run test:unit` — fix any failing TUI column tests**

Run: `mise run test:unit` Expected: `column_selection_wide_terminal` test will
fail (expects 9 columns, now 10). Update assertion to
`assert_eq!(cols.len(), 10);`

- [ ] **Step 8: Commit**

```bash
git add src/output/tui/columns.rs
git commit -m "feat(tui): add Owner column to TUI column system"
```

---

### Task 4: Add `owner` to `ColumnValues` and formatting

**Files:**

- Modify: `src/output/format.rs`

- [ ] **Step 1: Add `owner` field to `ColumnValues` struct**

Add after `remote` field: `pub owner: String,`

- [ ] **Step 2: Populate `owner` in `compute_column_values()`**

Add in the function body:

```rust
let owner = info.owner_email.clone().unwrap_or_default();
```

Add `owner,` to the return struct.

- [ ] **Step 2b: Update `column_content_width()` in TUI columns**

Now that `ColumnValues` has the `owner` field, update the placeholder from Task
3 in `src/output/tui/columns.rs`:

Change `Column::Owner => 0,` to `Column::Owner => v.owner.len() as u16,`

- [ ] **Step 3: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/output/format.rs
git commit -m "feat(format): add owner to ColumnValues"
```

---

### Task 5: Add Owner column to list command

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Add `owner` field to `TableRow` struct**

Add between `remote` and `branch_age`:

```rust
    /// Branch tip author email.
    owner: String,
```

- [ ] **Step 2: Populate owner in `print_table()` row construction**

In the row building closure (around line 372), add owner handling. For
non-worktree rows with dimmed styling:

```rust
owner: if use_color && is_non_worktree {
    if vals.owner.is_empty() { String::new() } else { styles::dim(&vals.owner) }
} else {
    vals.owner.clone()
},
```

- [ ] **Step 3: Add Owner to column header and cell mapping**

In the `col_headers` construction, add the Owner match arm:

```rust
ListColumn::Owner => "Owner",
```

In the data_cols cell mapping, add:

```rust
ListColumn::Owner => row.owner.as_str(),
```

- [ ] **Step 4: Add Owner to JSON output**

In `print_json()`, add a new block:

```rust
if all_columns || selected_columns.contains(&ListColumn::Owner) {
    obj.insert("owner".into(), serde_json::json!(info.owner_email));
}
```

- [ ] **Step 5: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/commands/list.rs
git commit -m "feat(list): render Owner column in table and JSON output"
```

---

### Task 6: Add Owner column to TUI render

**Files:**

- Modify: `src/output/tui/render.rs`

- [ ] **Step 1: Add `Column::Owner` to `render_cell()` match**

In the `render_cell` function, add a match arm for `Column::Owner`:

```rust
Column::Owner => {
    Cell::from(Line::from(Span::raw(vals.owner.clone())))
}
```

- [ ] **Step 2: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "feat(tui): render Owner column in sync/prune table"
```

---

### Task 7: Add Owner column YAML test for list

**Files:**

- Create: `tests/manual/scenarios/list/owner-column.yml`

- [ ] **Step 1: Write YAML test scenario**

```yaml
name: "List shows owner column"
description: "Owner column displays branch tip author email"

repos:
  - name: owner-test
    use_fixture: standard-remote

steps:
  - name: "Clone the repository"
    run: "git-worktree-clone $REMOTE_OWNER_TEST"
    expect:
      exit_code: 0

  - name: "Checkout a branch"
    run: "git-worktree-checkout develop"
    cwd: "$WORK_DIR/owner-test/main"
    expect:
      exit_code: 0

  - name: "List shows Owner column header"
    run: "git-worktree-list 2>&1"
    cwd: "$WORK_DIR/owner-test/main"
    expect:
      exit_code: 0
      stdout_contains:
        - "Owner"

  - name: "List with --columns owner shows only owner"
    run: "git-worktree-list --columns branch,owner 2>&1"
    cwd: "$WORK_DIR/owner-test/main"
    expect:
      exit_code: 0
      stdout_contains:
        - "Owner"

  - name: "JSON output includes owner field"
    run: "git-worktree-list --json 2>&1"
    cwd: "$WORK_DIR/owner-test/main"
    expect:
      exit_code: 0
      stdout_contains:
        - '"owner"'
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci list:owner-column` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/list/owner-column.yml
git commit -m "test(list): add YAML test for Owner column"
```

---

## Chunk 2: Entry Ordering & Local Branches in Prune

### Task 8: Implement worktrees-first ordering

**Files:**

- Modify: `src/commands/list.rs`
- Modify: `src/output/tui/state.rs`

- [ ] **Step 1: Update sorting in `list.rs`**

Replace the current alphabetical sort (line ~187):

```rust
merged.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
```

With a two-tier sort: `EntryKind` first (Worktree < LocalBranch < RemoteBranch),
then alphabetical within each kind:

```rust
merged.sort_by(|a, b| {
    let kind_order = |k: &EntryKind| match k {
        EntryKind::Worktree => 0,
        EntryKind::LocalBranch => 1,
        EntryKind::RemoteBranch => 2,
    };
    kind_order(&a.kind)
        .cmp(&kind_order(&b.kind))
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
});
```

- [ ] **Step 2: Also update the non-merged sort path**

The `collect_worktree_info` return path (line ~195) already returns worktrees
only, so no change needed there. But verify that `collect_worktree_info` itself
sorts alphabetically (it does, in the function body).

- [ ] **Step 3: Update TUI state ordering**

In `src/output/tui/state.rs`, in `TuiState::new()`, apply the same sort to the
`worktrees` vector after construction. The worktree_infos passed in may already
be sorted, but applying the sort in TuiState ensures consistency:

```rust
worktrees.sort_by(|a, b| {
    let kind_order = |k: &EntryKind| match k {
        EntryKind::Worktree => 0,
        EntryKind::LocalBranch => 1,
        EntryKind::RemoteBranch => 2,
    };
    kind_order(&a.info.kind)
        .cmp(&kind_order(&b.info.kind))
        .then_with(|| a.info.name.to_lowercase().cmp(&b.info.name.to_lowercase()))
});
```

- [ ] **Step 4: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/commands/list.rs src/output/tui/state.rs
git commit -m "feat(list): order entries worktrees-first, then branches"
```

---

### Task 9: Seed local-only gone branches into prune TUI

**Files:**

- Modify: `src/commands/prune.rs`
- Modify: `src/core/worktree/list.rs`

- [ ] **Step 1: Add `WorktreeInfo::local_branch_stub` constructor**

In `src/core/worktree/list.rs`, add a new constructor to `WorktreeInfo` for
creating stub entries for local-only branches (similar to `empty()` but with
correct `EntryKind`):

```rust
/// Create a stub entry for a local-only branch (no worktree).
/// Used by prune/sync to represent gone branches that are branch-only.
pub fn local_branch_stub(name: &str, owner_email: Option<String>) -> Self {
    Self {
        kind: EntryKind::LocalBranch,
        name: name.to_string(),
        path: None,
        is_current: false,
        is_default_branch: false,
        ahead: None,
        behind: None,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        remote_ahead: None,
        remote_behind: None,
        last_commit_timestamp: None,
        last_commit_subject: String::new(),
        branch_creation_timestamp: None,
        base_lines_inserted: None,
        base_lines_deleted: None,
        staged_lines_inserted: None,
        staged_lines_deleted: None,
        unstaged_lines_inserted: None,
        unstaged_lines_deleted: None,
        remote_lines_inserted: None,
        remote_lines_deleted: None,
        owner_email,
    }
}
```

- [ ] **Step 2: Identify local-only gone branches in prune TUI**

In `src/commands/prune.rs`, in `run_tui()`, after the fetch phase identifies
gone branches and before `TuiState::new()` is called, determine which gone
branches are local-only (have no associated worktree):

```rust
// Identify which gone branches have no worktree
let worktree_branch_set: HashSet<&str> = worktree_infos
    .iter()
    .map(|i| i.name.as_str())
    .collect();

let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
for branch in &gone_branches {
    if !worktree_branch_set.contains(branch.as_str()) {
        let owner_email = get_author_email_for_ref(branch, &cwd);
        worktree_infos.push(WorktreeInfo::local_branch_stub(branch, owner_email));
    }
}
```

Note: `get_author_email_for_ref` needs to be made `pub(crate)` in `list.rs`.

- [ ] **Step 3: Make `get_author_email_for_ref` public within crate**

In `src/core/worktree/list.rs`, change:

```rust
fn get_author_email_for_ref(
```

to:

```rust
pub(crate) fn get_author_email_for_ref(
```

- [ ] **Step 4: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/commands/prune.rs src/core/worktree/list.rs
git commit -m "feat(prune): show local-only gone branches in TUI table"
```

---

### Task 10: Add local-only-branches-in-prune YAML test

**Files:**

- Create: `tests/manual/scenarios/prune/local-branch-visible.yml`

- [ ] **Step 1: Write YAML test scenario**

This test creates a branch without a worktree, deletes the remote, and verifies
prune shows it:

```yaml
name: "Prune shows local-only branches"
description:
  "Local branches without worktrees appear in prune output when their remote is
  deleted"

repos:
  - name: prune-local-test
    use_fixture: standard-remote

steps:
  - name: "Clone the repository"
    run: "git-worktree-clone $REMOTE_PRUNE_LOCAL_TEST"
    expect:
      exit_code: 0

  - name: "Create a local branch without a worktree"
    run: |
      git branch local-only-branch origin/develop
    cwd: "$WORK_DIR/prune-local-test/main"
    expect:
      exit_code: 0

  - name: "Delete develop on remote"
    run: |
      tmp=$(mktemp -d)
      git clone "$REMOTE_PRUNE_LOCAL_TEST" "$tmp" 2>/dev/null
      cd "$tmp"
      git push origin --delete develop 2>/dev/null
      rm -rf "$tmp"
    expect:
      exit_code: 0

  - name: "Run prune (verbose for sequential output)"
    run: "git-worktree-prune --verbose --verbose 2>&1"
    cwd: "$WORK_DIR/prune-local-test/main"
    expect:
      exit_code: 0
      stdout_contains:
        - "local-only-branch"

  - name: "Verify local branch was deleted"
    run: "git branch --list local-only-branch"
    cwd: "$WORK_DIR/prune-local-test/main"
    expect:
      exit_code: 0
      stdout_not_contains:
        - "local-only-branch"
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci prune:local-branch-visible` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/prune/local-branch-visible.yml
git commit -m "test(prune): add YAML test for local-only branch visibility"
```

---

### Task 11: Seed local-only gone branches into sync TUI

**Files:**

- Modify: `src/commands/sync.rs`

- [ ] **Step 1: Add local-only gone branches to sync worktree_infos**

In `run_tui()`, the orchestrator thread identifies gone branches after fetch.
Apply the same logic as prune: for each gone branch with no worktree, add a
`WorktreeInfo::local_branch_stub` to the TUI state.

Since sync's orchestrator runs in a separate thread and gone branches are
discovered after TUI starts, the existing `TaskStarted` auto-creation path
handles this. However, to get proper `EntryKind::LocalBranch` metadata, send the
info through the event channel. Update the `TaskStarted` handler in
`TuiState::apply_event` to create `LocalBranch` entries when the branch name
doesn't match any known worktree:

In `src/output/tui/state.rs`, in the `TaskStarted` handler where it creates
placeholder rows (around line 172), check if the phase is `Prune` and if so,
create with `EntryKind::LocalBranch`:

```rust
if !branch_name.is_empty() && self.find_row_mut(branch_name).is_none() {
    let kind = if matches!(phase, OperationPhase::Prune) {
        EntryKind::LocalBranch
    } else {
        EntryKind::Worktree
    };
    self.worktrees.push(WorktreeRow {
        info: WorktreeInfo {
            kind,
            ..WorktreeInfo::empty(branch_name)
        },
        status: WorktreeStatus::Idle,
        prev_terminal_status: None,
        hook_warned: false,
        hook_failed: false,
        hook_sub_rows: Vec::new(),
    });
}
```

- [ ] **Step 2: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/state.rs
git commit -m "feat(tui): use LocalBranch kind for dynamically discovered prune rows"
```

---

## Chunk 3: Ownership-Gated Rebase/Push & `--include` Flag

### Task 12: Add `--include` flag to sync command

**Files:**

- Modify: `src/commands/sync.rs`

- [ ] **Step 1: Add `--include` arg to sync `Args` struct**

After the existing `force_with_lease` field:

```rust
#[arg(
    long,
    help = "Include additional branches in rebase/push (email, branch name, or 'unowned')"
)]
include: Vec<String>,
```

- [ ] **Step 2: Add ownership resolution types**

At the top of `sync.rs` (after imports), add:

```rust
/// Parsed `--include` value.
enum IncludeFilter {
    /// Include all branches regardless of owner.
    Unowned,
    /// Include branches owned by this email.
    Email(String),
    /// Include this specific branch by name.
    Branch(String),
}

impl IncludeFilter {
    fn parse(value: &str) -> Self {
        if value == "unowned" {
            Self::Unowned
        } else if value.contains('@') {
            Self::Email(value.to_string())
        } else {
            Self::Branch(value.to_string())
        }
    }
}

/// Check if a branch is included by the filters or by ownership.
fn is_branch_included(
    branch: &str,
    owner_email: Option<&str>,
    user_email: Option<&str>,
    filters: &[IncludeFilter],
) -> bool {
    // Check ownership first
    if let (Some(owner), Some(user)) = (owner_email, user_email) {
        if owner == user {
            return true;
        }
    }
    // Check include filters
    for filter in filters {
        match filter {
            IncludeFilter::Unowned => return true,
            IncludeFilter::Email(email) => {
                if owner_email == Some(email.as_str()) {
                    return true;
                }
            }
            IncludeFilter::Branch(name) => {
                if branch == name {
                    return true;
                }
            }
        }
    }
    false
}
```

- [ ] **Step 3: Run `mise run clippy`**

Run: `mise run clippy` Expected: PASS (may warn about unused functions — that's
fine, they'll be used in the next task)

- [ ] **Step 4: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): add --include flag and ownership resolution"
```

---

### Task 13: Split owned/unowned worktrees in sync DAG

**Files:**

- Modify: `src/core/worktree/sync_dag.rs`
- Modify: `src/commands/sync.rs`

- [ ] **Step 1: Update `SyncDag::build_sync` signature**

Change from:

```rust
pub fn build_sync(
    worktrees: Vec<(String, PathBuf)>,
    gone_branches: Vec<String>,
    rebase_branch: Option<String>,
    push: bool,
) -> Self
```

To:

```rust
pub fn build_sync(
    owned_worktrees: Vec<(String, PathBuf)>,
    unowned_worktrees: Vec<(String, PathBuf)>,
    gone_branches: Vec<String>,
    rebase_branch: Option<String>,
    push: bool,
) -> Self
```

- [ ] **Step 2: Update `build_sync` implementation**

Inside the function:

- Create `all_worktrees` by chaining owned + unowned
- Create `Update` tasks for ALL worktrees (both owned and unowned)
- Create `Rebase` tasks ONLY for owned worktrees
- Create `Push` tasks ONLY for owned worktrees
- Create an `owned_set: HashSet<String>` from `owned_worktrees` for fast lookup

- [ ] **Step 3: Update `build_prune` to pass empty unowned list**

```rust
pub fn build_prune(gone_branches: Vec<String>) -> Self {
    Self::build_sync(vec![], vec![], gone_branches, None, false)
}
```

- [ ] **Step 4: Update sync.rs DAG construction**

In the orchestrator thread, after identifying gone branches and live worktrees,
split by ownership:

```rust
let user_email = git_cmd.config_get("user.email").ok();
let include_filters: Vec<IncludeFilter> = shared_include
    .iter()
    .map(|v| IncludeFilter::parse(v))
    .collect();

// Build owner lookup from the worktree_infos collected before TUI started.
// shared_worktree_infos is an Arc<Vec<WorktreeInfo>> passed into the
// orchestrator closure.
let owner_lookup: HashMap<String, Option<String>> = shared_worktree_infos
    .iter()
    .map(|info| (info.name.clone(), info.owner_email.clone()))
    .collect();

let (owned, unowned): (Vec<_>, Vec<_>) = live_worktrees
    .into_iter()
    .partition(|(branch, _)| {
        is_branch_included(
            branch,
            owner_lookup.get(branch).and_then(|e| e.as_deref()),
            user_email.as_deref(),
            &include_filters,
        )
    });

let dag = SyncDag::build_sync(
    owned,
    unowned,
    gone_branches,
    shared_rebase_branch.as_ref().clone(),
    shared_push,
);
```

- [ ] **Step 5: Fix all unit tests in `sync_dag.rs`**

Update all test calls to `build_sync` to pass the new signature (add empty
`vec![]` for `unowned_worktrees` parameter where appropriate).

- [ ] **Step 6: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add src/core/worktree/sync_dag.rs src/commands/sync.rs
git commit -m "feat(sync): split owned/unowned worktrees in DAG construction"
```

---

### Task 14: Add ownership-rebase-push YAML test

**Files:**

- Create: `tests/manual/scenarios/sync/ownership-rebase-push.yml`

- [ ] **Step 1: Write YAML test scenario**

This test creates branches with different author emails, runs sync with
`--rebase --push`, and verifies only owned branches are rebased/pushed:

```yaml
name: "Sync ownership gates rebase and push"
description: "Only branches owned by the current user are rebased and pushed"

repos:
  - name: ownership-test
    use_fixture: standard-remote

steps:
  - name: "Clone the repository"
    run: "git-worktree-clone $REMOTE_OWNERSHIP_TEST"
    expect:
      exit_code: 0

  - name: "Create a branch owned by another user"
    run: |
      git-worktree-checkout develop
      cd develop
      GIT_AUTHOR_NAME="Other" GIT_AUTHOR_EMAIL="other@example.com" \
      GIT_COMMITTER_NAME="Other" GIT_COMMITTER_EMAIL="other@example.com" \
      git commit --allow-empty -m "Other's commit"
      git push origin develop
    cwd: "$WORK_DIR/ownership-test/main"
    expect:
      exit_code: 0

  - name: "Create a branch owned by current user"
    run: |
      git-worktree-checkout feature/test-feature
      cd ../feature/test-feature
      git commit --allow-empty -m "My commit"
      git push origin feature/test-feature
    cwd: "$WORK_DIR/ownership-test/main"
    expect:
      exit_code: 0

  - name: "Add a commit on main (for rebase to have work)"
    run: |
      echo "new content" >> README.md
      git add README.md
      git commit -m "Update main"
      git push origin main
    cwd: "$WORK_DIR/ownership-test/main"
    expect:
      exit_code: 0

  - name: "Record other's branch before sync"
    run: "cd develop && git rev-parse HEAD > /tmp/daft-ownership-develop-before"
    cwd: "$WORK_DIR/ownership-test"
    expect:
      exit_code: 0

  - name: "Run sync with --rebase and --push (verbose for output)"
    run: "git-worktree-sync --rebase main --push --verbose --verbose 2>&1"
    cwd: "$WORK_DIR/ownership-test/main"
    expect:
      exit_code: 0

  - name: "Verify other's branch was NOT rebased"
    run: |
      before=$(cat /tmp/daft-ownership-develop-before)
      after=$(cd develop && git rev-parse HEAD)
      [ "$before" = "$after" ]
    cwd: "$WORK_DIR/ownership-test"
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci sync:ownership-rebase-push` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/sync/ownership-rebase-push.yml
git commit -m "test(sync): add YAML test for ownership-gated rebase/push"
```

---

### Task 15: Add `--include` YAML tests

**Files:**

- Create: `tests/manual/scenarios/sync/include-unowned.yml`
- Create: `tests/manual/scenarios/sync/include-email.yml`
- Create: `tests/manual/scenarios/sync/include-branch.yml`

- [ ] **Step 1: Write `include-unowned.yml`**

Test that `--include unowned` causes all branches to be rebased/pushed:

```yaml
name: "Sync --include unowned"
description: "--include unowned causes all branches to be rebased and pushed"

repos:
  - name: include-all-test
    use_fixture: standard-remote

steps:
  - name: "Clone and setup branches with different owners"
    run: |
      git-worktree-clone $REMOTE_INCLUDE_ALL_TEST
      cd include-all-test/main
      git-worktree-checkout develop
      cd ../develop
      GIT_AUTHOR_NAME="Other" GIT_AUTHOR_EMAIL="other@example.com" \
      GIT_COMMITTER_NAME="Other" GIT_COMMITTER_EMAIL="other@example.com" \
      git commit --allow-empty -m "Other's commit"
      git push origin develop
    expect:
      exit_code: 0

  - name: "Add a commit on main"
    run: |
      echo "new" >> README.md
      git add README.md
      git commit -m "Update main"
      git push origin main
    cwd: "$WORK_DIR/include-all-test/main"
    expect:
      exit_code: 0

  - name: "Record develop before sync"
    run: "cd develop && git rev-parse HEAD > /tmp/daft-include-all-before"
    cwd: "$WORK_DIR/include-all-test"
    expect:
      exit_code: 0

  - name: "Run sync with --include unowned"
    run:
      "git-worktree-sync --rebase main --include unowned --verbose --verbose
      2>&1"
    cwd: "$WORK_DIR/include-all-test/main"
    expect:
      exit_code: 0

  - name: "Verify other's branch WAS rebased (include overrides)"
    run: |
      before=$(cat /tmp/daft-include-all-before)
      after=$(cd develop && git rev-parse HEAD)
      [ "$before" != "$after" ]
    cwd: "$WORK_DIR/include-all-test"
    expect:
      exit_code: 0
```

- [ ] **Step 2: Write `include-email.yml`**

```yaml
name: "Sync --include email"
description: "--include with an email includes that user's branches"

repos:
  - name: include-email-test
    use_fixture: standard-remote

steps:
  - name: "Clone and setup branches with different owners"
    run: |
      git-worktree-clone $REMOTE_INCLUDE_EMAIL_TEST
      cd include-email-test/main
      git-worktree-checkout develop
      cd ../develop
      GIT_AUTHOR_NAME="Other" GIT_AUTHOR_EMAIL="other@example.com" \
      GIT_COMMITTER_NAME="Other" GIT_COMMITTER_EMAIL="other@example.com" \
      git commit --allow-empty -m "Other's commit"
      git push origin develop
    expect:
      exit_code: 0

  - name: "Add a commit on main"
    run: |
      echo "new" >> README.md
      git add README.md
      git commit -m "Update main"
      git push origin main
    cwd: "$WORK_DIR/include-email-test/main"
    expect:
      exit_code: 0

  - name: "Record develop before sync"
    run: "cd develop && git rev-parse HEAD > /tmp/daft-include-email-before"
    cwd: "$WORK_DIR/include-email-test"
    expect:
      exit_code: 0

  - name: "Run sync with --include other@example.com"
    run:
      "git-worktree-sync --rebase main --include other@example.com --verbose
      --verbose 2>&1"
    cwd: "$WORK_DIR/include-email-test/main"
    expect:
      exit_code: 0

  - name: "Verify other's branch WAS rebased"
    run: |
      before=$(cat /tmp/daft-include-email-before)
      after=$(cd develop && git rev-parse HEAD)
      [ "$before" != "$after" ]
    cwd: "$WORK_DIR/include-email-test"
    expect:
      exit_code: 0
```

- [ ] **Step 3: Write `include-branch.yml`**

```yaml
name: "Sync --include branch name"
description: "--include with a branch name includes that specific branch"

repos:
  - name: include-branch-test
    use_fixture: standard-remote

steps:
  - name: "Clone and setup branches with different owners"
    run: |
      git-worktree-clone $REMOTE_INCLUDE_BRANCH_TEST
      cd include-branch-test/main
      git-worktree-checkout develop
      cd ../develop
      GIT_AUTHOR_NAME="Other" GIT_AUTHOR_EMAIL="other@example.com" \
      GIT_COMMITTER_NAME="Other" GIT_COMMITTER_EMAIL="other@example.com" \
      git commit --allow-empty -m "Other's commit"
      git push origin develop
    expect:
      exit_code: 0

  - name: "Add a commit on main"
    run: |
      echo "new" >> README.md
      git add README.md
      git commit -m "Update main"
      git push origin main
    cwd: "$WORK_DIR/include-branch-test/main"
    expect:
      exit_code: 0

  - name: "Record develop before sync"
    run: "cd develop && git rev-parse HEAD > /tmp/daft-include-branch-before"
    cwd: "$WORK_DIR/include-branch-test"
    expect:
      exit_code: 0

  - name: "Run sync with --include develop"
    run:
      "git-worktree-sync --rebase main --include develop --verbose --verbose
      2>&1"
    cwd: "$WORK_DIR/include-branch-test/main"
    expect:
      exit_code: 0

  - name: "Verify develop WAS rebased (included by name)"
    run: |
      before=$(cat /tmp/daft-include-branch-before)
      after=$(cd develop && git rev-parse HEAD)
      [ "$before" != "$after" ]
    cwd: "$WORK_DIR/include-branch-test"
    expect:
      exit_code: 0
```

- [ ] **Step 4: Run all include tests**

Run:
`mise run test:manual -- --ci sync:include-unowned sync:include-email sync:include-branch`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/sync/include-*.yml
git commit -m "test(sync): add YAML tests for --include flag variants"
```

---

## Chunk 4: Two-Section Layout & Section Divider

### Task 16: Add section tracking to TuiState

**Files:**

- Modify: `src/output/tui/state.rs`

- [ ] **Step 1: Add section boundary field to `TuiState`**

```rust
/// Index of the first unowned worktree row (None if no unowned section).
pub unowned_start_index: Option<usize>,
```

- [ ] **Step 2: Update `TuiState::new()` to accept section boundary**

Add parameter `unowned_start_index: Option<usize>` to `new()` and store it.

- [ ] **Step 3: Update all callers of `TuiState::new()`**

In `sync.rs` and `prune.rs`, pass the computed unowned start index. For prune,
pass `None` (no ownership sections).

- [ ] **Step 4: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/output/tui/state.rs src/commands/sync.rs src/commands/prune.rs
git commit -m "feat(tui): add section boundary tracking to TuiState"
```

---

### Task 17: Render section divider in TUI

**Files:**

- Modify: `src/output/tui/render.rs`

- [ ] **Step 1: Add section divider rendering**

In the table rendering function, after rendering the row at index
`unowned_start_index - 1` and before rendering the row at `unowned_start_index`,
insert a divider row. The divider should be a full-width dim line:
`── other branches ──`.

Implementation approach: ratatui's `Table` widget does not support column-
spanning cells. Instead, render the divider as a separate `Paragraph` widget
between the two table sections. Split the render area into three vertical
chunks: owned table, divider line (1 row high), unowned table. Use
`ratatui::widgets::Paragraph` with dim styling for the divider text
`── other branches ──`. If `unowned_start_index` is `None` or the unowned
section is empty, skip the divider and render a single table.

- [ ] **Step 2: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/render.rs
git commit -m "feat(tui): render section divider between owned and unowned branches"
```

---

### Task 18: Add two-section layout to list command

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Add section divider to list table**

In `print_table()`, after sorting entries with worktrees-first ordering, compute
the section boundary based on ownership. Insert a divider row
(`── other branches ──`) between the owned and unowned sections.

For list, the section split shows ownership grouping for informational purposes.
The list command does not have `--include` — the divider always appears when at
least one owned and one unowned branch exist. This is purely visual grouping;
list performs no write operations so the distinction is informational only.

Read `git config user.email` at the start of `run()` and pass it through to the
rendering logic. If `user.email` is not configured, skip the divider (treat all
branches as a single unsectioned list).

- [ ] **Step 2: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/commands/list.rs
git commit -m "feat(list): add two-section layout with ownership divider"
```

---

## Chunk 5: Temp Worktrees for Local Branch Rebase

### Task 19: Create temp worktree module

**Files:**

- Create: `src/core/worktree/temp_worktree.rs`
- Modify: `src/core/worktree/mod.rs`

- [ ] **Step 1: Create `temp_worktree.rs`**

```rust
//! Temporary worktree management for operations on local-only branches.
//!
//! Creates short-lived worktrees in `.daft-tmp/` for rebase operations on
//! branches that don't have a persistent worktree. Includes aggressive
//! cleanup: Drop guard, stale sweep on startup, and signal handling.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Sanitize a branch name for use as a directory name.
/// Replaces `/` with `--` to produce flat directory names.
fn sanitize_branch_name(branch: &str) -> String {
    branch.replace('/', "--")
}

/// Get the `.daft-tmp` directory path under the bare repo root.
pub fn tmp_dir(bare_root: &Path) -> PathBuf {
    bare_root.join(".daft-tmp")
}

/// Path for a specific branch's temp worktree.
pub fn worktree_path(bare_root: &Path, branch: &str) -> PathBuf {
    tmp_dir(bare_root).join(sanitize_branch_name(branch))
}

/// Create a temporary worktree for the given branch.
pub fn create(bare_root: &Path, branch: &str) -> Result<PathBuf> {
    let path = worktree_path(bare_root, branch);
    if path.exists() {
        // Stale from a previous crash — clean it up first.
        remove(&path)?;
    }
    std::fs::create_dir_all(path.parent().unwrap())
        .context("Failed to create .daft-tmp directory")?;

    let output = Command::new("git")
        .args(["worktree", "add", path.to_str().unwrap(), branch])
        .current_dir(bare_root)
        .output()
        .context("Failed to create temp worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git worktree add failed: {stderr}");
    }

    Ok(path)
}

/// Remove a temporary worktree using `git worktree remove`.
pub fn remove(path: &Path) -> Result<()> {
    let output = Command::new("git")
        .args([
            "worktree",
            "remove",
            "--force",
            path.to_str().unwrap_or(""),
        ])
        .output()
        .context("Failed to remove temp worktree")?;

    if !output.status.success() {
        // Fallback: try rm -rf + git worktree prune
        let _ = std::fs::remove_dir_all(path);
        let _ = Command::new("git")
            .args(["worktree", "prune"])
            .output();
    }

    Ok(())
}

/// Clean up all stale temp worktrees in `.daft-tmp/`.
/// Called at the start of sync/prune to handle leftovers from crashes.
pub fn cleanup_stale(bare_root: &Path) -> Result<()> {
    let tmp = tmp_dir(bare_root);
    if !tmp.exists() {
        return Ok(());
    }

    let entries = std::fs::read_dir(&tmp)
        .context("Failed to read .daft-tmp directory")?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let _ = remove(&path);
        }
    }

    // Remove the .daft-tmp directory itself if empty.
    let _ = std::fs::remove_dir(&tmp);

    Ok(())
}

/// RAII guard that removes a temp worktree on drop.
pub struct TempWorktreeGuard {
    path: Option<PathBuf>,
}

impl TempWorktreeGuard {
    pub fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Return the worktree path.
    pub fn path(&self) -> &Path {
        self.path.as_ref().expect("guard already consumed")
    }

    /// Consume the guard without removing the worktree.
    /// Use when the worktree was already removed explicitly.
    pub fn disarm(mut self) {
        self.path = None;
    }
}

impl Drop for TempWorktreeGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = remove(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(sanitize_branch_name("feat/login"), "feat--login");
        assert_eq!(
            sanitize_branch_name("feat/nested/deep"),
            "feat--nested--deep"
        );
        assert_eq!(sanitize_branch_name("simple"), "simple");
    }

    #[test]
    fn tmp_dir_path() {
        let root = Path::new("/repo");
        assert_eq!(tmp_dir(root), PathBuf::from("/repo/.daft-tmp"));
    }

    #[test]
    fn worktree_path_sanitizes() {
        let root = Path::new("/repo");
        assert_eq!(
            worktree_path(root, "feat/login"),
            PathBuf::from("/repo/.daft-tmp/feat--login")
        );
    }
}
```

- [ ] **Step 2: Add module to `mod.rs`**

In `src/core/worktree/mod.rs`, add: `pub mod temp_worktree;`

- [ ] **Step 3: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/core/worktree/temp_worktree.rs src/core/worktree/mod.rs
git commit -m "feat(core): add temp worktree module with cleanup guard"
```

---

### Task 20: Integrate temp worktrees into sync rebase

**Files:**

- Modify: `src/commands/sync.rs`

- [ ] **Step 1: Add stale cleanup at sync startup**

At the beginning of `run()` (or `run_tui()`), after resolving the project root,
call:

```rust
temp_worktree::cleanup_stale(&project_root)?;
```

- [ ] **Step 2: Update rebase task execution for local-only branches**

In the orchestrator task closure, in the `TaskId::Rebase` branch, check if the
branch has a worktree path. If not (local-only branch), create a temp worktree
before rebasing:

```rust
TaskId::Rebase(ref branch) => {
    let worktree_path = worktree_map.get(branch);

    if let Some(path) = worktree_path {
        // Regular worktree rebase (existing code)
        execute_rebase_task(branch, path, ...)
    } else {
        // Local-only branch: create temp worktree, rebase, remove
        let tmp_path = temp_worktree::create(&project_root, branch)?;
        let guard = temp_worktree::TempWorktreeGuard::new(tmp_path.clone());
        let result = execute_rebase_task(branch, &tmp_path, ...);
        drop(guard); // removes temp worktree
        result
    }
}
```

- [ ] **Step 3: Update push task for local-only branches**

Push doesn't need a worktree — `git push origin <branch>` works from anywhere.
Verify the existing push task execution already handles this (it should, since
it uses `git push` with explicit branch names).

- [ ] **Step 4: Update update task for local-only branches**

For local-only branches in the update phase, detect if a fast-forward is
possible by checking:

```rust
// Check if local branch is an ancestor of its upstream
let merge_base = Command::new("git")
    .args(["merge-base", "--is-ancestor", branch, &format!("{branch}@{{upstream}}")])
    .current_dir(&project_root)
    .status();
```

If the exit code is 0 (is ancestor = can fast-forward):
`git branch -f <branch> <branch>@{upstream}`

If exit code is 1 (diverged): return `TaskStatus::Succeeded` with message
"diverged" and set `FinalStatus::Diverged`.

If upstream doesn't exist: skip with "no upstream" status.

- [ ] **Step 4b: Ensure daft hooks are skipped for temp worktrees**

In the rebase task closure (Step 2), when creating the temp worktree path, do
NOT pass it through the hooks executor. The existing rebase code path may call
`run_hook(HookType::PreRemove, ...)` or similar. When the worktree path is under
`.daft-tmp/`, skip hook execution. Add a check:

```rust
let skip_hooks = tmp_path.to_str().map_or(false, |p| p.contains(".daft-tmp"));
```

Pass this flag to the hook executor or simply don't call `run_hook` when
`skip_hooks` is true.

- [ ] **Step 5: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/commands/sync.rs
git commit -m "feat(sync): use temp worktrees for local-only branch rebase"
```

---

### Task 21: Add temp-worktree-rebase YAML test

**Files:**

- Create: `tests/manual/scenarios/sync/temp-worktree-rebase.yml`

- [ ] **Step 1: Write YAML test scenario**

Test that a local-only branch (no worktree) gets rebased via temp worktree:

```yaml
name: "Sync rebases local-only branches via temp worktree"
description: "Branches without worktrees are rebased using temporary worktrees"

repos:
  - name: temp-wt-test
    use_fixture: standard-remote

steps:
  - name: "Clone the repository"
    run: "git-worktree-clone $REMOTE_TEMP_WT_TEST"
    expect:
      exit_code: 0

  - name: "Create a local branch without worktree and push it"
    run: |
      git branch my-feature origin/main
      git checkout my-feature
      git commit --allow-empty -m "Feature work"
      git push origin my-feature
      git checkout main
    cwd: "$WORK_DIR/temp-wt-test/main"
    expect:
      exit_code: 0

  - name: "Advance main"
    run: |
      echo "new content" >> README.md
      git add README.md
      git commit -m "Advance main"
      git push origin main
    cwd: "$WORK_DIR/temp-wt-test/main"
    expect:
      exit_code: 0

  - name: "Record my-feature before sync"
    run: "git rev-parse my-feature > /tmp/daft-temp-wt-before"
    cwd: "$WORK_DIR/temp-wt-test/main"
    expect:
      exit_code: 0

  - name: "Run sync with --rebase"
    run: "git-worktree-sync --rebase main --verbose --verbose 2>&1"
    cwd: "$WORK_DIR/temp-wt-test/main"
    expect:
      exit_code: 0

  - name: "Verify my-feature was rebased"
    run: |
      before=$(cat /tmp/daft-temp-wt-before)
      after=$(git rev-parse my-feature)
      [ "$before" != "$after" ]
    cwd: "$WORK_DIR/temp-wt-test/main"
    expect:
      exit_code: 0

  - name: "Verify no temp worktree remains"
    run: "test ! -d .daft-tmp"
    cwd: "$WORK_DIR/temp-wt-test"
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci sync:temp-worktree-rebase` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/sync/temp-worktree-rebase.yml
git commit -m "test(sync): add YAML test for temp worktree rebase"
```

---

### Task 22: Add stale cleanup to prune

**Files:**

- Modify: `src/commands/prune.rs`

- [ ] **Step 1: Add stale cleanup at prune startup**

At the beginning of `run()` (or `run_tui()`), call:

```rust
temp_worktree::cleanup_stale(&project_root)?;
```

- [ ] **Step 2: Run `mise run clippy` and `mise run test:unit`**

Run: `mise run clippy && mise run test:unit` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/commands/prune.rs
git commit -m "feat(prune): add stale temp worktree cleanup at startup"
```

---

## Chunk 6: Shell Completions, Man Pages & Final Integration

### Task 23: Update shell completions for `--include`

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `src/commands/completions/mod.rs`

Note: `fig.rs` generates completions from clap `Args` structs automatically, so
it picks up `--include` without manual changes.

- [ ] **Step 1: Add `--include` to bash completions**

In `DAFT_BASH_COMPLETIONS`, add `--include` to the `git-worktree-sync` case.

- [ ] **Step 2: Add `--include` to zsh completions**

In `DAFT_ZSH_COMPLETIONS`, add `--include` to the sync command's options.

- [ ] **Step 3: Add `--include` to fish completions**

In `DAFT_FISH_COMPLETIONS`, add `--include` completion for sync.

- [ ] **Step 4: Run `mise run clippy`**

Run: `mise run clippy` Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/commands/completions/
git commit -m "feat(completions): add --include flag to sync completions"
```

---

### Task 24: Update man pages

- [ ] **Step 1: Regenerate man pages**

Run: `mise run man:gen` Expected: Man page for `git-worktree-sync` updated with
`--include` flag.

- [ ] **Step 2: Verify man pages**

Run: `mise run man:verify` Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add man/
git commit -m "docs(man): regenerate man pages for --include flag"
```

---

### Task 25: Run full test suite

- [ ] **Step 1: Run formatting**

Run: `mise run fmt`

- [ ] **Step 2: Run clippy**

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 3: Run unit tests**

Run: `mise run test:unit` Expected: All pass.

- [ ] **Step 4: Run integration tests**

Run: `mise run test:integration` Expected: All pass.

- [ ] **Step 5: Run all YAML manual tests**

Run: `mise run test:manual -- --ci` Expected: All pass.

- [ ] **Step 6: Fix any failures and commit fixes**

---

### Task 26: Update documentation

**Files:**

- Modify: `docs/cli/daft-sync.md`
- Modify: `docs/cli/daft-list.md`
- Modify: `SKILL.md`

- [ ] **Step 1: Update sync CLI docs with `--include` flag**

Add documentation for:

- `--include` flag usage and accepted values
- Ownership-gated rebase/push behavior
- Owner column

- [ ] **Step 2: Update list CLI docs with Owner column**

Document the new Owner column and `--columns owner` usage.

- [ ] **Step 3: Update SKILL.md if it exists**

Per CLAUDE.md: "Update SKILL.md when changes affect how an agent should interact
with daft."

- [ ] **Step 4: Commit**

```bash
git add docs/ SKILL.md
git commit -m "docs: update CLI reference for ownership and --include"
```
