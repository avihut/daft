use crate::core::settings::{defaults, keys, parse_bool};
use crate::git::GitCommand;
use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::IsTerminal;

#[derive(Parser)]
#[command(name = "daft config remote-sync")]
#[command(about = "Configure remote sync behavior")]
pub struct Args {
    /// Enable all remote sync operations
    #[arg(long)]
    on: bool,

    /// Disable all remote sync operations
    #[arg(long, conflicts_with = "on")]
    off: bool,

    /// Show current remote sync settings
    #[arg(long)]
    status: bool,

    /// Write to global git config instead of local
    #[arg(long)]
    global: bool,
}

/// The three remote-sync settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SyncSettings {
    fetch: bool,
    push: bool,
    delete_remote: bool,
}

impl SyncSettings {
    /// Load from git config, respecting the --global flag.
    fn load(global: bool) -> Result<Self> {
        let git = GitCommand::new(false);
        let fetch_val = if global {
            git.config_get_global(keys::CHECKOUT_FETCH)?
        } else {
            git.config_get(keys::CHECKOUT_FETCH)?
        };
        let push_val = if global {
            git.config_get_global(keys::CHECKOUT_PUSH)?
        } else {
            git.config_get(keys::CHECKOUT_PUSH)?
        };
        let delete_val = if global {
            git.config_get_global(keys::BRANCH_DELETE_REMOTE)?
        } else {
            git.config_get(keys::BRANCH_DELETE_REMOTE)?
        };

        Ok(Self {
            fetch: fetch_val
                .map(|v| parse_bool(&v, defaults::CHECKOUT_FETCH))
                .unwrap_or(defaults::CHECKOUT_FETCH),
            push: push_val
                .map(|v| parse_bool(&v, defaults::CHECKOUT_PUSH))
                .unwrap_or(defaults::CHECKOUT_PUSH),
            delete_remote: delete_val
                .map(|v| parse_bool(&v, defaults::BRANCH_DELETE_REMOTE))
                .unwrap_or(defaults::BRANCH_DELETE_REMOTE),
        })
    }

    /// Save to git config.
    fn save(&self, global: bool) -> Result<()> {
        let git = GitCommand::new(false);
        let fetch_str = self.fetch.to_string();
        let push_str = self.push.to_string();
        let delete_str = self.delete_remote.to_string();

        if global {
            git.config_set_global(keys::CHECKOUT_FETCH, &fetch_str)?;
            git.config_set_global(keys::CHECKOUT_PUSH, &push_str)?;
            git.config_set_global(keys::BRANCH_DELETE_REMOTE, &delete_str)?;
        } else {
            git.config_set(keys::CHECKOUT_FETCH, &fetch_str)?;
            git.config_set(keys::CHECKOUT_PUSH, &push_str)?;
            git.config_set(keys::BRANCH_DELETE_REMOTE, &delete_str)?;
        }

        Ok(())
    }

    /// Determine the radio selection based on current values.
    fn radio_selection(&self) -> RadioSelection {
        if self.fetch && self.push && self.delete_remote {
            RadioSelection::FullSync
        } else if !self.fetch && !self.push && !self.delete_remote {
            RadioSelection::LocalOnly
        } else {
            RadioSelection::Custom
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioSelection {
    FullSync,
    LocalOnly,
    Custom,
}

/// Run the remote-sync config subcommand.
pub fn run(args: &[String]) -> Result<()> {
    // Parse args using clap
    let parsed = {
        let mut cli_args = vec!["daft config remote-sync".to_string()];
        cli_args.extend_from_slice(args);
        match Args::try_parse_from(cli_args) {
            Ok(args) => args,
            Err(e) => {
                // clap treats --help and --version as errors; print and exit cleanly
                e.print().ok();
                if e.use_stderr() {
                    std::process::exit(1);
                } else {
                    return Ok(());
                }
            }
        }
    };

    if parsed.status {
        return show_status(parsed.global);
    }

    if parsed.on {
        return set_all(true, parsed.global);
    }

    if parsed.off {
        return set_all(false, parsed.global);
    }

    // No flags: launch interactive TUI (only if stderr is a terminal)
    if !std::io::stderr().is_terminal() {
        anyhow::bail!(
            "Interactive mode requires a terminal.\n\
             Use --on, --off, or --status for non-interactive usage."
        );
    }

    run_tui(parsed.global)
}

/// Show current settings status.
fn show_status(global: bool) -> Result<()> {
    let settings = SyncSettings::load(global)?;
    let scope = if global { "global" } else { "local" };
    let preset = match settings.radio_selection() {
        RadioSelection::FullSync => "Full sync",
        RadioSelection::LocalOnly => "Local only",
        RadioSelection::Custom => "Custom",
    };

    eprintln!("Remote sync ({scope} config): {preset}");
    eprintln!();
    eprintln!(
        "  Fetch before checkout:     {}",
        if settings.fetch { "on" } else { "off" }
    );
    eprintln!(
        "  Push new branches:         {}",
        if settings.push { "on" } else { "off" }
    );
    eprintln!(
        "  Delete remote branches:    {}",
        if settings.delete_remote { "on" } else { "off" }
    );

    Ok(())
}

/// Set all remote sync settings on or off.
fn set_all(on: bool, global: bool) -> Result<()> {
    let settings = SyncSettings {
        fetch: on,
        push: on,
        delete_remote: on,
    };
    settings.save(global)?;

    let scope = if global { "global" } else { "local" };
    let state = if on { "enabled" } else { "disabled" };
    eprintln!("Remote sync {state} ({scope} config)");

    Ok(())
}

// ── TUI ───────────────────────────────────────────────────────────────────────

/// Index into the navigable items.
/// 0 = Full sync, 1 = Local only, 2 = Custom
/// 3 = Fetch checkbox, 4 = Push checkbox, 5 = Delete checkbox
const ITEM_FULL_SYNC: usize = 0;
const ITEM_LOCAL_ONLY: usize = 1;
const ITEM_CUSTOM: usize = 2;
const ITEM_FETCH: usize = 3;
const ITEM_PUSH: usize = 4;
const ITEM_DELETE: usize = 5;

struct TuiState {
    settings: SyncSettings,
    cursor: usize,
    global: bool,
}

impl TuiState {
    fn new(settings: SyncSettings, global: bool) -> Self {
        Self {
            settings,
            cursor: match settings.radio_selection() {
                RadioSelection::FullSync => ITEM_FULL_SYNC,
                RadioSelection::LocalOnly => ITEM_LOCAL_ONLY,
                RadioSelection::Custom => ITEM_CUSTOM,
            },
            global,
        }
    }

    fn radio_selection(&self) -> RadioSelection {
        self.settings.radio_selection()
    }

    /// Whether checkboxes are navigable (Custom mode or already on a checkbox).
    fn checkboxes_navigable(&self) -> bool {
        self.radio_selection() == RadioSelection::Custom
    }

    /// Maximum cursor position.
    fn max_cursor(&self) -> usize {
        if self.checkboxes_navigable() {
            ITEM_DELETE
        } else {
            ITEM_CUSTOM
        }
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.cursor < self.max_cursor() {
            self.cursor += 1;
        }
    }

    fn toggle(&mut self) {
        match self.cursor {
            ITEM_FULL_SYNC => {
                self.settings.fetch = true;
                self.settings.push = true;
                self.settings.delete_remote = true;
            }
            ITEM_LOCAL_ONLY => {
                self.settings.fetch = false;
                self.settings.push = false;
                self.settings.delete_remote = false;
            }
            ITEM_CUSTOM => {
                // Selecting "Custom" doesn't change checkboxes, just allows
                // individual toggling. If currently all-on or all-off, flip one
                // checkbox to make it truly custom.
                if self.radio_selection() != RadioSelection::Custom {
                    // Toggle fetch to break the all-same pattern
                    self.settings.fetch = !self.settings.fetch;
                }
            }
            ITEM_FETCH => self.settings.fetch = !self.settings.fetch,
            ITEM_PUSH => self.settings.push = !self.settings.push,
            ITEM_DELETE => self.settings.delete_remote = !self.settings.delete_remote,
            _ => {}
        }
    }
}

fn run_tui(global: bool) -> Result<()> {
    let settings = SyncSettings::load(global)?;
    let mut state = TuiState::new(settings, global);

    enable_raw_mode().context("Failed to enable raw mode")?;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<bool> {
        let backend = ratatui::backend::CrosstermBackend::new(std::io::stderr());
        let mut terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(10),
            },
        )?;

        loop {
            terminal.draw(|frame| {
                render_tui(&state, frame);
            })?;

            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            return Ok(false); // cancelled
                        }
                        KeyCode::Enter => {
                            return Ok(true); // save
                        }
                        KeyCode::Up | KeyCode::Char('k') => state.move_up(),
                        KeyCode::Down | KeyCode::Char('j') => state.move_down(),
                        KeyCode::Char(' ') => state.toggle(),
                        _ => {}
                    }
                }
            }
        }
    }));

    disable_raw_mode().context("Failed to disable raw mode")?;

    match result {
        Ok(Ok(true)) => {
            // Save settings
            state.settings.save(state.global)?;
            let scope = if state.global { "global" } else { "local" };
            let preset = match state.radio_selection() {
                RadioSelection::FullSync => "Full sync",
                RadioSelection::LocalOnly => "Local only",
                RadioSelection::Custom => "Custom",
            };
            eprintln!("Saved: {preset} ({scope} config)");
            Ok(())
        }
        Ok(Ok(false)) => {
            eprintln!("Cancelled");
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(panic) => {
            std::panic::resume_unwind(panic);
        }
    }
}

fn render_tui(state: &TuiState, frame: &mut ratatui::Frame) {
    let area = frame.area();
    let radio = state.radio_selection();
    let scope = if state.global {
        "global config"
    } else {
        "local config"
    };

    let dim_style = Style::default().fg(Color::DarkGray);
    let normal_style = Style::default();
    let highlight_style = Style::default().add_modifier(Modifier::BOLD);
    let title_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .fg(Color::Cyan);
    let scope_style = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // Title line
    lines.push(Line::from(vec![
        Span::styled(" Remote Sync", title_style),
        Span::raw("  "),
        Span::styled(scope, scope_style),
    ]));

    // Separator
    lines.push(Line::from(Span::styled(
        " ─────────────────────────────────────────────────",
        dim_style,
    )));

    // Radio items
    let radio_items = [
        (ITEM_FULL_SYNC, "Full sync", RadioSelection::FullSync),
        (ITEM_LOCAL_ONLY, "Local only", RadioSelection::LocalOnly),
        (ITEM_CUSTOM, "Custom", RadioSelection::Custom),
    ];

    for (idx, label, selection) in &radio_items {
        let cursor_char = if state.cursor == *idx {
            " \u{203A} "
        } else {
            "   "
        };
        let radio_char = if radio == *selection {
            "\u{25CF}" // ●
        } else {
            "\u{25CB}" // ○
        };
        let style = if state.cursor == *idx {
            highlight_style
        } else {
            normal_style
        };
        lines.push(Line::from(vec![
            Span::styled(cursor_char, style),
            Span::styled(format!("{radio_char} {label}"), style),
        ]));
    }

    // Checkboxes (indented under Custom)
    let checkboxes_visible = radio == RadioSelection::Custom;
    let checkbox_items = [
        (ITEM_FETCH, "Fetch before checkout", state.settings.fetch),
        (ITEM_PUSH, "Push new branches", state.settings.push),
        (
            ITEM_DELETE,
            "Delete remote branches",
            state.settings.delete_remote,
        ),
    ];

    if checkboxes_visible {
        for (i, (idx, label, checked)) in checkbox_items.iter().enumerate() {
            let cursor_char = if state.cursor == *idx {
                " \u{203A}"
            } else {
                "  "
            };
            let tree_char = if i < checkbox_items.len() - 1 {
                "\u{251C}" // ├
            } else {
                "\u{2514}" // └
            };
            let check_char = if *checked { "x" } else { " " };
            let style = if state.cursor == *idx {
                highlight_style
            } else {
                normal_style
            };
            lines.push(Line::from(vec![
                Span::styled(cursor_char, style),
                Span::styled(format!("   {tree_char} [{check_char}] {label}"), style),
            ]));
        }
    }

    // Blank line
    lines.push(Line::from(""));

    // Help line
    lines.push(Line::from(Span::styled(
        " \u{2191}\u{2193} navigate  space toggle  enter confirm  q cancel",
        dim_style,
    )));

    let text = ratatui::text::Text::from(lines);
    let widget = ratatui::widgets::Paragraph::new(text);
    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radio_selection_all_on() {
        let s = SyncSettings {
            fetch: true,
            push: true,
            delete_remote: true,
        };
        assert_eq!(s.radio_selection(), RadioSelection::FullSync);
    }

    #[test]
    fn test_radio_selection_all_off() {
        let s = SyncSettings {
            fetch: false,
            push: false,
            delete_remote: false,
        };
        assert_eq!(s.radio_selection(), RadioSelection::LocalOnly);
    }

    #[test]
    fn test_radio_selection_mixed() {
        let s = SyncSettings {
            fetch: true,
            push: false,
            delete_remote: false,
        };
        assert_eq!(s.radio_selection(), RadioSelection::Custom);
    }

    #[test]
    fn test_tui_state_move_up_at_top() {
        let s = SyncSettings {
            fetch: false,
            push: false,
            delete_remote: false,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = 0;
        state.move_up();
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn test_tui_state_move_down_stops_at_custom_when_not_custom() {
        let s = SyncSettings {
            fetch: false,
            push: false,
            delete_remote: false,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = ITEM_CUSTOM;
        state.move_down();
        // Should not go past Custom when in LocalOnly mode
        assert_eq!(state.cursor, ITEM_CUSTOM);
    }

    #[test]
    fn test_tui_state_move_down_into_checkboxes_when_custom() {
        let s = SyncSettings {
            fetch: true,
            push: false,
            delete_remote: false,
        };
        let mut state = TuiState::new(s, false);
        assert_eq!(state.radio_selection(), RadioSelection::Custom);
        state.cursor = ITEM_CUSTOM;
        state.move_down();
        assert_eq!(state.cursor, ITEM_FETCH);
    }

    #[test]
    fn test_toggle_full_sync_sets_all_true() {
        let s = SyncSettings {
            fetch: false,
            push: false,
            delete_remote: false,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = ITEM_FULL_SYNC;
        state.toggle();
        assert!(state.settings.fetch);
        assert!(state.settings.push);
        assert!(state.settings.delete_remote);
    }

    #[test]
    fn test_toggle_local_only_sets_all_false() {
        let s = SyncSettings {
            fetch: true,
            push: true,
            delete_remote: true,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = ITEM_LOCAL_ONLY;
        state.toggle();
        assert!(!state.settings.fetch);
        assert!(!state.settings.push);
        assert!(!state.settings.delete_remote);
    }

    #[test]
    fn test_toggle_individual_checkbox() {
        let s = SyncSettings {
            fetch: true,
            push: false,
            delete_remote: false,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = ITEM_PUSH;
        state.toggle();
        assert!(state.settings.push);
        // Others unchanged
        assert!(state.settings.fetch);
        assert!(!state.settings.delete_remote);
    }

    #[test]
    fn test_checkboxes_not_navigable_when_all_on() {
        let s = SyncSettings {
            fetch: true,
            push: true,
            delete_remote: true,
        };
        let state = TuiState::new(s, false);
        assert!(!state.checkboxes_navigable());
        assert_eq!(state.max_cursor(), ITEM_CUSTOM);
    }

    #[test]
    fn test_checkboxes_navigable_when_custom() {
        let s = SyncSettings {
            fetch: true,
            push: false,
            delete_remote: false,
        };
        let state = TuiState::new(s, false);
        assert!(state.checkboxes_navigable());
        assert_eq!(state.max_cursor(), ITEM_DELETE);
    }

    #[test]
    fn test_initial_cursor_matches_radio() {
        let full = SyncSettings {
            fetch: true,
            push: true,
            delete_remote: true,
        };
        assert_eq!(TuiState::new(full, false).cursor, ITEM_FULL_SYNC);

        let local = SyncSettings {
            fetch: false,
            push: false,
            delete_remote: false,
        };
        assert_eq!(TuiState::new(local, false).cursor, ITEM_LOCAL_ONLY);

        let custom = SyncSettings {
            fetch: true,
            push: false,
            delete_remote: true,
        };
        assert_eq!(TuiState::new(custom, false).cursor, ITEM_CUSTOM);
    }

    #[test]
    fn test_toggle_custom_from_full_sync_makes_custom() {
        let s = SyncSettings {
            fetch: true,
            push: true,
            delete_remote: true,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = ITEM_CUSTOM;
        state.toggle();
        assert_eq!(state.radio_selection(), RadioSelection::Custom);
    }

    #[test]
    fn test_cursor_clamps_when_leaving_custom_mode() {
        // Start in custom mode with cursor on a checkbox
        let s = SyncSettings {
            fetch: true,
            push: false,
            delete_remote: false,
        };
        let mut state = TuiState::new(s, false);
        state.cursor = ITEM_FETCH;
        // Toggle fetch off -> now all false -> LocalOnly mode
        state.toggle();
        assert_eq!(state.radio_selection(), RadioSelection::LocalOnly);
        // Cursor is still at ITEM_FETCH but max_cursor is now ITEM_CUSTOM
        // move_down should not go further
        state.cursor = ITEM_CUSTOM;
        state.move_down();
        assert_eq!(state.cursor, ITEM_CUSTOM);
    }
}
