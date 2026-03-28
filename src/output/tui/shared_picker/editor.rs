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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
    Frame,
};

use crate::core::shared::{self, WorktreeStatus};

const ACCENT: Color = Color::Indexed(208);
const DIM: Color = Color::DarkGray;

/// An active inline editing session for a shared file.
pub struct EditSession {
    /// The edtui editor state (text buffer, cursor, mode, etc.).
    pub state: EditorState,
    /// The edtui event handler (vim keybindings).
    handler: EditorEventHandler,
    /// Absolute path of the file being edited.
    file_path: PathBuf,
    /// Whether the file is the shared copy (true) or a local materialized copy (false).
    is_shared: bool,
    /// Display name of the worktree (shown in the header).
    worktree_name: String,
    /// File extension used for syntax highlighting (e.g. "rs", "sh", "toml").
    syntax_ext: String,
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
        let state = EditorState::new(lines);
        let handler = EditorEventHandler::emacs_mode();

        Some(Self {
            state,
            handler,
            file_path,
            is_shared,
            worktree_name,
            syntax_ext,
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
            return true;
        }

        self.handler.on_key_event(key, &mut self.state);
        false
    }

    /// Write the current buffer contents back to disk.
    pub fn save(&self) -> std::io::Result<()> {
        let content = lines_to_string(&self.state.lines);
        fs::write(&self.file_path, content)
    }

    /// Render the editor into the given area.
    ///
    /// Layout (top to bottom):
    /// 1. A one-line header showing the file target and current editor mode.
    /// 2. The edtui `EditorView` inside a bordered block (matching the preview frame).
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        self.render_header(frame, chunks[0]);

        // Bordered block matching the preview pane frame
        let block = ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(ACCENT))
            .title(Span::styled(" Edit ", Style::default().fg(ACCENT)));
        let inner = block.inner(chunks[1]);
        frame.render_widget(block, chunks[1]);

        self.render_editor(frame, inner);
    }

    /// Render the header line showing what's being edited.
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let (target, color) = if self.is_shared {
            ("Editing shared copy", Color::Green)
        } else {
            ("Editing materialized copy", Color::Yellow)
        };

        let mut spans = vec![Span::styled(
            format!(" {target}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )];

        if !self.is_shared {
            spans.push(Span::styled(
                format!(" ({})", self.worktree_name),
                Style::default().fg(color),
            ));
        }

        spans.push(Span::styled(" | ", Style::default().fg(DIM)));
        spans.push(Span::styled("Esc: save & exit", Style::default().fg(DIM)));

        frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
}
