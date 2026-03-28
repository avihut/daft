//! Inline file editor for the shared-file manage TUI.
//!
//! Wraps an edtui `EditorState` + `EditorEventHandler` to provide a
//! vim-style editing session inside the preview pane. The session knows
//! which file it is editing and whether the file is the shared copy or
//! a local materialized copy.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::KeyEvent;
use edtui::{EditorEventHandler, EditorMode, EditorState, EditorView, Lines, SyntaxHighlighter};
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
        let handler = EditorEventHandler::default();

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
    /// Returns `true` when the user wants to exit the editor (Esc pressed
    /// while already in Normal mode), signaling the caller to close the
    /// editing session. All other keys are consumed by edtui.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Detect Esc in Normal mode *before* handing to edtui — that is our
        // "exit editor" signal.
        if key.code == crossterm::event::KeyCode::Esc && self.state.mode == EditorMode::Normal {
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
    /// 2. The edtui `EditorView` filling the remaining space.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_editor(frame, chunks[1]);
    }

    /// Render the header line.
    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let target = if self.is_shared {
            "shared"
        } else {
            &self.worktree_name
        };

        let mode_name = self.state.mode.name();
        let mode_color = match self.state.mode {
            EditorMode::Normal => DIM,
            EditorMode::Insert => Color::Green,
            EditorMode::Visual => Color::Yellow,
            EditorMode::Search => Color::Cyan,
        };

        let line = Line::from(vec![
            Span::styled(
                " EDIT ",
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(target, Style::default().fg(Color::White)),
            Span::styled(" | ", Style::default().fg(DIM)),
            Span::styled(
                mode_name,
                Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" | ", Style::default().fg(DIM)),
            Span::styled("Esc(normal): exit  :w: save", Style::default().fg(DIM)),
        ]);

        frame.render_widget(Paragraph::new(line), area);
    }

    /// Render the edtui editor view.
    fn render_editor(&mut self, frame: &mut Frame, area: Rect) {
        let highlighter = SyntaxHighlighter::new("base16-ocean.dark", &self.syntax_ext).ok();

        EditorView::new(&mut self.state)
            .wrap(true)
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
