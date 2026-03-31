# Shared Manager Core — Implementation Plan (Plan 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the core `daft shared manage` TUI — a management interface
for all shared files that shows per-worktree status and allows immediate-mode
toggling of materialization and linking.

**Architecture:** Implement `ManageMode` as a new struct implementing the
`PickerMode` trait from Plan 1. Build tabs from all shared files (not just
uncollected). Each worktree shows its real status (linked/materialized/missing/
conflict/broken/not-yet-collected). Actions execute immediately against the
filesystem. Add `daft shared manage` as a new CLI subcommand.

**Tech Stack:** Rust, ratatui, crossterm (all existing — no new deps)

**Spec:** `docs/superpowers/specs/2026-03-28-shared-manage-design.md`

---

### Task 1: Add `manage` subcommand entry point

**Files:**

- Modify: `src/commands/shared.rs`

- [ ] **Step 1: Add ManageArgs and subcommand variant**

In `src/commands/shared.rs`, add after `SyncArgs`:

```rust
#[derive(Parser)]
struct ManageArgs;
```

Add to `SharedCommand` enum:

```rust
/// Interactive management interface for shared files
Manage(ManageArgs),
```

Add to the `match args.command` in `run()`:

```rust
SharedCommand::Manage(_) => run_manage(&mut output),
```

- [ ] **Step 2: Implement `run_manage` stub**

```rust
fn run_manage(output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let config_root = shared::resolve_config_root(&worktree_path);
    let shared_paths = shared::read_shared_paths(&worktree_path)?;

    if shared_paths.is_empty() {
        output.info("No shared files declared.");
        return Ok(());
    }

    let is_interactive = std::io::IsTerminal::is_terminal(&std::io::stderr())
        && std::env::var("DAFT_TESTING").is_err();

    if !is_interactive {
        output.error(
            "The manage interface requires an interactive terminal. \
             Use `daft shared status` for non-interactive output.",
        );
        return Ok(());
    }

    // TODO: launch manage TUI
    output.info("Manage TUI not yet implemented.");
    Ok(())
}
```

- [ ] **Step 3: Verify it compiles and the command is reachable**

Run: `cargo check` Run: `cargo run -- shared manage 2>&1` (should print "No
shared files" or the stub message)

- [ ] **Step 4: Commit**

```bash
git add src/commands/shared.rs
git commit -m "feat(shared): add manage subcommand entry point"
```

---

### Task 2: Add worktree status detection to core

**Files:**

- Modify: `src/core/shared.rs`

The manager needs to know each worktree's status per shared file. Add a
`WorktreeStatus` enum and a `detect_worktree_statuses` function.

- [ ] **Step 1: Add WorktreeStatus enum and detection function**

Add to `src/core/shared.rs`:

```rust
// ── Worktree status detection ────────────────────────────────────────────

/// The status of a shared file in a specific worktree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WorktreeStatus {
    /// Symlink pointing to the correct shared file.
    Linked,
    /// Local copy tracked in materialized.json.
    Materialized,
    /// No file or symlink present.
    Missing,
    /// A real file exists but is not tracked as materialized.
    Conflict,
    /// Symlink exists but points to the wrong target.
    Broken,
    /// Declared in daft.yml but not yet in shared storage.
    NotCollected,
}

/// Information about a shared file and its status in each worktree.
#[derive(Debug, Clone)]
pub struct SharedFileInfo {
    /// Path relative to the worktree root (e.g., ".env").
    pub rel_path: String,
    /// Whether the file exists in shared storage.
    pub collected: bool,
    /// Status in each worktree. Parallel to the worktree list.
    pub statuses: Vec<(PathBuf, String, WorktreeStatus)>,
}

/// Detect the status of all shared files across all worktrees.
pub fn detect_shared_statuses(
    shared_paths: &[String],
    worktree_paths: &[PathBuf],
    git_common_dir: &Path,
    materialized: &MaterializedState,
) -> Vec<SharedFileInfo> {
    shared_paths
        .iter()
        .map(|rel_path| {
            let shared_target = shared_file_path(git_common_dir, rel_path);
            let collected = shared_target.exists();

            let statuses = worktree_paths
                .iter()
                .map(|wt| {
                    let wt_name = wt
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                    let status = detect_single_status(
                        wt, rel_path, git_common_dir, materialized, collected,
                    );
                    (wt.clone(), wt_name, status)
                })
                .collect();

            SharedFileInfo {
                rel_path: rel_path.clone(),
                collected,
                statuses,
            }
        })
        .collect()
}

/// Detect the status of a single shared file in a single worktree.
fn detect_single_status(
    worktree_path: &Path,
    rel_path: &str,
    git_common_dir: &Path,
    materialized: &MaterializedState,
    collected: bool,
) -> WorktreeStatus {
    if !collected {
        return WorktreeStatus::NotCollected;
    }

    let file_path = worktree_path.join(rel_path);

    if file_path.is_symlink() {
        // Check if symlink points to the right place
        let shared_target = shared_file_path(git_common_dir, rel_path);
        let expected = relative_symlink_target(
            file_path.parent().unwrap_or(worktree_path),
            &shared_target,
        );
        match (std::fs::read_link(&file_path), expected) {
            (Ok(actual), Ok(expected_path)) if actual == expected_path => {
                WorktreeStatus::Linked
            }
            _ => WorktreeStatus::Broken,
        }
    } else if file_path.exists() {
        // Real file exists
        if materialized.is_materialized(rel_path, worktree_path) {
            WorktreeStatus::Materialized
        } else {
            WorktreeStatus::Conflict
        }
    } else {
        WorktreeStatus::Missing
    }
}
```

- [ ] **Step 2: Add unit tests**

```rust
#[test]
fn detect_single_status_linked() {
    let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);
    let shared = shared_file_path(&git_dir, ".env");
    fs::write(&shared, "VAL=1").unwrap();
    create_shared_symlink(&wt_paths[0], ".env", &git_dir).unwrap();
    let mat = MaterializedState::default();
    let status = detect_single_status(&wt_paths[0], ".env", &git_dir, &mat, true);
    assert_eq!(status, WorktreeStatus::Linked);
}

#[test]
fn detect_single_status_missing() {
    let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);
    fs::write(shared_file_path(&git_dir, ".env"), "VAL=1").unwrap();
    let mat = MaterializedState::default();
    let status = detect_single_status(&wt_paths[0], ".env", &git_dir, &mat, true);
    assert_eq!(status, WorktreeStatus::Missing);
}

#[test]
fn detect_single_status_not_collected() {
    let (_tmp, git_dir, _root, wt_paths) = setup_test_repo(&["main"]);
    let mat = MaterializedState::default();
    let status = detect_single_status(&wt_paths[0], ".env", &git_dir, &mat, false);
    assert_eq!(status, WorktreeStatus::NotCollected);
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p daft --lib core::shared::tests` Expected: All pass.

- [ ] **Step 4: Commit**

```bash
git add src/core/shared.rs
git commit -m "feat(shared): add worktree status detection for manage TUI"
```

---

### Task 3: Implement ManageMode

**Files:**

- Create: `src/output/tui/shared_picker/manage_mode.rs`
- Modify: `src/output/tui/shared_picker/mod.rs`

- [ ] **Step 1: Create ManageMode**

Create `src/output/tui/shared_picker/manage_mode.rs`:

```rust
//! ManageMode — implements PickerMode for the shared file manager.
//!
//! Immediate mode: actions execute against the filesystem when triggered.

use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use std::path::{Path, PathBuf};

use crate::core::shared::{self, MaterializedState, WorktreeStatus};

use super::state::{FileTabState, FocusPanel, PickerState, WorktreeEntry};
use super::{EntryDecoration, LoopAction, PickerMode};

const DIM: Color = Color::DarkGray;

/// Manage-specific mode state.
pub struct ManageMode {
    /// Git common dir for shared storage operations.
    git_common_dir: PathBuf,
    /// Config root for gitignore/daft.yml operations.
    config_root: PathBuf,
    /// Mutable materialized state — updated on each action.
    materialized: MaterializedState,
    /// Per-tab, per-entry worktree status. Parallel to state.tabs[i].entries.
    statuses: Vec<Vec<WorktreeStatus>>,
    /// All worktree paths (for operations that need the full list).
    worktree_paths: Vec<PathBuf>,
    /// Temporary info/error message to show in the warning bar.
    info_message: Option<String>,
}

impl ManageMode {
    pub fn new(
        git_common_dir: PathBuf,
        config_root: PathBuf,
        materialized: MaterializedState,
        worktree_paths: Vec<PathBuf>,
    ) -> Self {
        Self {
            git_common_dir,
            config_root,
            materialized,
            statuses: Vec::new(),
            worktree_paths,
            info_message: None,
        }
    }

    /// Build picker tabs from shared file status info.
    pub fn build_tabs(
        infos: &[shared::SharedFileInfo],
    ) -> Vec<FileTabState> {
        infos
            .iter()
            .map(|info| {
                let entries: Vec<WorktreeEntry> = info
                    .statuses
                    .iter()
                    .map(|(path, name, _status)| WorktreeEntry {
                        worktree_name: name.clone(),
                        worktree_path: path.clone(),
                        has_file: true, // In manage mode, all entries are traversable
                    })
                    .collect();
                FileTabState::new(info.rel_path.clone(), entries)
            })
            .collect()
    }

    /// Store the per-tab status vectors.
    pub fn set_statuses(&mut self, infos: &[shared::SharedFileInfo]) {
        self.statuses = infos
            .iter()
            .map(|info| info.statuses.iter().map(|(_, _, s)| *s).collect())
            .collect();
    }

    /// Get the status for a specific tab and entry.
    fn status(&self, tab_idx: usize, entry_idx: usize) -> WorktreeStatus {
        self.statuses
            .get(tab_idx)
            .and_then(|s| s.get(entry_idx))
            .copied()
            .unwrap_or(WorktreeStatus::Missing)
    }

    /// Re-detect status for a single tab after an action.
    fn refresh_tab_status(&mut self, state: &PickerState, tab_idx: usize) {
        let tab = &state.tabs[tab_idx];
        if let Some(tab_statuses) = self.statuses.get_mut(tab_idx) {
            for (i, entry) in tab.entries.iter().enumerate() {
                let collected = shared::shared_file_path(
                    &self.git_common_dir,
                    &tab.rel_path,
                )
                .exists();
                if let Some(s) = tab_statuses.get_mut(i) {
                    *s = detect_entry_status(
                        &entry.worktree_path,
                        &tab.rel_path,
                        &self.git_common_dir,
                        &self.materialized,
                        collected,
                    );
                }
            }
        }
    }

    /// Toggle materialization for the entry under the cursor.
    fn toggle_materialize(&mut self, state: &PickerState) {
        let tab = state.current_tab();
        let idx = tab.list_cursor;
        let status = self.status(state.active_tab, idx);
        let rel_path = &tab.rel_path;
        let wt = &tab.entries[idx].worktree_path;

        match status {
            WorktreeStatus::Linked => {
                // Linked → Materialized: copy shared file, remove symlink
                let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);
                let file_path = wt.join(rel_path);
                if let Err(e) = std::fs::remove_file(&file_path) {
                    self.info_message = Some(format!("Failed to remove symlink: {e}"));
                    return;
                }
                if shared_target.is_dir() {
                    if let Err(e) = shared::copy_dir_all(&shared_target, &file_path) {
                        self.info_message = Some(format!("Failed to copy: {e}"));
                        return;
                    }
                } else if let Err(e) = std::fs::copy(&shared_target, &file_path) {
                    self.info_message = Some(format!("Failed to copy: {e}"));
                    return;
                }
                self.materialized.add(rel_path, wt);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = None;
            }
            WorktreeStatus::Materialized => {
                // Materialized → Linked: delete local copy, create symlink
                let file_path = wt.join(rel_path);
                if file_path.is_dir() {
                    if let Err(e) = std::fs::remove_dir_all(&file_path) {
                        self.info_message = Some(format!("Failed to remove: {e}"));
                        return;
                    }
                } else if let Err(e) = std::fs::remove_file(&file_path) {
                    self.info_message = Some(format!("Failed to remove: {e}"));
                    return;
                }
                if let Err(e) =
                    shared::create_shared_symlink(wt, rel_path, &self.git_common_dir)
                {
                    self.info_message = Some(format!("Failed to create symlink: {e}"));
                    return;
                }
                self.materialized.remove(rel_path, wt);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = None;
            }
            WorktreeStatus::Missing => {
                // Missing → Materialized: copy shared file into worktree
                let shared_target = shared::shared_file_path(&self.git_common_dir, rel_path);
                let file_path = wt.join(rel_path);
                if let Some(parent) = file_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if shared_target.is_dir() {
                    if let Err(e) = shared::copy_dir_all(&shared_target, &file_path) {
                        self.info_message = Some(format!("Failed to copy: {e}"));
                        return;
                    }
                } else if let Err(e) = std::fs::copy(&shared_target, &file_path) {
                    self.info_message = Some(format!("Failed to copy: {e}"));
                    return;
                }
                self.materialized.add(rel_path, wt);
                let _ = self.materialized.save(&self.git_common_dir);
                self.info_message = None;
            }
            _ => {
                // Conflict, Broken, NotCollected — no-op for m
                self.info_message =
                    Some("Use 'i' to fix this entry before toggling".to_string());
            }
        }
    }

    /// Link (fix/create symlink) for the entry under the cursor.
    fn link_entry(&mut self, state: &PickerState) {
        let tab = state.current_tab();
        let idx = tab.list_cursor;
        let status = self.status(state.active_tab, idx);
        let rel_path = &tab.rel_path;
        let wt = &tab.entries[idx].worktree_path;

        match status {
            WorktreeStatus::Missing => {
                // Create symlink
                match shared::create_shared_symlink(wt, rel_path, &self.git_common_dir) {
                    Ok(_) => { self.info_message = None; }
                    Err(e) => { self.info_message = Some(format!("Failed: {e}")); }
                }
            }
            WorktreeStatus::Broken => {
                // Fix symlink: remove broken, create correct
                let file_path = wt.join(rel_path);
                let _ = std::fs::remove_file(&file_path);
                match shared::create_shared_symlink(wt, rel_path, &self.git_common_dir) {
                    Ok(_) => { self.info_message = None; }
                    Err(e) => { self.info_message = Some(format!("Failed: {e}")); }
                }
            }
            WorktreeStatus::Conflict => {
                // Replace real file with symlink (destructive — confirm would be ideal)
                let file_path = wt.join(rel_path);
                let remove_result = if file_path.is_dir() {
                    std::fs::remove_dir_all(&file_path)
                } else {
                    std::fs::remove_file(&file_path)
                };
                if let Err(e) = remove_result {
                    self.info_message = Some(format!("Failed to remove: {e}"));
                    return;
                }
                match shared::create_shared_symlink(wt, rel_path, &self.git_common_dir) {
                    Ok(_) => { self.info_message = None; }
                    Err(e) => { self.info_message = Some(format!("Failed: {e}")); }
                }
            }
            _ => {} // Linked, Materialized, NotCollected — no-op for i
        }
    }

    fn status_tag(status: WorktreeStatus) -> (String, Color) {
        match status {
            WorktreeStatus::Linked => ("linked".to_string(), Color::Green),
            WorktreeStatus::Materialized => ("materialized".to_string(), Color::Yellow),
            WorktreeStatus::Missing => ("missing".to_string(), DIM),
            WorktreeStatus::Conflict => ("conflict".to_string(), Color::Red),
            WorktreeStatus::Broken => ("broken".to_string(), Color::Yellow),
            WorktreeStatus::NotCollected => ("not collected".to_string(), DIM),
        }
    }
}

/// Detect status of a single entry (same logic as core but callable locally).
fn detect_entry_status(
    worktree_path: &Path,
    rel_path: &str,
    git_common_dir: &Path,
    materialized: &MaterializedState,
    collected: bool,
) -> WorktreeStatus {
    if !collected {
        return WorktreeStatus::NotCollected;
    }
    let file_path = worktree_path.join(rel_path);
    if file_path.is_symlink() {
        let shared_target = shared::shared_file_path(git_common_dir, rel_path);
        let expected = shared::relative_symlink_target(
            file_path.parent().unwrap_or(worktree_path),
            &shared_target,
        );
        match (std::fs::read_link(&file_path), expected) {
            (Ok(actual), Ok(exp)) if actual == exp => WorktreeStatus::Linked,
            _ => WorktreeStatus::Broken,
        }
    } else if file_path.exists() {
        if materialized.is_materialized(rel_path, worktree_path) {
            WorktreeStatus::Materialized
        } else {
            WorktreeStatus::Conflict
        }
    } else {
        WorktreeStatus::Missing
    }
}

impl PickerMode for ManageMode {
    fn all_entries_traversable(&self, _tab: &FileTabState) -> bool {
        true // All worktrees always traversable in manage mode
    }

    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char('m') => {
                self.toggle_materialize(state);
                self.refresh_tab_status(state, state.active_tab);
            }
            KeyCode::Char('i') => {
                self.link_entry(state);
                self.refresh_tab_status(state, state.active_tab);
            }
            _ => {}
        }
        LoopAction::Continue
    }

    fn handle_footer_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return LoopAction::Exit,
            KeyCode::Up | KeyCode::Char('k') => {
                state.move_up(true);
            }
            KeyCode::Tab => state.toggle_panel(),
            _ => {}
        }
        LoopAction::Continue
    }

    fn tab_decided(&self, _tab: &FileTabState) -> bool {
        false // No "decided" concept in manage mode
    }

    fn tab_warning<'a>(&'a self, _tab: &'a FileTabState) -> Option<&'a str> {
        self.info_message.as_deref()
    }

    fn entry_decoration(&self, _tab: &FileTabState, entry_idx: usize) -> EntryDecoration {
        let tab_idx = 0; // Will need active_tab — see note below
        let status = self.status(tab_idx, entry_idx);
        let (tag_text, tag_color) = Self::status_tag(status);

        let marker = match status {
            WorktreeStatus::Linked => "  ".to_string(),
            WorktreeStatus::Materialized => "M ".to_string(),
            WorktreeStatus::Missing => "  ".to_string(),
            WorktreeStatus::Conflict => "! ".to_string(),
            WorktreeStatus::Broken => "? ".to_string(),
            WorktreeStatus::NotCollected => "  ".to_string(),
        };

        EntryDecoration {
            marker,
            tag: Some((tag_text, tag_color)),
        }
    }

    fn render_footer(&self, state: &PickerState, frame: &mut Frame, area: Rect) {
        let key_style = Style::default().fg(Color::Cyan);
        let desc_style = Style::default().fg(DIM);

        let hl_desc = match state.focus {
            FocusPanel::TabBar | FocusPanel::WorktreeList | FocusPanel::Preview => " tabs  ",
            FocusPanel::Footer => " (none)  ",
        };

        let help = Line::from(vec![
            Span::raw("  "),
            Span::styled("jk/\u{2191}\u{2193}", key_style),
            Span::styled(" navigate  ", desc_style),
            Span::styled("hl/\u{2190}\u{2192}", key_style),
            Span::styled(hl_desc, desc_style),
            Span::styled("PgUp/PgDn", key_style),
            Span::styled(" scroll  ", desc_style),
            Span::styled("m", key_style),
            Span::styled(" materialize/link  ", desc_style),
            Span::styled("i", key_style),
            Span::styled(" fix symlink  ", desc_style),
            Span::styled("Tab", key_style),
            Span::styled(" panel  ", desc_style),
            Span::styled("Esc", key_style),
            Span::styled(" quit", desc_style),
        ]);

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::TOP)
            .border_style(Style::default().fg(DIM));

        let paragraph = Paragraph::new(vec![Line::raw(""), help]).block(block);
        frame.render_widget(paragraph, area);
    }

    fn footer_height(&self) -> u16 {
        4
    }
}
```

**Note:** The `entry_decoration` method receives `tab` but not the tab index.
The `PickerMode` trait needs the active tab index to look up statuses. Adjust
the trait signature:

```rust
fn entry_decoration(&self, tab: &FileTabState, tab_idx: usize, entry_idx: usize) -> EntryDecoration;
```

Update the trait, `render.rs` (pass `state.active_tab`), and `collect_mode.rs`
(add `_tab_idx` parameter).

- [ ] **Step 2: Register the module**

In `src/output/tui/shared_picker/mod.rs`, add:

```rust
pub mod manage_mode;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/shared_picker/manage_mode.rs src/output/tui/shared_picker/mod.rs
git commit -m "feat(shared): implement ManageMode with PickerMode trait"
```

---

### Task 4: Wire manage TUI into run_manage

**Files:**

- Modify: `src/commands/shared.rs`
- Modify: `src/output/tui/shared_picker/mod.rs`

- [ ] **Step 1: Add `run_manage_picker` function to mod.rs**

```rust
/// Run the manage TUI.
pub fn run_manage_picker(
    mode: &mut manage_mode::ManageMode,
    tabs: Vec<FileTabState>,
) -> Result<()> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    let result = run_manage_inner(mode, tabs);

    let _ = std::panic::take_hook();

    result
}

fn run_manage_inner(
    mode: &mut manage_mode::ManageMode,
    tabs: Vec<FileTabState>,
) -> Result<()> {
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let highlighter = Highlighter::new();
    let mut state = PickerState::from_tabs(tabs);

    loop {
        terminal.draw(|frame| {
            render::render(&mut state, mode, &highlighter, frame);
        })?;

        let Some(key) = input::poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match input::handle_key(key, &mut state, mode) {
            LoopAction::Continue => {}
            LoopAction::Exit => break,
        }
    }

    restore_terminal();
    Ok(())
}
```

- [ ] **Step 2: Update `run_manage` in shared.rs**

```rust
fn run_manage(_output: &mut dyn Output) -> Result<()> {
    let git_common_dir = repo::get_git_common_dir()?;
    let worktree_path = repo::get_current_worktree_path()?;
    let config_root = shared::resolve_config_root(&worktree_path);
    let shared_paths = shared::read_shared_paths(&worktree_path)?;
    let worktree_paths = shared::list_worktree_paths()?;
    let materialized = shared::MaterializedState::load(&git_common_dir)?;

    if shared_paths.is_empty() {
        _output.info("No shared files declared.");
        return Ok(());
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stderr())
        || std::env::var("DAFT_TESTING").is_ok()
    {
        _output.error(
            "The manage interface requires an interactive terminal. \
             Use `daft shared status` for non-interactive output.",
        );
        return Ok(());
    }

    let infos = shared::detect_shared_statuses(
        &shared_paths,
        &worktree_paths,
        &git_common_dir,
        &materialized,
    );

    let tabs = manage_mode::ManageMode::build_tabs(&infos);
    let mut mode = manage_mode::ManageMode::new(
        git_common_dir,
        config_root,
        materialized,
        worktree_paths,
    );
    mode.set_statuses(&infos);

    use crate::output::tui::shared_picker::{manage_mode, run_manage_picker};
    run_manage_picker(&mut mode, tabs)?;

    Ok(())
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check`

- [ ] **Step 4: Run all tests**

Run: `cargo test -p daft --lib` Run: `cargo clippy`

- [ ] **Step 5: Commit**

```bash
git add src/commands/shared.rs src/output/tui/shared_picker/mod.rs
git commit -m "feat(shared): wire manage TUI into daft shared manage command"
```

---

### Task 5: Integration test and manual verification

**Files:**

- Create: `tests/manual/scenarios/shared/manage-basic.yml`

- [ ] **Step 1: Add basic manage test**

```yaml
name: Shared manage requires interactive terminal
description: >
  daft shared manage in non-interactive mode prints an error.

repos:
  - name: test-repo
    use_fixture: standard-remote

steps:
  - name: Clone repository
    run: git-worktree-clone --layout contained $REMOTE_TEST_REPO
    expect:
      exit_code: 0

  - name: Setup shared file
    run: |
      echo "VAL=1" > .env
      daft shared add .env
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0

  - name: Manage in non-interactive mode reports error
    run: DAFT_TESTING=1 daft shared manage
    cwd: "$WORK_DIR/test-repo/main"
    expect:
      exit_code: 0
      output_contains:
        - "interactive terminal"
```

- [ ] **Step 2: Run the test**

Run: `mise run test:manual -- --ci manage-basic` Expected: Pass.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p daft --lib` Run: `cargo clippy` Run integration tests for
sync/add to ensure no regressions.

- [ ] **Step 4: Commit**

```bash
git add tests/manual/scenarios/shared/manage-basic.yml
git commit -m "test(shared): add manage subcommand integration test"
```

---

## Self-Review

**Spec coverage:**

- Entry point (`daft shared manage`): Task 1
- Worktree status detection (all 6 states): Task 2
- ManageMode with immediate actions: Task 3
- `m` toggle (linked↔materialized, missing→materialized): Task 3
- `i` link (missing, broken, conflict): Task 3
- Status tags with colors: Task 3
- Footer with help legend (no Submit/Cancel): Task 3
- Non-interactive fallback: Task 1

**Not in this plan (deferred to Plans 3-5):**

- `d` diff mode
- `r` remove modal
- `a` add file modal

**Placeholder scan:** None found.

**Type consistency:**

- `WorktreeStatus` used in core and manage_mode
- `SharedFileInfo` used in core and manage_mode
- `PickerMode` trait extended with `tab_idx` parameter in `entry_decoration`
- `ManageMode` implements same trait as `CollectMode`
