/// Release notes display for daft
///
/// Displays release notes from CHANGELOG.md in a scrollable interface
/// using the system pager (like git does).
use anyhow::{Context, Result};
use clap::Parser;
#[cfg(unix)]
use pager::Pager;
use regex::Regex;
use serde::Serialize;
use std::io::{self, IsTerminal, Write};
use termimad::crossterm::style::Color;
use termimad::MadSkin;

/// Embedded CHANGELOG.md content (compiled into the binary)
const CHANGELOG: &str = include_str!("../../CHANGELOG.md");

/// Represents a single release with its version, date, and content
#[derive(Debug, Clone, Serialize)]
pub struct Release {
    /// Version string (e.g., "1.0.5")
    pub version: String,
    /// Release date (e.g., "2026-01-24")
    pub date: Option<String>,
    /// Full content of the release notes
    pub content: String,
}

#[derive(Parser)]
#[command(name = "daft-release-notes")]
#[command(version = crate::VERSION, disable_version_flag = true)]
#[command(about = "Display release notes from the changelog")]
#[command(long_about = r#"
Displays release notes from daft's changelog in a scrollable interface
using the system pager (similar to how git displays man pages).

By default, shows all release notes. Use the VERSION argument to show
notes for a specific version, or use --list to see a summary of all
available versions.

The pager can be navigated using standard less commands:
  - Space/Page Down: scroll down one page
  - b/Page Up: scroll up one page
  - /pattern: search for text
  - n: find next match
  - q: quit
"#)]
pub struct Args {
    /// Show notes for a specific version (e.g., "1.0.5" or "v1.0.5")
    #[arg(value_name = "VERSION")]
    version: Option<String>,

    /// List all versions without full notes
    #[arg(short, long)]
    list: bool,

    /// Show only the latest N releases (default: all)
    #[arg(short = 'n', long, value_name = "N")]
    latest: Option<usize>,

    /// Output as JSON for scripting
    #[arg(long)]
    json: bool,

    /// Disable pager, print directly to stdout
    #[arg(long)]
    no_pager: bool,
}

pub fn run() -> Result<()> {
    // When called as a subcommand, skip "daft" and "release-notes" from args
    let mut args_vec: Vec<String> = std::env::args().collect();

    // If args start with [daft, release-notes, ...], remove "release-notes"
    // to make clap parse correctly (keep "daft" as program name)
    if args_vec.len() >= 2 && args_vec[1] == "release-notes" {
        args_vec.remove(1);
    }

    let args = Args::parse_from(&args_vec);

    // Parse the changelog
    let releases = parse_changelog(CHANGELOG)?;

    if releases.is_empty() {
        anyhow::bail!("No releases found in changelog");
    }

    // Filter releases based on arguments
    let filtered_releases = if let Some(ref version) = args.version {
        let normalized = normalize_version(version);
        let found = releases
            .into_iter()
            .find(|r| normalize_version(&r.version) == normalized);
        match found {
            Some(r) => vec![r],
            None => anyhow::bail!("Version {} not found in changelog", version),
        }
    } else if let Some(n) = args.latest {
        releases.into_iter().take(n).collect()
    } else {
        releases
    };

    // Output based on format
    if args.json {
        output_json(&filtered_releases, args.list)?;
    } else if args.list {
        output_list(&filtered_releases, args.no_pager)?;
    } else {
        output_full(&filtered_releases, args.no_pager)?;
    }

    Ok(())
}

/// Parse the changelog content into a list of releases
fn parse_changelog(content: &str) -> Result<Vec<Release>> {
    let mut releases = Vec::new();
    let mut current_version: Option<String> = None;
    let mut current_date: Option<String> = None;
    let mut current_content = String::new();

    for line in content.lines() {
        // Check if this is a version header: ## [version] - date
        if line.starts_with("## [") {
            // Save the previous release if we have one
            if let Some(version) = current_version.take() {
                releases.push(Release {
                    version,
                    date: current_date.take(),
                    content: current_content.trim().to_string(),
                });
                current_content.clear();
            }

            // Parse the new version header
            if let Some((version, date)) = parse_version_header(line) {
                current_version = Some(version);
                current_date = date;
            }
        } else if current_version.is_some() {
            // Accumulate content for current release
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Don't forget the last release
    if let Some(version) = current_version {
        releases.push(Release {
            version,
            date: current_date,
            content: current_content.trim().to_string(),
        });
    }

    Ok(releases)
}

/// Parse a version header line like "## [1.0.5] - 2026-01-24"
fn parse_version_header(line: &str) -> Option<(String, Option<String>)> {
    // Extract version from between [ and ]
    let start = line.find('[')?;
    let end = line.find(']')?;
    let version = line[start + 1..end].to_string();

    // Extract date if present (after " - ")
    let date = if let Some(date_start) = line.find(" - ") {
        let date_str = line[date_start + 3..].trim();
        if !date_str.is_empty() {
            Some(date_str.to_string())
        } else {
            None
        }
    } else {
        None
    };

    Some((version, date))
}

/// Normalize version string (remove leading 'v' or 'V' if present)
fn normalize_version(version: &str) -> String {
    version.trim_start_matches(['v', 'V']).to_lowercase()
}

/// Output releases as JSON
fn output_json(releases: &[Release], list_only: bool) -> Result<()> {
    let output = if list_only {
        // Just version and date
        let simplified: Vec<_> = releases
            .iter()
            .map(|r| {
                serde_json::json!({
                    "version": r.version,
                    "date": r.date,
                })
            })
            .collect();
        serde_json::to_string_pretty(&simplified)?
    } else {
        serde_json::to_string_pretty(releases)?
    };

    println!("{output}");
    Ok(())
}

/// Output just the version list
fn output_list(releases: &[Release], no_pager: bool) -> Result<()> {
    let mut output = String::new();

    output.push_str("Available releases:\n\n");

    for release in releases {
        if let Some(ref date) = release.date {
            output.push_str(&format!("  {} ({})\n", release.version, date));
        } else {
            output.push_str(&format!("  {}\n", release.version));
        }
    }

    output.push_str(&format!("\n{} releases total\n", releases.len()));

    display_with_pager(&output, no_pager)
}

/// Output full release notes
fn output_full(releases: &[Release], no_pager: bool) -> Result<()> {
    let mut markdown = String::new();

    for (i, release) in releases.iter().enumerate() {
        if i > 0 {
            markdown.push_str("\n---\n\n");
        }

        // Header
        if let Some(ref date) = release.date {
            markdown.push_str(&format!("## [{}] - {}\n", release.version, date));
        } else {
            markdown.push_str(&format!("## [{}]\n", release.version));
        }

        // Content
        if !release.content.is_empty() {
            markdown.push('\n');
            markdown.push_str(&release.content);
        }
        markdown.push('\n');
    }

    // Render markdown if outputting to a terminal
    let output = if io::stdout().is_terminal() {
        render_markdown(&markdown)
    } else {
        markdown
    };

    display_with_pager(&output, no_pager)
}

/// Render markdown to terminal-formatted text with colors
fn render_markdown(markdown: &str) -> String {
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

/// Create a custom skin with daft's orange accent color
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

/// Display content using pager if appropriate
fn display_with_pager(content: &str, no_pager: bool) -> Result<()> {
    // Only use pager if: stdout is TTY AND --no-pager not set
    #[cfg(unix)]
    if !no_pager && io::stdout().is_terminal() {
        // Set up pager with less-like options:
        // -F: quit if one screen
        // -I: case-insensitive search
        // -R: raw control chars (for potential color)
        // -X: don't clear screen on exit
        Pager::with_pager("less -FIRX").setup();
    }

    // Suppress unused variable warning on Windows
    #[cfg(not(unix))]
    let _ = no_pager;

    // After Pager::setup(), stdout goes to pager (if active)
    // or directly to terminal (if no pager)
    io::stdout()
        .write_all(content.as_bytes())
        .context("Failed to write output")?;
    io::stdout().flush().context("Failed to flush output")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_header() {
        let (version, date) = parse_version_header("## [1.0.5] - 2026-01-24").unwrap();
        assert_eq!(version, "1.0.5");
        assert_eq!(date, Some("2026-01-24".to_string()));

        let (version, date) = parse_version_header("## [0.1.0]").unwrap();
        assert_eq!(version, "0.1.0");
        assert_eq!(date, None);
    }

    #[test]
    fn test_normalize_version() {
        assert_eq!(normalize_version("v1.0.5"), "1.0.5");
        assert_eq!(normalize_version("1.0.5"), "1.0.5");
        assert_eq!(normalize_version("V1.0.5"), "1.0.5");
    }

    #[test]
    fn test_parse_changelog() {
        let content = r#"# Changelog

## [1.0.0] - 2026-01-24

### Features

- Initial release

## [0.9.0] - 2026-01-20

### Bug Fixes

- Fixed a bug
"#;
        let releases = parse_changelog(content).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].version, "1.0.0");
        assert_eq!(releases[0].date, Some("2026-01-24".to_string()));
        assert!(releases[0].content.contains("Initial release"));
        assert_eq!(releases[1].version, "0.9.0");
    }

    #[test]
    fn test_embedded_changelog_parses() {
        let releases = parse_changelog(CHANGELOG).unwrap();
        assert!(!releases.is_empty(), "CHANGELOG.md should have releases");
    }

    #[test]
    fn test_render_markdown_produces_ansi() {
        let md = "## Heading\n\n**bold** text";
        let rendered = render_markdown(md);
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
