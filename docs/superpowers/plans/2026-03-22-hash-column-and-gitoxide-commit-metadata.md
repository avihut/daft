# Hash Column and Gitoxide Commit Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an optional `hash` column showing abbreviated commit SHA to
list/prune/sync commands, and introduce a gitoxide fast path for commit metadata
retrieval.

**Architecture:** Extends the existing column system (ListColumn, SortColumn,
TUI Column) with a new `Hash` variant following the exact pattern used by
`Size`. Modifies `get_last_commit_info` to also return the short hash, and adds
gitoxide-native alternatives that avoid subprocess overhead.

**Tech Stack:** Rust, gix (gitoxide), clap, tabled, ratatui

---

## File Structure

| File                                          | Action | Responsibility                                       |
| --------------------------------------------- | ------ | ---------------------------------------------------- |
| `src/core/worktree/list.rs`                   | Modify | Add `last_commit_hash` field, update commit info fns |
| `src/git/oxide.rs`                            | Modify | Add gitoxide commit metadata functions               |
| `src/core/columns.rs`                         | Modify | Add `Hash` variant to `ListColumn`                   |
| `src/core/sort.rs`                            | Modify | Add `Hash` variant to `SortColumn`                   |
| `src/output/format.rs`                        | Modify | Add `hash` field to `ColumnValues`                   |
| `src/output/tui/columns.rs`                   | Modify | Add `Hash` variant to TUI `Column`                   |
| `src/output/tui/render.rs`                    | Modify | Add Hash cell rendering                              |
| `src/commands/list.rs`                        | Modify | Add Hash column to CLI table rendering               |
| `src/commands/completions/bash.rs`            | Modify | Add `hash` to column/sort completions                |
| `src/commands/completions/zsh.rs`             | Modify | Add `hash` to column/sort completions                |
| `src/commands/completions/fish.rs`            | Modify | Add `hash` to column/sort completions                |
| `src/commands/completions/fig.rs`             | Modify | Add `hash` to column/sort completions                |
| `tests/manual/scenarios/list/hash-column.yml` | Create | YAML test scenario                                   |

---

### Task 1: Add `last_commit_hash` to `WorktreeInfo` and update commit info functions

**Files:**

- Modify: `src/core/worktree/list.rs:46-99` (WorktreeInfo struct)
- Modify: `src/core/worktree/list.rs:104-165` (empty/local_branch_stub
  constructors)
- Modify: `src/core/worktree/list.rs:167-220` (refresh_dynamic_fields)
- Modify: `src/core/worktree/list.rs:320-364` (get_last_commit_info,
  get_last_commit_info_for_ref)
- Modify: `src/core/worktree/list.rs:730-731` (collect_worktree_info call site)
- Modify: `src/core/worktree/list.rs:879-880` (collect_branch_info local branch
  call site)
- Modify: `src/core/worktree/list.rs:969-970` (collect_branch_info remote branch
  call site)

- [ ] **Step 1: Add `last_commit_hash` field to `WorktreeInfo`**

In `src/core/worktree/list.rs`, add the field after `last_commit_subject`:

```rust
    /// Subject line of the last commit.
    pub last_commit_subject: String,
    /// Abbreviated hash (7 chars) of the last commit (None if unavailable).
    pub last_commit_hash: Option<String>,
```

- [ ] **Step 2: Update `empty()` and `local_branch_stub()` constructors**

Add `last_commit_hash: None` after `last_commit_subject: String::new()` in both
constructors.

- [ ] **Step 3: Update `get_last_commit_info` to return hash**

Change the function signature and implementation:

```rust
/// Get the last commit's Unix timestamp, abbreviated hash, and subject for a worktree.
///
/// Returns `(timestamp, hash, subject)` where timestamp is seconds since epoch.
fn get_last_commit_info(worktree_path: &Path) -> (Option<i64>, Option<String>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%h\x1f%s"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            let parts: Vec<&str> = trimmed.splitn(3, '\x1f').collect();
            if parts.len() == 3 {
                let timestamp = parts[0].parse::<i64>().ok();
                (timestamp, Some(parts[1].to_string()), parts[2].to_string())
            } else {
                (None, None, String::new())
            }
        }
        _ => (None, None, String::new()),
    }
}
```

- [ ] **Step 4: Update `get_last_commit_info_for_ref` the same way**

Same changes as Step 3 but with the `branch_ref` argument:

```rust
fn get_last_commit_info_for_ref(branch_ref: &str, cwd: &Path) -> (Option<i64>, Option<String>, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct\x1f%h\x1f%s", branch_ref])
        .current_dir(cwd)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            let parts: Vec<&str> = trimmed.splitn(3, '\x1f').collect();
            if parts.len() == 3 {
                let timestamp = parts[0].parse::<i64>().ok();
                (timestamp, Some(parts[1].to_string()), parts[2].to_string())
            } else {
                (None, None, String::new())
            }
        }
        _ => (None, None, String::new()),
    }
}
```

- [ ] **Step 5: Update all call sites to destructure the 3-tuple**

There are 5 call sites that destructure `(timestamp, subject)` and must become
`(timestamp, hash, subject)`:

1. `refresh_dynamic_fields` (~line 196):

```rust
let (ts, hash, subj) = get_last_commit_info(path);
self.last_commit_timestamp = ts;
self.last_commit_hash = hash;
self.last_commit_subject = subj;
```

2. `collect_worktree_info` (~line 731):

```rust
let (last_commit_timestamp, last_commit_hash, last_commit_subject) = get_last_commit_info(&entry.path);
```

3. `collect_branch_info` local branch (~line 879):

```rust
let (last_commit_timestamp, last_commit_hash, last_commit_subject) =
    get_last_commit_info_for_ref(branch, cwd);
```

4. `collect_branch_info` remote branch (~line 969):

```rust
let (last_commit_timestamp, last_commit_hash, last_commit_subject) =
    get_last_commit_info_for_ref(remote_branch, cwd);
```

5. All `WorktreeInfo { ... }` struct literals in these functions must include
   `last_commit_hash`.

- [ ] **Step 6: Verify it compiles**

Run: `cargo build 2>&1 | head -30`

Expected: Successful build (may have warnings from unused field, which is fine
at this stage).

- [ ] **Step 7: Run existing unit tests**

Run: `mise run test:unit`

Expected: All tests pass (the new field is populated but not yet consumed).

- [ ] **Step 8: Commit**

```bash
git add src/core/worktree/list.rs
git commit -m "feat: add last_commit_hash to WorktreeInfo and commit info functions"
```

---

### Task 2: Add `Hash` to `ListColumn` enum

**Files:**

- Modify: `src/core/columns.rs:14-35` (ListColumn enum)
- Modify: `src/core/columns.rs:39-52` (all())
- Modify: `src/core/columns.rs:56-68` (list_defaults())
- Modify: `src/core/columns.rs:78-91` (canonical_position())
- Modify: `src/core/columns.rs:94-107` (cli_name())
- Modify: `src/core/columns.rs:128-146` (FromStr)
- Test: `src/core/columns.rs` (existing + new tests)

- [ ] **Step 1: Write failing tests**

Add to the test module at the bottom of `src/core/columns.rs`:

```rust
#[test]
fn test_defaults_exclude_hash() {
    assert!(!ListColumn::list_defaults().contains(&ListColumn::Hash));
    assert!(!ListColumn::tui_defaults().contains(&ListColumn::Hash));
    assert!(ListColumn::all().contains(&ListColumn::Hash));
}

#[test]
fn test_modifier_add_hash() {
    let resolved = ColumnSelection::parse("+hash", CommandKind::List).unwrap();
    assert!(resolved.columns.contains(&ListColumn::Hash));
    // Hash should appear after Owner and before LastCommit
    let hash_pos = resolved
        .columns
        .iter()
        .position(|c| *c == ListColumn::Hash)
        .unwrap();
    let owner_pos = resolved
        .columns
        .iter()
        .position(|c| *c == ListColumn::Owner)
        .unwrap();
    let commit_pos = resolved
        .columns
        .iter()
        .position(|c| *c == ListColumn::LastCommit)
        .unwrap();
    assert!(hash_pos > owner_pos);
    assert!(hash_pos < commit_pos);
}

#[test]
fn test_hash_cli_name_roundtrip() {
    let col: ListColumn = "hash".parse().unwrap();
    assert_eq!(col, ListColumn::Hash);
    assert_eq!(col.cli_name(), "hash");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib core::columns::tests 2>&1 | tail -20`

Expected: Compilation error — `ListColumn::Hash` does not exist.

- [ ] **Step 3: Add `Hash` variant to enum**

In the `ListColumn` enum, add between `Owner` and `LastCommit`:

```rust
    /// Branch owner (from git author email).
    Owner,
    /// Abbreviated commit hash (7 chars) of the worktree HEAD.
    Hash,
    /// Last commit age + subject.
    LastCommit,
```

- [ ] **Step 4: Update `all()`**

Add `ListColumn::Hash` between `Owner` and `LastCommit`:

```rust
    pub fn all() -> &'static [ListColumn] {
        &[
            ListColumn::Annotation,
            ListColumn::Branch,
            ListColumn::Path,
            ListColumn::Size,
            ListColumn::Base,
            ListColumn::Changes,
            ListColumn::Remote,
            ListColumn::Age,
            ListColumn::Owner,
            ListColumn::Hash,
            ListColumn::LastCommit,
        ]
    }
```

- [ ] **Step 5: Keep `list_defaults()` and `tui_defaults()` unchanged**

Hash is NOT added to defaults (like Size). No changes needed.

- [ ] **Step 6: Update `canonical_position()`**

```rust
    pub fn canonical_position(self) -> u8 {
        match self {
            Self::Annotation => 1,
            Self::Branch => 2,
            Self::Path => 3,
            Self::Size => 4,
            Self::Base => 5,
            Self::Changes => 6,
            Self::Remote => 7,
            Self::Age => 8,
            Self::Owner => 9,
            Self::Hash => 10,
            Self::LastCommit => 11,
        }
    }
```

- [ ] **Step 7: Update `cli_name()`**

Add `Self::Hash => "hash"` between Owner and LastCommit.

- [ ] **Step 8: Update `FromStr`**

Add `"hash" => Ok(Self::Hash)` between owner and last-commit cases.

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::columns::tests 2>&1 | tail -20`

Expected: All tests pass, including the 3 new ones.

- [ ] **Step 10: Commit**

```bash
git add src/core/columns.rs
git commit -m "feat: add Hash variant to ListColumn enum"
```

---

### Task 3: Add `Hash` to `SortColumn` enum

**Files:**

- Modify: `src/core/sort.rs:14-37` (SortColumn enum)
- Modify: `src/core/sort.rs:41-43` (valid_names)
- Modify: `src/core/sort.rs:49-61` (to_list_column)
- Modify: `src/core/sort.rs:65-78` (display_name)
- Modify: `src/core/sort.rs:81-99` (parse)
- Modify: `src/core/sort.rs:120-201` (SortKey::compare)
- Test: `src/core/sort.rs` (existing tests)

- [ ] **Step 1: Write failing test**

Add to the sort test module:

```rust
#[test]
fn test_hash_sort_parse() {
    let spec = SortSpec::parse("hash").unwrap();
    assert_eq!(spec.keys.len(), 1);
    assert_eq!(spec.keys[0].column, SortColumn::Hash);
    assert_eq!(spec.keys[0].direction, SortDirection::Ascending);
}

#[test]
fn test_hash_sort_descending() {
    let spec = SortSpec::parse("-hash").unwrap();
    assert_eq!(spec.keys[0].column, SortColumn::Hash);
    assert_eq!(spec.keys[0].direction, SortDirection::Descending);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib core::sort::tests 2>&1 | tail -20`

Expected: Compilation error — `SortColumn::Hash` does not exist.

- [ ] **Step 3: Add `Hash` variant to `SortColumn`**

Add between `Owner` and `Activity`:

```rust
    /// Sort by branch owner email (case-insensitive).
    Owner,
    /// Sort by abbreviated commit hash (lexicographic).
    Hash,
    /// Sort by overall activity: `max(last_commit_timestamp, working_tree_mtime)`.
    Activity,
```

- [ ] **Step 4: Update `valid_names()`**

```rust
    pub fn valid_names() -> &'static str {
        "branch, path, size, base, changes, remote, age, owner, hash, activity, commit (alias: last-commit)"
    }
```

- [ ] **Step 5: Update `to_list_column()`**

Add `Self::Hash => Some(ListColumn::Hash)`.

- [ ] **Step 6: Update `display_name()`**

Add `Self::Hash => "Hash"`.

- [ ] **Step 7: Update `parse()`**

Add `"hash" => Ok(Self::Hash)` in the match.

- [ ] **Step 8: Update `SortKey::compare()`**

Add a new arm before the `Activity` arm:

```rust
            SortColumn::Hash => {
                self.compare_optional(
                    a.last_commit_hash.as_deref(),
                    b.last_commit_hash.as_deref(),
                )
            }
```

- [ ] **Step 9: Run tests to verify they pass**

Run: `cargo test -p daft --lib core::sort::tests 2>&1 | tail -20`

Expected: All tests pass.

- [ ] **Step 10: Commit**

```bash
git add src/core/sort.rs
git commit -m "feat: add Hash variant to SortColumn enum"
```

---

### Task 4: Add `hash` to `ColumnValues` and rendering

**Files:**

- Modify: `src/output/format.rs:188-313` (ColumnValues struct +
  compute_column_values)
- Modify: `src/commands/list.rs:662-737` (CLI table header + data mapping)
- Modify: `src/commands/list.rs:293-414` (JSON output)
- Modify: `src/output/tui/columns.rs:8-248` (TUI Column enum + all methods)
- Modify: `src/output/tui/render.rs` (TUI cell rendering)

- [ ] **Step 1: Add `hash` field to `ColumnValues`**

In `src/output/format.rs`, add after `owner`:

```rust
    pub owner: String,
    pub hash: String,
    pub is_old_branch: bool,
```

- [ ] **Step 2: Populate `hash` in `compute_column_values`**

In the `compute_column_values` function, add after the `owner` computation:

```rust
    let owner = info.owner_email.clone().unwrap_or_default();

    let hash = info.last_commit_hash.clone().unwrap_or_default();

    ColumnValues {
        branch,
        path,
        size,
        base,
        changes,
        remote,
        branch_age,
        last_commit_age,
        last_commit_subject,
        owner,
        hash,
        is_old_branch,
        is_old_commit,
    }
```

- [ ] **Step 3: Add `hash` field to `TableRow` struct**

In `src/commands/list.rs`, the `TableRow` struct (~line 137-158) holds
pre-formatted values for CLI table rendering. Add a `hash` field after `owner`:

```rust
    /// Branch owner (git author email).
    owner: String,
    /// Abbreviated commit hash.
    hash: String,
    /// Last commit: shorthand age + subject combined.
    last_commit: String,
```

- [ ] **Step 4: Populate `hash` in both `TableRow` construction sites**

There are two `TableRow { ... }` blocks — one for dimmed non-worktree rows
(~line 602) and one for normal rows (~line 643). Add `hash` to both:

Dimmed path (~line 602):

```rust
                    hash: if vals.hash.is_empty() {
                        vals.hash.clone()
                    } else {
                        styles::dim(&vals.hash)
                    },
```

Normal path (~line 643):

```rust
                    hash: vals.hash.clone(),
```

- [ ] **Step 5: Add `Hash` to CLI table header mapping**

In `src/commands/list.rs`, in the header label match (~line 666-676), add:

```rust
                ListColumn::Hash => "Hash",
```

- [ ] **Step 6: Add `Hash` to CLI table data mapping**

In the column-to-data match (~line 727-737), add:

```rust
                ListColumn::Hash => row.hash.as_str(),
```

- [ ] **Step 7: Add `Hash` to JSON output**

In the JSON output section (~line 323), add after the Size block:

```rust
            if selected_columns.contains(&ListColumn::Hash) {
                obj.insert(
                    "hash".into(),
                    serde_json::json!(info.last_commit_hash),
                );
            }
```

- [ ] **Step 8: Add `Hash` to TUI `Column` enum**

In `src/output/tui/columns.rs`, add `Hash` variant between `Owner` and
`LastCommit`:

```rust
    /// Branch owner (from git author email). Priority 9.
    Owner,
    /// Abbreviated commit hash. Priority 10.
    Hash,
    /// Last commit subject. Priority 11.
    LastCommit,
```

- [ ] **Step 9: Update all TUI Column methods**

Update `priority()`: `Self::Hash => 10`, `Self::LastCommit => 11`

Update `label()`: `Self::Hash => "Hash"`

Update `from_list_column()`: `ListColumn::Hash => Column::Hash`

Update `to_list_column()`: `Self::Hash => Some(ListColumn::Hash)`

Update `ALL_COLUMNS`: Add `Column::Hash` before `Column::LastCommit`.

- [ ] **Step 10: Update `column_content_width`**

Add Hash arm in the match inside `column_content_width`:

```rust
            Column::Hash => 7,
```

(Abbreviated hash is always 7 chars.)

- [ ] **Step 11: Add Hash cell rendering in TUI**

In `src/output/tui/render.rs`, there are two match blocks that map columns to
cell content:

1. The styled cell rendering (~line 516, function that returns `Cell`): add
   after `Column::Owner`:

```rust
        Column::Hash => Cell::from(vals.hash.clone()),
```

2. The `column_plain_text` function (~line 734, returns `String`): add after
   `Column::Owner`:

```rust
        Column::Hash => vals.hash.clone(),
```

- [ ] **Step 12: Verify it compiles and tests pass**

Run: `cargo build && mise run test:unit`

Expected: Build succeeds, all unit tests pass.

- [ ] **Step 13: Commit**

```bash
git add src/output/format.rs src/commands/list.rs src/output/tui/columns.rs src/output/tui/render.rs
git commit -m "feat: add hash column to CLI table and TUI rendering"
```

---

### Task 5: Add gitoxide commit metadata functions

**Files:**

- Modify: `src/git/oxide.rs` (add new functions)
- Modify: `src/core/worktree/list.rs` (integrate gitoxide path)

- [ ] **Step 1: Add `get_commit_metadata_for_ref` to oxide.rs**

In `src/git/oxide.rs`, add a new function:

```rust
/// Gitoxide-native commit metadata retrieval for a named ref.
///
/// Returns `(unix_timestamp, abbreviated_hash, subject)` for the tip commit
/// of the given ref. Faster than spawning `git log` subprocess.
pub fn get_commit_metadata_for_ref(
    repo: &Repository,
    ref_name: &str,
) -> Result<(i64, String, String)> {
    let reference = repo
        .find_reference(ref_name)
        .with_context(|| format!("Failed to find ref '{ref_name}'"))?;
    let commit = reference
        .peel_to_commit()
        .with_context(|| format!("Failed to peel '{ref_name}' to commit"))?;

    let timestamp = commit.time()?.seconds;
    let hash = commit.id().to_hex().to_string();
    let short_hash = hash[..7.min(hash.len())].to_string();
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();

    Ok((timestamp, short_hash, subject))
}

/// Gitoxide-native commit metadata retrieval for a worktree HEAD.
///
/// Opens the repo at the given worktree path and reads HEAD's commit.
/// Returns `(unix_timestamp, abbreviated_hash, subject)`.
pub fn get_commit_metadata_for_head(worktree_path: &std::path::Path) -> Result<(i64, String, String)> {
    let repo = gix::open(worktree_path)
        .with_context(|| format!("Failed to open repo at {}", worktree_path.display()))?;
    let commit = repo
        .head_commit()
        .context("Failed to read HEAD commit")?;

    let timestamp = commit.time()?.seconds;
    let hash = commit.id().to_hex().to_string();
    let short_hash = hash[..7.min(hash.len())].to_string();
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();

    Ok((timestamp, short_hash, subject))
}
```

- [ ] **Step 2: Add `&GitCommand` parameter to `refresh_dynamic_fields`**

In `src/core/worktree/list.rs`, update the method signature:

```rust
    pub fn refresh_dynamic_fields(&mut self, base_branch: &str, stat: Stat, git: &GitCommand) {
```

- [ ] **Step 3: Create a helper that dispatches to gitoxide or subprocess**

Add a helper function in `src/core/worktree/list.rs`:

```rust
/// Get commit metadata (timestamp, hash, subject) for a worktree HEAD.
/// Uses gitoxide when enabled, falls back to subprocess.
fn get_commit_metadata(
    worktree_path: &Path,
    git: &GitCommand,
) -> (Option<i64>, Option<String>, String) {
    if git.use_gitoxide {
        if let Ok((ts, hash, subj)) =
            crate::git::oxide::get_commit_metadata_for_head(worktree_path)
        {
            return (Some(ts), Some(hash), subj);
        }
    }
    get_last_commit_info(worktree_path)
}

/// Get commit metadata (timestamp, hash, subject) for a named ref.
/// Uses gitoxide when enabled, falls back to subprocess.
fn get_commit_metadata_for_ref_dispatched(
    branch_ref: &str,
    cwd: &Path,
    git: &GitCommand,
) -> (Option<i64>, Option<String>, String) {
    if git.use_gitoxide {
        if let Ok(repo) = git.gix_repo() {
            let full_ref = if branch_ref.starts_with("refs/") {
                branch_ref.to_string()
            } else {
                format!("refs/heads/{branch_ref}")
            };
            if let Ok((ts, hash, subj)) =
                crate::git::oxide::get_commit_metadata_for_ref(&repo, &full_ref)
            {
                return (Some(ts), Some(hash), subj);
            }
        }
    }
    get_last_commit_info_for_ref(branch_ref, cwd)
}
```

- [ ] **Step 4: Update call sites to use dispatched helpers**

Replace direct `get_last_commit_info` calls with the dispatched versions,
threading `git` through:

1. `refresh_dynamic_fields`: `get_commit_metadata(path, git)`
2. `collect_worktree_info`: `get_commit_metadata(&entry.path, git)`
3. `collect_branch_info` (local):
   `get_commit_metadata_for_ref_dispatched(branch, cwd, git)`
4. `collect_branch_info` (remote):
   `get_commit_metadata_for_ref_dispatched(remote_branch, cwd, git)`

- [ ] **Step 5: Update callers of `refresh_dynamic_fields`**

There are 3 call sites in `src/commands/sync.rs` (lines 747, 772, 792), all
inside closures that have `orch_settings` in scope. For each, construct a
`GitCommand` inline:

```rust
let git = GitCommand::new(false).with_gitoxide(orch_settings.use_gitoxide);
refreshed.refresh_dynamic_fields(&orch_base_branch, orch_stat, &git);
```

This matches the existing pattern at line 647 of sync.rs where a `GitCommand` is
already constructed from `orch_settings` inside the same orchestrator closure.

- [ ] **Step 6: Verify it compiles and tests pass**

Run: `cargo build && mise run test:unit`

Expected: Build succeeds, all unit tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/git/oxide.rs src/core/worktree/list.rs
git commit -m "feat: add gitoxide fast path for commit metadata retrieval"
```

---

### Task 6: Update shell completions

**Files:**

- Modify: `src/commands/completions/bash.rs:49` (column list)
- Modify: `src/commands/completions/zsh.rs:55-128` (column + sort lists)
- Modify: `src/commands/completions/fish.rs:108-145` (column + sort arrays)
- Modify: `src/commands/completions/fig.rs:200-245` (column + sort arrays)

- [ ] **Step 1: Update bash completions**

In `src/commands/completions/bash.rs`, the columns string (~line 49):

```
annotation branch path size base changes remote age owner last-commit
```

becomes:

```
annotation branch path size base changes remote age owner hash last-commit
```

Bash only has column completions (no sort completions).

- [ ] **Step 2: Update zsh completions**

In `src/commands/completions/zsh.rs`, add `hash` entries in all three sections:

Column names (after owner, before last-commit):

```
            'hash:Commit hash'\n
```

Column +modifiers (after +owner, before +last-commit):

```
            '+hash:Add commit hash'\n
```

Column -modifiers (after -owner, before -last-commit):

```
            '-hash:Remove commit hash'\n
```

Sort names (after owner, before activity):

```
            'hash:Sort by commit hash'\n
```

Sort +modifiers (after +owner, before +activity):

```
            '+hash:Sort by commit hash ascending'\n
```

Sort -modifiers (after -owner, before -activity):

```
            '-hash:Sort by commit hash descending'\n
```

- [ ] **Step 3: Update fish completions**

In `src/commands/completions/fish.rs`, add to the columns array (after owner,
before last-commit):

```rust
            ("hash", "Commit hash"),
```

Add to the sort array (after owner, before activity):

```rust
            ("hash", "Sort by commit hash"),
```

- [ ] **Step 4: Update fig completions**

In `src/commands/completions/fig.rs`, add to the column_defs array (after owner,
before last-commit):

```rust
                    ("hash", "Commit hash"),
```

Add to the sort_defs array (after owner, before activity):

```rust
                    ("hash", "Sort by commit hash"),
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`

Expected: Build succeeds.

- [ ] **Step 6: Commit**

```bash
git add src/commands/completions/
git commit -m "feat: add hash to shell completion definitions"
```

---

### Task 7: Add YAML manual test scenarios

**Files:**

- Create: `tests/manual/scenarios/list/hash-column.yml`

- [ ] **Step 1: Create the test scenario file**

```yaml
name: Hash column
description: --columns +hash shows abbreviated commit hash

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Add hash column with modifier
    run: NO_COLOR=1 git-worktree-list --columns=+hash 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Hash"
        - "Branch"
        - "Commit"
      output_matches:
        - "[0-9a-f]{7}"

  - name: Hash column in replace mode
    run: NO_COLOR=1 git-worktree-list --columns=branch,hash 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Hash"
        - "Branch"
      output_not_contains:
        - "Path"
        - "Age"
      output_matches:
        - "[0-9a-f]{7}"

  - name: Sort by hash
    run: NO_COLOR=1 git-worktree-list --columns=+hash --sort=hash 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Hash"
```

- [ ] **Step 2: Run the test scenario**

Run: `mise run test:manual -- --ci list:hash-column`

Expected: All steps pass.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/list/hash-column.yml
git commit -m "test: add YAML manual test scenario for hash column"
```

---

### Task 8: Update help text, man pages, and docs

**Files:**

- Modify: `src/commands/list.rs` (--columns help text)
- Modify: `src/commands/prune.rs` (--columns help text)
- Modify: `src/commands/sync.rs` (--columns help text)
- Regenerate: `man/` (man pages)

- [ ] **Step 1: Update --columns help text**

Find the `--columns` flag help text in `src/commands/list.rs`,
`src/commands/prune.rs`, and `src/commands/sync.rs`. Add `hash` to the list of
available column names in the help string. Search for the `about = "` or
`help = "` attribute on the columns field.

- [ ] **Step 2: Update --sort help text if it lists column names**

Check if the `--sort` flag help text explicitly lists sortable columns. If so,
add `hash`.

- [ ] **Step 3: Regenerate man pages**

Run: `mise run man:gen`

Expected: Man pages regenerated in `man/`.

- [ ] **Step 4: Verify man pages are up to date**

Run: `mise run man:verify`

Expected: Verification passes.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy`

Expected: No warnings, no formatting issues.

- [ ] **Step 6: Run full test suite**

Run: `mise run test`

Expected: All tests pass (unit + integration).

- [ ] **Step 7: Commit**

```bash
git add src/commands/list.rs src/commands/prune.rs src/commands/sync.rs man/
git commit -m "docs: update help text and man pages for hash column"
```
