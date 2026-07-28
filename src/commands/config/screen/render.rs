//! State → frame. No side effects, no input handling, no reads.
//!
//! Colour follows the project's budget: cyan means focus and nothing else, red
//! is a value that will not work, yellow is one that works but should not be
//! trusted, green is a write that landed, and dim is every piece of metadata.
//! A row carries at most one of them.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::modal::{Field, Modal, Option_};
use super::state::{Focus, Mode, RailEntry, Row, ScreenState, StatusKind};
use crate::commands::config::resolve::{Diagnostic, Layer, Resolved};

/// Below this the rail is dropped: three columns in eighty cells leaves the
/// values truncated, and the values are the thing people came for.
const RAIL_MIN_WIDTH: u16 = 100;
const RAIL_WIDTH: u16 = 22;
const DETAIL_HEIGHT: u16 = 14;

/// Whether the rail fits in a frame this wide.
pub fn rail_fits(width: u16) -> bool {
    width >= RAIL_MIN_WIDTH
}

/// How many list rows fit, given the frame — the event loop needs this to
/// scroll by a page and to keep the cursor visible.
pub fn list_height(area: Rect, filtering: bool) -> usize {
    let chrome = 1 /* header */ + 1 /* footer */ + u16::from(filtering) + DETAIL_HEIGHT;
    area.height.saturating_sub(chrome) as usize
}

pub fn draw(frame: &mut Frame, state: &ScreenState) {
    let area = frame.area();
    let show_rail = rail_fits(area.width);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                               // header
            Constraint::Length(u16::from(state.is_filtering())), // filter
            Constraint::Min(3),                                  // list
            Constraint::Length(DETAIL_HEIGHT),                   // detail
            Constraint::Length(1),                               // footer
        ])
        .split(area);

    draw_header(frame, rows[0], state, show_rail);
    if state.is_filtering() {
        draw_filter(frame, rows[1], state);
    }

    if show_rail {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(RAIL_WIDTH), Constraint::Min(20)])
            .split(rows[2]);
        draw_rail(frame, columns[0], state);
        draw_list(frame, columns[1], state);
    } else {
        draw_list(frame, rows[2], state);
    }

    draw_detail(frame, rows[3], state);
    draw_footer(frame, rows[4], state, show_rail);

    // Last, and over everything: the editor is modal, and a box the list
    // paints through would not read as one.
    if let Some(modal) = &state.modal {
        draw_modal(frame, area, modal);
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The value editor
// ─────────────────────────────────────────────────────────────────────────

/// Centre a box of at most `width` x `height` inside `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal) {
    let lines = modal_lines(modal);

    // Size the box from what goes in it. Guessing the row count is how the
    // hint line ends up clipped off the bottom by exactly one.
    let height = lines.len() as u16 + 2;
    let box_area = centered(area, 74, height);

    // Clear first: without it the list bleeds through the box.
    frame.render_widget(Clear, box_area);
    frame.render_widget(Block::bordered().cyan(), box_area);

    let inner = Rect {
        x: box_area.x + 2,
        y: box_area.y + 1,
        width: box_area.width.saturating_sub(4),
        height: box_area.height.saturating_sub(2),
    };

    frame.render_widget(Paragraph::new(lines), inner);
}

fn modal_lines(modal: &Modal) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            modal.spec.label.to_string().bold(),
            "  ".into(),
            modal.spec.key.to_string().dim(),
        ]),
        Line::from(modal.spec.help.to_string().dim()),
        Line::from(""),
        scope_row(modal),
    ];

    if modal.scope_is_inert {
        lines.push(Line::from(
            "   daft reads this key from global config only".yellow(),
        ));
    }
    lines.push(Line::from(""));

    match &modal.field {
        Field::Options { cursor } => {
            for (index, option) in modal.options.iter().enumerate() {
                lines.push(option_row(option, index == *cursor));
            }
        }
        Field::Text {
            buffer,
            caret,
            on_unset,
        } => {
            lines.push(text_row(buffer, *caret, !*on_unset));
            if let Some(feedback) = modal.text_feedback() {
                let span = if modal.text_is_valid() {
                    feedback.dim()
                } else {
                    feedback.red()
                };
                lines.push(Line::from(vec!["      ".into(), span]));
            }
            if let Some(unset) = modal.options.last() {
                lines.push(option_row(unset, *on_unset));
            }
        }
    }

    lines.push(Line::from(""));
    match &modal.error {
        Some(error) => lines.push(Line::from(format!("✗ {error}").red())),
        None => lines.push(modal_hints(modal)),
    }

    lines
}

/// `scope:  (•) local   ( ) global`
fn scope_row(modal: &Modal) -> Line<'static> {
    let mut spans: Vec<Span> = vec!["scope: ".dim()];
    for scope in &modal.scopes {
        let chosen = *scope == modal.scope;
        spans.push(if chosen {
            "  (•) ".cyan()
        } else {
            "  ( ) ".dim()
        });
        spans.push(if chosen {
            scope.label().bold()
        } else {
            Span::from(scope.label())
        });
    }
    Line::from(spans)
}

fn option_row(option: &Option_, selected: bool) -> Line<'static> {
    let marker: Span = if selected {
        " (•) ".cyan()
    } else {
        " ( ) ".into()
    };
    match option {
        Option_::Value { value, gloss } => Line::from(vec![
            marker,
            Span::from(pad(value, 18)),
            "  ".into(),
            gloss.clone().dim(),
        ]),
        // Unset always names what it would reveal — otherwise it reads as
        // "delete" rather than "fall back to".
        Option_::Unset { reveals } => Line::from(vec![
            marker,
            Span::from(pad("unset", 18)),
            "  ".into(),
            format!("inherit: {reveals}").dim(),
        ]),
    }
}

fn text_row(buffer: &str, caret: usize, focused: bool) -> Line<'static> {
    let marker: Span = if focused {
        " (•) ".cyan()
    } else {
        " ( ) ".into()
    };
    let before: String = buffer.chars().take(caret).collect();
    let after: String = buffer.chars().skip(caret).collect();
    let mut spans = vec![marker, Span::from(before)];
    if focused {
        spans.push("▏".cyan());
    }
    spans.push(Span::from(after));
    if buffer.trim().is_empty() && focused {
        spans.push("  type a value".dim());
    }
    Line::from(spans)
}

fn modal_hints(modal: &Modal) -> Line<'static> {
    let mut hints: Vec<Span> = Vec::new();
    let push = |key: &str, what: &str, hints: &mut Vec<Span>| {
        hints.push(format!(" {key} ").bold());
        hints.push(format!("{what}  ").dim());
    };
    if modal.is_picking() {
        push("j/k", "choose", &mut hints);
    } else {
        push("↑/↓", "value or unset", &mut hints);
    }
    if modal.scopes.len() > 1 {
        push("tab", "scope", &mut hints);
    }
    push("enter", "apply", &mut hints);
    push("esc", "cancel", &mut hints);
    Line::from(hints)
}

// ─────────────────────────────────────────────────────────────────────────
// Header
// ─────────────────────────────────────────────────────────────────────────

fn draw_header(frame: &mut Frame, area: Rect, state: &ScreenState, show_rail: bool) {
    let mut spans = vec!["Daft Settings".bold(), "  ".into()];

    match &state.repo_label {
        Some(repo) => spans.push(repo.as_str().dim()),
        None => spans.push("no repository — global config only".dim()),
    }

    spans.push("   ".into());
    spans.push("writes: ".dim());
    // The pill is the one piece of header state that changes what a keystroke
    // does, so it gets the emphasis and the scope name spelled out.
    spans.push(
        format!(" {} ", state.write_scope.label())
            .bold()
            .on_dark_gray(),
    );

    if state.issue_count() > 0 && show_rail {
        spans.push("   ".into());
        spans.push(format!("{} issue(s)", state.issue_count()).yellow());
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_filter(frame: &mut Frame, area: Rect, state: &ScreenState) {
    let needle = state.filter.as_deref().unwrap_or("");
    let count = state.visible_count();
    // The caret marks a prompt that is taking keystrokes. Without it a
    // committed filter looks like it is still swallowing them.
    let caret: Span = if state.is_prompt_open() {
        "▏".cyan()
    } else {
        " ".into()
    };
    let line = Line::from(vec![
        "/".cyan(),
        needle.into(),
        caret,
        "  ".into(),
        format!("{count} matching").dim(),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

// ─────────────────────────────────────────────────────────────────────────
// Rail
// ─────────────────────────────────────────────────────────────────────────

fn draw_rail(frame: &mut Frame, area: Rect, state: &ScreenState) {
    let focused = state.focus == Focus::Rail;
    let mut lines: Vec<Line> = Vec::new();

    for (index, entry) in state.rail().iter().enumerate() {
        let selected = focused && index == state.rail_cursor();
        let active = match entry {
            RailEntry::Mode(mode) => *mode == state.mode,
            RailEntry::Category(category) => state
                .selected()
                .is_some_and(|r| r.spec.category == *category),
        };

        let (label, count) = match entry {
            RailEntry::Mode(mode) => (mode.label().to_string(), state.rail_count(*entry)),
            RailEntry::Category(category) => {
                (category.label().to_string(), state.rail_count(*entry))
            }
        };

        // A blank line before the categories: the two halves of the rail do
        // different things, and the gap is cheaper than a heading.
        if matches!(entry, RailEntry::Category(_))
            && index > 0
            && matches!(state.rail()[index - 1], RailEntry::Mode(_))
        {
            lines.push(Line::from(""));
        }

        let name = if active {
            label.clone().bold()
        } else {
            Span::from(label.clone())
        };
        let width = usize::from(area.width).saturating_sub(label.chars().count() + 6);
        let mut spans = vec![
            if selected { "▌".cyan() } else { " ".into() },
            " ".into(),
            name,
            " ".repeat(width).into(),
            format!("{count:>3}").dim(),
        ];
        if matches!(entry, RailEntry::Mode(Mode::Issues)) {
            spans[2] = label.clone().yellow();
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

// ─────────────────────────────────────────────────────────────────────────
// List
// ─────────────────────────────────────────────────────────────────────────

fn draw_list(frame: &mut Frame, area: Rect, state: &ScreenState) {
    let height = area.height as usize;
    let focused = state.focus == Focus::List;

    if state.rows().is_empty() {
        let message = if state.is_filtering() {
            "Nothing matches that."
        } else {
            "Nothing to show."
        };
        frame.render_widget(Paragraph::new(message.dim()), area);
        return;
    }

    let label_width = 34usize;
    let value_width = 22usize;

    let mut lines: Vec<Line> = Vec::new();
    for (index, row) in state
        .rows()
        .iter()
        .enumerate()
        .skip(state.scroll)
        .take(height)
    {
        match row {
            Row::Header(category) => {
                lines.push(Line::from(vec![
                    "  ".into(),
                    category.label().dim().underlined(),
                ]));
            }
            Row::Setting(setting) => {
                let Some(resolved) = state.config.settings.get(*setting) else {
                    continue;
                };
                let selected = focused && index == state.cursor();
                lines.push(setting_line(resolved, selected, label_width, value_width));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn setting_line(
    resolved: &Resolved,
    selected: bool,
    label_width: usize,
    value_width: usize,
) -> Line<'static> {
    let label = truncate(&resolved.spec.label, label_width);
    let value = truncate(resolved.effective_display(), value_width);

    // Severity first: a value that will not parse reads red however it got
    // there, and a row never carries two accents at once.
    let worst = severity(resolved);
    let value_span = match worst {
        Some(Severity::Invalid) => value.clone().red(),
        Some(Severity::Warning) => value.clone().yellow(),
        None if resolved.is_set() => value.clone().bold(),
        None => Span::from(value.clone()),
    };

    Line::from(vec![
        if selected { "▌ ".cyan() } else { "  ".into() },
        Span::from(pad(&label, label_width)),
        "  ".into(),
        value_span,
        " ".repeat(value_width.saturating_sub(value.chars().count()))
            .into(),
        "  ".into(),
        resolved.origin.label().dim(),
    ])
}

/// What a row's worst diagnostic is, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Invalid,
    Warning,
}

fn severity(resolved: &Resolved) -> Option<Severity> {
    let mut worst = None;
    for diagnostic in &resolved.diagnostics {
        let this = match diagnostic {
            Diagnostic::Invalid { .. } => Severity::Invalid,
            Diagnostic::Deprecated { .. }
            | Diagnostic::Inert { .. }
            | Diagnostic::EnvShadow { .. } => Severity::Warning,
        };
        worst = Some(match worst {
            Some(Severity::Invalid) => Severity::Invalid,
            _ => this,
        });
    }
    worst
}

// ─────────────────────────────────────────────────────────────────────────
// Detail
// ─────────────────────────────────────────────────────────────────────────

fn draw_detail(frame: &mut Frame, area: Rect, state: &ScreenState) {
    let block = Block::new().title_top("".dim());
    let inner = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: area.height,
    };
    frame.render_widget(block, area);

    let Some(resolved) = state.selected() else {
        return;
    };

    let mut lines: Vec<Line> = vec![
        Line::from("─".repeat(area.width as usize).dim()),
        Line::from(vec![
            resolved.spec.label.to_string().bold(),
            "  ".into(),
            resolved.spec.key.to_string().dim(),
        ]),
        Line::from(resolved.spec.help.to_string()),
        Line::from(""),
    ];

    // The ladder: every layer's answer, and which one won.
    for (index, rung) in resolved.rungs.iter().enumerate() {
        let winner = resolved.winner == Some(index);
        let value = rung.value.clone().unwrap_or_else(|| "—".to_string());

        let value_span = match (&rung.value, rung.inert.is_some()) {
            (Some(_), true) => value.clone().yellow(),
            (Some(_), false) if winner => value.clone().bold(),
            (Some(_), false) => Span::from(value.clone()),
            (None, _) => value.clone().dim(),
        };

        let mut spans = vec![
            if winner { " ● ".cyan() } else { "   ".into() },
            Span::from(pad(&rung.layer.label(), 14)),
            value_span,
        ];
        if rung.inert.is_some() && rung.value.is_some() {
            spans.push("  set here, but never read".yellow());
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        "   → effective: ".dim(),
        resolved.effective_display().to_string().bold(),
        "  ".into(),
        format!("({})", resolved.origin.label()).dim(),
    ]));

    for diagnostic in &resolved.diagnostics {
        lines.push(diagnostic_line(diagnostic));
    }

    // Values-format hint, when there is nothing to pick from a list.
    if resolved.spec.ty.variants().is_none()
        && let Some(hint) = resolved.spec.ty.format_hint()
    {
        lines.push(Line::from(vec!["   format: ".dim(), hint.dim()]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn diagnostic_line(diagnostic: &Diagnostic) -> Line<'static> {
    match diagnostic {
        Diagnostic::Invalid { layer, value, .. } => Line::from(vec![
            "   ✗ ".red(),
            format!("the {} value {value:?} is not valid", layer.label()).red(),
        ]),
        Diagnostic::Deprecated { alias, replacement } => Line::from(vec![
            "   ! ".yellow(),
            format!("{alias} is retired — move the value to {replacement}").yellow(),
        ]),
        Diagnostic::Inert { scope, .. } => Line::from(vec![
            "   ! ".yellow(),
            format!("the {} value is set but never read", scope.label()).yellow(),
        ]),
        Diagnostic::EnvShadow { layer, .. } => Line::from(vec![
            "   ! ".yellow(),
            match layer {
                Layer::Env(var) => format!("${var} outranks every config file"),
                _ => "a process-scoped value outranks every config file".to_string(),
            }
            .yellow(),
        ]),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Footer
// ─────────────────────────────────────────────────────────────────────────

fn draw_footer(frame: &mut Frame, area: Rect, state: &ScreenState, show_rail: bool) {
    // The status line takes the footer when there is something to report: the
    // result of what you just did matters more than the key hints you already
    // used.
    if let Some(status) = &state.status {
        let span = match status.kind {
            StatusKind::Success => status.text.clone().green(),
            StatusKind::Error => status.text.clone().red(),
            StatusKind::Info => status.text.clone().dim(),
        };
        frame.render_widget(Paragraph::new(Line::from(span)), area);
        return;
    }

    let mut hints: Vec<Span> = Vec::new();
    let push = |key: &str, what: &str, hints: &mut Vec<Span>| {
        hints.push(format!(" {key} ").bold());
        hints.push(format!("{what}  ").dim());
    };

    if state.is_prompt_open() {
        push("esc", "clear filter", &mut hints);
        push("enter", "keep it", &mut hints);
    } else {
        if state.is_filtering() {
            push("esc", "clear filter", &mut hints);
        }
        push("j/k", "move", &mut hints);
        if show_rail {
            // No rail means no second pane, so the key that switches between
            // them is not offered — advertising it would promise a place to
            // go. The same width that costs the rail costs the shortcuts that
            // only save a keystroke over `enter`.
            push("tab", "panes", &mut hints);
            push("enter", "edit", &mut hints);
            push("space", "toggle", &mut hints);
            push("u", "unset", &mut hints);
            push("/", "filter", &mut hints);
            push("s", other_scope(state), &mut hints);
            push("q", "quit", &mut hints);
        } else {
            push("enter", "edit", &mut hints);
            push("/", "filter", &mut hints);
            push("q", "quit", &mut hints);
            hints.push("rail hidden — widen".dim());
        }
    }

    frame.render_widget(Paragraph::new(Line::from(hints)), area);
}

fn other_scope(state: &ScreenState) -> &'static str {
    use crate::git::ConfigScope;
    match state.write_scope {
        ConfigScope::Local => "global",
        _ => "local",
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Text helpers
// ─────────────────────────────────────────────────────────────────────────

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return truncate(text, width);
    }
    format!("{text}{}", " ".repeat(width - len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::resolve::{Snapshot, resolve_all};
    use crate::commands::config::screen::state::ScreenState;
    use crate::core::settings::keys;
    use crate::git::{ConfigEntry, ConfigScope};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn entry(key: &str, value: &str, scope: ConfigScope) -> ConfigEntry {
        ConfigEntry {
            key: key.to_string(),
            value: value.to_string(),
            scope,
            origin_path: None,
        }
    }

    fn state_with(entries: Vec<ConfigEntry>) -> ScreenState {
        let config = resolve_all(&Snapshot {
            entries,
            in_repo: true,
            ..Default::default()
        });
        ScreenState::new(config, true, Some("daft".to_string()))
    }

    /// Render once and return the frame as plain text, line by line.
    fn painted(state: &ScreenState, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, state)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn the_screen_paints_its_furniture() {
        let state = state_with(vec![]);
        let lines = painted(&state, 120, 40);
        let all = lines.join("\n");

        assert!(all.contains("Daft Settings"), "{all}");
        assert!(all.contains("writes:"), "the scope pill is load-bearing");
        assert!(all.contains("local"), "the pill names the scope");
        assert!(all.contains("Checkout"), "the rail lists categories");
        assert!(all.contains("effective:"), "the detail panel resolves");
        assert!(all.contains("quit"), "the footer shows the way out");
    }

    #[test]
    fn the_ladder_marks_the_winning_layer() {
        let mut state = state_with(vec![
            entry(keys::MERGE_STYLE, "squash", ConfigScope::Global),
            entry(keys::MERGE_STYLE, "rebase", ConfigScope::Local),
        ]);
        state.jump_to(crate::core::settings_spec::Category::Merge);
        while state
            .selected()
            .is_some_and(|r| r.spec.key != keys::MERGE_STYLE)
        {
            state.move_down();
        }

        let lines = painted(&state, 120, 40);
        let ladder: Vec<&String> = lines.iter().filter(|l| l.contains('●')).collect();
        assert_eq!(ladder.len(), 1, "exactly one layer wins: {lines:?}");
        assert!(
            ladder[0].contains("local") && ladder[0].contains("rebase"),
            "the winner must be the local value: {:?}",
            ladder[0]
        );
    }

    #[test]
    fn the_rail_disappears_on_a_narrow_terminal_and_says_so() {
        let state = state_with(vec![]);

        let wide = painted(&state, 120, 40).join("\n");
        assert!(
            wide.contains("Push & Sync"),
            "the rail is there when it fits"
        );

        let narrow = painted(&state, 80, 40).join("\n");
        assert!(
            narrow.contains("rail hidden"),
            "a missing pane must explain itself: {narrow}"
        );
        assert!(
            narrow.contains("Daft Settings") && narrow.contains("effective:"),
            "everything else still renders"
        );
    }

    #[test]
    fn the_filter_line_appears_only_while_filtering() {
        let mut state = state_with(vec![]);
        assert!(!painted(&state, 120, 40).join("\n").contains("matching"));

        state.start_filter();
        state.filter_push('m');
        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("matching"), "{all}");
        assert!(all.contains("clear filter"), "the footer switches hints");
    }

    #[test]
    fn a_status_line_replaces_the_key_hints() {
        let mut state = state_with(vec![]);
        state.set_status("Set daft.autocd = false (local)", StatusKind::Success);
        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("Set daft.autocd"), "{all}");
        assert!(
            !all.contains("quit"),
            "what just happened outranks the hint you already used"
        );
    }

    #[test]
    fn an_invalid_value_is_called_out_in_the_detail_panel() {
        let mut state = state_with(vec![entry(
            keys::CHECKOUT_FETCH,
            "maybe",
            ConfigScope::Local,
        )]);
        while state
            .selected()
            .is_some_and(|r| r.spec.key != keys::CHECKOUT_FETCH)
        {
            state.move_down();
        }

        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("is not valid"), "{all}");
        assert!(
            all.contains("false"),
            "the effective value is still the default"
        );
    }

    #[test]
    fn an_empty_result_says_so_rather_than_painting_nothing() {
        let mut state = state_with(vec![]);
        state.start_filter();
        for ch in "zzzznothing".chars() {
            state.filter_push(ch);
        }
        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("Nothing matches"), "{all}");
    }

    #[test]
    fn the_editor_shows_the_scope_the_options_and_the_way_out() {
        let mut state = state_with(vec![]);
        while state
            .selected()
            .is_some_and(|r| r.spec.key != keys::MERGE_STYLE)
        {
            state.move_down();
        }
        state.open_modal();

        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("Merge style"), "{all}");
        assert!(all.contains("scope:"), "the write target is never implicit");
        assert!(all.contains("local") && all.contains("global"));
        assert!(all.contains("squash"), "the variants are listed");
        assert!(
            all.contains("unset") && all.contains("inherit: merge"),
            "unset has to say what it reveals, or it reads as delete"
        );
        assert!(all.contains("apply") && all.contains("cancel"));
    }

    #[test]
    fn the_editor_keeps_its_refusal_next_to_the_field() {
        let mut state = state_with(vec![]);
        while state
            .selected()
            .is_some_and(|r| r.spec.key != keys::UPDATE_CHECK)
        {
            state.move_down();
        }
        state.open_modal();
        if let Some(modal) = state.modal.as_mut() {
            modal.error = Some("refused for a reason".to_string());
        }

        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("refused for a reason"), "{all}");
    }

    #[test]
    fn a_text_setting_gets_a_field_and_a_live_format_hint() {
        let mut state = state_with(vec![]);
        while state
            .selected()
            .is_some_and(|r| r.spec.key != keys::hooks::TIMEOUT)
        {
            state.move_down();
        }
        state.open_modal();
        if let Some(modal) = state.modal.as_mut() {
            for ch in "abc".chars() {
                modal.type_char(ch);
            }
        }

        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("abc"), "the typed text shows: {all}");
        assert!(
            all.contains("expected a whole number"),
            "a value that will not parse says so while you type: {all}"
        );
    }

    #[test]
    fn the_editor_fits_a_terminal_that_barely_has_room() {
        let mut state = state_with(vec![]);
        state.open_modal();
        for (width, height) in [(20, 6), (40, 10), (60, 12), (200, 60)] {
            let _ = painted(&state, width, height);
        }
    }

    #[test]
    fn rendering_survives_a_terminal_too_small_to_be_useful() {
        // Not a design target, but a panic here takes the user's shell with
        // it — every layout constraint has to tolerate a squeeze.
        let state = state_with(vec![]);
        for (width, height) in [(20, 5), (40, 8), (100, 3), (200, 60)] {
            let _ = painted(&state, width, height);
        }
    }

    #[test]
    fn text_helpers_do_not_split_multi_byte_characters() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 10), "abc");
        // A label with an em dash or an accented character must not panic.
        assert_eq!(truncate("wörktree — layout", 6).chars().count(), 6);
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("abcdef", 3).chars().count(), 3);
    }
}
