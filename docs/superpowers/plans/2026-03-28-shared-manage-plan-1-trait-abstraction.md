# Shared Picker Trait Abstraction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract a reusable TUI shell from the collect picker and define a
`PickerMode` trait so the same shell can drive both the collect picker (sync)
and the future manager. The collect picker must work exactly as before after the
refactoring — no behavior changes.

**Architecture:** Move the existing `collect_picker/` to `shared_picker/`, split
generic navigation/rendering from collect-specific logic. Define a `PickerMode`
trait that the shell calls for mode-specific behavior (action keys, footer
rendering, entry decorations, traversal rules). Implement `CollectMode` as the
first (and initially only) mode. Wire `run_collect_picker` to use the new shell.

**Tech Stack:** Rust, ratatui, crossterm, syntect (all existing — no new deps)

**Spec:** `docs/superpowers/specs/2026-03-28-shared-manage-design.md`
(Architecture section)

---

### Task 1: Rename module and create file structure

**Files:**

- Rename: `src/output/tui/collect_picker/` → `src/output/tui/shared_picker/`
- Create: `src/output/tui/shared_picker/shell.rs`
- Create: `src/output/tui/shared_picker/dialog.rs`
- Create: `src/output/tui/shared_picker/collect_mode.rs`
- Modify: `src/output/tui/mod.rs`
- Modify: `src/commands/shared.rs` (update import path)

- [ ] **Step 1: Rename the directory**

```bash
mv src/output/tui/collect_picker src/output/tui/shared_picker
```

- [ ] **Step 2: Update `src/output/tui/mod.rs`**

Change:

```rust
pub mod collect_picker;
```

to:

```rust
pub mod shared_picker;
```

- [ ] **Step 3: Update import in `src/commands/shared.rs`**

Change:

```rust
use crate::output::tui::collect_picker::{run_collect_picker, PickerOutcome};
```

to:

```rust
use crate::output::tui::shared_picker::{run_collect_picker, PickerOutcome};
```

- [ ] **Step 4: Create empty stub files**

Create `src/output/tui/shared_picker/shell.rs`:

```rust
//! TUI shell: terminal setup, event loop, panic hook.
```

Create `src/output/tui/shared_picker/dialog.rs`:

```rust
//! Reusable confirmation dialogs.
```

Create `src/output/tui/shared_picker/collect_mode.rs`:

```rust
//! Collect mode: batch selection of uncollected shared files for sync.
```

- [ ] **Step 5: Update `src/output/tui/shared_picker/mod.rs` to include new
      modules**

Add after existing module declarations:

```rust
mod shell;
mod dialog;
pub mod collect_mode;
```

- [ ] **Step 6: Verify it compiles**

Run: `cargo check` Expected: Compiles (dead_code warnings for empty stubs OK).

- [ ] **Step 7: Run all tests**

Run: `cargo test -p daft --lib` Expected: All tests pass (just a rename + stub
files).

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(shared): rename collect_picker to shared_picker, add stubs"
```

---

### Task 2: Define the PickerMode trait and LoopAction

**Files:**

- Modify: `src/output/tui/shared_picker/mod.rs`

This task defines the trait contract that modes must implement. No
implementation yet — just the trait and supporting types.

- [ ] **Step 1: Add the trait and types to `mod.rs`**

Add these definitions to `src/output/tui/shared_picker/mod.rs` (after the module
declarations, before the existing code):

```rust
use crossterm::event::KeyCode;
use ratatui::{
    style::Color,
    text::Span,
    Frame,
    layout::Rect,
};

use state::{FileTabState, FocusPanel, PickerState};

/// What the event loop should do after handling an action.
pub enum LoopAction {
    /// Keep running the event loop.
    Continue,
    /// Close the TUI and return to the caller.
    Exit,
}

/// Marker and tag decoration for a worktree entry.
pub struct EntryDecoration {
    /// 2-char marker before the name (e.g., "✓ ", "M ", "  ").
    pub marker: String,
    /// Optional colored tag after the name (e.g., "materialized", "linked").
    pub tag: Option<(String, Color)>,
}

/// Trait defining mode-specific behavior for the shared file picker TUI.
///
/// The TUI shell handles terminal management, navigation (jk/hl/Tab/Esc/
/// PgUp/PgDn), rendering layout (tabs, body, preview), and the help legend
/// structure. The mode handles action semantics: what happens when the user
/// presses action keys, what the footer shows, how entries are decorated,
/// and whether all entries are traversable.
pub trait PickerMode {
    /// Whether all worktree entries are traversable, regardless of `has_file`.
    ///
    /// In collect mode this returns `tab.selected.is_some()` (entries without
    /// files are skipped until a source is selected). In manage mode this
    /// always returns `true`.
    fn all_entries_traversable(&self, tab: &FileTabState) -> bool;

    /// Handle a mode-specific key press in the worktree list panel.
    ///
    /// Navigation keys (jk, hl, Tab, Esc, PgUp/PgDn) are handled by the
    /// shell before calling this. This receives only unhandled keys.
    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction;

    /// Handle a mode-specific key press in the footer panel.
    ///
    /// Navigation keys (jk, hl) are handled by the shell. This receives
    /// action keys (Enter, Space, etc.).
    fn handle_footer_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction;

    /// Whether the given tab has been "decided" (shows a checkmark in the tab bar).
    fn tab_decided(&self, tab: &FileTabState) -> bool;

    /// Warning text to show between the tab bar and body, if any.
    fn tab_warning<'a>(&'a self, tab: &'a FileTabState) -> Option<&'a str>;

    /// Get the marker and tag for a worktree entry.
    fn entry_decoration(&self, tab: &FileTabState, entry_idx: usize) -> EntryDecoration;

    /// Render the footer area (buttons, status, help legend).
    fn render_footer(&self, state: &PickerState, frame: &mut Frame, area: Rect);

    /// The height needed for the footer area.
    fn footer_height(&self) -> u16;
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: Compiles (the trait is defined but not used yet).

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/shared_picker/mod.rs
git commit -m "refactor(shared): define PickerMode trait and supporting types"
```

---

### Task 3: Extract generic state into PickerState

**Files:**

- Modify: `src/output/tui/shared_picker/state.rs`

The current `CollectPickerState` contains both generic navigation state and
collect-specific state (`submitted`, `cancelled`, `footer_cursor`). This task
renames it to `PickerState` and removes collect-specific fields. Those fields
will move to `CollectMode` in a later task.

**Important:** This is a refactoring step. The existing tests will temporarily
break because `CollectPickerState` is renamed. We fix them in the next task when
`CollectMode` wraps `PickerState`.

- [ ] **Step 1: Rename `CollectPickerState` to `PickerState` and remove
      collect-specific fields**

In `src/output/tui/shared_picker/state.rs`:

1. Remove the import of `CollectDecision`, `CompareResult`, `UncollectedFile`
   and `shared` from `crate::core::shared`. Keep `std::path::PathBuf` and
   `std::time::Duration`.

2. Remove `FooterButton` enum (it's collect-specific — Submit/Cancel).

3. Rename `CollectPickerState` to `PickerState` and remove `submitted`,
   `cancelled`, and `footer_cursor` fields:

```rust
/// Top-level state for the shared file picker TUI.
#[derive(Debug)]
pub struct PickerState {
    pub tabs: Vec<FileTabState>,
    pub active_tab: usize,
    pub focus: FocusPanel,
}
```

4. Remove the `new()` constructor (it takes `Vec<UncollectedFile>` which is
   collect-specific). Add a simpler `from_tabs`:

```rust
impl PickerState {
    /// Create picker state from pre-built tabs.
    pub fn from_tabs(tabs: Vec<FileTabState>) -> Self {
        let focus = if tabs.first().is_some_and(|t| t.is_stub) {
            FocusPanel::TabBar
        } else {
            FocusPanel::WorktreeList
        };
        Self {
            tabs,
            active_tab: 0,
            focus,
        }
    }
```

5. Remove these collect-specific methods: `toggle_selection`,
   `toggle_materialized`, `activate_footer`, `footer_next`, `decided_count`,
   `decidable_count`, `all_decided`, `has_any_selection`, `undecided_files`,
   `into_decisions`.

6. Keep all navigation methods: `set_cursor`, `current_tab`, `next_tab`,
   `prev_tab`, `move_down`, `move_up`, `page_down`, `page_up`, `toggle_panel`.

7. **Change `move_down` and `move_up`** to accept a `all_traversable: bool`
   parameter instead of checking `tab.selected.is_some()`:

In `move_down`, change:

```rust
let has_selection = tab.selected.is_some();
```

to the parameter `all_traversable`.

In `move_up`, same change. In the Footer → WorktreeList transition:

```rust
let last = if !all_traversable {
```

8. Change `page_down` and `page_up` signatures — no changes needed, they don't
   check selection.

9. Remove the `COMPARE_TIMEOUT` constant.

10. Remove the entire `#[cfg(test)] mod tests` block — those tests are
    collect-mode-specific and will be moved.

- [ ] **Step 2: Update `FileTabState`**

Keep `FileTabState` as-is with all fields including `selected`, `materialized`,
and `compare_warning`. These are used by collect mode but live in the shared
state since the renderer needs access to them. Manage mode will use the same
struct with different semantics for some fields.

Add a builder method:

```rust
impl FileTabState {
    /// Create a new tab with the given entries.
    pub fn new(rel_path: String, entries: Vec<WorktreeEntry>) -> Self {
        let is_stub = !entries.iter().any(|e| e.has_file);
        let initial_cursor = entries.iter().position(|e| e.has_file).unwrap_or(0);
        let len = entries.len();
        Self {
            rel_path,
            entries,
            list_cursor: initial_cursor,
            selected: None,
            materialized: vec![false; len],
            preview_scroll: 0,
            preview_content_lines: 0,
            preview_viewport_height: 0,
            is_stub,
            compare_warning: None,
        }
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check` Expected: Compiler errors from `input.rs`, `render.rs`, and
`mod.rs` still referencing `CollectPickerState` — we fix those in the next
tasks.

- [ ] **Step 4: Commit (WIP)**

```bash
git add src/output/tui/shared_picker/state.rs
git commit -m "wip: extract PickerState from CollectPickerState"
```

---

### Task 4: Extract confirmation dialog

**Files:**

- Modify: `src/output/tui/shared_picker/dialog.rs`
- Modify: `src/output/tui/shared_picker/mod.rs`

Move `show_confirm_dialog` from `mod.rs` to `dialog.rs` as a reusable component.

- [ ] **Step 1: Implement `dialog.rs`**

Move the `show_confirm_dialog` function from `mod.rs` to `dialog.rs`. It needs
the `poll_key` function from `input.rs`, so import it:

```rust
//! Reusable confirmation dialogs.

use anyhow::Result;
use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::time::Duration;

use super::input::poll_key;

/// Generic yes/no confirmation dialog rendered as an overlay.
/// Supports h/l and arrow keys to toggle focus, Enter to confirm selection.
pub fn show_confirm_dialog(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    title: &str,
    body_lines: &[&str],
) -> Result<bool> {
    // (paste the existing show_confirm_dialog body from mod.rs)
```

Copy the entire function body from `mod.rs` lines 180-251.

- [ ] **Step 2: Remove the function from `mod.rs`**

Delete `show_confirm_dialog` from `mod.rs`. Keep `show_cancel_confirm` and
`show_partial_submit_confirm` for now (they'll move to collect_mode later).

- [ ] **Step 3: Update callers to use `dialog::show_confirm_dialog`**

In `mod.rs`, change the calls in `show_cancel_confirm` and
`show_partial_submit_confirm` to use `dialog::show_confirm_dialog`.

- [ ] **Step 4: Verify it compiles**

Run: `cargo check`

- [ ] **Step 5: Commit**

```bash
git add src/output/tui/shared_picker/dialog.rs src/output/tui/shared_picker/mod.rs
git commit -m "refactor(shared): extract confirmation dialog to dialog.rs"
```

---

### Task 5: Extract terminal shell

**Files:**

- Modify: `src/output/tui/shared_picker/shell.rs`
- Modify: `src/output/tui/shared_picker/mod.rs`

Move terminal setup/teardown, panic hook, and the event loop skeleton to
`shell.rs`. The event loop becomes generic over `PickerMode`.

- [ ] **Step 1: Implement `shell.rs`**

```rust
//! TUI shell: terminal setup, event loop, panic hook.

use anyhow::Result;
use crossterm::{
    cursor,
    event::DisableMouseCapture,
    event::EnableMouseCapture,
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::Terminal;
use std::io;
use std::time::Duration;

use super::highlight::Highlighter;
use super::input;
use super::render;
use super::state::PickerState;
use super::{LoopAction, PickerMode};

/// Restore the terminal to its normal state.
fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show
    );
}

/// Run the shared picker TUI with the given mode.
///
/// Sets up the terminal, runs the event loop, restores on exit/panic.
/// Returns the `PickerState` after the loop exits so the caller can
/// extract results.
pub fn run_picker(
    mut state: PickerState,
    mode: &mut dyn PickerMode,
) -> Result<PickerState> {
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    let result = run_picker_inner(&mut state, mode);

    let _ = std::panic::take_hook();

    match result {
        Ok(()) => Ok(state),
        Err(e) => Err(e),
    }
}

fn run_picker_inner(
    state: &mut PickerState,
    mode: &mut dyn PickerMode,
) -> Result<()> {
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let highlighter = Highlighter::new();

    loop {
        terminal.draw(|frame| {
            render::render(state, mode, &highlighter, frame);
        })?;

        let Some(key) = input::poll_key(Duration::from_millis(100)) else {
            continue;
        };

        match input::handle_key(key, state, mode) {
            LoopAction::Continue => {}
            LoopAction::Exit => break,
        }
    }

    restore_terminal();
    Ok(())
}
```

- [ ] **Step 2: Remove terminal code from `mod.rs`**

Remove `restore_terminal`, `run_collect_picker_inner` from `mod.rs`. Update
`run_collect_picker` to use `shell::run_picker`.

- [ ] **Step 3: Verify it compiles**

Run: `cargo check` Expected: Errors from `render::render` and
`input::handle_key` not matching new signatures yet — fixed in next tasks.

- [ ] **Step 4: Commit (WIP)**

```bash
git add src/output/tui/shared_picker/shell.rs src/output/tui/shared_picker/mod.rs
git commit -m "wip: extract terminal shell to shell.rs"
```

---

### Task 6: Update input.rs to use PickerMode

**Files:**

- Modify: `src/output/tui/shared_picker/input.rs`

Change `handle_key` to accept `&mut dyn PickerMode`, route navigation keys in
the shell, and delegate action keys to the mode.

- [ ] **Step 1: Rewrite input.rs**

```rust
//! Keyboard input handling for the shared picker TUI.

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

use super::state::{FocusPanel, PickerState};
use super::{LoopAction, PickerMode};

/// Poll for a key event (blocks up to `timeout`).
pub fn poll_key(timeout: Duration) -> Option<KeyEvent> {
    if event::poll(timeout).ok()? {
        if let Event::Key(key) = event::read().ok()? {
            return Some(key);
        }
    }
    None
}

/// Handle a key event: shell handles navigation, mode handles actions.
pub fn handle_key(
    key: KeyEvent,
    state: &mut PickerState,
    mode: &mut dyn PickerMode,
) -> LoopAction {
    // Ctrl+C always exits
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return LoopAction::Exit;
    }

    match state.focus {
        FocusPanel::TabBar => handle_tab_bar(key.code, state, mode),
        FocusPanel::WorktreeList => handle_worktree_list(key.code, state, mode),
        FocusPanel::Preview => handle_preview(key.code, state, mode),
        FocusPanel::Footer => handle_footer(key.code, state, mode),
    }
}

fn handle_tab_bar(key: KeyCode, state: &mut PickerState, _mode: &mut dyn PickerMode) -> LoopAction {
    let all_traversable = true; // unused for tab bar but needed for API
    match key {
        KeyCode::Right | KeyCode::Char('l') => state.next_tab(),
        KeyCode::Left | KeyCode::Char('h') => state.prev_tab(),
        KeyCode::Down | KeyCode::Char('j') => state.move_down(all_traversable),
        KeyCode::Char('q') | KeyCode::Esc => state.focus = FocusPanel::Footer,
        _ => {}
    }
    LoopAction::Continue
}

fn handle_worktree_list(key: KeyCode, state: &mut PickerState, mode: &mut dyn PickerMode) -> LoopAction {
    let all_traversable = mode.all_entries_traversable(state.current_tab());
    match key {
        // Navigation — handled by shell
        KeyCode::Down | KeyCode::Char('j') => { state.move_down(all_traversable); }
        KeyCode::Up | KeyCode::Char('k') => { state.move_up(all_traversable); }
        KeyCode::Right | KeyCode::Char('l') => { state.next_tab(); }
        KeyCode::Left | KeyCode::Char('h') => { state.prev_tab(); }
        KeyCode::Char('q') | KeyCode::Esc => { state.focus = FocusPanel::Footer; }
        KeyCode::Tab => { state.toggle_panel(); }
        // Action keys — delegated to mode
        _ => return mode.handle_list_key(key, state),
    }
    LoopAction::Continue
}

fn handle_preview(key: KeyCode, state: &mut PickerState, mode: &mut dyn PickerMode) -> LoopAction {
    let all_traversable = mode.all_entries_traversable(state.current_tab());
    match key {
        KeyCode::Down | KeyCode::Char('j') => { state.move_down(all_traversable); }
        KeyCode::Up | KeyCode::Char('k') => { state.move_up(all_traversable); }
        KeyCode::PageDown => { state.page_down(); }
        KeyCode::PageUp => { state.page_up(); }
        KeyCode::Right | KeyCode::Char('l') => { state.next_tab(); }
        KeyCode::Left | KeyCode::Char('h') => { state.prev_tab(); }
        KeyCode::Char('q') | KeyCode::Esc => { state.focus = FocusPanel::Footer; }
        KeyCode::Tab => { state.toggle_panel(); }
        _ => {}
    }
    LoopAction::Continue
}

fn handle_footer(key: KeyCode, state: &mut PickerState, mode: &mut dyn PickerMode) -> LoopAction {
    match key {
        // Navigation — handled by shell
        KeyCode::Up | KeyCode::Char('k') => {
            let all_traversable = mode.all_entries_traversable(state.current_tab());
            state.move_up(all_traversable);
        }
        KeyCode::Tab => { state.toggle_panel(); }
        // Everything else — delegated to mode (including Esc, hl, Enter, Space)
        _ => return mode.handle_footer_key(key, state),
    }
    LoopAction::Continue
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit (WIP)**

```bash
git add src/output/tui/shared_picker/input.rs
git commit -m "wip: update input.rs to use PickerMode trait"
```

---

### Task 7: Update render.rs to use PickerMode

**Files:**

- Modify: `src/output/tui/shared_picker/render.rs`

Change the render functions to accept `&dyn PickerMode` and use trait methods
for entry decorations, tab decided state, warnings, and footer.

- [ ] **Step 1: Update render function signatures**

Change `render` signature:

```rust
pub fn render(
    state: &mut PickerState,
    mode: &dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
)
```

Replace `CollectPickerState` with `PickerState` everywhere.

- [ ] **Step 2: Update `render_tabs`**

Change the `has_decision` check:

```rust
let has_decision = mode.tab_decided(tab);
```

- [ ] **Step 3: Update `render_warning`**

Change to use mode:

```rust
fn render_warning(tab: &FileTabState, mode: &dyn PickerMode, frame: &mut Frame, area: Rect) {
    if let Some(msg) = mode.tab_warning(tab) {
```

- [ ] **Step 4: Update `render_worktree_list`**

Replace the inline marker/tag logic with:

```rust
let decoration = mode.entry_decoration(tab, idx);
let marker = &decoration.marker;
// ... use decoration.tag for the tag span
```

- [ ] **Step 5: Update `render_footer`**

Replace the inline footer rendering with a delegation:

```rust
fn render_footer(state: &PickerState, mode: &dyn PickerMode, frame: &mut Frame, area: Rect) {
    mode.render_footer(state, frame, area);
}
```

- [ ] **Step 6: Update layout to use `mode.footer_height()`**

In the main `render` function:

```rust
Constraint::Length(mode.footer_height()),
```

- [ ] **Step 7: Verify it compiles**

Run: `cargo check`

- [ ] **Step 8: Commit (WIP)**

```bash
git add src/output/tui/shared_picker/render.rs
git commit -m "wip: update render.rs to use PickerMode trait"
```

---

### Task 8: Implement CollectMode

**Files:**

- Modify: `src/output/tui/shared_picker/collect_mode.rs`

This is the key task — implement `CollectMode` as a struct that holds
collect-specific state and implements `PickerMode`. Port all the
collect-specific logic from the old `CollectPickerState`.

- [ ] **Step 1: Implement CollectMode**

```rust
//! Collect mode: batch selection of uncollected shared files for sync.

use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::path::PathBuf;
use std::time::Duration;

use crate::core::shared::{self, CollectDecision, CompareResult, UncollectedFile};

use super::state::{FileTabState, FocusPanel, PickerState, WorktreeEntry};
use super::{EntryDecoration, LoopAction, PickerMode};

const COMPARE_TIMEOUT: Duration = Duration::from_secs(1);

/// Footer button for collect mode.
#[derive(Debug, Clone, Copy, PartialEq)]
enum FooterButton {
    Submit,
    Cancel,
}

/// Collect mode state and behavior.
pub struct CollectMode {
    footer_cursor: FooterButton,
    submitted: bool,
    cancelled: bool,
}

impl CollectMode {
    pub fn new() -> Self {
        Self {
            footer_cursor: FooterButton::Submit,
            submitted: false,
            cancelled: false,
        }
    }

    /// Build picker tabs from uncollected files.
    pub fn build_tabs(uncollected: Vec<UncollectedFile>) -> Vec<FileTabState> {
        uncollected
            .into_iter()
            .map(|uf| {
                let entries: Vec<WorktreeEntry> = uf
                    .worktrees
                    .into_iter()
                    .map(|w| WorktreeEntry {
                        worktree_name: w.worktree_name,
                        worktree_path: w.worktree_path,
                        has_file: w.has_file,
                    })
                    .collect();
                FileTabState::new(uf.rel_path, entries)
            })
            .collect()
    }

    /// Build collect decisions from the current picker state.
    pub fn into_decisions(state: PickerState) -> Vec<CollectDecision> {
        state
            .tabs
            .into_iter()
            .filter_map(|tab| {
                if tab.is_stub {
                    return None;
                }
                tab.selected.map(|idx| {
                    let materialize_in = tab
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|&(i, _)| i != idx && tab.materialized[i])
                        .map(|(_, e)| e.worktree_path.clone())
                        .collect();
                    CollectDecision {
                        rel_path: tab.rel_path,
                        source_worktree: tab.entries[idx].worktree_path.clone(),
                        materialize_in,
                    }
                })
            })
            .collect()
    }

    fn toggle_selection(state: &mut PickerState) {
        // (port the existing toggle_selection logic from state.rs)
    }

    fn toggle_materialized(state: &mut PickerState) {
        // (port the existing toggle_materialized logic)
    }

    fn has_any_selection(state: &PickerState) -> bool {
        state.tabs.iter().any(|t| t.selected.is_some())
    }

    fn decided_count(state: &PickerState) -> usize {
        state.tabs.iter().filter(|t| !t.is_stub && t.selected.is_some()).count()
    }

    fn decidable_count(state: &PickerState) -> usize {
        state.tabs.iter().filter(|t| !t.is_stub).count()
    }

    fn all_decided(state: &PickerState) -> bool {
        Self::decided_count(state) == Self::decidable_count(state)
    }

    fn undecided_files(state: &PickerState) -> Vec<&str> {
        state.tabs.iter()
            .filter(|t| !t.is_stub && t.selected.is_none())
            .map(|t| t.rel_path.as_str())
            .collect()
    }
}

impl PickerMode for CollectMode {
    fn all_entries_traversable(&self, tab: &FileTabState) -> bool {
        tab.selected.is_some()
    }

    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Char(' ') | KeyCode::Enter => Self::toggle_selection(state),
            KeyCode::Char('m') => Self::toggle_materialized(state),
            _ => {}
        }
        LoopAction::Continue
    }

    fn handle_footer_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        match key {
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                self.footer_cursor = match self.footer_cursor {
                    FooterButton::Submit => FooterButton::Cancel,
                    FooterButton::Cancel => FooterButton::Submit,
                };
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                if Self::has_any_selection(state) {
                    // Need confirmation — but we can't show dialog from here.
                    // Set cancelled flag and return Exit; the caller handles it.
                    self.cancelled = true;
                }
                return LoopAction::Exit;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.footer_cursor {
                    FooterButton::Submit => {
                        self.submitted = true;
                        return LoopAction::Exit;
                    }
                    FooterButton::Cancel => {
                        self.cancelled = true;
                        return LoopAction::Exit;
                    }
                }
            }
            _ => {}
        }
        LoopAction::Continue
    }

    fn tab_decided(&self, tab: &FileTabState) -> bool {
        tab.selected.is_some() || tab.is_stub
    }

    fn tab_warning<'a>(&'a self, tab: &'a FileTabState) -> Option<&'a str> {
        tab.compare_warning.as_deref()
    }

    fn entry_decoration(&self, tab: &FileTabState, entry_idx: usize) -> EntryDecoration {
        let is_selected = tab.selected == Some(entry_idx);
        let has_selection = tab.selected.is_some();
        let is_materialized = has_selection && tab.materialized[entry_idx];

        let marker = if is_selected {
            "\u{2713} ".to_string()
        } else if is_materialized {
            "M ".to_string()
        } else {
            "  ".to_string()
        };

        let tag = if has_selection && !is_selected {
            if is_materialized {
                Some(("materialized".to_string(), Color::Yellow))
            } else {
                Some(("linked".to_string(), Color::Cyan))
            }
        } else {
            None
        };

        EntryDecoration { marker, tag }
    }

    fn render_footer(&self, state: &PickerState, frame: &mut Frame, area: Rect) {
        // (port the existing render_footer logic, using self.footer_cursor,
        //  Self::decided_count(state), Self::decidable_count(state), etc.)
    }

    fn footer_height(&self) -> u16 {
        5
    }
}
```

Note: The confirmation dialogs (cancel confirm, partial submit confirm) need
special handling. Since the mode can't directly drive a dialog from within
`handle_footer_key` (it returns `LoopAction`), the approach is:

- `handle_footer_key` sets `self.submitted` or `self.cancelled` and returns
  `LoopAction::Exit`
- The caller (`run_collect_picker`) checks these flags and shows confirmation
  dialogs if needed, re-entering the event loop if the user declines

Port the complete bodies of `toggle_selection` and `toggle_materialized` from
the old `state.rs` as static methods on `CollectMode`.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check`

- [ ] **Step 3: Commit (WIP)**

```bash
git add src/output/tui/shared_picker/collect_mode.rs
git commit -m "wip: implement CollectMode with PickerMode trait"
```

---

### Task 9: Wire up run_collect_picker with the new architecture

**Files:**

- Modify: `src/output/tui/shared_picker/mod.rs`

Rewrite `run_collect_picker` to use `CollectMode` + `shell::run_picker`. Handle
the confirmation dialog flow (submit/cancel with prompts).

- [ ] **Step 1: Rewrite `run_collect_picker`**

```rust
pub fn run_collect_picker(uncollected: Vec<UncollectedFile>) -> Result<PickerOutcome> {
    let tabs = collect_mode::CollectMode::build_tabs(uncollected);
    let mut state = PickerState::from_tabs(tabs);
    let mut mode = collect_mode::CollectMode::new();

    loop {
        state = shell::run_picker(state, &mut mode)?;

        if mode.is_submitted() {
            if !CollectMode::all_decided(&state) {
                // Show partial submit confirmation
                // If declined, reset and re-enter loop
                // If confirmed, return decisions
            }
            return Ok(PickerOutcome::Decisions(
                CollectMode::into_decisions(state),
            ));
        }

        if mode.is_cancelled() {
            if CollectMode::has_any_selection(&state) {
                // Show cancel confirmation
                // If declined, reset and re-enter loop
                // If confirmed, return cancelled
            }
            return Ok(PickerOutcome::Cancelled);
        }

        // Plain exit (no selection made)
        return Ok(PickerOutcome::Cancelled);
    }
}
```

This needs refinement — the confirmation dialogs need terminal access. The
approach: `shell::run_picker` returns when the mode signals `Exit`. Then
`run_collect_picker` can show dialogs using a separate terminal session, and
re-enter `run_picker` if the user declines.

Actually, simpler: have `shell::run_picker` also return the terminal (or
re-create it for dialogs). OR: handle dialogs inside the mode by having the
shell pass the terminal to the mode on exit.

The cleanest approach for Plan 1: keep the event loop and dialog handling in
`mod.rs` (like today), but use `PickerMode` for the per-frame rendering and
input handling. The shell manages terminal and frame loop; the outer function
manages dialogs. This means `shell.rs` exposes lower-level primitives rather
than a single `run_picker` function.

Adjust `shell.rs` to expose:

```rust
pub fn setup_terminal() -> Result<Terminal<...>> { ... }
pub fn restore_terminal() { ... }
pub fn install_panic_hook() { ... }
pub fn remove_panic_hook() { ... }
```

And `run_collect_picker` in `mod.rs` uses these directly, keeping the event loop
and dialog flow inline.

- [ ] **Step 2: Move confirmation dialog callers to work with new types**

Update `show_cancel_confirm` and `show_partial_submit_confirm` to stay in
`mod.rs` (they're collect-specific).

- [ ] **Step 3: Verify all tests pass**

Run: `cargo test -p daft --lib` Expected: All tests pass.

Run: `cargo clippy` Expected: Zero warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "refactor(shared): wire collect picker to use PickerMode trait"
```

---

### Task 10: Move collect-mode tests

**Files:**

- Modify: `src/output/tui/shared_picker/collect_mode.rs`

Port all the tests from the old `state.rs` test module to `collect_mode.rs`.
Update them to use `CollectMode` and `PickerState` instead of
`CollectPickerState`.

- [ ] **Step 1: Port all 15 tests**

Move each test, updating:

- `CollectPickerState::new(files)` →
  `PickerState::from_tabs(CollectMode::build_tabs(files))`
- Direct state field access stays the same (tabs, entries, etc.)
- `state.toggle_selection()` → `CollectMode::toggle_selection(&mut state)`
- `state.toggle_materialized()` → `CollectMode::toggle_materialized(&mut state)`
- etc.

- [ ] **Step 2: Run tests**

Run: `cargo test -p daft --lib shared_picker` Expected: All 15 tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/shared_picker/collect_mode.rs
git commit -m "refactor(shared): port collect-mode tests to collect_mode.rs"
```

---

### Task 11: Integration test verification

**Files:**

- No changes — just verification

- [ ] **Step 1: Run all unit tests**

Run: `cargo test -p daft --lib` Expected: All tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo clippy` Expected: Zero warnings.

- [ ] **Step 3: Run integration tests**

Run the shared scenarios:

```bash
for test in sync sync-collect sync-collect-materialize sync-collect-sibling add-basic declare link-on-create materialize-and-link remove; do
  echo "=== $test ===" && mise run test:manual -- --ci $test 2>&1 | tail -3
done
```

Expected: All pass.

- [ ] **Step 4: Final commit (if any formatting/lint fixes)**

```bash
git add -A
git commit -m "style: format and lint fixes for trait abstraction refactor"
```

---

## Self-Review

**Spec coverage:**

- Trait-based TUI abstraction: Tasks 2-9
- CollectMode implementing the trait: Task 8
- Same visual behavior: Tasks 6-7 (render/input use trait methods)
- No new features: correct, this is purely a refactoring plan
- Future ManageMode: the trait is designed for it but not implemented here

**Placeholder scan:** Task 8 Step 1 has `// (port the existing ... logic)`
markers. These are instructions to copy specific code from the old files, not
TBDs. The implementer has the full source code of the old files to copy from.
For clarity: `toggle_selection` is the 60-line method at state.rs lines 296-356,
`toggle_materialized` is at lines 362-371, and `render_footer` is at render.rs
lines 361-436.

**Type consistency:**

- `PickerState` used consistently in shell, input, render, collect_mode
- `PickerMode` trait referenced in shell, input, render
- `FileTabState` and `WorktreeEntry` unchanged
- `EntryDecoration` defined in mod.rs, used in render.rs and collect_mode.rs
- `LoopAction` defined in mod.rs, used in input.rs, shell.rs, collect_mode.rs
