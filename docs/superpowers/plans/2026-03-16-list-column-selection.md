# Column Selection Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development
> (if subagents available) or superpowers:executing-plans to implement this
> plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `--columns` flag to list, sync, and prune commands that controls
which columns are displayed and in what order, with replace mode and +/-
modifier mode.

**Architecture:** A shared `ListColumn` enum defines all available columns. A
new `ColumnSelection` type encapsulates parsing and resolving the
comma-separated column spec (replace vs. modifier mode). Settings, CLI args, and
rendering all consume `ResolvedColumns` (which tracks the mode) as the resolved
column list.

**Tech Stack:** Rust, clap, tabled (list table), ratatui (TUI table), serde_json
(JSON filtering)

---

## Chunk 1: Core Column Types and Parsing

### Task 1: Define the shared `ListColumn` enum

The existing `Column` enum in `src/output/tui/columns.rs` is TUI-specific (has
`Status` variant, priority-based sizing). We need a separate user-facing enum
for the `--columns` flag that maps CLI names to column identifiers.

**Files:**

- Create: `src/core/columns.rs`
- Modify: `src/core/mod.rs`

- [ ] **Step 1: Write tests for ListColumn parsing and display**

In `src/core/columns.rs`, add the enum and tests at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_column_cli_names() {
        assert_eq!("annotation".parse::<ListColumn>().unwrap(), ListColumn::Annotation);
        assert_eq!("branch".parse::<ListColumn>().unwrap(), ListColumn::Branch);
        assert_eq!("path".parse::<ListColumn>().unwrap(), ListColumn::Path);
        assert_eq!("base".parse::<ListColumn>().unwrap(), ListColumn::Base);
        assert_eq!("remote".parse::<ListColumn>().unwrap(), ListColumn::Remote);
        assert_eq!("changes".parse::<ListColumn>().unwrap(), ListColumn::Changes);
        assert_eq!("age".parse::<ListColumn>().unwrap(), ListColumn::Age);
        assert_eq!("last-commit".parse::<ListColumn>().unwrap(), ListColumn::LastCommit);
    }

    #[test]
    fn test_list_column_display_roundtrip() {
        for col in ListColumn::all() {
            let name = col.cli_name();
            assert_eq!(name.parse::<ListColumn>().unwrap(), *col);
        }
    }

    #[test]
    fn test_list_column_canonical_position() {
        let all = ListColumn::all();
        for (i, col) in all.iter().enumerate() {
            assert!(
                i == 0 || col.canonical_position() > all[i - 1].canonical_position(),
                "Columns must be in ascending canonical position order"
            );
        }
    }

    #[test]
    fn test_unknown_column_parse_fails() {
        assert!("unknown".parse::<ListColumn>().is_err());
        assert!("status".parse::<ListColumn>().is_err());
        assert!("".parse::<ListColumn>().is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib core::columns` Expected: compilation error — module and
types don't exist yet.

- [ ] **Step 3: Implement ListColumn enum**

Create `src/core/columns.rs`:

```rust
//! Column definitions for the `--columns` flag on list/sync/prune commands.
//!
//! Defines the user-facing column names, canonical ordering, and per-command
//! default sets. Separate from the TUI `Column` enum which includes TUI-only
//! variants like `Status`.

use std::fmt;
use std::str::FromStr;

/// A column that can be selected via `--columns` on list, sync, and prune.
///
/// Variants are ordered by canonical display position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListColumn {
    /// Current/default branch annotation markers (>, ◉).
    Annotation,
    /// Branch name.
    Branch,
    /// Worktree path.
    Path,
    /// Ahead/behind base branch.
    Base,
    /// Ahead/behind remote tracking branch.
    Remote,
    /// Local changes (staged/unstaged/untracked).
    Changes,
    /// Branch age since creation.
    Age,
    /// Last commit age + subject.
    LastCommit,
}

impl ListColumn {
    /// All columns in canonical display order.
    pub fn all() -> &'static [ListColumn] {
        &[
            ListColumn::Annotation,
            ListColumn::Branch,
            ListColumn::Path,
            ListColumn::Base,
            ListColumn::Remote,
            ListColumn::Changes,
            ListColumn::Age,
            ListColumn::LastCommit,
        ]
    }

    /// The default column set for the list command.
    pub fn list_defaults() -> &'static [ListColumn] {
        Self::all()
    }

    /// The default column set for sync and prune commands.
    /// (Status is pinned separately by TUI code, not included here.)
    pub fn tui_defaults() -> &'static [ListColumn] {
        Self::all()
    }

    /// Canonical display position (used to order columns in modifier mode).
    pub fn canonical_position(self) -> u8 {
        match self {
            Self::Annotation => 1,
            Self::Branch => 2,
            Self::Path => 3,
            Self::Base => 4,
            Self::Remote => 5,
            Self::Changes => 6,
            Self::Age => 7,
            Self::LastCommit => 8,
        }
    }

    /// CLI-facing name for this column.
    pub fn cli_name(self) -> &'static str {
        match self {
            Self::Annotation => "annotation",
            Self::Branch => "branch",
            Self::Path => "path",
            Self::Base => "base",
            Self::Remote => "remote",
            Self::Changes => "changes",
            Self::Age => "age",
            Self::LastCommit => "last-commit",
        }
    }

    /// All valid CLI column names, for use in error messages.
    pub fn valid_names() -> String {
        Self::all()
            .iter()
            .map(|c| c.cli_name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for ListColumn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.cli_name())
    }
}

impl FromStr for ListColumn {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "annotation" => Ok(Self::Annotation),
            "branch" => Ok(Self::Branch),
            "path" => Ok(Self::Path),
            "base" => Ok(Self::Base),
            "remote" => Ok(Self::Remote),
            "changes" => Ok(Self::Changes),
            "age" => Ok(Self::Age),
            "last-commit" => Ok(Self::LastCommit),
            _ => Err(format!(
                "unknown column '{}'\n  valid columns: {}",
                s.trim(),
                Self::valid_names()
            )),
        }
    }
}
```

- [ ] **Step 4: Register the module**

In `src/core/mod.rs`, add:

```rust
pub mod columns;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib core::columns` Expected: all 4 tests pass.

- [ ] **Step 6: Run clippy and fmt**

Run: `mise run fmt && mise run clippy` Expected: no warnings, no formatting
changes.

- [ ] **Step 7: Commit**

```bash
git add src/core/columns.rs src/core/mod.rs
git commit -m "feat: add ListColumn enum for column selection"
```

---

### Task 2: Implement ColumnSelection parser

Parses the `--columns` value string into a resolved `Vec<ListColumn>`. Handles
replace mode, modifier mode (+/-), mixed-mode error, and all validation.

**Files:**

- Modify: `src/core/columns.rs`

- [ ] **Step 1: Write tests for ColumnSelection parsing**

Add to the `tests` module in `src/core/columns.rs`:

```rust
    // ── ColumnSelection tests ──

    #[test]
    fn test_replace_mode() {
        let resolved = ColumnSelection::parse("branch,path,age", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, vec![ListColumn::Branch, ListColumn::Path, ListColumn::Age]);
        assert!(resolved.explicit);
    }

    #[test]
    fn test_replace_mode_custom_order() {
        let resolved = ColumnSelection::parse("age,branch,path", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, vec![ListColumn::Age, ListColumn::Branch, ListColumn::Path]);
        assert!(resolved.explicit);
    }

    #[test]
    fn test_modifier_mode_subtract() {
        let resolved = ColumnSelection::parse("-annotation,-last-commit", CommandKind::List).unwrap();
        let expected: Vec<ListColumn> = ListColumn::list_defaults()
            .iter()
            .copied()
            .filter(|c| *c != ListColumn::Annotation && *c != ListColumn::LastCommit)
            .collect();
        assert_eq!(resolved.columns, expected);
        assert!(!resolved.explicit);
    }

    #[test]
    fn test_modifier_mode_canonical_order() {
        // Order of modifiers doesn't affect result order
        let r1 = ColumnSelection::parse("-last-commit,-annotation", CommandKind::List).unwrap();
        let r2 = ColumnSelection::parse("-annotation,-last-commit", CommandKind::List).unwrap();
        assert_eq!(r1.columns, r2.columns);
    }

    #[test]
    fn test_modifier_add_idempotent() {
        let resolved = ColumnSelection::parse("+branch,+path", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, ListColumn::list_defaults().to_vec());
    }

    #[test]
    fn test_modifier_subtract_nondefault_idempotent() {
        // Subtracting a column that's not in defaults is silently ignored
        let resolved = ColumnSelection::parse("-annotation", CommandKind::List).unwrap();
        assert!(!resolved.columns.contains(&ListColumn::Annotation));
    }

    #[test]
    fn test_mixed_mode_error() {
        let err = ColumnSelection::parse("branch,+age", CommandKind::List).unwrap_err();
        assert!(err.contains("cannot mix"), "Got: {err}");
    }

    #[test]
    fn test_empty_result_error() {
        let all_minus: String = ListColumn::all()
            .iter()
            .map(|c| format!("-{}", c.cli_name()))
            .collect::<Vec<_>>()
            .join(",");
        let err = ColumnSelection::parse(&all_minus, CommandKind::List).unwrap_err();
        assert!(err.contains("no columns remaining"), "Got: {err}");
    }

    #[test]
    fn test_unknown_column_error() {
        let err = ColumnSelection::parse("branch,foo", CommandKind::List).unwrap_err();
        assert!(err.contains("unknown column 'foo'"), "Got: {err}");
    }

    #[test]
    fn test_duplicate_replace_error() {
        let err = ColumnSelection::parse("branch,path,branch", CommandKind::List).unwrap_err();
        assert!(err.contains("duplicate"), "Got: {err}");
    }

    #[test]
    fn test_duplicate_modifier_idempotent() {
        let resolved = ColumnSelection::parse("+branch,+branch", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, ListColumn::list_defaults().to_vec());
    }

    #[test]
    fn test_whitespace_trimmed() {
        let resolved = ColumnSelection::parse("branch , path , age", CommandKind::List).unwrap();
        assert_eq!(resolved.columns, vec![ListColumn::Branch, ListColumn::Path, ListColumn::Age]);
    }

    #[test]
    fn test_status_on_sync_errors() {
        let err = ColumnSelection::parse("status,branch", CommandKind::Sync).unwrap_err();
        assert!(err.contains("cannot be controlled"), "Got: {err}");
    }

    #[test]
    fn test_status_modifier_on_prune_errors() {
        let err = ColumnSelection::parse("+status", CommandKind::Prune).unwrap_err();
        assert!(err.contains("cannot be controlled"), "Got: {err}");
    }

    #[test]
    fn test_status_on_list_unknown() {
        let err = ColumnSelection::parse("status,branch", CommandKind::List).unwrap_err();
        assert!(err.contains("unknown column 'status'"), "Got: {err}");
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib core::columns` Expected: compilation errors —
`ColumnSelection` and `CommandKind` don't exist.

- [ ] **Step 3: Implement ColumnSelection and CommandKind**

Add to `src/core/columns.rs`, above the `tests` module:

```rust
/// Which command is requesting column selection. Affects validation rules
/// (e.g., `status` is pinned on Sync/Prune but unavailable on List).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    List,
    Sync,
    Prune,
}

/// The resolved column list with mode information.
///
/// Tracks whether the user used replace mode (explicit column set) or modifier
/// mode (add/remove from defaults). The TUI uses this to decide whether
/// responsive column dropping is allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedColumns {
    pub columns: Vec<ListColumn>,
    /// True if the user explicitly chose columns (replace mode).
    /// False if they used modifier mode or if defaults are used.
    pub explicit: bool,
}

impl ResolvedColumns {
    pub fn defaults(defaults: &[ListColumn]) -> Self {
        Self {
            columns: defaults.to_vec(),
            explicit: false,
        }
    }
}

/// Parses and resolves a `--columns` value into a concrete column list.
pub struct ColumnSelection;

impl ColumnSelection {
    /// Parse a comma-separated column spec into a resolved column list.
    ///
    /// Supports three modes:
    /// - Replace: `branch,path,age` — exact columns, exact order
    /// - Modifier: `+col,-col` — add/remove from defaults, canonical order
    /// - Mixed: error
    pub fn parse(input: &str, command: CommandKind) -> Result<ResolvedColumns, String> {
        let tokens: Vec<&str> = input.split(',').map(|s| s.trim()).collect();
        if tokens.is_empty() || tokens.iter().all(|t| t.is_empty()) {
            return Err("no columns specified".to_string());
        }

        let has_modifier = tokens.iter().any(|t| t.starts_with('+') || t.starts_with('-'));
        let has_plain = tokens.iter().any(|t| !t.starts_with('+') && !t.starts_with('-') && !t.is_empty());

        if has_modifier && has_plain {
            return Err(
                "cannot mix column names with +/- modifiers\n  \
                 use either replace mode:   --columns branch,path,age\n  \
                 or modifier mode:          --columns -annotation,-remote"
                    .to_string(),
            );
        }

        if has_modifier {
            let columns = Self::parse_modifier(&tokens, command)?;
            Ok(ResolvedColumns { columns, explicit: false })
        } else {
            let columns = Self::parse_replace(&tokens, command)?;
            Ok(ResolvedColumns { columns, explicit: true })
        }
    }

    fn parse_replace(tokens: &[&str], command: CommandKind) -> Result<Vec<ListColumn>, String> {
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            Self::check_status_token(token, command)?;
            let col: ListColumn = token.parse()?;
            if !seen.insert(col) {
                return Err(format!("duplicate column '{}'", col.cli_name()));
            }
            result.push(col);
        }

        if result.is_empty() {
            return Err("no columns specified".to_string());
        }

        Ok(result)
    }

    fn parse_modifier(tokens: &[&str], command: CommandKind) -> Result<Vec<ListColumn>, String> {
        let defaults = match command {
            CommandKind::List => ListColumn::list_defaults(),
            CommandKind::Sync | CommandKind::Prune => ListColumn::tui_defaults(),
        };
        let mut active: std::collections::HashSet<ListColumn> = defaults.iter().copied().collect();

        for token in tokens {
            if token.is_empty() {
                continue;
            }
            let (prefix, name) = if let Some(rest) = token.strip_prefix('+') {
                ('+', rest)
            } else if let Some(rest) = token.strip_prefix('-') {
                ('-', rest)
            } else {
                // Should not reach here due to mode detection, but handle gracefully
                return Err(format!("expected +/- prefix on '{token}'"));
            };

            Self::check_status_token(name, command)?;
            let col: ListColumn = name.parse()?;

            match prefix {
                '+' => { active.insert(col); }
                '-' => { active.remove(&col); }
                _ => unreachable!(),
            }
        }

        if active.is_empty() {
            let modifiers = tokens.join(",");
            return Err(format!(
                "no columns remaining after applying modifiers\n  modifiers: {modifiers}"
            ));
        }

        // Return in canonical order
        let mut result: Vec<ListColumn> = active.into_iter().collect();
        result.sort_by_key(|c| c.canonical_position());
        Ok(result)
    }

    /// Check if the token is "status" and produce the appropriate error.
    fn check_status_token(name: &str, command: CommandKind) -> Result<(), String> {
        if name.trim().to_lowercase() == "status" {
            match command {
                CommandKind::List => {
                    // Falls through to unknown column error via FromStr
                }
                CommandKind::Sync | CommandKind::Prune => {
                    return Err(
                        "'status' column cannot be controlled on this command\n  \
                         it is always shown as the first column"
                            .to_string(),
                    );
                }
            }
        }
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib core::columns` Expected: all tests pass.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy` Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/core/columns.rs
git commit -m "feat: add ColumnSelection parser with replace and modifier modes"
```

---

### Task 3: Add column config to settings

Add `daft.list.columns`, `daft.sync.columns`, `daft.prune.columns` config keys
to `DaftSettings`.

**Files:**

- Modify: `src/core/settings.rs`

- [ ] **Step 1: Write test for column config defaults**

Add to the `tests` module in `src/core/settings.rs`:

```rust
    #[test]
    fn test_default_column_settings() {
        let settings = DaftSettings::default();
        assert!(settings.list_columns.is_none());
        assert!(settings.sync_columns.is_none());
        assert!(settings.prune_columns.is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib core::settings::tests::test_default_column_settings`
Expected: compilation error — fields don't exist.

- [ ] **Step 3: Add config keys, defaults, and fields**

In `src/core/settings.rs`:

Add to `keys` module (after `PRUNE_STAT`):

```rust
    /// Config key for list.columns setting.
    pub const LIST_COLUMNS: &str = "daft.list.columns";

    /// Config key for sync.columns setting.
    pub const SYNC_COLUMNS: &str = "daft.sync.columns";

    /// Config key for prune.columns setting.
    pub const PRUNE_COLUMNS: &str = "daft.prune.columns";
```

Add fields to `DaftSettings` struct (after `prune_stat`):

```rust
    /// Column selection for list command (None = use defaults).
    pub list_columns: Option<String>,

    /// Column selection for sync command (None = use defaults).
    pub sync_columns: Option<String>,

    /// Column selection for prune command (None = use defaults).
    pub prune_columns: Option<String>,
```

Add to `Default` impl (after `prune_stat`):

```rust
            list_columns: None,
            sync_columns: None,
            prune_columns: None,
```

Add to `load()` method (after the prune_stat block):

```rust
        if let Some(value) = git.config_get(keys::LIST_COLUMNS)? {
            if !value.is_empty() {
                settings.list_columns = Some(value);
            }
        }

        if let Some(value) = git.config_get(keys::SYNC_COLUMNS)? {
            if !value.is_empty() {
                settings.sync_columns = Some(value);
            }
        }

        if let Some(value) = git.config_get(keys::PRUNE_COLUMNS)? {
            if !value.is_empty() {
                settings.prune_columns = Some(value);
            }
        }
```

Add the same 3 blocks to `load_global()` using `config_get_global`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib core::settings::tests::test_default_column_settings`
Expected: PASS.

- [ ] **Step 5: Run clippy and fmt**

Run: `mise run fmt && mise run clippy` Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add src/core/settings.rs
git commit -m "feat: add list/sync/prune column config keys to settings"
```

---

## Chunk 2: List Command Integration

### Task 4: Add `--columns` flag to list command

Wire the `--columns` flag into `git-worktree-list` and resolve it against
settings.

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Add the `--columns` flag to Args**

In `src/commands/list.rs`, add to the `Args` struct (after the `stat` field):

```rust
    #[arg(
        long,
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col"
    )]
    columns: Option<String>,
```

- [ ] **Step 2: Resolve columns in `run()`**

In `run()`, after `let stat = args.stat.unwrap_or(settings.list_stat);`, add:

```rust
    let columns_input = args.columns.or(settings.list_columns);
    let resolved = match columns_input {
        Some(ref input) => {
            ColumnSelection::parse(input, CommandKind::List).map_err(|e| anyhow::anyhow!("{e}"))?
        }
        None => ResolvedColumns::defaults(ListColumn::list_defaults()),
    };
    let selected_columns = &resolved.columns;
```

Add the necessary imports at the top of the file:

```rust
use crate::core::columns::{ColumnSelection, CommandKind, ListColumn, ResolvedColumns};
```

- [ ] **Step 3: Pass `selected_columns` to `print_table` and `print_json`**

Update the function signatures and calls:

```rust
    if args.json {
        return print_json(&infos, &project_root, &cwd, stat, &selected_columns);
    }

    print_table(&infos, &project_root, &cwd, stat, &selected_columns);
```

Update `print_table` signature:

```rust
fn print_table(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
    selected_columns: &[ListColumn],
) {
```

Update `print_json` signature:

```rust
fn print_json(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
    selected_columns: &[ListColumn],
) -> Result<()> {
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build` Expected: compiles (columns not yet used inside functions,
just threaded through).

- [ ] **Step 5: Commit**

```bash
git add src/commands/list.rs
git commit -m "feat: add --columns flag to list command"
```

---

### Task 5: Filter table output by selected columns

Modify `print_table` to dynamically build columns based on `selected_columns`.

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Write a YAML test for replace mode**

Create `tests/manual/scenarios/list/columns-replace.yml`:

```yaml
name: Columns replace mode
description: --columns flag selects exact columns in specified order

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: List with subset of columns
    run: NO_COLOR=1 git-worktree-list --columns branch,path 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Branch"
        - "Path"
      output_excludes:
        - "Base"
        - "Changes"
        - "Remote"
        - "Age"
        - "Last Commit"

  - name: List with reversed column order
    run: NO_COLOR=1 git-worktree-list --columns age,branch 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Age"
        - "Branch"
```

- [ ] **Step 2: Implement table column filtering**

In `print_table`, replace the static header/row building with dynamic logic
driven by `selected_columns`. The key changes:

Replace the `data_headers` and header-building block (the section starting at
`let data_headers = [` through `builder.push_record(header);`) with:

```rust
    // Build header from selected columns
    let col_headers: Vec<(&str, ListColumn)> = selected_columns
        .iter()
        .filter(|c| **c != ListColumn::Annotation)
        .map(|c| {
            let label = match c {
                ListColumn::Branch => "Branch",
                ListColumn::Path => "Path",
                ListColumn::Base => "Base",
                ListColumn::Remote => "Remote",
                ListColumn::Changes => "Changes",
                ListColumn::Age => "Age",
                ListColumn::LastCommit => "Last Commit",
                ListColumn::Annotation => unreachable!(),
            };
            (label, *c)
        })
        .collect();

    let show_annotations = selected_columns.contains(&ListColumn::Annotation)
        && (has_any_current || has_any_default);

    let header: Vec<String> = if show_annotations {
        std::iter::once("".to_string())
            .chain(col_headers.iter().map(|(h, _)| {
                if use_color { styles::dim(h) } else { h.to_string() }
            }))
            .collect()
    } else {
        col_headers
            .iter()
            .map(|(h, _)| {
                if use_color { styles::dim(h) } else { h.to_string() }
            })
            .collect()
    };
    builder.push_record(header);
```

Replace the row-building loop (the section starting at `for row in &rows {`
through the closing `}` of the loop) with:

```rust
    for row in &rows {
        let data_cols: Vec<&str> = col_headers
            .iter()
            .map(|(_, c)| match c {
                ListColumn::Branch => row.name.as_str(),
                ListColumn::Path => row.path.as_str(),
                ListColumn::Base => row.base.as_str(),
                ListColumn::Remote => row.remote.as_str(),
                ListColumn::Changes => row.head.as_str(),
                ListColumn::Age => row.branch_age.as_str(),
                ListColumn::LastCommit => row.last_commit.as_str(),
                ListColumn::Annotation => unreachable!(),
            })
            .collect();
        if show_annotations {
            let mut record = vec![row.annotation.as_str()];
            record.extend(data_cols);
            builder.push_record(record);
        } else {
            builder.push_record(data_cols);
        }
    }
```

- [ ] **Step 3: Run the YAML test**

Run: `mise run test:manual -- --ci list:columns-replace` Expected: PASS.

- [ ] **Step 4: Run full unit tests**

Run: `mise run test:unit` Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src/commands/list.rs tests/manual/scenarios/list/columns-replace.yml
git commit -m "feat: filter list table output by selected columns"
```

---

### Task 6: Filter JSON output by selected columns

Modify `print_json` to only include keys matching the selected columns.

**Files:**

- Modify: `src/commands/list.rs`

- [ ] **Step 1: Write a YAML test for JSON column filtering**

Create `tests/manual/scenarios/list/columns-json.yml`:

```yaml
name: Columns JSON filtering
description: --columns flag filters JSON output keys

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: JSON with subset of columns
    run: git-worktree-list --json --columns branch,path 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - '"name"'
        - '"path"'
        - '"kind"'
      output_excludes:
        - '"ahead"'
        - '"behind"'
        - '"staged"'
        - '"remote_ahead"'
        - '"branch_age"'
        - '"last_commit_age"'
        - '"is_current"'

  - name: JSON without --columns shows all fields
    run: git-worktree-list --json 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - '"name"'
        - '"ahead"'
        - '"staged"'
        - '"branch_age"'
```

- [ ] **Step 2: Implement JSON filtering**

In `print_json`, replace the entry-building logic. Instead of unconditionally
adding all keys, conditionally add them based on `selected_columns`:

```rust
fn print_json(
    infos: &[crate::core::worktree::list::WorktreeInfo],
    project_root: &std::path::Path,
    cwd: &std::path::Path,
    stat: Stat,
    selected_columns: &[ListColumn],
) -> Result<()> {
    let now = Utc::now().timestamp();
    let all_columns = selected_columns == ListColumn::list_defaults();

    let entries: Vec<serde_json::Value> = infos
        .iter()
        .map(|info| {
            let mut obj = serde_json::Map::new();

            if all_columns || selected_columns.contains(&ListColumn::Branch) {
                obj.insert("kind".into(), serde_json::json!(match info.kind {
                    EntryKind::Worktree => "worktree",
                    EntryKind::LocalBranch => "branch",
                    EntryKind::RemoteBranch => "remote",
                }));
                obj.insert("name".into(), serde_json::json!(info.name));
            }

            if all_columns || selected_columns.contains(&ListColumn::Annotation) {
                obj.insert("is_current".into(), serde_json::json!(info.is_current));
                obj.insert("is_default_branch".into(), serde_json::json!(info.is_default_branch));
            }

            if all_columns || selected_columns.contains(&ListColumn::Path) {
                let rel_path = info
                    .path
                    .as_ref()
                    .map(|p| relative_display_path(p, project_root, cwd));
                obj.insert("path".into(), serde_json::json!(rel_path));
            }

            if all_columns || selected_columns.contains(&ListColumn::Base) {
                obj.insert("ahead".into(), serde_json::json!(info.ahead));
                obj.insert("behind".into(), serde_json::json!(info.behind));
                if stat == Stat::Lines {
                    obj.insert("base_lines_inserted".into(), serde_json::json!(info.base_lines_inserted));
                    obj.insert("base_lines_deleted".into(), serde_json::json!(info.base_lines_deleted));
                }
            }

            if all_columns || selected_columns.contains(&ListColumn::Changes) {
                obj.insert("staged".into(), serde_json::json!(info.staged));
                obj.insert("unstaged".into(), serde_json::json!(info.unstaged));
                obj.insert("untracked".into(), serde_json::json!(info.untracked));
                if stat == Stat::Lines {
                    obj.insert("staged_lines_inserted".into(), serde_json::json!(info.staged_lines_inserted));
                    obj.insert("staged_lines_deleted".into(), serde_json::json!(info.staged_lines_deleted));
                    obj.insert("unstaged_lines_inserted".into(), serde_json::json!(info.unstaged_lines_inserted));
                    obj.insert("unstaged_lines_deleted".into(), serde_json::json!(info.unstaged_lines_deleted));
                }
            }

            if all_columns || selected_columns.contains(&ListColumn::Remote) {
                obj.insert("remote_ahead".into(), serde_json::json!(info.remote_ahead));
                obj.insert("remote_behind".into(), serde_json::json!(info.remote_behind));
                if stat == Stat::Lines {
                    obj.insert("remote_lines_inserted".into(), serde_json::json!(info.remote_lines_inserted));
                    obj.insert("remote_lines_deleted".into(), serde_json::json!(info.remote_lines_deleted));
                }
            }

            if all_columns || selected_columns.contains(&ListColumn::Age) {
                let branch_age = info
                    .branch_creation_timestamp
                    .map(|ts| shorthand_from_seconds(now - ts))
                    .unwrap_or_default();
                obj.insert("branch_age".into(), serde_json::json!(branch_age));
            }

            if all_columns || selected_columns.contains(&ListColumn::LastCommit) {
                let last_commit_age = info
                    .last_commit_timestamp
                    .map(|ts| shorthand_from_seconds(now - ts))
                    .unwrap_or_default();
                obj.insert("last_commit_age".into(), serde_json::json!(last_commit_age));
                obj.insert("last_commit_subject".into(), serde_json::json!(info.last_commit_subject));
            }

            serde_json::Value::Object(obj)
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}
```

- [ ] **Step 3: Run the YAML test**

Run: `mise run test:manual -- --ci list:columns-json` Expected: PASS.

- [ ] **Step 4: Run the full existing JSON test to ensure no regression**

Run: `mise run test:manual -- --ci list:json` Expected: PASS (backward
compatible — no `--columns` means all keys present).

- [ ] **Step 5: Commit**

```bash
git add src/commands/list.rs tests/manual/scenarios/list/columns-json.yml
git commit -m "feat: filter list JSON output by selected columns"
```

---

### Task 7: Add modifier mode and config YAML tests for list

**Files:**

- Create: `tests/manual/scenarios/list/columns-modifier.yml`
- Create: `tests/manual/scenarios/list/columns-config.yml`
- Create: `tests/manual/scenarios/list/columns-errors.yml`

- [ ] **Step 1: Write modifier mode test**

Create `tests/manual/scenarios/list/columns-modifier.yml`:

```yaml
name: Columns modifier mode
description: --columns with +/- modifiers add/remove from defaults

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Remove annotation column
    run: NO_COLOR=1 git-worktree-list --columns -annotation 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Branch"
        - "Path"

  - name: Remove multiple columns
    run:
      NO_COLOR=1 git-worktree-list --columns -annotation,-last-commit,-remote
      2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Branch"
        - "Path"
        - "Base"
        - "Changes"
        - "Age"
      output_excludes:
        - "Remote"
        - "Last Commit"
```

- [ ] **Step 2: Write config test**

Create `tests/manual/scenarios/list/columns-config.yml`:

```yaml
name: Columns config
description: daft.list.columns config sets default column selection

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Set columns config
    run: git config daft.list.columns "branch,path"
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: List uses config columns
    run: NO_COLOR=1 git-worktree-list 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Branch"
        - "Path"
      output_excludes:
        - "Base"
        - "Changes"

  - name: CLI flag overrides config
    run: NO_COLOR=1 git-worktree-list --columns branch,path,age 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "Branch"
        - "Path"
        - "Age"
```

- [ ] **Step 3: Write error case test**

Create `tests/manual/scenarios/list/columns-errors.yml`:

```yaml
name: Columns error cases
description: --columns produces clear error messages for invalid input

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Unknown column name
    run: git-worktree-list --columns branch,foo 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "unknown column 'foo'"

  - name: Mixed mode error
    run: git-worktree-list --columns branch,+age 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "cannot mix"

  - name: Duplicate column error
    run: git-worktree-list --columns branch,path,branch 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "duplicate"
```

- [ ] **Step 4: Run all new YAML tests**

Run:
`mise run test:manual -- --ci list:columns-modifier list:columns-config list:columns-errors`
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/manual/scenarios/list/columns-modifier.yml \
        tests/manual/scenarios/list/columns-config.yml \
        tests/manual/scenarios/list/columns-errors.yml
git commit -m "test: add YAML tests for list column selection"
```

---

## Chunk 3: TUI Integration (Sync and Prune)

### Task 8: Thread columns through TUI state

Add a `columns` field to `TuiState` and pass it from sync/prune commands.

**Files:**

- Modify: `src/output/tui/state.rs`
- Modify: `src/output/tui/render.rs`
- Modify: `src/output/tui/columns.rs`
- Modify: `src/commands/sync.rs`
- Modify: `src/commands/prune.rs`

- [ ] **Step 1: Add `ListColumn`-to-TUI `Column` mapping**

In `src/output/tui/columns.rs`, add a conversion method:

```rust
use crate::core::columns::ListColumn;

impl Column {
    /// Convert from a user-facing `ListColumn` to the TUI `Column`.
    pub fn from_list_column(lc: ListColumn) -> Self {
        match lc {
            ListColumn::Annotation => Column::Annotation,
            ListColumn::Branch => Column::Branch,
            ListColumn::Path => Column::Path,
            ListColumn::Base => Column::Base,
            ListColumn::Remote => Column::Remote,
            ListColumn::Changes => Column::Changes,
            ListColumn::Age => Column::Age,
            ListColumn::LastCommit => Column::LastCommit,
        }
    }
}
```

- [ ] **Step 2: Add columns to TuiState**

In `src/output/tui/state.rs`, add to `TuiState`:

```rust
    /// User-selected columns (None = use responsive selection).
    pub columns: Option<Vec<Column>>,
    /// If true, the user explicitly chose columns (replace mode) and responsive
    /// column dropping should be disabled for them.
    pub columns_explicit: bool,
```

Update `TuiState::new()` to accept and store columns:

```rust
    pub fn new(
        phases: Vec<OperationPhase>,
        worktree_infos: Vec<WorktreeInfo>,
        project_root: PathBuf,
        cwd: PathBuf,
        stat: Stat,
        verbose: u8,
        columns: Option<Vec<Column>>,
        columns_explicit: bool,
    ) -> Self {
```

And in the `Self { ... }` block, add: `columns, columns_explicit,`

- [ ] **Step 3: Update all TuiState::new() call sites**

There are call sites in `sync.rs`, `prune.rs`, and several test functions in
`state.rs`. Add `None` as the columns argument to all existing call sites
(preserving current behavior).

In `src/commands/sync.rs`, find the `TuiState::new(` call and add `None, false,`
as the last two arguments before the closing `)`.

In `src/commands/prune.rs`, same change.

In `src/output/tui/state.rs`, find all `TuiState::new(` calls in the
`#[cfg(test)]` module and add `None, false,` as the last two arguments.

- [ ] **Step 4: Use columns in render_table**

In `src/output/tui/render.rs`, replace the line:

```rust
    let columns = select_columns(area.width, &state.worktrees, &row_vals);
```

with:

```rust
    let columns = match (&state.columns, state.columns_explicit) {
        // Replace mode: user explicitly chose columns, don't responsively drop.
        (Some(user_cols), true) => user_cols.clone(),
        // Modifier mode: user tweaked defaults, responsive dropping still applies.
        (Some(user_cols), false) => {
            // Use select_columns but restrict to the user's modified set.
            let responsive = select_columns(area.width, &state.worktrees, &row_vals);
            responsive
                .into_iter()
                .filter(|c| matches!(c, Column::Status) || user_cols.contains(c))
                .collect()
        }
        // No column selection: fully responsive.
        (None, _) => select_columns(area.width, &state.worktrees, &row_vals),
    };
    // Status is always prepended for TUI commands.
    let columns = if !columns.contains(&Column::Status) {
        let mut with_status = vec![Column::Status];
        with_status.extend(columns);
        with_status
    } else {
        columns
    };
```

- [ ] **Step 5: Verify it compiles and existing tests pass**

Run: `cargo build && mise run test:unit` Expected: compiles, all tests pass (no
behavior change yet — columns is `None` everywhere).

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/state.rs src/output/tui/render.rs \
        src/output/tui/columns.rs src/commands/sync.rs src/commands/prune.rs
git commit -m "feat: thread column selection through TUI state"
```

---

### Task 9: Add `--columns` flag to sync and prune commands

Wire the flag and resolve it, passing to TuiState.

**Files:**

- Modify: `src/commands/sync.rs`
- Modify: `src/commands/prune.rs`

- [ ] **Step 1: Add --columns to sync Args**

In `src/commands/sync.rs`, add to the `Args` struct:

```rust
    #[arg(
        long,
        help = "Columns to display (comma-separated). Replace: branch,path,age. Modify defaults: +col,-col"
    )]
    columns: Option<String>,
```

- [ ] **Step 2: Resolve and pass to TuiState in sync**

In the `run_tui()` function of `sync.rs`, after settings/stat resolution, add
column resolution:

```rust
    use crate::core::columns::{ColumnSelection, CommandKind, ListColumn, ResolvedColumns};
    use crate::output::tui::columns::Column;

    let columns_input = args.columns.or(settings.sync_columns);
    let (tui_columns, columns_explicit) = match columns_input {
        Some(ref input) => {
            let resolved = ColumnSelection::parse(input, CommandKind::Sync)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let tui_cols: Vec<Column> = resolved.columns.iter()
                .map(|c| Column::from_list_column(*c))
                .collect();
            (Some(tui_cols), resolved.explicit)
        }
        None => (None, false),
    };
```

Then pass `tui_columns, columns_explicit` instead of `None, false` to the
`TuiState::new()` call.

- [ ] **Step 3: Repeat for prune**

Same pattern in `src/commands/prune.rs`: add the `--columns` flag, resolve it,
pass to `TuiState::new()`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build` Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/commands/sync.rs src/commands/prune.rs
git commit -m "feat: add --columns flag to sync and prune commands"
```

---

### Task 10: Add YAML tests for sync/prune --columns

**Files:**

- Create: `tests/manual/scenarios/sync/columns.yml`
- Create: `tests/manual/scenarios/prune/columns.yml`

- [ ] **Step 1: Write sync columns test**

Create `tests/manual/scenarios/sync/columns.yml`:

```yaml
name: Sync columns flag
description: --columns flag works on sync command

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Status column cannot be controlled
    run: git-worktree-sync --columns status,branch 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "cannot be controlled"

  - name: Status modifier also rejected
    run: git-worktree-sync --columns +status 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "cannot be controlled"
```

- [ ] **Step 2: Write prune columns test**

Create `tests/manual/scenarios/prune/columns.yml`:

```yaml
name: Prune columns flag
description: --columns flag works on prune command

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone the repository
    run: git-worktree-clone $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Status column cannot be controlled
    run: git-worktree-prune --columns status,branch 2>&1
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 1
      output_contains:
        - "cannot be controlled"
```

- [ ] **Step 3: Run the tests**

Run: `mise run test:manual -- --ci sync:columns prune:columns` Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/sync/columns.yml \
        tests/manual/scenarios/prune/columns.yml
git commit -m "test: add YAML tests for sync/prune --columns flag"
```

---

## Chunk 4: Shell Completions, Docs, and Final Verification

### Task 11: Add column value completions to shell generators

The `--columns` flag name is auto-discovered by clap introspection, but value
completions (column names, +/- prefixed variants) require custom logic in each
shell generator.

**Files:**

- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`
- Modify: `src/commands/completions/fig.rs`

- [ ] **Step 1: Read existing completion generators**

Read all four files to understand the current patterns for custom value
completions.

- [ ] **Step 2: Add column completions to bash**

In `bash.rs`, add a case in the completion function that triggers after
`--columns`. Complete with all column names (plain, `+`-prefixed, `-`-prefixed).
Support comma-separated values by completing after the last comma.

- [ ] **Step 3: Add column completions to zsh**

In `zsh.rs`, add a `--columns` argument spec with value completion using the
column name list.

- [ ] **Step 4: Add column completions to fish**

In `fish.rs`, add completions for `--columns` flag values for the three
commands.

- [ ] **Step 5: Add column completions to fig**

In `fig.rs`, add column name suggestions for the `--columns` argument in the Fig
spec generator.

- [ ] **Step 6: Verify completions build**

Run: `cargo build` Expected: compiles.

- [ ] **Step 7: Commit**

```bash
git add src/commands/completions/
git commit -m "feat: add shell completions for --columns flag values"
```

---

### Task 12: Update CLI documentation

**Files:**

- Modify: `docs/cli/daft-list.md`
- Modify: `docs/cli/git-worktree-list.md`
- Modify: `docs/cli/daft-sync.md`
- Modify: `docs/cli/git-worktree-sync.md`
- Modify: `docs/cli/daft-prune.md`
- Modify: `docs/cli/git-worktree-prune.md`

- [ ] **Step 1: Read existing docs to understand format**

Read all six files to understand the Options table format.

- [ ] **Step 2: Add --columns to list docs**

Add to the Options table in both `daft-list.md` and `git-worktree-list.md`:

```markdown
| `--columns <COLUMNS>` | Columns to display (comma-separated). Use
`branch,path,age` for exact columns and order, or `+col,-col` to add/remove from
defaults. |
```

- [ ] **Step 3: Add --columns to sync docs**

Same addition for `daft-sync.md` and `git-worktree-sync.md`.

- [ ] **Step 4: Add --columns to prune docs**

Same addition for `daft-prune.md` and `git-worktree-prune.md`.

- [ ] **Step 5: Commit**

```bash
git add docs/cli/
git commit -m "docs: add --columns flag to list, sync, and prune CLI docs"
```

---

### Task 13: Update help text and regenerate man pages

**Files:**

- Modify: `src/commands/list.rs` (long_about)
- Modify: `src/commands/sync.rs` (long_about if present)
- Modify: `src/commands/prune.rs` (long_about if present)

- [ ] **Step 1: Update list command long_about**

In `src/commands/list.rs`, add to the `long_about` text (before the closing
`"#)]`):

```
Use --columns to select which columns are shown and in what order.
  Replace mode:  --columns branch,path,age (exact set and order)
  Modifier mode: --columns -annotation,-last-commit (remove from defaults)
Defaults can be set in git config with daft.list.columns.
```

- [ ] **Step 2: Regenerate man pages**

Run: `mise run man:gen` Expected: man pages regenerated.

- [ ] **Step 3: Verify man pages**

Run: `mise run man:verify` Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add src/commands/list.rs src/commands/sync.rs src/commands/prune.rs man/
git commit -m "docs: update help text and regenerate man pages for --columns"
```

---

### Task 14: Update SKILL.md

**Files:**

- Modify: `SKILL.md`

- [ ] **Step 1: Read current SKILL.md**

Read the file to understand its structure.

- [ ] **Step 2: Document the --columns flag**

Add documentation about the `--columns` flag to the relevant command sections in
SKILL.md. Include that it's available on list, sync, and prune, supports replace
and modifier modes, and can be configured via git config.

- [ ] **Step 3: Commit**

```bash
git add SKILL.md
git commit -m "docs: document --columns flag in SKILL.md"
```

---

### Task 15: Full verification

- [ ] **Step 1: Run formatter**

Run: `mise run fmt` Expected: no changes.

- [ ] **Step 2: Run clippy**

Run: `mise run clippy` Expected: zero warnings.

- [ ] **Step 3: Run all unit tests**

Run: `mise run test:unit` Expected: all pass.

- [ ] **Step 4: Run all list YAML tests**

Run: `mise run test:manual -- --ci list` Expected: all pass (existing + new).

- [ ] **Step 5: Run full integration suite**

Run: `mise run test:integration` Expected: all pass.

- [ ] **Step 6: Run full CI simulation**

Run: `mise run ci` Expected: all checks pass.
