# Inline File Editor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add inline file editing to the manage TUI's preview pane using the
edtui crate, with syntax highlighting, Vim keybindings, and a color-coded header
showing whether the shared or materialized copy is being edited.

**Architecture:** When the user presses Enter on the preview pane, an
`EditSession` is created holding an `edtui::EditorState`. The manage event loop
intercepts all keys while editing is active, routing them to edtui's event
handler. The renderer swaps the Paragraph preview for an EditorView widget with
a header line. Esc in Normal mode saves and exits.

**Tech Stack:** edtui 0.11 (Vim editor widget for ratatui), syntect 5.x (syntax
highlighting, already a dep)

---

## File Structure

- **Modify:** `Cargo.toml` — add `edtui` dependency
- **Create:** `src/output/tui/shared_picker/editor.rs` — `EditSession` struct,
  enter/save/render logic
- **Modify:** `src/output/tui/shared_picker/mod.rs` — register `editor` module,
  add `render_editor` to `PickerMode` trait, modify manage event loop
- **Modify:** `src/output/tui/shared_picker/manage_mode.rs` — add `edit_state`
  field, implement `render_editor`, wire Enter in preview to start editing
- **Modify:** `src/output/tui/shared_picker/render.rs` — change render chain to
  `&mut dyn PickerMode`, call `render_editor` in preview area
- **Modify:** `src/output/tui/shared_picker/input.rs` — add Enter handling in
  `handle_preview`

---

### Task 1: Add edtui dependency

**Files:**

- Modify: `Cargo.toml`

- [ ] **Step 1: Add edtui to dependencies**

In `Cargo.toml`, after the `similar = "2.6"` line, add:

```toml
edtui = { version = "0.11", features = ["syntax-highlighting"] }
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml
git commit -m "chore: add edtui dependency for inline file editing"
```

---

### Task 2: Create EditSession module

**Files:**

- Create: `src/output/tui/shared_picker/editor.rs`
- Modify: `src/output/tui/shared_picker/mod.rs`

- [ ] **Step 1: Create editor.rs with EditSession struct and core methods**

Create `src/output/tui/shared_picker/editor.rs`:

```rust
//! Inline file editor for the manage picker TUI.
//!
//! Wraps edtui's EditorState and EditorView to provide in-place file
//! editing within the preview pane. Handles enter/exit, saving, and
//! rendering with a color-coded header.

use crossterm::event::KeyEvent;
use edtui::EditorMode;
use edtui::EditorState;
use edtui::EditorEventHandler;
use edtui::EditorView;
use edtui::SyntaxHighlighter;
use edtui::EditorTheme;
use edtui_jagged::Lines;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::shared::{self, WorktreeStatus};

const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;

/// Active editing session state.
pub struct EditSession {
    /// The edtui editor state (owns the text buffer).
    pub editor: EditorState,
    /// The edtui event handler (Vim keybindings).
    pub handler: EditorEventHandler,
    /// Resolved file path to save to.
    pub file_path: PathBuf,
    /// Whether we're editing the shared copy (true) or a materialized copy (false).
    pub is_shared: bool,
    /// Worktree name (for the header).
    pub worktree_name: String,
    /// File extension for syntax highlighting (e.g., "rs", "sh").
    pub syntax_ext: String,
    /// Whether the previous key event left us in Normal mode via Esc.
    /// Used to detect the second Esc that should exit the editor.
    was_normal_before_key: bool,
}

impl EditSession {
    /// Create a new editing session.
    ///
    /// Reads the file at `file_path` and initializes the editor buffer.
    /// Returns `None` if the file cannot be read.
    pub fn new(
        file_path: PathBuf,
        is_shared: bool,
        worktree_name: String,
        rel_path: &str,
    ) -> Option<Self> {
        let content = fs::read_to_string(&file_path).ok()?;
        let editor = EditorState::new(Lines::from(content.as_str()));
        let handler = EditorEventHandler::default();

        // Determine syntax extension, mapping .env files to shell
        let syntax_ext = resolve_syntax_ext(rel_path);

        Some(Self {
            editor,
            handler,
            file_path,
            is_shared,
            worktree_name,
            syntax_ext,
            was_normal_before_key: true, // Start in Normal mode
        })
    }

    /// Handle a key event. Returns `true` if the editor should exit (save and close).
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let was_normal = self.editor.mode == EditorMode::Normal;
        self.handler.on_key_event(key, &mut self.editor);
        let is_normal_now = self.editor.mode == EditorMode::Normal;

        // Exit on Esc when already in Normal mode (second Esc)
        if was_normal && is_normal_now && key.code == crossterm::event::KeyCode::Esc {
            return true;
        }

        false
    }

    /// Save the editor buffer to disk.
    pub fn save(&self) -> anyhow::Result<()> {
        let content = lines_to_string(&self.editor.lines);
        fs::write(&self.file_path, content)?;
        Ok(())
    }

    /// Render the editor in the given area.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Split: header (1 line) + editor (rest)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        // Header
        let header = self.render_header(chunks[0].width);
        frame.render_widget(Paragraph::new(header), chunks[0]);

        // Editor
        let border_color = ACCENT;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(Span::styled(
                format!(" Edit — {} ", self.mode_label()),
                Style::default().fg(border_color),
            ));

        let inner = block.inner(chunks[1]);
        frame.render_widget(block, chunks[1]);

        let highlighter = SyntaxHighlighter::new("base16-ocean.dark", &self.syntax_ext);

        let view = EditorView::new(&mut self.editor)
            .wrap(true)
            .syntax_highlighter(Some(highlighter));
        view.render(inner, frame.buffer_mut());
    }

    fn render_header(&self, _width: u16) -> Line<'static> {
        if self.is_shared {
            Line::from(Span::styled(
                " Editing shared copy",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(Span::styled(
                format!(" Editing materialized copy ({})", self.worktree_name),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))
        }
    }

    fn mode_label(&self) -> &'static str {
        match self.editor.mode {
            EditorMode::Normal => "NORMAL",
            EditorMode::Insert => "INSERT",
            EditorMode::Visual => "VISUAL",
        }
    }
}

/// Determine the syntax highlighting extension for a file.
/// Maps .env and derivatives to "sh" for shell syntax.
fn resolve_syntax_ext(rel_path: &str) -> String {
    let path = Path::new(rel_path);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");

    if name == ".env" || name.starts_with(".env.") {
        return "sh".to_string();
    }

    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt")
        .to_string()
}

/// Convert edtui Lines (Jagged<char>) to a String for saving.
fn lines_to_string(lines: &Lines) -> String {
    let mut result = String::new();
    let len = lines.len();
    for i in 0..len {
        if let Some(row) = lines.get(i) {
            let line_str: String = row.iter().collect();
            result.push_str(&line_str);
        }
        if i < len - 1 {
            result.push('\n');
        }
    }
    result
}

/// Try to create an EditSession for the current worktree entry.
///
/// Returns `None` if the entry is not editable (Missing, Broken, etc.)
/// or if the file cannot be read.
pub fn try_start_edit(
    status: WorktreeStatus,
    worktree_path: &Path,
    rel_path: &str,
    git_common_dir: &Path,
    worktree_name: &str,
) -> Option<EditSession> {
    let (file_path, is_shared) = match status {
        WorktreeStatus::Linked => {
            // Edit the shared copy
            let shared = shared::shared_file_path(git_common_dir, rel_path);
            (shared, true)
        }
        WorktreeStatus::Materialized => {
            // Edit the local materialized copy
            let local = worktree_path.join(rel_path);
            (local, false)
        }
        // Not editable
        _ => return None,
    };

    // Don't edit directories
    if file_path.is_dir() {
        return None;
    }

    EditSession::new(file_path, is_shared, worktree_name.to_string(), rel_path)
}
```

- [ ] **Step 2: Register the module in mod.rs**

In `src/output/tui/shared_picker/mod.rs`, add the module declaration after
`mod dialog;`:

```rust
pub mod editor;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check` Expected: may have warnings about unused code — that's fine,
we'll wire it up next.

- [ ] **Step 4: Commit**

```bash
git add src/output/tui/shared_picker/editor.rs src/output/tui/shared_picker/mod.rs
git commit -m "feat(shared): add EditSession module for inline file editing"
```

---

### Task 3: Add edit_state to ManageMode and wire Enter in preview

**Files:**

- Modify: `src/output/tui/shared_picker/manage_mode.rs`
- Modify: `src/output/tui/shared_picker/input.rs`

- [ ] **Step 1: Add edit_state field to ManageMode**

In `src/output/tui/shared_picker/manage_mode.rs`, add to the `ManageMode` struct
after the `pending_add: bool` field:

```rust
    /// Active inline file editor session, if any.
    pub edit_state: Option<super::editor::EditSession>,
```

- [ ] **Step 2: Initialize edit_state to None in run_manage_picker_inner**

In `src/output/tui/shared_picker/mod.rs`, in the `run_manage_picker_inner`
function where `ManageMode` is constructed, add `edit_state: None` to the struct
literal:

```rust
    let mut mode = ManageMode {
        git_common_dir,
        config_root,
        materialized,
        statuses: Vec::new(),
        worktree_paths,
        worktree_root,
        info_message: None,
        diff_pivot: None,
        pending_remove: false,
        pending_add: false,
        edit_state: None,
    };
```

- [ ] **Step 3: Add start_edit method to ManageMode**

In `src/output/tui/shared_picker/manage_mode.rs`, add this method in the
`impl ManageMode` block (before the `PickerMode` impl):

```rust
    /// Start an inline editing session for the currently highlighted entry.
    fn start_edit(&mut self, state: &PickerState) {
        if state.is_virtual_tab() || state.tabs.is_empty() {
            return;
        }
        let tab = state.current_tab();
        let tab_idx = state.active_tab;
        let entry_idx = tab.list_cursor;

        let status = self
            .statuses
            .get(tab_idx)
            .and_then(|t| t.get(entry_idx))
            .copied()
            .unwrap_or(WorktreeStatus::Missing);

        let entry = &tab.entries[entry_idx];

        self.edit_state = super::editor::try_start_edit(
            status,
            &entry.worktree_path,
            &tab.rel_path,
            &self.git_common_dir,
            &entry.worktree_name,
        );
    }
```

- [ ] **Step 4: Add Enter handling in handle_preview for manage mode**

In `src/output/tui/shared_picker/input.rs`, in the `handle_preview` function,
add `Enter` to the match before the `_ => {}` catch-all:

```rust
        KeyCode::Enter => {
            return mode.handle_list_key(key.code, state);
        }
```

Then in `src/output/tui/shared_picker/manage_mode.rs`, in the `handle_list_key`
method of the `PickerMode` impl, add a case for Enter when focused on Preview.
Find the existing match block and add before the catch-all:

Actually, the cleaner approach: handle Enter in `handle_preview` by delegating
to mode. But `handle_preview` takes `&dyn PickerMode` (immutable). Let me use a
different approach — add Enter handling directly in the manage event loop.

Replace the approach: In `src/output/tui/shared_picker/input.rs`, in
`handle_preview`, add Enter:

```rust
        KeyCode::Enter => {
            // Delegate to mode for edit initiation
            return mode.handle_list_key(key.code, state);
        }
```

Wait — `handle_preview` takes `mode: &dyn PickerMode` (immutable ref), but
`handle_list_key` takes `&mut self`. The function signature is:

```rust
fn handle_preview(key: KeyEvent, state: &mut PickerState, mode: &dyn PickerMode) -> LoopAction
```

We need `&mut dyn PickerMode` here. Let me change the approach: handle Enter in
the manage event loop directly, or change handle_preview's signature.

The simplest change: in `handle_preview`, change the signature to take
`mode: &mut dyn PickerMode`:

In `src/output/tui/shared_picker/input.rs`:

Change:

```rust
fn handle_preview(key: KeyEvent, state: &mut PickerState, mode: &dyn PickerMode) -> LoopAction {
```

To:

```rust
fn handle_preview(key: KeyEvent, state: &mut PickerState, mode: &mut dyn PickerMode) -> LoopAction {
```

And in `handle_key`, the call to `handle_preview` already passes
`mode: &mut dyn PickerMode` from the function signature, so the types align.

Then add Enter to the match:

```rust
        KeyCode::Enter => return mode.handle_list_key(KeyCode::Enter, state),
```

- [ ] **Step 5: Handle Enter in ManageMode's handle_list_key for preview focus**

In `src/output/tui/shared_picker/manage_mode.rs`, the existing `handle_list_key`
method handles keys when focus is WorktreeList. But Enter from Preview should
start editing. Add at the top of `handle_list_key`:

```rust
    fn handle_list_key(&mut self, key: KeyCode, state: &mut PickerState) -> LoopAction {
        // Enter from Preview panel starts inline editing
        if key == KeyCode::Enter && state.focus == FocusPanel::Preview {
            self.start_edit(state);
            return LoopAction::Continue;
        }
        // ... existing match block
```

Add the necessary import at the top of manage_mode.rs:

```rust
use super::state::FocusPanel;
```

(Check if FocusPanel is already imported — it likely is.)

- [ ] **Step 6: Verify it compiles**

Run: `cargo check` Expected: compiles. Edit won't do anything visible yet —
we'll wire rendering and event loop interception next.

- [ ] **Step 7: Commit**

```bash
git add src/output/tui/shared_picker/manage_mode.rs src/output/tui/shared_picker/input.rs src/output/tui/shared_picker/mod.rs
git commit -m "feat(shared): wire Enter in preview to start inline editing"
```

---

### Task 4: Intercept keys in manage event loop during editing

**Files:**

- Modify: `src/output/tui/shared_picker/mod.rs`

- [ ] **Step 1: Modify run_manage_event_loop to intercept keys during editing**

In `src/output/tui/shared_picker/mod.rs`, modify the `run_manage_event_loop`
function. Replace the existing key handling with an edit-mode check:

```rust
fn run_manage_event_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<io::Stderr>>,
    state: &mut PickerState,
    mode: &mut ManageMode,
    highlighter: &Highlighter,
) -> Result<()> {
    loop {
        terminal.draw(|frame| {
            render::render(state, mode, highlighter, frame);
        })?;

        let Some(key) = input::poll_key(Duration::from_millis(100)) else {
            continue;
        };

        // When editing, route all keys to the editor
        if let Some(ref mut session) = mode.edit_state {
            if session.handle_key(key) {
                // Editor requested exit — save and close
                let _ = session.save();
                mode.edit_state = None;
            }
            continue;
        }

        match input::handle_key(key, state, mode) {
            LoopAction::Continue => {}
            LoopAction::Exit => return Ok(()),
        }

        // Check if the mode needs to show a modal (e.g. remove confirmation).
        if mode.needs_modal() {
            mode.show_modal(terminal, state)?;
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check` Expected: compiles. Keys now route to the editor when
edit_state is Some.

- [ ] **Step 3: Commit**

```bash
git add src/output/tui/shared_picker/mod.rs
git commit -m "feat(shared): intercept keys in event loop during editing"
```

---

### Task 5: Render the editor in the preview pane

**Files:**

- Modify: `src/output/tui/shared_picker/render.rs`
- Modify: `src/output/tui/shared_picker/mod.rs` (PickerMode trait)

- [ ] **Step 1: Add render_editor method to PickerMode trait**

In `src/output/tui/shared_picker/mod.rs`, add to the `PickerMode` trait:

```rust
    /// Render an inline editor in place of the preview pane.
    /// Returns `true` if an editor was rendered (preview should be skipped).
    fn render_editor(&mut self, _frame: &mut Frame, _area: Rect) -> bool {
        false
    }
```

Add `use ratatui::layout::Rect;` to the imports if not already present (it is —
it's used in `render_footer`).

- [ ] **Step 2: Implement render_editor for ManageMode**

In `src/output/tui/shared_picker/manage_mode.rs`, add in the
`impl PickerMode for ManageMode` block:

```rust
    fn render_editor(&mut self, frame: &mut Frame, area: Rect) -> bool {
        if let Some(ref mut session) = self.edit_state {
            session.render(frame, area);
            true
        } else {
            false
        }
    }
```

Ensure `Rect` is imported (add `use ratatui::layout::Rect;` if needed — check
existing imports).

- [ ] **Step 3: Change render chain to pass &mut dyn PickerMode**

In `src/output/tui/shared_picker/render.rs`, change all function signatures in
the render chain from `mode: &dyn PickerMode` to `mode: &mut dyn PickerMode`:

1. `pub fn render(state, mode: &mut dyn PickerMode, ...)` (line ~28)
2. `fn render_body(state, mode: &mut dyn PickerMode, ...)` (line ~120)
3. `fn render_split_body(state, mode: &mut dyn PickerMode, ...)` (line ~181)
4. `fn render_preview(state, mode: &mut dyn PickerMode, ...)` (line ~271)

Also update `render_warning`, `render_worktree_list` if they take
`mode: &dyn PickerMode` — check each signature and update to `&mut`.

Note: `render_tabs` takes `mode: &dyn PickerMode` for read-only access. It can
stay as `&dyn` if it only calls `&self` methods. But since we need `&mut`
through the chain, change it too for consistency, or split the borrow. The
simplest: change all to `&mut dyn PickerMode`.

- [ ] **Step 4: Call render_editor in render_preview**

In `src/output/tui/shared_picker/render.rs`, modify `render_preview` to check
for an active editor before rendering the Paragraph:

```rust
fn render_preview(
    state: &mut PickerState,
    mode: &mut dyn PickerMode,
    highlighter: &Highlighter,
    frame: &mut Frame,
    area: Rect,
) {
    // If mode has an active editor, render it instead of the preview
    if mode.render_editor(frame, area) {
        return;
    }

    // ... existing preview rendering code unchanged
    let tab = state.current_tab();
    let is_focused = state.focus == FocusPanel::Preview;
    // ... rest of the function
```

- [ ] **Step 5: Verify it compiles and runs**

Run: `cargo clippy` Expected: no errors, possibly no warnings

Run: `cargo test` Expected: all tests pass

- [ ] **Step 6: Commit**

```bash
git add src/output/tui/shared_picker/render.rs src/output/tui/shared_picker/mod.rs src/output/tui/shared_picker/manage_mode.rs
git commit -m "feat(shared): render edtui editor in preview pane"
```

---

### Task 6: End-to-end verification and cleanup

**Files:**

- Modify: `src/output/tui/shared_picker/editor.rs` (if fixes needed)

- [ ] **Step 1: Run full test suite**

Run: `mise run clippy` Expected: zero warnings

Run: `mise run test:unit` Expected: all tests pass

Run: `mise run fmt:check` Expected: no formatting issues (run `mise run fmt` if
needed)

- [ ] **Step 2: Manual testing checklist**

Test the following in a sandbox with `daft shared manage`:

1. Navigate to Preview pane with Tab
2. Press Enter — editor opens with syntax-highlighted content
3. Header shows "Editing shared copy" (green) for linked entries
4. Header shows "Editing materialized copy (name)" (yellow) for materialized
   entries
5. Type `i` to enter Insert mode — header shows "INSERT"
6. Make edits, then Esc to Normal mode
7. Esc again — editor closes, preview shows updated content
8. Verify the file on disk was updated
9. Press Enter on a Missing entry — nothing happens
10. Press Enter on a directory — nothing happens

- [ ] **Step 3: Fix any issues found**

Address any compilation, rendering, or behavioral issues.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(shared): inline file editor in manage TUI preview pane"
```
