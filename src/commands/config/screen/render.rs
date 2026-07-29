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
use crate::commands::config::resolve::{
    Diagnostic, Layer, Resolved, ResolvedBehavior, ResolvedSet,
};
use crate::commands::config::write::WriteScope;

/// Below this the rail is dropped: three columns in eighty cells leaves the
/// values truncated, and the values are the thing people came for.
const RAIL_MIN_WIDTH: u16 = 100;
const RAIL_WIDTH: u16 = 22;
const DETAIL_HEIGHT: u16 = 14;

/// Whether the rail fits in a frame this wide.
pub fn rail_fits(width: u16) -> bool {
    width >= RAIL_MIN_WIDTH
}

/// The vertical split: header, filter, list, detail, footer.
///
/// One function because two callers need the same answer. Deriving the list's
/// height by subtracting a chrome total instead is wrong twice over: it drifts
/// the moment a pane is added, and it disagrees with the solver *today* on a
/// short terminal, where `Min(3)` outranks the detail pane's fixed height and
/// the detail pane is what gives way. Subtracting a fixed 16 from a 16-row
/// frame yields zero rows, which reads as "nothing is visible" and stops the
/// scroll from ever following the cursor.
fn panes(area: Rect, filtering: bool) -> [Rect; 5] {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                    // header
            Constraint::Length(u16::from(filtering)), // filter
            Constraint::Min(3),                       // list
            Constraint::Length(DETAIL_HEIGHT),        // detail
            Constraint::Length(1),                    // footer
        ])
        .split(area);
    [rows[0], rows[1], rows[2], rows[3], rows[4]]
}

/// How many list rows fit, given the frame — the event loop needs this to
/// scroll by a page and to keep the cursor visible.
pub fn list_height(area: Rect, filtering: bool) -> usize {
    panes(area, filtering)[2].height as usize
}

pub fn draw(frame: &mut Frame, state: &ScreenState) {
    let area = frame.area();
    let show_rail = rail_fits(area.width);

    let rows = panes(area, state.is_filtering());

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
            modal.subject.label().bold(),
            "  ".into(),
            modal.subject.name().dim(),
        ]),
        Line::from(modal.subject.help().dim()),
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
        let label = modal.scope_label(*scope);
        spans.push(if chosen {
            label.bold()
        } else {
            Span::from(label)
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
    // does, so it gets the emphasis and the scope name spelled out — named for
    // the row under the cursor, because the two scopes are different *places*
    // per backend. "global" over a daft.yml row would promise a user-wide
    // setting and deliver an edit to the repository's committed file.
    let write_scope = match state.write_scope {
        crate::git::ConfigScope::Global => WriteScope::Global,
        _ => WriteScope::Local,
    };
    let scope = match state.selected() {
        Some(resolved) => write_scope.label_for(&resolved.spec),
        None => state.write_scope.label(),
    };
    spans.push(format!(" {scope} ").bold().on_dark_gray());

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
            RailEntry::Behaviors => state.selected_behavior().is_some(),
            RailEntry::Category(category) => state
                .selected()
                .is_some_and(|r| r.spec.category == *category),
        };

        let label = match entry {
            RailEntry::Mode(mode) => mode.label().to_string(),
            RailEntry::Behaviors => "Behaviors".to_string(),
            RailEntry::Category(category) => category.label().to_string(),
        };
        let count = state.rail_count(*entry);

        // A blank line before the rest: the halves of the rail do different
        // things, and the gap is cheaper than a heading.
        if matches!(entry, RailEntry::Category(_) | RailEntry::Behaviors)
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
    let width = area.width as usize;
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
        let current = index == state.cursor();
        match row {
            Row::Header(category) => {
                lines.push(Line::from(vec![
                    "  ".into(),
                    category.label().dim().underlined(),
                ]));
            }
            Row::Spacer => lines.push(Line::from("")),
            Row::Setting(setting) => {
                let Some(resolved) = state.config.settings.get(*setting) else {
                    continue;
                };
                let spans = setting_spans(resolved, label_width, value_width);
                lines.push(list_row(spans, current, focused, width));
            }
            Row::BehaviorHeader => {
                lines.push(Line::from(vec![
                    "  ".into(),
                    "Behaviors".dim().underlined(),
                ]));
            }
            Row::Behavior(behavior) => {
                let Some(resolved) = state.config.behaviors.get(*behavior) else {
                    continue;
                };
                let spans = behavior_spans(resolved, &state.config, label_width, value_width);
                lines.push(list_row(spans, current, focused, width));
            }
            Row::StrayHeader => {
                lines.push(Line::from(vec![
                    "  ".into(),
                    "Set in config, but not settings daft knows"
                        .yellow()
                        .underlined(),
                ]));
            }
            Row::Stray(stray) => {
                let Some(entry) = state.config.unrecognized.get(*stray) else {
                    continue;
                };
                let value = truncate(&entry.value, value_width);
                let spans = vec![
                    Span::from(pad(&entry.key, label_width)),
                    "  ".into(),
                    value.clone().yellow(),
                    " ".repeat(value_width.saturating_sub(value.chars().count()))
                        .into(),
                    "  ".into(),
                    format!("{} — ignored", entry.scope.label()).dim(),
                ];
                lines.push(list_row(spans, current, focused, width));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Finish a list row: mark it, pad it out, and lay the highlight across all
/// of it.
///
/// The padding is what makes the highlight a *row* rather than a stripe behind
/// the text — a `Paragraph` styles only the cells it writes, so a line that
/// stops at its last word leaves the rest of the highlight missing.
///
/// The two markers say different things and both are needed: the background is
/// where you are, and it stays put when `tab` hands the keys to the rail —
/// which moves this cursor, so a row that went dark with focus would leave a
/// category jump landing out of sight. The cyan bar is where the keys are.
fn list_row(
    spans: Vec<Span<'static>>,
    current: bool,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let marker: Span = if current && focused {
        "▌ ".cyan()
    } else {
        "  ".into()
    };
    let mut all = vec![marker];
    all.extend(spans);

    if !current {
        return Line::from(all);
    }
    let used: usize = all.iter().map(|span| span.content.chars().count()).sum();
    all.push(" ".repeat(width.saturating_sub(used)).into());
    Line::from(all).on_dark_gray()
}

fn setting_spans(
    resolved: &Resolved,
    label_width: usize,
    value_width: usize,
) -> Vec<Span<'static>> {
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

    vec![
        Span::from(pad(&label, label_width)),
        "  ".into(),
        value_span,
        " ".repeat(value_width.saturating_sub(value.chars().count()))
            .into(),
        "  ".into(),
        resolved.origin.label().dim(),
    ]
}

/// A behavior's row: its label, the state it is in, and what it stands for.
///
/// The third column holds the member count rather than an origin. A behavior
/// has no origin — it is derived — and putting a scope name there would be the
/// single-scope claim this row exists to stop making.
fn behavior_spans(
    behavior: &ResolvedBehavior,
    config: &ResolvedSet,
    label_width: usize,
    value_width: usize,
) -> Vec<Span<'static>> {
    let label = truncate(behavior.spec.label, label_width);
    let state = truncate(behavior.state_label(), value_width);

    // Custom is the one state worth an accent: it means the members disagree,
    // which is the only thing about a behavior a user might need to act on.
    let state_span = if behavior.preset().is_some() {
        if behavior.is_set(&config.settings) {
            state.clone().bold()
        } else {
            Span::from(state.clone())
        }
    } else {
        state.clone().yellow()
    };

    vec![
        Span::from(pad(&label, label_width)),
        "  ".into(),
        state_span,
        " ".repeat(value_width.saturating_sub(state.chars().count()))
            .into(),
        "  ".into(),
        format!("{} settings", behavior.members.len()).dim(),
    ]
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

    if let Some(stray) = state.selected_stray() {
        frame.render_widget(Paragraph::new(stray_detail(stray, area.width)), inner);
        return;
    }

    if let Some(behavior) = state.selected_behavior() {
        frame.render_widget(
            Paragraph::new(behavior_detail(behavior, &state.config, area.width)),
            inner,
        );
        return;
    }

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

    // The ladder: every layer's answer, and which one daft reads.
    let reads_from = resolved.reads_from();
    for (index, rung) in resolved.rungs.iter().enumerate() {
        let winner = reads_from == Some(index);
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

/// What to say about a `daft.*` key that is not a setting.
///
/// The value is real, it is in a real config file, and it does nothing. The
/// panel's job is to say why — and the usual why is a mis-cased subsection,
/// which git treats as a different key and which reads as correct to anyone
/// scanning for it.
fn stray_detail(entry: &crate::git::ConfigEntry, width: u16) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from("─".repeat(width as usize).dim()),
        Line::from(vec![
            entry.key.clone().bold(),
            "  ".into(),
            "not a daft setting".yellow(),
        ]),
        Line::from(
            "Set in your config, read by nothing. daft ignores keys it does not know.".to_string(),
        ),
        Line::from(""),
        Line::from(vec!["   value:  ".dim(), entry.value.clone().into()]),
        Line::from(vec!["   scope:  ".dim(), entry.scope.label().into()]),
    ];

    if let Some(path) = &entry.origin_path {
        lines.push(Line::from(vec![
            "   file:   ".dim(),
            path.display().to_string().dim(),
        ]));
    }

    lines.push(Line::from(""));

    // Suggest the real key when one is close. Git compares subsections
    // case-sensitively, so `daft.checkoutbranch.carry` is a different key from
    // `daft.checkoutBranch.carry` — and the only visible difference is one
    // letter's case.
    let keys: Vec<String> = crate::core::settings_spec::all_specs()
        .into_iter()
        .map(|spec| spec.key.to_string())
        .collect();
    match crate::suggest::find_similar(&entry.key, &keys, 1).first() {
        Some(near) => lines.push(Line::from(vec![
            "   ! ".yellow(),
            format!("did you mean {near}?").yellow(),
        ])),
        None => lines.push(Line::from(vec![
            "   ! ".yellow(),
            "remove it, or check the spelling against `daft config list`".yellow(),
        ])),
    }

    lines
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

/// A behavior's detail: what it sets, where each member's value comes from,
/// and which state that adds up to.
///
/// This *is* its provenance. A behavior has no rung of its own, so the only
/// honest ladder is its members' — one line each, with the scope that decided
/// it. Collapsing that to a single origin is what the old `--status` did, and
/// it is how a global value could hide behind a local-looking answer.
fn behavior_detail(
    behavior: &ResolvedBehavior,
    config: &ResolvedSet,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = vec![
        Line::from("─".repeat(width as usize).dim()),
        Line::from(vec![
            behavior.spec.label.to_string().bold(),
            "  ".into(),
            behavior.spec.name.to_string().dim(),
        ]),
        Line::from(behavior.spec.help.to_string()),
        Line::from(""),
        Line::from("What it sets".dim().underlined()),
    ];

    for index in &behavior.members {
        let member = &config.settings[*index];
        lines.push(Line::from(vec![
            "  ".into(),
            Span::from(pad(&member.spec.key, 34)),
            "  ".into(),
            Span::from(pad(member.effective_display(), 10)).bold(),
            "  ".into(),
            member.origin.label().dim(),
        ]));
    }

    lines.push(Line::from(""));
    let state_span = if behavior.preset().is_some() {
        behavior.state_label().bold()
    } else {
        behavior.state_label().yellow()
    };
    lines.push(Line::from(vec!["  → ".into(), state_span]));

    if let Some(note) = behavior.divergence_note(&config.settings) {
        lines.push(Line::from(vec!["    ".into(), note.dim()]));
    }

    lines
}

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

    frame.render_widget(
        Paragraph::new(footer_hints(state, show_rail, area.width as usize)),
        area,
    );
}

/// The key hints, in most-useful-first order, cut to what the width holds.
///
/// A budget rather than a fixed list, because a fixed list silently loses its
/// tail: the wide set plus a filter's own `esc` hint already ran eleven columns
/// past the width where the rail appears, and the first thing a clipped line
/// drops is the way out. So `q` is reserved along with the note explaining a
/// missing pane, and the rest fill what is left — a narrow terminal loses the
/// shortcut that saves a keystroke over `enter`, never `enter` itself.
fn footer_hints(state: &ScreenState, show_rail: bool, width: usize) -> Line<'static> {
    let prompt = state.is_prompt_open();

    let mut wanted: Vec<(&str, String)> = Vec::new();
    if prompt {
        wanted.push(("esc", "clear filter".to_string()));
        wanted.push(("enter", "keep it".to_string()));
    } else {
        if state.is_filtering() {
            wanted.push(("esc", "clear filter".to_string()));
        }
        wanted.push(("j/k", "move".to_string()));
        wanted.push(("enter", "edit".to_string()));
        wanted.push(("/", "filter".to_string()));
        wanted.push(("[/]", "section".to_string()));
        wanted.push(("space", "toggle".to_string()));
        wanted.push(("u", "unset".to_string()));
        wanted.push(("s", other_scope(state).to_string()));
        // No rail means no second pane, so the key that crosses between them
        // is not offered — advertising it would promise a place to go.
        if show_rail {
            wanted.push(("tab", "panes".to_string()));
        }
    }

    // `q` types a letter into the filter rather than quitting, so it is not
    // offered while the prompt is open.
    let note = (!show_rail).then_some("rail hidden — widen");
    let reserved = if prompt { 0 } else { hint_width("q", "quit") }
        + note.map_or(0, |note| note.chars().count());

    let mut budget = width.saturating_sub(reserved);
    let mut spans: Vec<Span> = Vec::new();
    for (key, what) in &wanted {
        let cost = hint_width(key, what);
        if cost > budget {
            break;
        }
        budget -= cost;
        spans.extend(hint_spans(key, what));
    }
    if !prompt {
        spans.extend(hint_spans("q", "quit"));
    }
    if let Some(note) = note {
        spans.push(note.dim());
    }

    Line::from(spans)
}

fn hint_width(key: &str, what: &str) -> usize {
    key.chars().count() + what.chars().count() + 4
}

fn hint_spans(key: &str, what: &str) -> [Span<'static>; 2] {
    [format!(" {key} ").bold(), format!("{what}  ").dim()]
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
        let mut state = state_as_opened(entries);
        // The screen opens on a behavior row; these tests are about how a
        // setting draws, so step past it.
        for _ in 0..40 {
            if state.selected().is_some() {
                return state;
            }
            state.move_down();
        }
        panic!("no setting row found");
    }

    /// The screen exactly as it opens, cursor on the first behavior.
    fn state_as_opened(entries: Vec<ConfigEntry>) -> ScreenState {
        let config = resolve_all(&Snapshot {
            entries,
            in_repo: true,
            ..Default::default()
        });
        ScreenState::new(config, true, Some("daft".to_string()))
    }

    /// Render once and return the raw frame — for the assertions that are
    /// about colour rather than text.
    fn frame_of(state: &ScreenState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, state)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The rows whose last cell carries the highlight — a row counts as
    /// highlighted only if it is highlighted all the way to the edge.
    fn highlighted_rows(buffer: &ratatui::buffer::Buffer, width: u16, height: u16) -> Vec<u16> {
        (0..height)
            .filter(|y| buffer[(width - 1, *y)].style().bg == Some(ratatui::style::Color::DarkGray))
            .collect()
    }

    fn text_of(buffer: &ratatui::buffer::Buffer, y: u16, width: u16) -> String {
        (0..width)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    }

    /// The whole frame as one string, for "does this word appear" checks.
    fn render(state: &ScreenState, width: u16, height: u16) -> String {
        painted(state, width, height).join("\n")
    }

    /// Render once and return the frame as plain text, line by line.
    fn painted(state: &ScreenState, width: u16, height: u16) -> Vec<String> {
        let buffer = frame_of(state, width, height);
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

    // ── Behaviors ────────────────────────────────────────────────────────

    /// A behavior's detail panel *is* its provenance: it has no rung of its
    /// own, so the only honest ladder is its members' — each with the scope
    /// that decided it.
    #[test]
    fn a_behavior_shows_every_member_and_where_its_value_came_from() {
        let state = state_as_opened(vec![]);
        let text = render(&state, 120, 40);

        assert!(text.contains("Behaviors"), "the heading: {text}");
        assert!(text.contains("Remote sync"), "the row label");
        assert!(text.contains("Local only"), "the state it is in");
        assert!(text.contains("What it sets"), "the member ladder");
        for member in state.config.behaviors[0].spec.members {
            assert!(text.contains(member), "member {member} is listed");
        }
    }

    /// Custom is the one state worth an accent, and it has to name what is out
    /// of step — a bare "Custom" restates the problem politely.
    #[test]
    fn a_behavior_in_disagreement_names_what_diverges() {
        let state = state_as_opened(vec![
            entry(keys::CHECKOUT_FETCH, "true", ConfigScope::Local),
            entry(keys::BRANCH_DELETE_REMOTE, "true", ConfigScope::Local),
        ]);
        let text = render(&state, 120, 40);

        assert!(text.contains("Custom"), "{text}");
        assert!(
            text.contains("closest to Full sync"),
            "and which state it is closest to: {text}"
        );
        assert!(
            text.contains(keys::CHECKOUT_PUSH),
            "and the member that differs: {text}"
        );
    }

    #[test]
    fn the_editor_on_a_behavior_offers_its_states_not_true_and_false() {
        let mut state = state_as_opened(vec![]);
        state.open_modal();
        let text = render(&state, 120, 40);

        assert!(text.contains("Full sync"), "{text}");
        assert!(text.contains("Local only"), "{text}");
        assert!(
            !text.contains("(•) true") && !text.contains("( ) true"),
            "a behavior takes states, not booleans: {text}"
        );
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
    fn the_cursor_row_is_highlighted_all_the_way_across() {
        let (width, height) = (120u16, 40u16);
        let state = state_with(vec![]);
        let buffer = frame_of(&state, width, height);

        let rows = highlighted_rows(&buffer, width, height);
        assert_eq!(rows.len(), 1, "exactly one row is the cursor's: {rows:?}");

        let line = text_of(&buffer, rows[0], width);
        let label = truncate(&state.selected().unwrap().spec.label, 34);
        assert!(
            line.contains(label.trim()),
            "the highlight sits on some other row: {line:?}"
        );
        assert!(line.contains('▌'), "the focused pane also marks its row");
        assert_eq!(
            buffer[(0, rows[0])].style().bg,
            Some(ratatui::style::Color::Reset),
            "the rail is a different pane — the highlight stops at its edge"
        );
    }

    #[test]
    fn the_cursor_row_keeps_its_highlight_when_the_rail_takes_the_keys() {
        // Walking the rail moves the list cursor, so a highlight that went out
        // with focus would leave every category jump landing out of sight. The
        // cyan bar is what goes; the row stays lit.
        let (width, height) = (120u16, 40u16);
        let mut state = state_with(vec![]);
        state.focus_rail();
        let buffer = frame_of(&state, width, height);

        let rows = highlighted_rows(&buffer, width, height);
        assert_eq!(rows.len(), 1, "the cursor is still somewhere: {rows:?}");
        assert!(
            !text_of(&buffer, rows[0], width).contains('▌'),
            "the bar means the keys are here, and they are not"
        );
    }

    #[test]
    fn the_sections_are_separated_by_a_blank_line() {
        // Narrow enough to drop the rail, so every line is a list row and a
        // blank one means the gap rather than an empty rail cell.
        let lines = painted(&state_with(vec![]), 80, 44);
        let heading = lines
            .iter()
            .position(|line| line.contains("Push & Sync"))
            .expect("the second category paints");
        assert!(
            lines[heading - 1].trim().is_empty(),
            "the sections run together: {:?}",
            &lines[heading - 2..=heading]
        );
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
    fn the_ladder_marks_a_row_that_nothing_sets() {
        // Most rows are this row: nothing sets them and the default applies.
        // A ladder that marks nothing there does not answer the question the
        // panel exists to answer.
        let all = painted(&state_with(vec![]), 120, 40).join("\n");
        let marked: Vec<&str> = all.lines().filter(|line| line.contains('●')).collect();
        assert_eq!(marked.len(), 1, "{all}");
        assert!(
            marked[0].contains("default"),
            "the mark belongs on the layer the effective line names: {:?}",
            marked[0]
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
    fn the_footer_fits_every_width_it_is_drawn_at() {
        // It has overflowed twice, and a clipped hint line loses its tail —
        // which is where the way out lives.
        let mut state = state_with(vec![]);
        for width in [40usize, 60, 80, 99, 100, 101, 110, 120, 200] {
            for show_rail in [true, false] {
                let line = footer_hints(&state, show_rail, width);
                assert!(
                    line.width() <= width,
                    "{}-column footer in {width} columns (rail: {show_rail}): {line:?}",
                    line.width()
                );
            }
        }

        // And in the two states that add hints of their own.
        state.start_filter();
        for width in [80usize, 100, 120] {
            assert!(footer_hints(&state, true, width).width() <= width);
        }
        state.commit_filter();
        for width in [80usize, 100, 120] {
            assert!(footer_hints(&state, true, width).width() <= width);
        }
    }

    #[test]
    fn the_footer_keeps_the_way_out_when_it_has_to_drop_hints() {
        let state = state_with(vec![]);
        let cramped = footer_hints(&state, false, 44);
        let text: String = cramped.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("quit"), "{text:?}");
        assert!(
            text.contains("rail hidden"),
            "a missing pane still explains itself: {text:?}"
        );
        assert!(
            !text.contains("toggle"),
            "the keystroke-savers are what goes: {text:?}"
        );
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
    fn a_stray_key_is_listed_and_explained() {
        use crate::commands::config::screen::state::Mode;

        let mut state = state_with(vec![entry(
            "daft.checkoutbranch.carry",
            "false",
            ConfigScope::Local,
        )]);
        state.mode = Mode::Issues;
        state.rebuild();
        state.move_to_bottom();

        let all = painted(&state, 120, 40).join("\n");
        assert!(all.contains("not settings daft knows"), "{all}");
        assert!(all.contains("daft.checkoutbranch.carry"));
        assert!(all.contains("ignored"), "the row says it does nothing");
        // And the panel names the key it was probably meant to be — the whole
        // difference is one letter's case, which is unreadable otherwise.
        assert!(
            all.contains("did you mean daft.checkoutBranch.carry?"),
            "{all}"
        );
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
    fn the_viewport_matches_the_pane_the_layout_actually_gave_the_list() {
        // Deriving the height by subtracting a fixed chrome total disagreed
        // with the solver exactly where it mattered: `Min(3)` outranks the
        // detail pane's fixed height, so on a short terminal the detail pane
        // gives way and the list keeps rows the arithmetic said were gone.
        // At sixteen rows it returned zero, `follow_cursor` early-returned on
        // it, and the list froze while the cursor walked off the bottom.
        for height in [10, 16, 17, 19, 24, 40] {
            let area = Rect::new(0, 0, 120, height);
            let measured = list_height(area, false);
            let drawn = panes(area, false)[2].height as usize;
            assert_eq!(
                measured, drawn,
                "at {height} rows the scroll and the paint disagree"
            );
            assert!(
                measured > 0,
                "a list with no rows can never follow its cursor ({height} rows)"
            );
        }
    }

    #[test]
    fn a_short_terminal_still_scrolls_to_follow_the_cursor() {
        let mut state = state_with(vec![]);
        let height = list_height(Rect::new(0, 0, 120, 16), false);
        state.move_to_bottom();
        state.follow_cursor(height);
        assert!(
            state.scroll > 0,
            "the cursor is past the last visible row, so the viewport has to move"
        );
        assert!(
            state.cursor() >= state.scroll && state.cursor() < state.scroll + height,
            "cursor {} outside viewport [{}, {})",
            state.cursor(),
            state.scroll,
            state.scroll + height
        );
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
