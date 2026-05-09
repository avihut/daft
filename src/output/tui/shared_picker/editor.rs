//! Inline file editor for the shared-file manage TUI.
//!
//! Wraps an edtui `EditorState` + `EditorEventHandler` to provide a
//! vim-style editing session inside the preview pane. The session knows
//! which file it is editing and whether the file is the shared copy or
//! a local materialized copy.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::KeyEvent;
use edtui::{
    EditorEventHandler, EditorState, EditorTheme, EditorView, LineNumbers, Lines, SyntaxHighlighter,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
    Frame,
};

use crate::core::shared::{self, WorktreeStatus};

/// Whether edtui's `From<crossterm::event::KeyCode>` impl can convert this
/// key code without panicking.
///
/// edtui (as of 0.11.x) implements the conversion as an explicit `match` with
/// a final `_ => unimplemented!()` arm. Any code outside the cases listed in
/// edtui's source crashes the process on the first keypress. Mirror that
/// whitelist here and drop everything else upstream of the handler.
///
/// Source of truth: `events/key/input.rs` in the `edtui` crate.
fn is_edtui_supported(code: crossterm::event::KeyCode) -> bool {
    use crossterm::event::KeyCode::*;
    matches!(
        code,
        Char(_)
            | Enter
            | Esc
            | Backspace
            | Delete
            | Tab
            | Left
            | Right
            | Up
            | Down
            | Home
            | End
            | PageUp
            | PageDown
    )
}

/// An active inline editing session for a shared file.
pub struct EditSession {
    /// The edtui editor state (text buffer, cursor, mode, etc.).
    pub state: EditorState,
    /// The edtui event handler (vim keybindings).
    handler: EditorEventHandler,
    /// Absolute path of the file being edited.
    file_path: PathBuf,
    /// Whether the file is the shared copy (true) or a local materialized copy (false).
    pub is_shared: bool,
    /// Display name of the worktree (shown in the header).
    worktree_name: String,
    /// File extension used for syntax highlighting (e.g. "rs", "sh", "toml").
    syntax_ext: String,
    /// Original file content at load time (for skip-save-if-unchanged).
    original_content: String,
}

impl EditSession {
    /// Create a new editing session by reading the file at `file_path`.
    ///
    /// Returns `None` if the file cannot be read (e.g. missing, binary, or
    /// permission error).
    pub fn new(
        file_path: PathBuf,
        is_shared: bool,
        worktree_name: String,
        syntax_ext: String,
    ) -> Option<Self> {
        let content = fs::read_to_string(&file_path).ok()?;
        let lines = Lines::from(content.as_str());
        let mut state = EditorState::new(lines);
        state.mode = edtui::EditorMode::Insert; // Emacs mode requires Insert mode
        let handler = EditorEventHandler::emacs_mode();

        Some(Self {
            state,
            handler,
            file_path,
            is_shared,
            worktree_name,
            syntax_ext,
            original_content: content,
        })
    }

    /// Route a key event to the edtui handler.
    ///
    /// Returns `true` when the user presses Esc, signaling the caller to
    /// save and close the editing session.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Only handle key press events — ignore Release/Repeat which can
        // cause double-processing or unresponsive behavior.
        if key.kind != crossterm::event::KeyEventKind::Press {
            return false;
        }

        if key.code == crossterm::event::KeyCode::Esc {
            // Some terminals send Shift+Tab (and other keys) as an Esc-prefixed
            // escape sequence that crossterm may split into a bare Esc event
            // followed by additional characters. Peek for a follow-up event
            // within a short window — if one arrives, this Esc was part of a
            // sequence, not a standalone press.
            if crossterm::event::poll(std::time::Duration::from_millis(20)).unwrap_or(false) {
                // Consume the follow-up (part of the escape sequence)
                let _ = crossterm::event::read();
                return false;
            }
            return true;
        }

        // edtui's `From<crossterm::KeyCode>` impl panics with `unimplemented!()`
        // on any key code outside the whitelist below (see daft#345 — Shift+Tab
        // arrives as `BackTab+SHIFT`, which would otherwise crash the process).
        // Drop unsupported keys silently rather than forwarding them.
        if !is_edtui_supported(key.code) {
            return false;
        }

        self.handler.on_key_event(key, &mut self.state);
        false
    }

    /// Write the current buffer contents back to disk if changed.
    pub fn save(&self) -> std::io::Result<()> {
        let content = lines_to_string(&self.state.lines);
        if content != self.original_content {
            fs::write(&self.file_path, content)?;
        }
        Ok(())
    }

    /// Render the editor into the given area.
    ///
    /// The block title shows the editing target (shared/materialized) on the
    /// left and "Esc: save & exit" on the right, all in the frame color.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let (title, color) = if self.is_shared {
            (" Editing shared copy ".to_string(), Color::Green)
        } else {
            (
                format!(" Editing {} (materialized) ", self.worktree_name),
                Color::Yellow,
            )
        };

        let title_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
        let hint_style = Style::default().fg(color);

        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(color))
            .title(Span::styled(title, title_style))
            .title_bottom(
                Line::from(Span::styled(" Esc: save & exit ", hint_style)).right_aligned(),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);

        self.render_editor(frame, inner);
    }

    /// Render the edtui editor view.
    fn render_editor(&mut self, frame: &mut Frame, area: Rect) {
        // edtui bundles its own theme set with hyphenated names
        let highlighter = SyntaxHighlighter::new("base16-ocean-dark", &self.syntax_ext).ok();

        let theme =
            EditorTheme::default().line_numbers_style(Style::default().fg(Color::Indexed(239)));

        EditorView::new(&mut self.state)
            .theme(theme)
            .wrap(true)
            .line_numbers(LineNumbers::Absolute)
            .syntax_highlighter(highlighter)
            .render(area, frame.buffer_mut());
    }
}

/// Attempt to start an editing session for the currently highlighted entry.
///
/// Resolves the file path based on the worktree status:
/// - `Linked` -> edit the shared copy (the symlink target)
/// - `Materialized` -> edit the local copy in the worktree
/// - Other statuses -> returns `None` (not editable)
pub fn try_start_edit(
    status: WorktreeStatus,
    rel_path: &str,
    worktree_path: &Path,
    worktree_name: &str,
    git_common_dir: &Path,
) -> Option<EditSession> {
    let (file_path, is_shared) = match status {
        WorktreeStatus::Linked => {
            let shared = shared::shared_file_path(git_common_dir, rel_path);
            (shared, true)
        }
        WorktreeStatus::Materialized => {
            let local = worktree_path.join(rel_path);
            (local, false)
        }
        _ => return None,
    };

    // Only edit regular files (not directories)
    if !file_path.is_file() {
        return None;
    }

    let syntax_ext = resolve_syntax_ext(rel_path);
    EditSession::new(file_path, is_shared, worktree_name.to_string(), syntax_ext)
}

/// Map a relative file path to a syntax extension for highlighting.
///
/// Recognizes `.env` and derivatives (`.env.local`, `.env.example`) as shell
/// syntax. For all other files, uses the file extension directly.
fn resolve_syntax_ext(rel_path: &str) -> String {
    let path = Path::new(rel_path);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    if name == ".env" || name.starts_with(".env.") {
        return "sh".to_string();
    }

    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("txt")
        .to_string()
}

/// Convert edtui `Lines` (a `Jagged<char>`) back to a `String`.
///
/// Uses the built-in `to_string()` method on `Lines` which joins rows with
/// newlines.
fn lines_to_string(lines: &Lines) -> String {
    lines.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_syntax_ext_rust() {
        assert_eq!(resolve_syntax_ext("src/main.rs"), "rs");
    }

    #[test]
    fn test_resolve_syntax_ext_toml() {
        assert_eq!(resolve_syntax_ext("Cargo.toml"), "toml");
    }

    #[test]
    fn test_resolve_syntax_ext_env() {
        assert_eq!(resolve_syntax_ext(".env"), "sh");
    }

    #[test]
    fn test_resolve_syntax_ext_env_local() {
        assert_eq!(resolve_syntax_ext(".env.local"), "sh");
    }

    #[test]
    fn test_resolve_syntax_ext_env_example() {
        assert_eq!(resolve_syntax_ext(".env.example"), "sh");
    }

    #[test]
    fn test_resolve_syntax_ext_no_extension() {
        assert_eq!(resolve_syntax_ext("Makefile"), "txt");
    }

    #[test]
    fn test_resolve_syntax_ext_nested_path() {
        assert_eq!(resolve_syntax_ext("config/database.yml"), "yml");
    }

    #[test]
    fn test_lines_roundtrip() {
        let original = "line one\nline two\nline three";
        let lines = Lines::from(original);
        let result = lines_to_string(&lines);
        assert_eq!(result, original);
    }

    #[test]
    fn test_lines_roundtrip_empty() {
        let original = "";
        let lines = Lines::from(original);
        let result = lines_to_string(&lines);
        assert_eq!(result, original);
    }

    #[test]
    fn test_lines_roundtrip_single_line() {
        let original = "hello world";
        let lines = Lines::from(original);
        let result = lines_to_string(&lines);
        assert_eq!(result, original);
    }

    /// Regression for daft#345: Shift+Tab arrives from crossterm as
    /// `KeyCode::BackTab`, which edtui's `From` impl turns into
    /// `unimplemented!()`. The filter must reject it.
    #[test]
    fn back_tab_is_filtered_before_edtui() {
        assert!(!is_edtui_supported(crossterm::event::KeyCode::BackTab));
    }

    /// Other crossterm key codes that edtui doesn't handle must also be
    /// filtered — otherwise users hit the same panic via different keys
    /// (function keys, Insert, media keys, etc.).
    #[test]
    fn other_unsupported_codes_are_filtered() {
        use crossterm::event::KeyCode;
        for code in [
            KeyCode::F(1),
            KeyCode::F(12),
            KeyCode::Insert,
            KeyCode::CapsLock,
            KeyCode::ScrollLock,
            KeyCode::NumLock,
            KeyCode::PrintScreen,
            KeyCode::Pause,
            KeyCode::Menu,
            KeyCode::Null,
        ] {
            assert!(
                !is_edtui_supported(code),
                "expected {code:?} to be filtered"
            );
        }
    }

    /// The whitelist must keep the codes edtui *does* handle — a typo here
    /// would break ordinary editing.
    #[test]
    fn supported_codes_pass_the_filter() {
        use crossterm::event::KeyCode;
        for code in [
            KeyCode::Char('a'),
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Tab,
            KeyCode::Left,
            KeyCode::Right,
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::PageUp,
            KeyCode::PageDown,
        ] {
            assert!(is_edtui_supported(code), "expected {code:?} to be allowed");
        }
    }
}
