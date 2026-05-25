---
name: daft-tui
description: Use when adding, refactoring, or auditing TUI code in daft — pickers, inline renderers, modal screens, key handling, terminal lifecycle, or anywhere `enable_raw_mode` / `EnterAlternateScreen` appears. Covers daft's ratatui 0.30 conventions: panic-safe terminal restore, Stylize-first styling, hjkl + arrow keybindings, edtui integration, presenter/driver/render layering, and the ratatui vs dialoguer prompt boundary.
---

# daft TUI

Conventions and architecture map for daft's terminal UI surface. Read before
writing or revising any code under `src/output/tui/` or any command that calls
`enable_raw_mode` / `EnterAlternateScreen`.

## Stack

| Dependency  | Version | Role                                       |
| ----------- | ------- | ------------------------------------------ |
| `ratatui`   | 0.30    | Rendering (default features off, crossterm) |
| `crossterm` | 0.29    | Terminal lifecycle + input events           |
| `edtui`     | 0.11    | Embedded text editor widget (syntax-highlighted) |
| `dialoguer` | 0.12    | Plain confirmation / selection prompts (no ratatui) |
| `console`   | 0.16    | Text-mode status output (no ratatui)        |

Two hard properties to keep in mind:

1. **TUI is synchronous.** No tokio inside `src/output/tui/`. Async event
   handling patterns from generic ratatui guides don't apply — events are read
   with blocking `event::read()`.
2. **`panic = "abort"` in `Cargo.toml`.** Drop guards do **not** run on panic
   in release builds. Panic hooks are the only mechanism that reliably restores
   the terminal when something blows up mid-frame.

## Layering

```
commands/<thing>.rs    Entry point. Owns the event loop only when not using
                        shared_picker. Owns the panic-hook install/restore.
        │
        ▼
src/output/tui/
  mod.rs               Public entry points + inline viewport setup.
  presenter.rs         Adapts hook lifecycle into DagEvent over mpsc::Sender.
                        Used by sync/prune to feed live state into the renderer.
  driver.rs            Synchronous event loop. Reads keys, updates TuiState,
                        triggers redraws.
  render.rs            Pure state → frame. Reads TuiState, builds widgets,
                        renders into ratatui Frame. No side effects.
  state.rs / columns.rs Data types (TuiState, FinalStatus, Column, …).

  shared_picker/       Reusable picker subsystem (collect/remove/add modals).
    mod.rs             Picker entry points + panic hook install (canonical
                        example at lines 160 and 296).
    shell.rs           restore_terminal(): single source of truth for the
                        full cleanup sequence.
    render.rs          Frame composition for the picker.
    input.rs           Key dispatch (hjkl + arrow canonical pattern).
    editor.rs          edtui wrapper with the is_edtui_supported guard.
    {collect_mode,
     remove_modal,
     add_modal}.rs     Per-mode state and key handling.
```

Read existing files in that order before adding new ones. `shared_picker/` is
the model — if you need a new modal, look at `add_modal.rs` and follow its
shape rather than rolling a fresh one.

## Panic-safe terminal lifecycle

Every TUI path that enters raw mode MUST install a panic hook that restores
the terminal. Drop guards are **not enough** because daft ships with
`panic = "abort"` — Drop doesn't run on abort.

The canonical pattern lives at `src/output/tui/shared_picker/mod.rs:160`:

```rust
pub fn run_some_picker(args: ...) -> Result<Outcome> {
    // Install panic hook BEFORE entering raw mode
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev_hook(info);
    }));

    let outcome = run_some_picker_inner(args);

    // Reset the panic hook to default on the success path
    let _ = std::panic::take_hook();

    outcome
}

fn run_some_picker_inner(args: ...) -> Result<Outcome> {
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let result = run_event_loop(&mut terminal, ...);

    restore_terminal(); // always reachable on success
    result
}
```

`restore_terminal()` (in `shared_picker/shell.rs`) is the single source of
truth for the cleanup sequence:

```rust
pub fn restore_terminal() {
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        io::stderr(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show,
    );
}
```

Rules:

- **Install the hook before `enable_raw_mode`**, not after — a panic between
  the two would otherwise hit the default hook with raw mode active.
- **Use `take_hook` + chain to `prev_hook`** so other installed hooks (e.g.
  test runners' backtrace printers) still fire after the restore.
- **Restore the default hook on the success path** with `let _ = take_hook()`
  so the picker's hook doesn't leak into post-TUI code.
- **Render to `stderr`, not `stdout`.** daft's shell wrapper reads
  `DAFT_CD_FILE` from stdout; rendering to stdout corrupts the cd-redirect
  channel. Every picker in `shared_picker/` uses stderr.
- **Don't write your own restore sequence.** Call `restore_terminal()`. If you
  need to alter the cleanup steps (e.g., add bracketed-paste-off), change it
  in `shell.rs` and audit all callers.

## Styling: Stylize over `Style::default()`

ratatui's developer guide is explicit:

> "Speaking of `Stylize`, you should use it over the more verbose style
> setters."

This is the upstream convention, not a stylistic preference — the trait was
introduced in v0.24 specifically to make styling ergonomic, and v0.30 (daft's
version) extended Stylize methods directly onto `Style`.

Prefer:

```rust
use ratatui::style::Stylize;

"running".cyan()
"failed".red().bold()
" 3 errors ".dim().on_dark_gray()

Line::from(vec![
    " q ".bold().cyan(),
    "quit ".dim(),
])

Block::bordered().title("Progress").cyan()
```

Avoid:

```rust
Style::default().fg(Color::Cyan)                          // verbose
Style::new().fg(Color::Red).add_modifier(Modifier::BOLD)  // verbose
Style::default().fg(Color::Rgb(0, 200, 200))              // bad: hardcoded RGB
```

When touching an existing file that uses verbose `Style::default()`, migrate
the styles you touch to Stylize. Don't migrate untouched files in the same
PR — that's churn unrelated to the change.

Color palette conventions in daft:

| Semantic    | Stylize call                                  |
| ----------- | --------------------------------------------- |
| Primary     | `.cyan()` (sometimes `.green()` for success)  |
| Error       | `.red()`                                      |
| Warning     | `.yellow()` (use sparingly)                   |
| Muted       | `.dim()` or `.dark_gray()`                    |
| Highlight bg | `.on_dark_gray()` or `.on_cyan()`            |

Don't hardcode RGB. Named colors respect the user's terminal theme; RGB
locks them to one palette.

## Inline viewport vs alternate screen

daft uses **both** depending on context:

- **Inline viewport** (`src/output/tui/mod.rs`) for the sync/prune progress
  table. The terminal stays in the normal scrollback; the table updates in
  place and stays in history when done. Use this for live progress that the
  user wants to scroll back to later.
- **Alternate screen** (`shared_picker/`) for interactive pickers. Full-screen
  takeover, restored on exit. Use this when the screen is a navigable UI, not
  a transcript.

Decision rule: if the output should remain visible after the command exits,
use inline viewport. If it's a modal interaction the user dismisses, use
alternate screen.

## Keybindings: hjkl + arrows together

Per the project rule in CLAUDE.md, every TUI screen with arrow navigation
must also accept hjkl. Match both in the same arm:

```rust
match key.code {
    KeyCode::Up    | KeyCode::Char('k')                       => state.move_up(),
    KeyCode::Down  | KeyCode::Char('j')                       => state.move_down(),
    KeyCode::Left  | KeyCode::Char('h')                       => state.collapse_or_prev(),
    KeyCode::Right | KeyCode::Char('l')                       => state.expand_or_next(),
    KeyCode::Char('q') | KeyCode::Esc                         => break,
    KeyCode::Enter                                            => state.commit(),
    _ => {}
}
```

See `src/output/tui/shared_picker/input.rs` and `remove_modal.rs` for the
canonical dispatch.

When a screen also contains an edtui editor, **don't dispatch hjkl as
navigation while the editor has focus** — edtui interprets them as
text-editing commands. Route keys through the focus-aware handler in
`shared_picker/editor.rs`.

## edtui integration

edtui's `From<crossterm::event::KeyCode>` impl crashes the process on any
`KeyCode` variant it doesn't recognise — a real footgun. daft wraps edtui
behind an `is_edtui_supported` guard in
`src/output/tui/shared_picker/editor.rs` that mirrors the conversion's
supported arms and drops unsupported keys silently.

Rules:

- **Never** call `editor.on_key_event(EditorEvent::from(key))` directly.
  Route through the wrapper's filtered dispatcher.
- When bumping `edtui` in `Cargo.toml`, audit `events/key/input.rs` in the
  upgraded crate and reconcile `is_edtui_supported`. The `SYNC WITH EDTUI`
  comment on the edtui line in `Cargo.toml` is the trigger.

## Prompt-library boundary

Not every interactive prompt belongs in ratatui:

| Use ratatui when                              | Use dialoguer/console when                   |
| --------------------------------------------- | -------------------------------------------- |
| Multiple panels or live state                 | Single `[y/N]` confirmation                  |
| Custom rendering (tables, progress, syntax)   | Plain "Select one:" list, no rendering       |
| Live updates from a background channel        | Linear flow with no redraws                  |
| Modal navigation (tabs, focus shifts)         | One-shot question, command continues after   |

ratatui for a confirmation prompt is over-engineering. dialoguer for anything
multi-panel is under-engineering. If you find yourself reaching for ratatui
to draw a single line, use `dialoguer::Confirm` instead.

## Anti-patterns

- **Raw mode without a panic hook.** With `panic = "abort"`, raw mode leaks
  if anything panics. Always install the hook before `enable_raw_mode`.
- **Drop-only terminal restoration.** RAII guards are debug-only effective.
  Don't rely on them for release-build safety.
- **`color_eyre`.** daft uses `anyhow` for error handling throughout. Don't
  introduce a parallel error story for TUI code.
- **tokio inside `src/output/tui/`.** Sync by design. If you need async work,
  do it before or after the TUI session, not during.
- **Hardcoded RGB colors.** Use named colors via Stylize so the user's
  terminal theme applies.
- **Writing to stdout from a picker.** Stdout is reserved for the
  `DAFT_CD_FILE` redirect path and command output. Pickers render to stderr.
- **Spawning a parallel picker variant.** If you need a new picker, extend
  `shared_picker/` (a new mode or modal). Don't fork the subsystem.

## Quick reference

| Need                     | Look at                                                |
| ------------------------ | ------------------------------------------------------ |
| Add a picker variant     | `shared_picker/{collect_mode,remove_modal,add_modal}.rs` |
| Install a panic hook     | `shared_picker/mod.rs:160`                              |
| Restore the terminal     | `shared_picker/shell.rs::restore_terminal`              |
| hjkl + arrow dispatch    | `shared_picker/input.rs`                                |
| Embed a text editor      | `shared_picker/editor.rs`                               |
| Inline progress table    | `src/output/tui/{mod,driver,render}.rs`                 |
| Style something          | `use ratatui::style::Stylize;` then `.cyan()`, `.bold()` |
| Confirm `[y/N]`          | `dialoguer::Confirm` (no ratatui)                       |
