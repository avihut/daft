# daft list — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to
> implement this plan task-by-task.

**Goal:** Add a `daft list` command that prints a compact, colored table of all
active worktrees with rich git status info.

**Architecture:** New command module `src/commands/list.rs` with core logic in
`src/core/worktree/list.rs`. Uses `tabled` crate for table rendering. Collects
worktree data from `git worktree list --porcelain`, then enriches each entry
with ahead/behind counts, dirty status, and last commit info via per-worktree
git calls.

**Tech Stack:** Rust, clap (CLI args), tabled (table rendering), existing
`GitCommand` wrapper, existing `styles.rs` color helpers.

---

## Output Format

Compact aligned columns, borderless:

```
  > feat/auth        ./auth       +3 -1  *  2h ago   Add OAuth2 flow
    fix/header       ./header            *  5d ago   Fix header z-index
    master           ./master               1h ago   Release v2.1.0
    feat/dashboard   ./dashboard  +12       30m ago  Add chart component
```

Columns: current marker (`>`), branch name, relative path, ahead/behind base,
dirty (`*`), commit age, commit subject. Colors: `>` cyan, `*` yellow, `+N`
green, `-N` red, age >7d dim.

---

### Task 1: Add `tabled` dependency

**Files:**

- Modify: `Cargo.toml:89` (after `indicatif` line)

**Step 1: Add tabled to Cargo.toml**

In `Cargo.toml`, add after line 89 (`indicatif = "0.18"`):

```toml
tabled = { version = "0.17", default-features = false, features = ["std"] }
```

**Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles successfully with new dependency.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tabled dependency for worktree list table rendering"
```

---

### Task 2: Create core worktree list logic

**Files:**

- Create: `src/core/worktree/list.rs`
- Modify: `src/core/worktree/mod.rs` (add `pub mod list;`)

**Step 1: Add module declaration**

In `src/core/worktree/mod.rs`, add `pub mod list;` in alphabetical order among
existing modules.

**Step 2: Create the core list module**

Create `src/core/worktree/list.rs` with:

```rust
//! Core logic for listing worktrees with rich status information.

use crate::git::GitCommand;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Information about a single worktree for display.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    /// Branch name (stripped of refs/heads/), or "(detached)" for detached HEAD
    pub name: String,
    /// Absolute path to the worktree
    pub path: PathBuf,
    /// Whether this is the currently active worktree
    pub is_current: bool,
    /// Number of commits ahead of the base branch (None if this IS the base branch)
    pub ahead: Option<usize>,
    /// Number of commits behind the base branch (None if this IS the base branch)
    pub behind: Option<usize>,
    /// Whether the worktree has uncommitted changes
    pub is_dirty: bool,
    /// Human-readable age of the last commit (e.g., "2 hours ago")
    pub last_commit_age: String,
    /// Subject line of the last commit
    pub last_commit_subject: String,
}

/// Collect information about all worktrees in the current repository.
///
/// `base_branch` is the branch to compute ahead/behind against (e.g., "master" or "main").
/// `current_worktree_path` is the canonicalized path of the current worktree.
pub fn collect_worktree_info(
    git: &GitCommand,
    base_branch: &str,
    current_worktree_path: &Path,
) -> Result<Vec<WorktreeInfo>> {
    let porcelain = git.worktree_list_porcelain()?;
    let entries = parse_porcelain(&porcelain);

    let mut infos = Vec::new();

    for entry in entries {
        // Skip bare entries
        if entry.is_bare {
            continue;
        }

        let path = PathBuf::from(&entry.path);
        let name = entry
            .branch
            .as_deref()
            .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_string())
            .unwrap_or_else(|| "(detached)".to_string());

        let is_current = path
            .canonicalize()
            .ok()
            .map(|p| p == current_worktree_path)
            .unwrap_or(false);

        // Compute ahead/behind relative to base branch
        let (ahead, behind) = if name == base_branch {
            (None, None)
        } else {
            get_ahead_behind(base_branch, &name, &path)
        };

        // Check dirty status
        let is_dirty = git.has_uncommitted_changes_in(&path).unwrap_or(false);

        // Get last commit info
        let (last_commit_age, last_commit_subject) = get_last_commit_info(&path);

        infos.push(WorktreeInfo {
            name,
            path,
            is_current,
            ahead,
            behind,
            is_dirty,
            last_commit_age,
            last_commit_subject,
        });
    }

    // Sort alphabetically by name
    infos.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Ok(infos)
}

/// A raw worktree entry parsed from porcelain output.
struct PorcelainEntry {
    path: String,
    branch: Option<String>,
    is_bare: bool,
}

/// Parse `git worktree list --porcelain` output into entries.
fn parse_porcelain(output: &str) -> Vec<PorcelainEntry> {
    let mut entries = Vec::new();
    let mut current_path = None;
    let mut current_branch = None;
    let mut is_bare = false;

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            // If we have a previous entry, push it
            if let Some(prev_path) = current_path.take() {
                entries.push(PorcelainEntry {
                    path: prev_path,
                    branch: current_branch.take(),
                    is_bare,
                });
            }
            current_path = Some(path.to_string());
            current_branch = None;
            is_bare = false;
        } else if let Some(branch) = line.strip_prefix("branch ") {
            current_branch = Some(branch.to_string());
        } else if line == "bare" {
            is_bare = true;
        }
    }

    // Push the last entry
    if let Some(path) = current_path {
        entries.push(PorcelainEntry {
            path,
            branch: current_branch,
            is_bare,
        });
    }

    entries
}

/// Get ahead/behind counts for a branch relative to a base branch.
fn get_ahead_behind(base: &str, branch: &str, worktree_path: &Path) -> (Option<usize>, Option<usize>) {
    // Use git rev-list --left-right --count base...branch
    let output = Command::new("git")
        .args(["rev-list", "--left-right", "--count", &format!("{base}...{branch}")])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let parts: Vec<&str> = stdout.trim().split('\t').collect();
            if parts.len() == 2 {
                let behind = parts[0].parse::<usize>().unwrap_or(0);
                let ahead = parts[1].parse::<usize>().unwrap_or(0);
                (Some(ahead), Some(behind))
            } else {
                (Some(0), Some(0))
            }
        }
        _ => (Some(0), Some(0)),
    }
}

/// Get the last commit age and subject for a worktree.
fn get_last_commit_info(worktree_path: &Path) -> (String, String) {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%cr\x1f%s"])
        .current_dir(worktree_path)
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let trimmed = stdout.trim();
            if let Some((age, subject)) = trimmed.split_once('\x1f') {
                (age.to_string(), subject.to_string())
            } else {
                ("unknown".to_string(), String::new())
            }
        }
        _ => ("unknown".to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_porcelain_basic() {
        let output = "\
worktree /home/user/project
HEAD abc123
branch refs/heads/master

worktree /home/user/project/feature
HEAD def456
branch refs/heads/feature/auth

";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "/home/user/project");
        assert_eq!(
            entries[0].branch.as_deref(),
            Some("refs/heads/master")
        );
        assert!(!entries[0].is_bare);
        assert_eq!(entries[1].path, "/home/user/project/feature");
        assert_eq!(
            entries[1].branch.as_deref(),
            Some("refs/heads/feature/auth")
        );
    }

    #[test]
    fn test_parse_porcelain_skips_bare() {
        let output = "\
worktree /home/user/project/.git
HEAD abc123
bare

worktree /home/user/project/main
HEAD def456
branch refs/heads/main

";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_bare);
        assert!(!entries[1].is_bare);
    }

    #[test]
    fn test_parse_porcelain_detached_head() {
        let output = "\
worktree /home/user/project/detached
HEAD abc123
detached

";
        let entries = parse_porcelain(output);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].branch.is_none());
    }
}
```

**Step 3: Verify it compiles**

Run: `cargo check` Expected: Compiles successfully.

**Step 4: Run the unit tests**

Run: `cargo test --lib core::worktree::list` Expected: All 3 tests pass.

**Step 5: Commit**

```bash
git add src/core/worktree/list.rs src/core/worktree/mod.rs
git commit -m "feat(list): add core worktree list data collection logic"
```

---

### Task 3: Create the `daft list` command module

**Files:**

- Create: `src/commands/list.rs`
- Modify: `src/commands/mod.rs:15` (add `pub mod list;` between `init` and
  `multi_remote`)

**Step 1: Add module declaration**

In `src/commands/mod.rs`, add `pub mod list;` in alphabetical order (between
`init` and `multi_remote`).

**Step 2: Create the command module**

Create `src/commands/list.rs`:

```rust
//! git-worktree-list - List worktrees with rich status information
//!
//! Displays all worktrees in a compact, colored table showing branch name,
//! relative path, ahead/behind counts versus the base branch, dirty status,
//! last commit age, and last commit subject.

use crate::{
    core::{
        repo::{get_current_worktree_path, get_project_root, resolve_initial_branch},
        worktree::list::{self, WorktreeInfo},
    },
    git::GitCommand,
    is_git_repository,
    logging::init_logging,
    styles,
};
use anyhow::Result;
use clap::Parser;
use tabled::{
    settings::{object::Columns, Modify, Style, Width},
    Table, Tabled,
};

#[derive(Parser)]
#[command(name = "git-worktree-list")]
#[command(version = crate::VERSION)]
#[command(about = "List worktrees with rich status information")]
#[command(long_about = r#"
Displays all worktrees in a compact table with status information:

  > feat/auth        ./auth       +3 -1  *  2h ago   Add OAuth2 flow
    fix/header       ./header            *  5d ago   Fix header z-index
    master           ./master               1h ago   Release v2.1.0

Columns show: current worktree marker (>), branch name, relative path,
commits ahead/behind the base branch, dirty indicator (*), last commit
age, and last commit subject.

The base branch is auto-detected from git config (init.defaultBranch)
or defaults to "master".
"#)]
pub struct Args {
    /// Output as JSON for scripting
    #[arg(long, help = "Output as JSON")]
    json: bool,

    /// Be verbose; show detailed progress
    #[arg(short, long, help = "Be verbose; show detailed progress")]
    verbose: bool,
}

pub fn run() -> Result<()> {
    let args = Args::parse_from(crate::get_clap_args("git-worktree-list"));
    run_with_args(args)
}

fn run_with_args(args: Args) -> Result<()> {
    init_logging(args.verbose);

    if !is_git_repository()? {
        anyhow::bail!("Not inside a Git repository");
    }

    let git = GitCommand::new(false);
    let base_branch = resolve_initial_branch(&None);
    let current_path = get_current_worktree_path()?
        .canonicalize()
        .unwrap_or_else(|_| get_current_worktree_path().unwrap_or_default());
    let project_root = get_project_root()?;

    let infos = list::collect_worktree_info(&git, &base_branch, &current_path)?;

    if infos.is_empty() {
        println!("No worktrees found.");
        return Ok(());
    }

    if args.json {
        print_json(&infos, &project_root)?;
    } else {
        print_table(&infos, &project_root);
    }

    Ok(())
}

/// A row in the table, formatted for display.
#[derive(Tabled)]
struct TableRow {
    #[tabled(rename = "")]
    current: String,
    #[tabled(rename = "Branch")]
    name: String,
    #[tabled(rename = "Path")]
    path: String,
    #[tabled(rename = "Base")]
    base: String,
    #[tabled(rename = "")]
    dirty: String,
    #[tabled(rename = "Age")]
    age: String,
    #[tabled(rename = "Last Commit")]
    subject: String,
}

fn print_table(infos: &[WorktreeInfo], project_root: &std::path::Path) {
    let use_color = styles::colors_enabled();

    let rows: Vec<TableRow> = infos
        .iter()
        .map(|info| {
            let current = if info.is_current {
                if use_color {
                    styles::cyan(">")
                } else {
                    ">".to_string()
                }
            } else {
                " ".to_string()
            };

            let relative_path = info
                .path
                .strip_prefix(project_root)
                .map(|p| format!("./{}", p.display()))
                .unwrap_or_else(|_| info.path.display().to_string());

            let base = format_ahead_behind(info.ahead, info.behind, use_color);

            let dirty = if info.is_dirty {
                if use_color {
                    styles::yellow("*")
                } else {
                    "*".to_string()
                }
            } else {
                " ".to_string()
            };

            let age = format_age(&info.last_commit_age, use_color);

            TableRow {
                current,
                name: info.name.clone(),
                path: relative_path,
                base,
                dirty,
                age,
                subject: info.last_commit_subject.clone(),
            }
        })
        .collect();

    let mut table = Table::new(rows);
    table
        .with(Style::blank())
        .with(Modify::new(Columns::last()).with(Width::truncate(40).suffix("...")));

    // Print without header row
    let output = table.to_string();
    // Skip the first line (header) and the blank line after it
    let lines: Vec<&str> = output.lines().collect();
    // tabled with Style::blank() doesn't add separator lines, just header + data
    // Skip first line (header)
    for line in lines.iter().skip(1) {
        println!("{line}");
    }
}

fn format_ahead_behind(ahead: Option<usize>, behind: Option<usize>, use_color: bool) -> String {
    match (ahead, behind) {
        (None, None) => String::new(), // This IS the base branch
        (Some(a), Some(b)) => {
            let mut parts = Vec::new();
            if a > 0 {
                let s = format!("+{a}");
                parts.push(if use_color { styles::green(&s) } else { s });
            }
            if b > 0 {
                let s = format!("-{b}");
                parts.push(if use_color { styles::red(&s) } else { s });
            }
            parts.join(" ")
        }
        _ => String::new(),
    }
}

fn format_age(age: &str, use_color: bool) -> String {
    if !use_color {
        return age.to_string();
    }
    // Dim ages older than 7 days
    if age.contains("week")
        || age.contains("month")
        || age.contains("year")
        || (age.contains("day") && !age.starts_with("1 day") && !age.starts_with("2 day")
            && !age.starts_with("3 day") && !age.starts_with("4 day")
            && !age.starts_with("5 day") && !age.starts_with("6 day")
            && !age.starts_with("7 day"))
    {
        styles::dim(age)
    } else {
        age.to_string()
    }
}

fn print_json(infos: &[WorktreeInfo], project_root: &std::path::Path) -> Result<()> {
    let entries: Vec<serde_json::Value> = infos
        .iter()
        .map(|info| {
            let relative_path = info
                .path
                .strip_prefix(project_root)
                .map(|p| format!("./{}", p.display()))
                .unwrap_or_else(|_| info.path.display().to_string());

            serde_json::json!({
                "name": info.name,
                "path": relative_path,
                "is_current": info.is_current,
                "ahead": info.ahead,
                "behind": info.behind,
                "is_dirty": info.is_dirty,
                "last_commit_age": info.last_commit_age,
                "last_commit_subject": info.last_commit_subject,
            })
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries)?);
    Ok(())
}
```

**Step 3: Verify it compiles**

Run: `cargo check` Expected: Compiles successfully.

**Step 4: Commit**

```bash
git add src/commands/list.rs src/commands/mod.rs
git commit -m "feat(list): add daft list command module with table rendering"
```

---

### Task 4: Wire up routing in main.rs

**Files:**

- Modify: `src/main.rs`

**Step 1: Add git-worktree-list route**

In `src/main.rs`, add after line 49 (`"git-sync" => commands::sync::run(),`):

```rust
"git-worktree-list" => commands::list::run(),
```

**Step 2: Add daft verb alias**

In the `"git-daft" | "daft"` match block, add after line 95
(`"sync" => commands::sync::run(),`):

```rust
"list" => commands::list::run(),
```

**Step 3: Add worktree- prefixed variant**

In the worktree- prefixed section, add after line 110
(`"worktree-flow-eject" => commands::flow_eject::run(),`):

```rust
"worktree-list" => commands::list::run(),
```

**Step 4: Add to DAFT_SUBCOMMANDS**

In `src/suggest.rs`, add `"list"` in alphabetical order (after `"init"`, before
`"multi-remote"`), and add `"worktree-list"` in alphabetical order (after
`"worktree-init"`, before `"worktree-prune"`).

**Step 5: Verify it compiles and unit tests pass**

Run: `cargo check && cargo test --lib suggest` Expected: Compiles, suggest tests
pass (including the alphabetical order test).

**Step 6: Commit**

```bash
git add src/main.rs src/suggest.rs
git commit -m "feat(list): wire up command routing for daft list"
```

---

### Task 5: Add shortcuts

**Files:**

- Modify: `src/shortcuts.rs`

**Step 1: Add git-style shortcut**

In `src/shortcuts.rs`, add to the Git style section (after `gwtsync` entry,
before the Shell style comment):

```rust
Shortcut {
    alias: "gwtls",
    command: "git-worktree-list",
    style: ShortcutStyle::Git,
},
```

**Step 2: Update the valid_commands list in tests**

In the `test_all_shortcuts_map_to_valid_commands` test, add
`"git-worktree-list"` to the `valid_commands` array.

**Step 3: Update the git shortcuts count test**

In `test_shortcuts_for_style`, update the git style count assertion:
`assert_eq!(git_shortcuts.len(), 10);` (was 9).

**Step 4: Run tests**

Run: `cargo test --lib shortcuts` Expected: All tests pass.

**Step 5: Commit**

```bash
git add src/shortcuts.rs
git commit -m "feat(list): add gwtls shortcut alias"
```

---

### Task 6: Add to help documentation

**Files:**

- Modify: `src/commands/docs.rs`

**Step 1: Add import**

In `src/commands/docs.rs` line 10, add `list` to the imports:

```rust
use crate::commands::{
    carry, checkout, clone, completions, doctor, fetch, flow_adopt, flow_eject, hooks, init,
    list, multi_remote, prune, release_notes, shell_init, shortcuts, sync, worktree_branch,
};
```

**Step 2: Add to "maintain your worktrees" category**

In `get_command_categories()`, add to the "maintain your worktrees" category
(before the `worktree-branch` entry):

```rust
CommandEntry {
    display_name: "worktree-list",
    command: list::Args::command(),
},
```

**Step 3: Update the short aliases line**

On line 185, update the aliases line to include `list`:

```rust
println!("   clone, init, carry, update, list, prune, rename, sync, remove, adopt, eject");
```

**Step 4: Verify it compiles**

Run: `cargo check` Expected: Compiles successfully.

**Step 5: Commit**

```bash
git add src/commands/docs.rs
git commit -m "feat(list): add list to help documentation"
```

---

### Task 7: Add to xtask (man pages and CLI docs)

**Files:**

- Modify: `xtask/src/main.rs`

**Step 1: Add to COMMANDS array**

In `xtask/src/main.rs`, add `"git-worktree-list"` to the COMMANDS array (after
`"git-worktree-flow-eject"`, before `"git-sync"`).

**Step 2: Add to DAFT_VERBS**

Add a new entry to the DAFT_VERBS array (after `daft-init`, before `daft-go`):

```rust
DaftVerbEntry {
    daft_name: "daft-list",
    source_command: "git-worktree-list",
    about_override: None,
},
```

**Step 3: Add to get_command_for_name()**

Add a match arm:

```rust
"git-worktree-list" => Some(daft::commands::list::Args::command()),
```

**Step 4: Add daft_verb_tip()**

Add a match arm in the `daft_verb_tip()` function:

```rust
"git-worktree-list" => Some(
    "::: tip\nThis command is also available as `daft list`. See [daft list](./daft-list.md).\n:::\n",
),
```

**Step 5: Add related_commands()**

Add a match arm in `related_commands()`:

```rust
"git-worktree-list" => vec![
    "git-worktree-checkout",
    "git-worktree-prune",
    "git-worktree-branch",
],
```

**Step 6: Run xtask unit tests**

Run: `cargo test -p xtask` Expected: All tests pass (including
`test_all_commands_have_valid_handlers`).

**Step 7: Commit**

```bash
git add xtask/src/main.rs
git commit -m "feat(list): add to xtask for man page and CLI doc generation"
```

---

### Task 8: Add to shell completions

**Files:**

- Modify: `src/commands/completions/mod.rs`
- Modify: `src/commands/completions/bash.rs`
- Modify: `src/commands/completions/zsh.rs`
- Modify: `src/commands/completions/fish.rs`

**Step 1: Update completions/mod.rs**

Add `"git-worktree-list"` to the COMMANDS array (after
`"git-worktree-flow-eject"`, before `"daft-remove"`).

Add to `get_command_for_name()`:

```rust
"git-worktree-list" => Some(crate::commands::list::Args::command()),
```

Add a new verb alias group entry in VERB_ALIAS_GROUPS:

```rust
(&["list"], "git-worktree-list"),
```

**Step 2: Update bash.rs**

In the `DAFT_BASH_COMPLETIONS` string constant:

1. Add a `list)` case in the verb dispatch section (after `sync)` case):

```bash
            list)
                COMP_WORDS=("git-worktree-list" "${COMP_WORDS[@]:2}")
                COMP_CWORD=$((COMP_CWORD - 1))
                _git_worktree_list
                return
                ;;
```

2. Add `list` to the subcommand list in the `COMPREPLY` line (add after
   `prune`).

**Step 3: Update zsh.rs**

In the `DAFT_ZSH_COMPLETIONS` string constant:

1. Add a `list)` case in the verb dispatch section:

```zsh
            list)
                words=("git-worktree-list" "${(@)words[3,-1]}")
                CURRENT=$((CURRENT - 1))
                __git_worktree_list_impl
                return
                ;;
```

2. Add `list` to the subcommand list in the `_arguments` alternatives.

**Step 4: Update fish.rs**

In `DAFT_FISH_COMPLETIONS` constant:

1. Add a `complete` line for the `list` subcommand:

```fish
complete -c daft -n '__fish_use_subcommand' -a 'list' -d 'List worktrees with status'
```

2. Add `list` to the branch completion trigger line if appropriate.

**Step 5: Verify it compiles**

Run: `cargo check` Expected: Compiles successfully.

**Step 6: Commit**

```bash
git add src/commands/completions/
git commit -m "feat(list): add shell completions for daft list"
```

---

### Task 9: Generate man pages and CLI docs

**Step 1: Generate man pages**

Run: `mise run man:gen` Expected: Generates `man/git-worktree-list.1` and
`man/daft-list.1`.

**Step 2: Generate CLI docs**

Run: `cargo run -p xtask -- gen-cli-docs` Expected: Generates
`docs/cli/git-worktree-list.md`.

**Step 3: Verify man pages**

Run: `mise run man:verify` Expected: All man pages are up-to-date.

**Step 4: Commit**

```bash
git add man/ docs/cli/git-worktree-list.md
git commit -m "docs(list): generate man pages and CLI reference"
```

---

### Task 10: Add integration tests

**Files:**

- Create: `tests/integration/test_list.sh`
- Modify: `tests/integration/test_all.sh` (add test_list.sh to the test list)

**Step 1: Update test framework symlinks**

In `tests/integration/test_framework.sh`, add `"git-worktree-list"` and
`"gwtls"` to the `symlink_names` array in `ensure_rust_binaries()`.

**Step 2: Create the integration test file**

Create `tests/integration/test_list.sh`:

```bash
#!/bin/bash

# Integration tests for git-worktree-list / daft list

source "$(dirname "${BASH_SOURCE[0]}")/test_framework.sh"

# Test basic list with single worktree
test_list_basic() {
    local remote_repo=$(create_test_remote "test-repo-list-basic" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-basic"

    local output
    output=$(git-worktree-list 2>&1) || return 1

    # Should show main worktree
    if ! echo "$output" | grep -q "main"; then
        log_error "Expected 'main' in output, got: $output"
        return 1
    fi

    return 0
}

# Test list with multiple worktrees
test_list_multiple() {
    local remote_repo=$(create_test_remote "test-repo-list-multiple" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-multiple"

    git-worktree-checkout develop || return 1
    git-worktree-checkout feature/test || return 1

    local output
    output=$(git-worktree-list 2>&1) || return 1

    # Should show all worktrees
    if ! echo "$output" | grep -q "main"; then
        log_error "Expected 'main' in output"
        return 1
    fi
    if ! echo "$output" | grep -q "develop"; then
        log_error "Expected 'develop' in output"
        return 1
    fi
    if ! echo "$output" | grep -q "feature/test"; then
        log_error "Expected 'feature/test' in output"
        return 1
    fi

    return 0
}

# Test list marks current worktree
test_list_current_marker() {
    local remote_repo=$(create_test_remote "test-repo-list-current" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-current"

    git-worktree-checkout develop || return 1

    # Run from main worktree
    cd main
    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || return 1

    # The main line should have the > marker
    if ! echo "$output" | grep "main" | grep -q ">"; then
        log_error "Expected '>' marker on main worktree line"
        return 1
    fi

    return 0
}

# Test list with dirty worktree
test_list_dirty() {
    local remote_repo=$(create_test_remote "test-repo-list-dirty" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-dirty"

    # Make main dirty
    echo "dirty" >> main/README.md

    local output
    output=$(NO_COLOR=1 git-worktree-list 2>&1) || return 1

    # Should show dirty indicator
    if ! echo "$output" | grep "main" | grep -q "\*"; then
        log_error "Expected '*' dirty indicator on main worktree"
        return 1
    fi

    return 0
}

# Test list --json output
test_list_json() {
    local remote_repo=$(create_test_remote "test-repo-list-json" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-json"

    local output
    output=$(git-worktree-list --json 2>&1) || return 1

    # Should be valid JSON
    if ! echo "$output" | python3 -m json.tool > /dev/null 2>&1; then
        log_error "Expected valid JSON output, got: $output"
        return 1
    fi

    # Should contain expected fields
    if ! echo "$output" | grep -q '"name"'; then
        log_error "Expected 'name' field in JSON"
        return 1
    fi

    return 0
}

# Test list via daft verb alias
test_list_daft_verb() {
    local remote_repo=$(create_test_remote "test-repo-list-verb" "main")

    git-worktree-clone "$remote_repo" || return 1
    cd "test-repo-list-verb"

    local output
    output=$(daft list 2>&1) || return 1

    if ! echo "$output" | grep -q "main"; then
        log_error "Expected 'main' in daft list output"
        return 1
    fi

    return 0
}

# Test list outside git repo
test_list_not_git_repo() {
    cd "$WORK_DIR"
    mkdir not-a-repo
    cd not-a-repo

    if git-worktree-list 2>/dev/null; then
        log_error "Expected failure outside git repo"
        return 1
    fi

    return 0
}

# --- Run Tests ---
setup
run_test test_list_basic
run_test test_list_multiple
run_test test_list_current_marker
run_test test_list_dirty
run_test test_list_json
run_test test_list_daft_verb
run_test test_list_not_git_repo
teardown
```

**Step 3: Add to test_all.sh**

In `tests/integration/test_all.sh`, add `test_list.sh` to the list of test files
sourced/run.

**Step 4: Make executable**

Run: `chmod +x tests/integration/test_list.sh`

**Step 5: Run integration tests**

Run: `mise run test:integration` (or run the specific test:
`bash tests/integration/test_list.sh`) Expected: All tests pass.

**Step 6: Commit**

```bash
git add tests/integration/test_list.sh tests/integration/test_all.sh tests/integration/test_framework.sh
git commit -m "test(list): add integration tests for daft list"
```

---

### Task 11: Final verification

**Step 1: Format code**

Run: `mise run fmt`

**Step 2: Run clippy**

Run: `mise run clippy` Expected: Zero warnings.

**Step 3: Run all unit tests**

Run: `mise run test:unit` Expected: All pass.

**Step 4: Run integration tests**

Run: `mise run test:integration` Expected: All pass.

**Step 5: Manual smoke test**

Run `daft list` from the current repository to verify it works end-to-end.

**Step 6: Commit any fixes**

If fmt or clippy required changes, commit them.
