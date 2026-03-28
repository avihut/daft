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
    /// Detects the syntax from the file extension. Recognizes `.env` and
    /// derivatives (`.env.local`, `.env.example`, etc.) as shell syntax.
    /// Falls back to plain text if the extension is unknown.
    pub fn highlight(&self, content: &str, file_path: &str) -> Vec<Line<'static>> {
        let path = Path::new(file_path);
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        let syntax = if is_dotenv_file(path) {
            self.syntax_set.find_syntax_by_extension("sh")
        } else {
            self.syntax_set.find_syntax_by_extension(extension)
        }
        .or_else(|| self.syntax_set.find_syntax_by_first_line(content))
        .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let theme = &self.theme_set.themes["base16-ocean.dark"];
        let mut highlighter = HighlightLines::new(syntax, theme);

        LinesWithEndings::from(content)
            .map(|line| {
                let ranges = highlighter
                    .highlight_line(line, &self.syntax_set)
                    .unwrap_or_default();
                let spans: Vec<Span<'static>> = ranges
                    .into_iter()
                    .map(|(style, text)| Span::styled(text.to_string(), syntect_to_ratatui(style)))
                    .collect();
                Line::from(spans)
            })
            .collect()
    }
}

/// Check if a path is a dotenv file (`.env`, `.env.local`, `.env.example`, etc.).
fn is_dotenv_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == ".env" || name.starts_with(".env.")
}

/// Convert a syntect style to a ratatui style.
fn syntect_to_ratatui(style: highlighting::Style) -> Style {
    let fg = style.foreground;
    let mut ratatui_style = Style::default().fg(Color::Rgb(fg.r, fg.g, fg.b));

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
