# Sync Declared-but-Uncollected Shared Files — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** When `daft shared sync` encounters files declared in `daft.yml` but
not yet collected into `.git/.daft/shared/`, present an interactive TUI that
lets the user pick which worktree's copy to promote to shared storage (or stub
files that don't exist anywhere), then execute the collection.

**Architecture:** Detection and collection logic lives in `src/core/shared.rs`.
The interactive TUI is a new ratatui alternate-screen app in
`src/output/tui/collect_picker/` with tabbed file selection, split worktree
list + syntax-highlighted preview, and a footer with submit/cancel. The existing
`run_sync()` in `src/commands/shared.rs` is extended to detect uncollected
files, launch the TUI when interactive, and execute the resulting decisions.

**Tech Stack:** Rust, ratatui + crossterm (TUI), syntect (syntax highlighting),
existing shared file infrastructure.

---

### Task 1: Add `syntect` dependency

**Files:**

- Modify: `Cargo.toml:73-105`

- [ ] **Step 1: Add syntect to Cargo.toml**

In `Cargo.toml`, add `syntect` after the `dialoguer` line (line 105):

```toml
syntect = { version = "5.2", default-features = false, features = ["default-syntaxes", "default-themes", "regex-onig"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add syntect dependency for syntax highlighting"
```

---

### Task 2: Types and detection logic

**Files:**

- Modify: `src/core/shared.rs`
- Test: unit tests in `src/core/shared.rs`

This task adds the data types for uncollected file discovery and the function
that scans worktrees to build the list.

- [ ] **Step 1: Write the failing test for detection**

Add at the bottom of `src/core/shared.rs` (inside an existing `#[cfg(test)]`
module, or create one):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper: create a minimal git-like directory structure for testing.
    /// Returns (git_common_dir, project_root, vec of worktree paths).
    fn setup_test_repo(
        worktree_names: &[&str],
    ) -> (TempDir, PathBuf, PathBuf, Vec<PathBuf>) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let git_dir = root.join(".git");
        fs::create_dir_all(git_dir.join(".daft/shared")).unwrap();

        let mut wt_paths = Vec::new();
        for name in worktree_names {
            let wt = root.join(name);
            fs::create_dir_all(&wt).unwrap();
            wt_paths.push(wt);
        }

        (tmp, git_dir, root, wt_paths)
    }

    #[test]
    fn detect_uncollected_finds_copies_across_worktrees() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main", "develop"]);

        // Create .env in both worktrees with different content
        fs::write(wt_paths[0].join(".env"), "FROM_MAIN=1").unwrap();
        fs::write(wt_paths[1].join(".env"), "FROM_DEVELOP=1").unwrap();

        let declared = vec![".env".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rel_path, ".env");
        assert_eq!(result[0].copies.len(), 2);
        assert_eq!(result[0].copies[0].worktree_path, wt_paths[0]);
        assert_eq!(result[0].copies[1].worktree_path, wt_paths[1]);
    }

    #[test]
    fn detect_uncollected_skips_already_collected() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);

        // File exists in shared storage already
        fs::write(
            shared_file_path(&git_dir, ".env"),
            "SHARED=1",
        )
        .unwrap();
        fs::write(wt_paths[0].join(".env"), "LOCAL=1").unwrap();

        let declared = vec![".env".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);

        assert!(result.is_empty());
    }

    #[test]
    fn detect_uncollected_file_in_no_worktree() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);

        let declared = vec![".secrets".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].rel_path, ".secrets");
        assert!(result[0].copies.is_empty());
    }

    #[test]
    fn detect_uncollected_ignores_symlinked_worktrees() {
        let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);

        // Create a symlink in the worktree (already linked) — should not count
        let shared_target = shared_file_path(&git_dir, ".env");
        fs::write(&shared_target, "SHARED=1").unwrap();

        let link_path = wt_paths[0].join(".env");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&shared_target, &link_path).unwrap();

        let declared = vec![".env".to_string()];
        let result = detect_uncollected(&declared, &wt_paths, &git_dir);

        // .env IS in shared storage, so detect_uncollected skips it entirely
        assert!(result.is_empty());
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p daft --lib shared::tests -- --nocapture` Expected: FAIL —
`detect_uncollected` doesn't exist yet.

- [ ] **Step 3: Add the types and implement `detect_uncollected`**

Add these types and function to `src/core/shared.rs`, above the `#[cfg(test)]`
module:

```rust
// ── Uncollected file detection ───────────────────────────────────────────

/// A worktree that has a real (non-symlink) copy of a declared shared file.
#[derive(Debug, Clone)]
pub struct WorktreeCopy {
    /// Absolute path of the worktree directory.
    pub worktree_path: PathBuf,
    /// Worktree display name (directory basename).
    pub worktree_name: String,
}

/// A declared shared file that has not yet been collected into shared storage.
#[derive(Debug, Clone)]
pub struct UncollectedFile {
    /// Path relative to the worktree root (e.g., ".env").
    pub rel_path: String,
    /// Worktrees that have a real copy of this file.
    /// Empty if no worktree has it (will be stubbed).
    pub copies: Vec<WorktreeCopy>,
}

/// Scan worktrees for declared shared paths that are not yet in shared storage.
///
/// Returns one `UncollectedFile` per declared path that has no file in
/// `.git/.daft/shared/`. Each entry lists the worktrees that have a real
/// (non-symlink) copy of that file.
pub fn detect_uncollected(
    declared_paths: &[String],
    worktree_paths: &[PathBuf],
    git_common_dir: &Path,
) -> Vec<UncollectedFile> {
    let mut uncollected = Vec::new();

    for rel_path in declared_paths {
        let shared_target = shared_file_path(git_common_dir, rel_path);

        // Already collected — skip
        if shared_target.exists() {
            continue;
        }

        let mut copies = Vec::new();
        for wt in worktree_paths {
            let file_path = wt.join(rel_path);
            // Only count real files/dirs, not symlinks
            if file_path.exists() && !file_path.is_symlink() {
                let name = wt
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                copies.push(WorktreeCopy {
                    worktree_path: wt.clone(),
                    worktree_name: name,
                });
            }
        }

        uncollected.push(UncollectedFile {
            rel_path: rel_path.clone(),
            copies,
        });
    }

    uncollected
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p daft --lib shared::tests -- --nocapture` Expected: All 4
tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/shared.rs
git commit -m "feat(shared): add uncollected file detection logic"
```

---

### Task 3: Collection execution logic

**Files:**

- Modify: `src/core/shared.rs`
- Modify: `src/commands/shared.rs` (import `copy_dir_all`)
- Test: unit tests in `src/core/shared.rs`

This task adds the function that executes collect decisions — moving the chosen
worktree copy to shared storage, marking others as materialized, linking
worktrees that lack the file, and stubbing files that exist nowhere.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `src/core/shared.rs`:

```rust
#[test]
fn execute_collect_moves_to_shared_and_links() {
    let (_tmp, git_dir, root, wt_paths) = setup_test_repo(&["main", "develop", "feature"]);

    // main has a copy, develop has a copy, feature does not
    fs::write(wt_paths[0].join(".env"), "FROM_MAIN=1").unwrap();
    fs::write(wt_paths[1].join(".env"), "FROM_DEVELOP=1").unwrap();

    let decision = CollectDecision {
        rel_path: ".env".to_string(),
        source: CollectSource::FromWorktree(wt_paths[0].clone()),
    };

    let mut materialized = MaterializedState::default();
    execute_collect(&decision, &wt_paths, &git_dir, &root, &mut materialized).unwrap();

    // Shared storage should have the file from main
    let shared = shared_file_path(&git_dir, ".env");
    assert!(shared.exists());
    assert_eq!(fs::read_to_string(&shared).unwrap(), "FROM_MAIN=1");

    // main: should now be a symlink
    let main_env = wt_paths[0].join(".env");
    assert!(main_env.is_symlink());

    // develop: should still be a real file, marked materialized
    let dev_env = wt_paths[1].join(".env");
    assert!(!dev_env.is_symlink());
    assert!(materialized.is_materialized(".env", &wt_paths[1]));

    // feature: should be a symlink (had no file)
    let feat_env = wt_paths[2].join(".env");
    assert!(feat_env.is_symlink());
}

#[test]
fn execute_collect_stubs_when_no_source() {
    let (_tmp, git_dir, root, wt_paths) = setup_test_repo(&["main"]);

    let decision = CollectDecision {
        rel_path: ".secrets".to_string(),
        source: CollectSource::Stub,
    };

    let mut materialized = MaterializedState::default();
    execute_collect(&decision, &wt_paths, &git_dir, &root, &mut materialized).unwrap();

    // Shared storage should have an empty stub
    let shared = shared_file_path(&git_dir, ".secrets");
    assert!(shared.exists());
    assert_eq!(fs::read_to_string(&shared).unwrap(), "");

    // main: should be a symlink
    let main_secrets = wt_paths[0].join(".secrets");
    assert!(main_secrets.is_symlink());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p daft --lib shared::tests -- --nocapture` Expected: FAIL —
`CollectDecision`, `CollectSource`, `execute_collect` don't exist.

- [ ] **Step 3: Implement the types and `execute_collect`**

Add to `src/core/shared.rs`:

```rust
// ── Collection execution ─────────────────────────────────────────────────

/// Where to source the shared file from.
#[derive(Debug, Clone, PartialEq)]
pub enum CollectSource {
    /// Use the copy from this worktree.
    FromWorktree(PathBuf),
    /// No copy exists; create an empty stub.
    Stub,
}

/// A decision about how to collect a declared-but-uncollected shared file.
#[derive(Debug, Clone)]
pub struct CollectDecision {
    /// Path relative to the worktree root (e.g., ".env").
    pub rel_path: String,
    /// Where to get the file content from.
    pub source: CollectSource,
}

/// Execute a single collection decision.
///
/// 1. Moves the file from the chosen worktree to `.git/.daft/shared/`, or
///    creates an empty stub if `source` is `Stub`.
/// 2. Creates symlinks in the source worktree and any worktrees missing the
///    file.
/// 3. Marks all other worktrees that have a real copy as materialized.
/// 4. Ensures the `.gitignore` entry exists.
pub fn execute_collect(
    decision: &CollectDecision,
    worktree_paths: &[PathBuf],
    git_common_dir: &Path,
    project_root: &Path,
    materialized: &mut MaterializedState,
) -> Result<()> {
    let rel_path = &decision.rel_path;
    let shared_target = shared_file_path(git_common_dir, rel_path);

    // Ensure parent dirs in shared storage
    if let Some(parent) = shared_target.parent() {
        fs::create_dir_all(parent)?;
    }

    match &decision.source {
        CollectSource::FromWorktree(source_wt) => {
            let source_file = source_wt.join(rel_path);

            // Move to shared storage (rename, fallback to copy+delete)
            if fs::rename(&source_file, &shared_target).is_err() {
                if source_file.is_dir() {
                    copy_dir_all(&source_file, &shared_target)?;
                    fs::remove_dir_all(&source_file)?;
                } else {
                    fs::copy(&source_file, &shared_target)?;
                    fs::remove_file(&source_file)?;
                }
            }

            // Create symlink in source worktree (file was moved out)
            create_shared_symlink(source_wt, rel_path, git_common_dir)?;
        }
        CollectSource::Stub => {
            // Create empty file
            fs::write(&shared_target, "")
                .with_context(|| format!("Failed to create stub for {rel_path}"))?;
        }
    }

    // Process remaining worktrees
    for wt in worktree_paths {
        // Skip the source worktree (already handled above)
        if let CollectSource::FromWorktree(source_wt) = &decision.source {
            if wt == source_wt {
                continue;
            }
        }

        let file_path = wt.join(rel_path);
        if file_path.exists() && !file_path.is_symlink() {
            // Real copy exists — mark as materialized, don't touch it
            materialized.add(rel_path, wt);
        } else if !file_path.exists() {
            // No file — create symlink
            create_shared_symlink(wt, rel_path, git_common_dir)?;
        }
        // If it's already a symlink, leave it alone
    }

    // Ensure .gitignore entry
    crate::core::layout::ensure_gitignore_entry(project_root, rel_path)?;

    Ok(())
}

/// Recursively copy a directory tree.
pub fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}
```

Note: `copy_dir_all` currently lives as a private function in
`src/commands/shared.rs`. Move it to `src/core/shared.rs` as `pub` and update
the call site in `src/commands/shared.rs` to use `shared::copy_dir_all`.

- [ ] **Step 4: Move `copy_dir_all` from commands to core**

In `src/commands/shared.rs`, remove the `copy_dir_all` function (lines 582-596)
and replace the two call sites (lines 221, 223) with `shared::copy_dir_all`:

```rust
// Line 221 (in run_add):
shared::copy_dir_all(&full_path, &shared_target)?;
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p daft --lib shared::tests -- --nocapture` Expected: All tests
PASS.

- [ ] **Step 6: Commit**

```bash
git add src/core/shared.rs src/commands/shared.rs
git commit -m "feat(shared): add collection execution logic for sync"
```

---

### Task 4: TUI module scaffold and state machine

**Files:**

- Create: `src/output/tui/collect_picker/mod.rs`
- Create: `src/output/tui/collect_picker/state.rs`
- Modify: `src/output/tui/mod.rs` (add module)
- Test: unit tests in `state.rs`

- [ ] **Step 1: Create the module directory**

Run: `mkdir -p src/output/tui/collect_picker`

- [ ] **Step 2: Write the state machine tests**

Create `src/output/tui/collect_picker/state.rs`:

```rust
//! State management for the collect picker TUI.

use crate::core::shared::{CollectDecision, CollectSource, UncollectedFile};
use std::path::PathBuf;

/// Which panel has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusPanel {
    WorktreeList,
    Preview,
    Footer,
}

/// A footer button.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FooterButton {
    Submit,
    Cancel,
}

/// State for a single file tab.
#[derive(Debug, Clone)]
pub struct FileTabState {
    /// The declared shared file path.
    pub rel_path: String,
    /// Worktree copies available for selection.
    pub copies: Vec<CopyEntry>,
    /// Index of the highlighted entry in the worktree list.
    pub list_cursor: usize,
    /// Index of the selected (confirmed) worktree, or None.
    pub selected: Option<usize>,
    /// Vertical scroll offset in the preview panel.
    pub preview_scroll: usize,
    /// Whether this file has no copies (will be stubbed).
    pub is_stub: bool,
}

/// A single entry in the worktree list.
#[derive(Debug, Clone)]
pub struct CopyEntry {
    pub worktree_name: String,
    pub worktree_path: PathBuf,
}

/// Top-level state for the collect picker.
#[derive(Debug)]
pub struct CollectPickerState {
    /// One tab per uncollected file.
    pub tabs: Vec<FileTabState>,
    /// Index of the currently active tab.
    pub active_tab: usize,
    /// Which panel has focus.
    pub focus: FocusPanel,
    /// Which footer button is highlighted.
    pub footer_cursor: FooterButton,
    /// Whether the user has submitted.
    pub submitted: bool,
    /// Whether the user has cancelled.
    pub cancelled: bool,
}

impl CollectPickerState {
    /// Build state from detected uncollected files.
    pub fn new(uncollected: Vec<UncollectedFile>) -> Self {
        let tabs: Vec<FileTabState> = uncollected
            .into_iter()
            .map(|uf| {
                let is_stub = uf.copies.is_empty();
                let copies: Vec<CopyEntry> = uf
                    .copies
                    .into_iter()
                    .map(|c| CopyEntry {
                        worktree_name: c.worktree_name,
                        worktree_path: c.worktree_path,
                    })
                    .collect();
                FileTabState {
                    rel_path: uf.rel_path,
                    copies,
                    list_cursor: 0,
                    selected: None,
                    preview_scroll: 0,
                    is_stub,
                }
            })
            .collect();

        Self {
            tabs,
            active_tab: 0,
            focus: FocusPanel::WorktreeList,
            footer_cursor: FooterButton::Submit,
            submitted: false,
            cancelled: false,
        }
    }

    /// The currently active tab.
    pub fn current_tab(&self) -> &FileTabState {
        &self.tabs[self.active_tab]
    }

    /// The currently active tab (mutable).
    pub fn current_tab_mut(&mut self) -> &mut FileTabState {
        &mut self.tabs[self.active_tab]
    }

    /// Move to the next tab (wraps around).
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            // Reset focus to worktree list when switching tabs
            if !self.current_tab().is_stub {
                self.focus = FocusPanel::WorktreeList;
            }
        }
    }

    /// Move to the previous tab (wraps around).
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = if self.active_tab == 0 {
                self.tabs.len() - 1
            } else {
                self.active_tab - 1
            };
            if !self.current_tab().is_stub {
                self.focus = FocusPanel::WorktreeList;
            }
        }
    }

    /// Move cursor down in the currently focused panel.
    pub fn move_down(&mut self) {
        let tab = &self.tabs[self.active_tab];
        match self.focus {
            FocusPanel::WorktreeList => {
                if tab.is_stub {
                    return;
                }
                let max = tab.copies.len().saturating_sub(1);
                if self.tabs[self.active_tab].list_cursor >= max {
                    // Navigate to footer
                    self.focus = FocusPanel::Footer;
                } else {
                    self.tabs[self.active_tab].list_cursor += 1;
                }
            }
            FocusPanel::Preview => {
                self.tabs[self.active_tab].preview_scroll += 1;
            }
            FocusPanel::Footer => {
                // Already at bottom, no-op
            }
        }
    }

    /// Move cursor up in the currently focused panel.
    pub fn move_up(&mut self) {
        match self.focus {
            FocusPanel::WorktreeList => {
                let cursor = &mut self.tabs[self.active_tab].list_cursor;
                *cursor = cursor.saturating_sub(1);
            }
            FocusPanel::Preview => {
                let scroll = &mut self.tabs[self.active_tab].preview_scroll;
                *scroll = scroll.saturating_sub(1);
            }
            FocusPanel::Footer => {
                // Navigate back to worktree list
                if !self.current_tab().is_stub {
                    self.focus = FocusPanel::WorktreeList;
                }
            }
        }
    }

    /// Toggle focus between worktree list and preview panels.
    pub fn toggle_panel(&mut self) {
        if self.current_tab().is_stub {
            return;
        }
        self.focus = match self.focus {
            FocusPanel::WorktreeList => FocusPanel::Preview,
            FocusPanel::Preview => FocusPanel::WorktreeList,
            FocusPanel::Footer => FocusPanel::WorktreeList,
        };
    }

    /// Select/deselect the highlighted worktree in the current tab.
    pub fn toggle_selection(&mut self) {
        if self.focus != FocusPanel::WorktreeList || self.current_tab().is_stub {
            return;
        }
        let tab = &mut self.tabs[self.active_tab];
        let cursor = tab.list_cursor;
        if tab.selected == Some(cursor) {
            tab.selected = None;
        } else {
            tab.selected = Some(cursor);
        }
    }

    /// Activate the focused footer button.
    pub fn activate_footer(&mut self) {
        if self.focus != FocusPanel::Footer {
            return;
        }
        match self.footer_cursor {
            FooterButton::Submit => self.submitted = true,
            FooterButton::Cancel => self.cancelled = true,
        }
    }

    /// Move footer cursor between Submit and Cancel.
    pub fn footer_next(&mut self) {
        if self.focus == FocusPanel::Footer {
            self.footer_cursor = match self.footer_cursor {
                FooterButton::Submit => FooterButton::Cancel,
                FooterButton::Cancel => FooterButton::Submit,
            };
        }
    }

    /// How many files have a selection (or are stubs).
    pub fn decided_count(&self) -> usize {
        self.tabs
            .iter()
            .filter(|t| t.selected.is_some() || t.is_stub)
            .count()
    }

    /// Whether all files have a decision.
    pub fn all_decided(&self) -> bool {
        self.decided_count() == self.tabs.len()
    }

    /// Whether any non-stub file has a selection.
    pub fn has_any_selection(&self) -> bool {
        self.tabs.iter().any(|t| t.selected.is_some())
    }

    /// Files that were not given a selection (excluding stubs).
    pub fn undecided_files(&self) -> Vec<&str> {
        self.tabs
            .iter()
            .filter(|t| !t.is_stub && t.selected.is_none())
            .map(|t| t.rel_path.as_str())
            .collect()
    }

    /// Build `CollectDecision`s from the current state.
    pub fn into_decisions(self) -> Vec<CollectDecision> {
        self.tabs
            .into_iter()
            .filter_map(|tab| {
                if tab.is_stub {
                    Some(CollectDecision {
                        rel_path: tab.rel_path,
                        source: CollectSource::Stub,
                    })
                } else {
                    tab.selected.map(|idx| CollectDecision {
                        rel_path: tab.rel_path,
                        source: CollectSource::FromWorktree(
                            tab.copies[idx].worktree_path.clone(),
                        ),
                    })
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::shared::WorktreeCopy;

    fn make_uncollected(
        rel_path: &str,
        worktrees: &[(&str, &str)],
    ) -> UncollectedFile {
        UncollectedFile {
            rel_path: rel_path.to_string(),
            copies: worktrees
                .iter()
                .map(|(name, path)| WorktreeCopy {
                    worktree_name: name.to_string(),
                    worktree_path: PathBuf::from(path),
                })
                .collect(),
        }
    }

    #[test]
    fn new_state_initializes_correctly() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main"), ("dev", "/repo/dev")]),
            make_uncollected(".secrets", &[]),
        ];
        let state = CollectPickerState::new(files);

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.active_tab, 0);
        assert_eq!(state.focus, FocusPanel::WorktreeList);
        assert!(!state.tabs[0].is_stub);
        assert!(state.tabs[1].is_stub);
        assert_eq!(state.tabs[0].copies.len(), 2);
    }

    #[test]
    fn tab_navigation_wraps() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main")]),
            make_uncollected(".idea", &[("main", "/repo/main")]),
        ];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.active_tab, 0);
        state.next_tab();
        assert_eq!(state.active_tab, 1);
        state.next_tab();
        assert_eq!(state.active_tab, 0);
        state.prev_tab();
        assert_eq!(state.active_tab, 1);
    }

    #[test]
    fn toggle_selection_sets_and_clears() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main"), ("dev", "/repo/dev")],
        )];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.current_tab().selected, None);
        state.toggle_selection(); // Select cursor 0
        assert_eq!(state.current_tab().selected, Some(0));
        state.toggle_selection(); // Deselect
        assert_eq!(state.current_tab().selected, None);
    }

    #[test]
    fn move_down_navigates_to_footer() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main")],
        )];
        let mut state = CollectPickerState::new(files);

        assert_eq!(state.focus, FocusPanel::WorktreeList);
        state.move_down(); // cursor 0 was at max (only 1 entry)
        assert_eq!(state.focus, FocusPanel::Footer);
    }

    #[test]
    fn move_up_from_footer_returns_to_list() {
        let files = vec![make_uncollected(
            ".env",
            &[("main", "/repo/main")],
        )];
        let mut state = CollectPickerState::new(files);

        state.focus = FocusPanel::Footer;
        state.move_up();
        assert_eq!(state.focus, FocusPanel::WorktreeList);
    }

    #[test]
    fn into_decisions_builds_correctly() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main"), ("dev", "/repo/dev")]),
            make_uncollected(".secrets", &[]),
        ];
        let mut state = CollectPickerState::new(files);

        // Select "dev" for .env
        state.tabs[0].list_cursor = 1;
        state.toggle_selection();

        let decisions = state.into_decisions();
        assert_eq!(decisions.len(), 2);
        assert_eq!(decisions[0].rel_path, ".env");
        assert_eq!(
            decisions[0].source,
            CollectSource::FromWorktree(PathBuf::from("/repo/dev"))
        );
        assert_eq!(decisions[1].rel_path, ".secrets");
        assert_eq!(decisions[1].source, CollectSource::Stub);
    }

    #[test]
    fn undecided_files_excludes_stubs_and_selected() {
        let files = vec![
            make_uncollected(".env", &[("main", "/repo/main")]),
            make_uncollected(".idea", &[("main", "/repo/main")]),
            make_uncollected(".secrets", &[]),
        ];
        let mut state = CollectPickerState::new(files);

        // Select only .env
        state.toggle_selection();

        let undecided = state.undecided_files();
        assert_eq!(undecided, vec![".idea"]);
    }
}
```

- [ ] **Step 3: Create the module file**

Create `src/output/tui/collect_picker/mod.rs`:

```rust
//! Interactive TUI for collecting declared-but-uncollected shared files.
//!
//! Presents a tabbed interface where each tab represents a declared shared
//! file. The user selects which worktree's copy to promote to shared storage,
//! with a syntax-highlighted preview of the file content.

mod highlight;
mod input;
mod render;
pub mod state;

pub use state::CollectPickerState;
```

- [ ] **Step 4: Create stub files for submodules**

Create `src/output/tui/collect_picker/highlight.rs`:

```rust
//! Syntax highlighting for file preview using syntect.
```

Create `src/output/tui/collect_picker/input.rs`:

```rust
//! Keyboard input handling for the collect picker TUI.
```

Create `src/output/tui/collect_picker/render.rs`:

```rust
//! Ratatui rendering for the collect picker TUI.
```

- [ ] **Step 5: Register the module**

In `src/output/tui/mod.rs`, add after line 6:

```rust
pub mod collect_picker;
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p daft --lib collect_picker::state::tests -- --nocapture`
Expected: All tests PASS.

- [ ] **Step 7: Commit**

```bash
git add src/output/tui/collect_picker/
git commit -m "feat(shared): add collect picker state machine with tests"
```

---

### Task 5: Keyboard input handling

**Files:**

- Modify: `src/output/tui/collect_picker/input.rs`

- [ ] **Step 1: Implement the input handler**

Write `src/output/tui/collect_picker/input.rs`:

```rust
//! Keyboard input handling for the collect picker TUI.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::state::{CollectPickerState, FocusPanel, FooterButton};

/// Result of processing a key event.
pub enum InputResult {
    /// Continue the event loop.
    Continue,
    /// User requested cancel (Esc/q).
    Cancel,
    /// User activated Submit from the footer.
    Submit,
}

/// Poll for a key event (blocks up to `timeout`).
/// Returns `None` if no event within the timeout.
pub fn poll_key(timeout: Duration) -> Option<KeyEvent> {
    if event::poll(timeout).ok()? {
        if let Event::Key(key) = event::read().ok()? {
            return Some(key);
        }
    }
    None
}

/// Handle a key event and update state. Returns what the main loop should do.
pub fn handle_key(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    // Global shortcuts
    match key.code {
        KeyCode::Esc => return InputResult::Cancel,
        KeyCode::Char('q') if state.focus != FocusPanel::Preview => {
            return InputResult::Cancel;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            return InputResult::Cancel;
        }
        _ => {}
    }

    match state.focus {
        FocusPanel::WorktreeList => handle_worktree_list(key, state),
        FocusPanel::Preview => handle_preview(key, state),
        FocusPanel::Footer => handle_footer(key, state),
    }
}

fn handle_worktree_list(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        // Vertical navigation
        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
        KeyCode::Up | KeyCode::Char('k') => state.move_up(),

        // Tab navigation
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(),

        // Selection
        KeyCode::Char(' ') | KeyCode::Enter => state.toggle_selection(),

        // Panel toggle
        KeyCode::Tab => state.toggle_panel(),

        _ => {}
    }
    InputResult::Continue
}

fn handle_preview(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        // Scroll preview
        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
        KeyCode::Up | KeyCode::Char('k') => state.move_up(),

        // Tab navigation
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(),

        // Panel toggle
        KeyCode::Tab => state.toggle_panel(),

        _ => {}
    }
    InputResult::Continue
}

fn handle_footer(key: KeyEvent, state: &mut CollectPickerState) -> InputResult {
    match key.code {
        // Vertical navigation — up returns to list
        KeyCode::Up | KeyCode::Char('k') => state.move_up(),

        // Horizontal navigation between buttons
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
            state.footer_next();
        }

        // Tab navigation between file tabs
        KeyCode::Tab => state.toggle_panel(),

        // Activate button
        KeyCode::Enter | KeyCode::Char(' ') => {
            state.activate_footer();
            match state.footer_cursor {
                FooterButton::Submit if state.submitted => return InputResult::Submit,
                FooterButton::Cancel if state.cancelled => return InputResult::Cancel,
                _ => {}
            }
        }

        _ => {}
    }
    InputResult::Continue
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles (the module is imported but not yet called
from anywhere).

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/collect_picker/input.rs
git commit -m "feat(shared): add keyboard input handling for collect picker"
```

---

### Task 6: Syntax highlighting

**Files:**

- Modify: `src/output/tui/collect_picker/highlight.rs`

- [ ] **Step 1: Implement syntax highlighting conversion**

Write `src/output/tui/collect_picker/highlight.rs`:

```rust
//! Syntax highlighting for file preview using syntect.
//!
//! Converts syntect highlighting output to ratatui `Line`s for rendering
//! in the preview panel.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::path::Path;
use syntect::{
    easy::HighlightLines,
    highlighting::{self, ThemeSet},
    parsing::SyntaxSet,
    util::LinesWithEndings,
};

/// Cached syntax highlighting resources.
pub struct Highlighter {
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl Highlighter {
    /// Create a new highlighter with default syntaxes and themes.
    pub fn new() -> Self {
        Self {
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
        }
    }

    /// Highlight file content and return ratatui `Line`s.
    ///
    /// Detects the syntax from the file extension. Falls back to plain text
    /// if the extension is unknown.
    pub fn highlight(&self, content: &str, file_path: &str) -> Vec<Line<'static>> {
        let extension = Path::new(file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");

        let syntax = self
            .syntax_set
            .find_syntax_by_extension(extension)
            .or_else(|| self.syntax_set.find_syntax_by_first_line(content))
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        // Use a dark theme that works well on most terminals
        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        LinesWithEndings::from(content)
            .map(|line| {
                let ranges = highlighter
                    .highlight_line(line, &self.syntax_set)
                    .unwrap_or_default();
                let spans: Vec<Span<'static>> = ranges
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.to_string(), syntect_to_ratatui(style))
                    })
                    .collect();
                Line::from(spans)
            })
            .collect()
    }

    /// Produce plain (unstyled) lines for content that can't be highlighted.
    pub fn plain(content: &str) -> Vec<Line<'static>> {
        content
            .lines()
            .map(|line| Line::raw(line.to_string()))
            .collect()
    }
}

/// Convert a syntect style to a ratatui style.
fn syntect_to_ratatui(style: highlighting::Style) -> Style {
    let fg = style.foreground;
    let mut ratatui_style =
        Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));

    if style.font_style.contains(highlighting::FontStyle::BOLD) {
        ratatui_style = ratatui_style.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(highlighting::FontStyle::ITALIC) {
        ratatui_style = ratatui_style.add_modifier(Modifier::ITALIC);
    }
    if style
        .font_style
        .contains(highlighting::FontStyle::UNDERLINE)
    {
        ratatui_style = ratatui_style.add_modifier(Modifier::UNDERLINED);
    }

    ratatui_style
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/collect_picker/highlight.rs
git commit -m "feat(shared): add syntect-based syntax highlighting for preview"
```

---

### Task 7: TUI renderer

**Files:**

- Modify: `src/output/tui/collect_picker/render.rs`

This is the largest rendering task. Draws tabs, the split worktree list +
preview panel, and the footer with submit/cancel buttons.

- [ ] **Step 1: Implement the renderer**

Write `src/output/tui/collect_picker/render.rs`:

```rust
//! Ratatui rendering for the collect picker TUI.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs, Wrap},
    Frame,
};

use super::highlight::Highlighter;
use super::state::{CollectPickerState, FileTabState, FocusPanel, FooterButton};

/// Accent color matching the project's ACCENT_COLOR_INDEX (orange 208).
const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;
const GREEN: Color = Color::Green;
const SELECTED_BG: Color = Color::Indexed(236);

/// Render the entire collect picker UI.
pub fn render(state: &CollectPickerState, highlighter: &Highlighter, frame: &mut Frame) {
    let area = frame.area();

    // Clear the screen
    frame.render_widget(Clear, area);

    // Layout: tabs (2 rows) | body (fill) | footer (3 rows)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Tabs
            Constraint::Min(5),   // Body
            Constraint::Length(3), // Footer
        ])
        .split(area);

    render_tabs(state, frame, chunks[0]);
    render_body(state, highlighter, frame, chunks[1]);
    render_footer(state, frame, chunks[2]);
}

/// Render the tab bar at the top.
fn render_tabs(state: &CollectPickerState, frame: &mut Frame, area: Rect) {
    let titles: Vec<Line> = state
        .tabs
        .iter()
        .map(|tab| {
            let has_decision = tab.selected.is_some() || tab.is_stub;
            let icon = if has_decision { " \u{2713}" } else { "" };
            let style = if has_decision {
                Style::default().fg(GREEN)
            } else {
                Style::default()
            };
            Line::from(Span::styled(
                format!(" {}{} ", tab.rel_path, icon),
                style,
            ))
        })
        .collect();

    let tabs = Tabs::new(titles)
        .select(state.active_tab)
        .highlight_style(
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
        )
        .divider(Span::raw(" | "));

    frame.render_widget(tabs, area);
}

/// Render the main body — either the split worktree/preview or the stub
/// message.
fn render_body(
    state: &CollectPickerState,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    let tab = state.current_tab();

    if tab.is_stub {
        render_stub_body(tab, frame, area);
    } else {
        render_split_body(state, tab, highlighter, frame, area);
    }
}

/// Render the stub body for files that exist in no worktree.
fn render_stub_body(tab: &FileTabState, frame: &mut Frame, area: Rect) {
    let text = vec![
        Line::raw(""),
        Line::styled(
            "  No copies found in any worktree.",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(
            "  This file will be created as an empty stub",
            Style::default().fg(DIM),
        ),
        Line::styled(
            "  and linked to all worktrees.",
            Style::default().fg(DIM),
        ),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            format!(" {} ", tab.rel_path),
            Style::default()
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

/// Render the split body with worktree list (left) and preview (right).
fn render_split_body(
    state: &CollectPickerState,
    tab: &FileTabState,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    // Split horizontally: 30% worktree list, 70% preview
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);

    render_worktree_list(state, tab, frame, chunks[0]);
    render_preview(state, tab, highlighter, frame, chunks[1]);
}

/// Render the worktree list panel (left).
fn render_worktree_list(
    state: &CollectPickerState,
    tab: &FileTabState,
    frame: &mut Frame,
    area: Rect,
) {
    let is_focused = state.focus == FocusPanel::WorktreeList;
    let border_color = if is_focused { ACCENT } else { DIM };

    let items: Vec<ListItem> = tab
        .copies
        .iter()
        .enumerate()
        .map(|(idx, copy)| {
            let is_cursor = idx == tab.list_cursor && is_focused;
            let is_selected = tab.selected == Some(idx);

            let marker = if is_selected {
                "\u{2713} "
            } else {
                "  "
            };
            let pointer = if is_cursor { "\u{25b8} " } else { "  " };

            let style = if is_selected {
                Style::default()
                    .fg(GREEN)
                    .add_modifier(Modifier::BOLD)
            } else if is_cursor {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(DIM)
            };

            let bg_style = if is_cursor {
                style.bg(SELECTED_BG)
            } else {
                style
            };

            ListItem::new(Line::from(vec![
                Span::styled(pointer, bg_style),
                Span::styled(marker, bg_style),
                Span::styled(copy.worktree_name.clone(), bg_style),
            ]))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " Worktrees ",
            Style::default().fg(border_color),
        ));

    let list = List::new(items).block(block);
    frame.render_widget(list, area);
}

/// Render the file preview panel (right).
fn render_preview(
    state: &CollectPickerState,
    tab: &FileTabState,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    let is_focused = state.focus == FocusPanel::Preview;
    let border_color = if is_focused { ACCENT } else { DIM };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            " Preview ",
            Style::default().fg(border_color),
        ));

    // Read file content from the currently highlighted worktree
    let content = if tab.copies.is_empty() {
        String::new()
    } else {
        let wt = &tab.copies[tab.list_cursor];
        let file_path = wt.worktree_path.join(&tab.rel_path);
        std::fs::read_to_string(&file_path).unwrap_or_else(|_| "(unable to read file)".into())
    };

    let highlighted_lines = if content.is_empty() {
        vec![Line::styled(
            "(empty file)",
            Style::default().fg(DIM),
        )]
    } else {
        highlighter.highlight(&content, &tab.rel_path)
    };

    let paragraph = Paragraph::new(highlighted_lines)
        .block(block)
        .scroll((tab.preview_scroll as u16, 0));

    frame.render_widget(paragraph, area);
}

/// Render the footer with Submit and Cancel buttons.
fn render_footer(state: &CollectPickerState, frame: &mut Frame, area: Rect) {
    let is_focused = state.focus == FocusPanel::Footer;
    let all_decided = state.all_decided();

    let submit_check = if all_decided { " \u{2713}" } else { "" };

    let submit_style = if is_focused && state.footer_cursor == FooterButton::Submit {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(ACCENT)
    };

    let cancel_style = if is_focused && state.footer_cursor == FooterButton::Cancel {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Red)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };

    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(format!(" Submit{submit_check} "), submit_style),
        Span::raw("  "),
        Span::styled(" Cancel ", cancel_style),
        Span::raw("  "),
        Span::styled(
            format!(
                "{}/{} files ready",
                state.decided_count(),
                state.tabs.len()
            ),
            Style::default().fg(DIM),
        ),
    ]);

    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(DIM));

    let paragraph = Paragraph::new(line).block(block);
    frame.render_widget(paragraph, area);
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles (functions are defined but not yet called
from the main loop).

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/collect_picker/render.rs
git commit -m "feat(shared): add ratatui renderer for collect picker"
```

---

### Task 8: Main TUI event loop and confirmation dialogs

**Files:**

- Modify: `src/output/tui/collect_picker/mod.rs`

This task implements the top-level `run_collect_picker()` function that sets up
the terminal, runs the event loop, and handles the confirmation dialogs for
cancel-with-selections and partial submit.

- [ ] **Step 1: Implement the event loop and confirmations**

Replace `src/output/tui/collect_picker/mod.rs` with:

```rust
//! Interactive TUI for collecting declared-but-uncollected shared files.
//!
//! Presents a tabbed interface where each tab represents a declared shared
//! file. The user selects which worktree's copy to promote to shared storage,
//! with a syntax-highlighted preview of the file content.

mod highlight;
mod input;
mod render;
pub mod state;

use anyhow::Result;
use crossterm::{
    cursor,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::time::Duration;

use crate::core::shared::{CollectDecision, UncollectedFile};
use highlight::Highlighter;
use input::InputResult;
pub use state::CollectPickerState;

/// Outcome of the collect picker.
pub enum PickerOutcome {
    /// User submitted — execute these decisions.
    Decisions(Vec<CollectDecision>),
    /// User cancelled — do nothing.
    Cancelled,
}

/// Run the interactive collect picker TUI.
///
/// Enters alternate screen mode, runs the event loop, and returns the user's
/// decisions. Restores the terminal on exit (including on panic).
pub fn run_collect_picker(uncollected: Vec<UncollectedFile>) -> Result<PickerOutcome> {
    // Set up terminal
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let highlighter = Highlighter::new();
    let mut state = CollectPickerState::new(uncollected);

    let result = run_event_loop(&mut terminal, &mut state, &highlighter);

    // Restore terminal
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    match result {
        Ok(true) => Ok(PickerOutcome::Decisions(state.into_decisions())),
        Ok(false) => Ok(PickerOutcome::Cancelled),
        Err(e) => Err(e),
    }
}

/// Inner event loop. Returns `Ok(true)` for submit, `Ok(false)` for cancel.
fn run_event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    state: &mut CollectPickerState,
    highlighter: &Highlighter,
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            render::render(state, highlighter, frame);
        })?;

        let Some(key) = input::poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match input::handle_key(key, state) {
            InputResult::Continue => {}
            InputResult::Cancel => {
                if state.has_any_selection() {
                    // Ask confirmation
                    let confirmed = show_cancel_confirm(terminal)?;
                    if confirmed {
                        return Ok(false);
                    }
                    // Reset cancel flag so the loop continues
                    state.cancelled = false;
                } else {
                    return Ok(false);
                }
            }
            InputResult::Submit => {
                if !state.all_decided() {
                    let undecided = state.undecided_files()
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                    let confirmed = show_partial_submit_confirm(terminal, &undecided)?;
                    if confirmed {
                        return Ok(true);
                    }
                    // Reset submit flag so the loop continues
                    state.submitted = false;
                } else {
                    return Ok(true);
                }
            }
        }
    }
}

/// Show a "are you sure you want to cancel?" dialog.
/// Returns true if the user confirms cancellation.
fn show_cancel_confirm(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
) -> Result<bool> {
    show_confirm_dialog(
        terminal,
        "Cancel sync?",
        &["You have selections that will be lost.", "Are you sure?"],
    )
}

/// Show a "partial submit" confirmation dialog.
/// Returns true if the user confirms the partial submit.
fn show_partial_submit_confirm(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    undecided: &[String],
) -> Result<bool> {
    let mut lines = vec!["The following files have no copy selected:".to_string(), String::new()];
    for file in undecided {
        lines.push(format!("  \u{2022} {file}"));
    }
    lines.push(String::new());
    lines.push("They will be skipped. Continue?".to_string());

    let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
    show_confirm_dialog(terminal, "Partial submit", &line_refs)
}

/// Generic yes/no confirmation dialog rendered as an overlay.
/// Returns true for yes (Enter/y), false for no (Esc/n).
fn show_confirm_dialog(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
) -> Result<bool> {
    loop {
        terminal.draw(|frame| {
            let area = frame.area();

            // Center a box in the terminal
            let dialog_width = 50u16.min(area.width.saturating_sub(4));
            let dialog_height = (body_lines.len() as u16 + 5).min(area.height.saturating_sub(2));
            let x = (area.width.saturating_sub(dialog_width)) / 2;
            let y = (area.height.saturating_sub(dialog_height)) / 2;
            let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

            // Clear the area behind the dialog
            frame.render_widget(Clear, dialog_area);

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow))
                .title(Span::styled(
                    format!(" {title} "),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));

            let mut text_lines: Vec<Line> = body_lines
                .iter()
                .map(|&line| Line::raw(line.to_string()))
                .collect();
            text_lines.push(Line::raw(""));
            text_lines.push(Line::from(vec![
                Span::styled(
                    " [Y]es ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(" [N]o ", Style::default().fg(Color::DarkGray)),
            ]));

            let paragraph = Paragraph::new(text_lines)
                .block(block)
                .wrap(Wrap { trim: false });

            frame.render_widget(paragraph, dialog_area);
        })?;

        if let Some(key) = input::poll_key(Duration::from_millis(100)) {
            match key.code {
                crossterm::event::KeyCode::Char('y') | crossterm::event::KeyCode::Enter => {
                    return Ok(true);
                }
                crossterm::event::KeyCode::Char('n') | crossterm::event::KeyCode::Esc => {
                    return Ok(false);
                }
                _ => {}
            }
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles with no errors.

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/collect_picker/mod.rs
git commit -m "feat(shared): add TUI event loop with confirmation dialogs"
```

---

### Task 9: Wire into `run_sync`

**Files:**

- Modify: `src/commands/shared.rs`

This task updates `run_sync()` to detect uncollected files, launch the TUI when
interactive, execute the resulting decisions, and then proceed with the normal
symlink sync.

- [ ] **Step 1: Write the failing integration test first**

Create `tests/manual/scenarios/shared/sync-collect.yml`:

```yaml
name: Shared sync collects declared files
description: >
  When daft.yml declares shared files but they haven't been collected, sync in
  non-interactive mode reports what needs collection.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and create two worktrees
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd $WORK_DIR/test-repo
      git-worktree-checkout develop
    expect:
      exit_code: 0

  - name: Create .env in both worktrees and declare in daft.yml
    run: |
      echo "FROM_MAIN=1" > main/.env
      echo "FROM_DEVELOP=1" > develop/.env
      echo ".env" >> .gitignore
      printf 'shared:\n  - .env\n' > daft.yml
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Sync reports uncollected in non-interactive mode
    run: daft shared sync
    cwd: "$WORK_DIR/test-repo/main"
    env:
      DAFT_TESTING: "1"
    expect:
      exit_code: 0
      output_contains:
        - "1 declared file"
        - "not yet collected"
        - ".env"
```

- [ ] **Step 2: Run to verify it fails**

Run: `mise run test:manual -- --ci sync-collect` Expected: FAIL — current sync
doesn't print anything about uncollected files.

- [ ] **Step 3: Update `run_sync` in `src/commands/shared.rs`**

Replace the `run_sync` function (lines 539-580) with:

```rust
fn run_sync(output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let project_root = repo::get_project_root()?;
    let shared_paths = shared::read_shared_paths(&project_root)?;
    let worktree_paths = shared::list_worktree_paths()?;
    let mut materialized = shared::MaterializedState::load(&git_common_dir)?;

    if shared_paths.is_empty() {
        output.info("No shared files declared.");
        return Ok(());
    }

    // Phase 1: Detect declared-but-uncollected files
    let uncollected =
        shared::detect_uncollected(&shared_paths, &worktree_paths, &git_common_dir);

    if !uncollected.is_empty() {
        let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stderr())
            && std::env::var("DAFT_TESTING").is_err();

        if is_interactive {
            // Launch interactive TUI
            use crate::output::tui::collect_picker::{run_collect_picker, PickerOutcome};

            match run_collect_picker(uncollected)? {
                PickerOutcome::Decisions(decisions) => {
                    for decision in &decisions {
                        shared::execute_collect(
                            decision,
                            &worktree_paths,
                            &git_common_dir,
                            &project_root,
                            &mut materialized,
                        )?;
                        output.success(&format!("Collected: {}", decision.rel_path));
                    }
                    materialized.save(&git_common_dir)?;
                }
                PickerOutcome::Cancelled => {
                    output.info("Sync cancelled.");
                    return Ok(());
                }
            }
        } else {
            // Non-interactive: report what needs collection
            let count = uncollected.len();
            let files: Vec<&str> = uncollected.iter().map(|u| u.rel_path.as_str()).collect();
            output.info(&format!(
                "{} declared file{} not yet collected: {}",
                count,
                if count == 1 { "" } else { "s" },
                files.join(", ")
            ));
            output.info("Run `daft shared sync` interactively to collect them.");
        }
    }

    // Phase 2: Normal sync — ensure symlinks for all collected shared files
    for wt in &worktree_paths {
        let wt_name = wt
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        for rel_path in &shared_paths {
            if materialized.is_materialized(rel_path, wt) {
                continue;
            }

            match shared::create_shared_symlink(wt, rel_path, &git_common_dir)? {
                shared::LinkResult::Created => {
                    output.success(&format!("{}: {} \u{2192} symlinked", wt_name, rel_path));
                }
                shared::LinkResult::AlreadyLinked => {}
                shared::LinkResult::Conflict => {
                    output.warning(&format!(
                        "{}: {} exists (not shared) \u{2014} run `daft shared link {}` to replace",
                        wt_name, rel_path, rel_path
                    ));
                }
                shared::LinkResult::NoSource => {}
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Add import at top of `src/commands/shared.rs`**

Add `use std::io::IsTerminal;` to the imports if not already present. The `use`
for `crate::output::tui::collect_picker` is inside the function body
(conditional import), so no top-level change is needed for that.

- [ ] **Step 5: Verify it compiles and the test passes**

Run: `cargo check` Run: `mise run test:manual -- --ci sync-collect` Expected:
Both pass.

- [ ] **Step 6: Commit**

```bash
git add src/commands/shared.rs tests/manual/scenarios/shared/sync-collect.yml
git commit -m "feat(shared): sync detects uncollected files and launches interactive picker"
```

---

### Task 10: Integration tests for end-to-end collection

**Files:**

- Create: `tests/manual/scenarios/shared/sync-collect-stub.yml`
- Modify: `tests/manual/scenarios/shared/sync-collect.yml` (extend)

These tests verify the non-interactive paths (the TUI itself requires manual
testing). We test that after collection has happened (simulated by directly
placing files in shared storage), sync creates the expected symlinks.

- [ ] **Step 1: Add stub collection test**

Create `tests/manual/scenarios/shared/sync-collect-stub.yml`:

```yaml
name: Shared sync creates stubs for missing files
description: >
  When a declared shared file doesn't exist in any worktree AND has been stubbed
  into shared storage, sync links it to all worktrees.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and create worktree
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd $WORK_DIR/test-repo
      git-worktree-checkout develop
    expect:
      exit_code: 0

  - name: Declare .secrets in daft.yml and manually stub it
    run: |
      printf 'shared:\n  - .secrets\n' > daft.yml
      mkdir -p .git/.daft/shared
      touch .git/.daft/shared/.secrets
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Sync links stub to all worktrees
    run: daft shared sync
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify main has symlink
    run: test -L .secrets
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Verify develop has symlink
    run: test -L .secrets
    cwd: "$WORK_DIR/test-repo/develop"
    expect:
      exit_code: 0
```

- [ ] **Step 2: Run all shared tests**

Run: `mise run test:manual -- --ci shared` Expected: All shared scenarios PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/shared/
git commit -m "test(shared): add integration tests for sync collection"
```

---

### Task 11: Unit tests for `execute_collect` with `.gitignore`

**Files:**

- Modify: `src/core/shared.rs` (tests module)

- [ ] **Step 1: Add test verifying gitignore entry is created**

Add to the `tests` module in `src/core/shared.rs`:

```rust
#[test]
fn execute_collect_ensures_gitignore_entry() {
    let (_tmp, git_dir, root, wt_paths) = setup_test_repo(&["main"]);

    fs::write(wt_paths[0].join(".env"), "VAL=1").unwrap();

    // Create a .gitignore without the entry
    fs::write(root.join(".gitignore"), "*.log\n").unwrap();

    let decision = CollectDecision {
        rel_path: ".env".to_string(),
        source: CollectSource::FromWorktree(wt_paths[0].clone()),
    };

    let mut materialized = MaterializedState::default();
    execute_collect(&decision, &wt_paths, &git_dir, &root, &mut materialized).unwrap();

    let gitignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(gitignore.contains(".env"));
}
```

- [ ] **Step 2: Run the test**

Run:
`cargo test -p daft --lib shared::tests::execute_collect_ensures_gitignore_entry -- --nocapture`
Expected: PASS (the `execute_collect` function already calls
`ensure_gitignore_entry`).

- [ ] **Step 3: Commit**

```bash
git add src/core/shared.rs
git commit -m "test(shared): verify execute_collect creates .gitignore entry"
```

---

### Task 12: Reload materialized state after collection

**Files:**

- Modify: `src/commands/shared.rs`

After executing collection decisions, `MaterializedState` is saved. The normal
sync phase (Phase 2) already uses the in-memory `materialized` variable, which
has been updated by `execute_collect`. Verify this works correctly.

- [ ] **Step 1: Add an integration test for materialization during sync**

Create `tests/manual/scenarios/shared/sync-collect-materialize.yml`:

```yaml
name: Shared sync materializes non-selected worktree copies
description: >
  After sync collects a file from one worktree, other worktrees that had their
  own copy keep their local version and are marked as materialized. This test
  simulates the post-collection state.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone and create worktrees
    run: |
      git-worktree-clone --layout contained $REMOTE_TEST_REPO
      cd $WORK_DIR/test-repo
      git-worktree-checkout develop
    expect:
      exit_code: 0

  - name: Simulate collection from main with develop materialized
    run: |
      printf 'shared:\n  - .env\n' > daft.yml
      echo ".env" >> .gitignore
      mkdir -p .git/.daft/shared
      echo "FROM_MAIN=1" > .git/.daft/shared/.env
      echo "FROM_DEVELOP=1" > develop/.env
      printf '{ ".env": ["%s"] }' "$WORK_DIR/test-repo/develop" > .git/.daft/materialized.json
    cwd: "$WORK_DIR/test-repo"
    expect:
      exit_code: 0

  - name: Sync respects materialized state
    run: daft shared sync
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Main has symlink
    run: test -L .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Develop still has its local copy (not a symlink)
    run: |
      test -f .env && ! test -L .env
    cwd: "$WORK_DIR/test-repo/develop"
    expect:
      exit_code: 0

  - name: Develop local copy has original content
    run: "true"
    expect:
      exit_code: 0
      file_contains:
        - path: "$WORK_DIR/test-repo/develop/.env"
          content: "FROM_DEVELOP=1"
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci sync-collect-materialize` Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add tests/manual/scenarios/shared/sync-collect-materialize.yml
git commit -m "test(shared): verify sync respects materialized state after collection"
```

---

### Task 13: Formatting, linting, and full test suite

**Files:**

- All modified files

- [ ] **Step 1: Format**

Run: `mise run fmt`

- [ ] **Step 2: Lint**

Run: `mise run clippy` Expected: Zero warnings.

- [ ] **Step 3: Unit tests**

Run: `mise run test:unit` Expected: All pass.

- [ ] **Step 4: Integration tests**

Run: `mise run test:integration` Expected: All pass.

- [ ] **Step 5: Fix any issues found**

If any step above fails, fix the issue and re-run.

- [ ] **Step 6: Final commit if any formatting/lint fixes**

```bash
git add -A
git commit -m "style: format and lint fixes for sync collect feature"
```

---

## Self-Review Checklist

1. **Spec coverage:**

   - Uncollected file detection: Task 2
   - TUI with tabs: Task 7 (render) + Task 4 (state)
   - Left panel worktree list with navigation: Task 7 (render) + Task 5 (input)
   - Right panel syntax-highlighted preview: Task 7 + Task 6
   - Tab key toggles panels: Task 5 (input) + Task 4 (state)
   - Arrow + hjkl navigation: Task 5
   - Space/Enter selection with checkmark: Task 4 (state) + Task 7 (render)
   - Tab navigation with hl/arrow: Task 5
   - Footer with Submit/Cancel: Task 7 (render) + Task 5 (input)
   - Esc/q to cancel: Task 5
   - Cancel confirmation when selections exist: Task 8
   - Partial submit confirmation with file list: Task 8
   - Submit checkmark when all decided: Task 7 (render)
   - Stub files when no copies exist: Task 3 (execution) + Task 7 (render stub
     body)
   - Non-selected copies marked materialized: Task 3
   - Missing worktrees get symlinks: Task 3
   - .gitignore entries ensured: Task 3

2. **Placeholder scan:** No TBD/TODO/placeholders found.

3. **Type consistency:**
   - `UncollectedFile` used consistently in detection (Task 2) and TUI state
     (Task 4)
   - `CollectDecision`/`CollectSource` used in execution (Task 3) and
     state->decisions (Task 4)
   - `LinkResult` enum used unchanged from existing code
   - `MaterializedState` used in execution and sync
   - `Highlighter::highlight` and `Highlighter::plain` called correctly in
     render
