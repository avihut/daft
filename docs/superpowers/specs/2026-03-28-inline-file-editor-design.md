# Inline File Editor for Manage TUI

## Goal

Add inline file editing to the manage TUI's preview pane, allowing users to edit
shared and materialized files without leaving the interface. Uses the `edtui`
crate for a Vim-style editor widget with syntax highlighting that matches the
existing read-only preview.

## Entry and Exit

- **Enter edit mode:** Press `Enter` while focused on the Preview pane.
- **Exit edit mode:** Press `Esc` in edtui's Normal mode. Saves immediately to
  disk and returns to the read-only preview.
- Non-editable entries (Missing, Broken, NotCollected) ignore `Enter`.

## What Gets Edited

The file path depends on the worktree's status:

- **Linked:** Edits the shared copy at `.git/.daft/shared/<rel_path>`. The
  worktree file is a symlink to this path, so all linked worktrees see the
  change.
- **Materialized:** Edits the local copy at `<worktree_path>/<rel_path>`. Only
  this worktree is affected.

Conflict status is not editable (the user should resolve the conflict first via
materialize or link).

## Editor Header

A single-line header above the editor area, color-coded by target:

- **Shared copy** (linked worktree): `"Editing shared copy"` in green.
- **Materialized copy:** `"Editing materialized copy (<worktree_name>)"` in
  yellow.

## Editor Widget

The `edtui` crate provides:

- `EditorState` — owns the text buffer.
- `EditorView` — ratatui widget rendered in the preview pane area (minus the
  header line).
- `EditorEventHandler` — routes key events to the editor.

Configuration:

- **Syntax highlighting:** `SyntaxHighlighter::new("base16-ocean.dark", ext)`
  with `.env` files mapped to `"sh"` extension, matching the existing
  `Highlighter` output.
- **Line numbers:** Enabled (edtui default).
- **Word wrap:** Enabled via `.wrap(true)`.
- **Keybindings:** Vim (edtui default) — Normal mode for navigation, Insert mode
  for editing, Esc returns to Normal.

## Input Handling

While edit mode is active, all key events route to
`EditorEventHandler.on_key_event()`. The rest of the manage UI (worktree list,
tabs, footer) is frozen.

The one exception: `Esc` when already in edtui's Normal mode exits edit mode.
The flow is: `Esc` in Insert mode transitions to Normal mode (standard Vim
behavior). A subsequent `Esc` while already in Normal mode exits the editor,
saves, and returns to the preview. This means two Esc presses to exit from
Insert mode, one from Normal mode — matching Vim conventions.

## Save Behavior

On exit (Esc in Normal mode):

1. Read the buffer content from `EditorState`.
2. Write to the resolved file path (shared or materialized).
3. Clear the edit session.
4. Return to read-only preview.

No confirmation dialog — consistent with manage mode's immediate-action model.

## Architecture

### New State

`ManageMode` gains a new field:

```rust
pub edit_state: Option<EditSession>,
```

`EditSession` contains:

- `editor: EditorState` — the edtui buffer.
- `handler: EditorEventHandler` — processes key events.
- `file_path: PathBuf` — resolved path to save to.
- `is_shared: bool` — true if editing the shared copy, false if materialized.
- `worktree_name: String` — for the header display.
- `syntax_ext: String` — file extension for syntax highlighting.

### Event Loop Integration

In the manage event loop (`run_manage_event_loop`):

1. Check `mode.edit_state.is_some()`.
2. If editing: route all keys to the editor handler. On Esc in Normal mode, save
   and clear `edit_state`.
3. If not editing: normal key routing through `input::handle_key`.

### Render Integration

In `render_preview`:

1. Check if manage mode has an active edit session.
2. If editing: render the header line, then `EditorView` in the remaining area.
3. If not editing: render the normal syntax-highlighted `Paragraph` (current
   behavior).

The `preview_override` trait method is not used for editing — it returns
`Vec<Line>` which is read-only. The renderer checks the edit state directly.

### Entering Edit Mode

When `Enter` is pressed in the Preview panel:

1. Determine the current worktree entry and its status.
2. If status is Linked or Materialized: a. Resolve the file path (shared target
   or worktree local copy). b. Read the file content. c. Create `EditorState`
   with the content. d. Create `EditorEventHandler` (Vim mode). e. Store as
   `edit_state = Some(EditSession { ... })`.
3. Otherwise: no-op.

### New Dependency

```toml
edtui = "0.11"
```

The crate depends on `syntect ^5` (compatible with our `syntect = "5.2"`) and
`ratatui-core ^0.1` / `ratatui-widgets ^0.3` (compatible with our
`ratatui = "0.30"`).
