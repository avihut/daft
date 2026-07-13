//! Shared terminal markdown renderer.
//!
//! Renders CommonMark to ANSI-styled text using termimad with daft's orange
//! accent skin, and rewrites `[text](url)` links as OSC 8 terminal hyperlinks.
//! Used by any command that shows embedded markdown to a human in a terminal
//! (`release-notes`, `skill show`, …); machine-readable and redirected output
//! paths bypass this and emit the raw source instead.

use regex::Regex;
use termimad::MadSkin;
use termimad::crossterm::style::Color;

/// Render markdown to terminal-formatted text with colors and OSC 8 links.
pub fn render(markdown: &str) -> String {
    let processed = markdown_links_to_osc8(markdown);
    let skin = create_daft_skin();
    skin.term_text(&processed).to_string()
}

/// Convert markdown links `[text](url)` to OSC 8 terminal hyperlinks.
///
/// OSC 8 is an escape sequence supported by modern terminals (iTerm2, Kitty,
/// GNOME Terminal, Windows Terminal, WezTerm, etc.) that makes text clickable.
/// Terminals that don't support it simply display the link text, which is a
/// graceful degradation from the raw markdown syntax.
fn markdown_links_to_osc8(markdown: &str) -> String {
    let link_re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").expect("valid regex");
    link_re
        .replace_all(markdown, "\x1b]8;;$2\x1b\\\x1b[34m$1\x1b[39m\x1b]8;;\x1b\\")
        .into_owned()
}

/// Create a custom skin with daft's orange accent color.
fn create_daft_skin() -> MadSkin {
    // Daft orange accent color
    let orange = Color::Rgb {
        r: 255,
        g: 140,
        b: 0,
    };
    let light_orange = Color::Rgb {
        r: 255,
        g: 180,
        b: 100,
    };

    let mut skin = MadSkin::default();

    // Headers in orange
    skin.set_headers_fg(orange);

    // Bold text in orange
    skin.bold.set_fg(orange);

    // Italic in a lighter orange
    skin.italic.set_fg(light_orange);

    // Inline code with subtle styling
    skin.inline_code.set_fg(Color::Rgb {
        r: 200,
        g: 200,
        b: 200,
    });

    // Bullet points in orange
    skin.bullet.set_fg(orange);

    // Horizontal rules in orange
    skin.horizontal_rule.set_fg(orange);

    skin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_markdown_produces_ansi() {
        let md = "## Heading\n\n**bold** text";
        let rendered = render(md);
        // Rendered output should contain ANSI escape codes (start with \x1b[)
        assert!(
            rendered.contains("\x1b["),
            "Rendered markdown should contain ANSI codes"
        );
    }

    #[test]
    fn test_markdown_links_to_osc8() {
        let input = "see [#42](https://github.com/org/repo/pull/42) for details";
        let result = markdown_links_to_osc8(input);
        assert!(
            result.contains("\x1b]8;;https://github.com/org/repo/pull/42\x1b\\"),
            "should contain OSC 8 open sequence with URL"
        );
        assert!(
            result.contains("\x1b[34m#42\x1b[39m"),
            "should contain blue-colored link text"
        );
        assert!(
            !result.contains("[#42]"),
            "should not contain raw markdown link syntax"
        );
    }

    #[test]
    fn test_markdown_links_to_osc8_multiple() {
        let input = "[a](https://a.com) and [b](https://b.com)";
        let result = markdown_links_to_osc8(input);
        assert!(result.contains("\x1b]8;;https://a.com\x1b\\\x1b[34ma\x1b[39m\x1b]8;;\x1b\\"));
        assert!(result.contains("\x1b]8;;https://b.com\x1b\\\x1b[34mb\x1b[39m\x1b]8;;\x1b\\"));
    }

    #[test]
    fn test_markdown_links_to_osc8_no_links() {
        let input = "plain text with no links";
        let result = markdown_links_to_osc8(input);
        assert_eq!(result, input);
    }
}
