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

use super::modal::{Field, Modal, Option_, Subject};
use super::state::{Focus, Mode, RailEntry, Row, ScreenState, StatusKind};
use crate::commands::config::resolve::{
    Diagnostic, Layer, Resolved, ResolvedBehavior, ResolvedSet, values_agree,
};
use crate::commands::config::write::WriteScope;
use crate::core::settings_spec::BehaviorSpec;

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
        draw_modal(frame, area, modal, &state.config);
    }
}

/// Word-wrap `text` into lines of at most `width` cells, indenting every line
/// after the first by `indent`.
///
/// Prose on this screen is registry copy — a sentence that names three keys, or
/// a preset's whole rationale — and truncating it cuts the end, which is where
/// the qualification lives. "Fetch before checkout, push new branches, and
/// delete the remote branch when removing one" reads as a licence to fetch when
/// it stops after "push new b".
fn wrapped(text: &str, width: usize, indent: usize) -> Vec<String> {
    // Wrap every line to the narrower width rather than letting the first use
    // the full one: it costs a few cells on a hanging-indent paragraph and it
    // cannot overflow, which the other way round can.
    let room = width.saturating_sub(indent).max(8);
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.chars().count() + 1 + word.chars().count() <= room {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        }
        // A word too long for the room is split rather than pushed past the
        // edge — a path or a key can be longer than a narrow box.
        while current.chars().count() > room {
            lines.push(current.chars().take(room).collect());
            current = current.chars().skip(room).collect();
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }

    let pad = " ".repeat(indent);
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line
            } else {
                format!("{pad}{line}")
            }
        })
        .collect()
}

/// `wrapped`, as dim lines ready to push.
fn prose(text: &str, width: usize, indent: usize) -> Vec<Line<'static>> {
    wrapped(text, width, indent)
        .into_iter()
        .map(|line| Line::from(line.dim()))
        .collect()
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

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal, config: &ResolvedSet) {
    // Wide enough for a member table and its warning column, and never wider
    // than the frame. The content is wrapped to fit, so the width is a
    // preference rather than an assumption.
    let box_width = 92.min(area.width.saturating_sub(4)).max(24);
    let content = usize::from(box_width.saturating_sub(4));

    // Four rows go to chrome: the border, and the one-row margin `centered`
    // keeps above and below. Budgeting only for the border is how the last line
    // gets clipped anyway — by exactly the amount the margin takes.
    let rows = usize::from(area.height.saturating_sub(4));
    let lines = modal_lines(modal, config, content, rows);

    // Size the box from what goes in it. Guessing the row count is how the
    // hint line ends up clipped off the bottom by exactly one.
    let height = lines.len() as u16 + 2;
    let box_area = centered(area, box_width, height);

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

/// The overlay's contents, at the most detail that fits in `rows`.
///
/// `Paragraph` is top-aligned and the box is clamped to the frame, so anything
/// that does not fit falls off the *bottom* — which is where the key hints live
/// and, when a write has been refused, the reason for the refusal. An invisible
/// refusal is worse than a missing explanation, so a short terminal sheds whole
/// blocks in [`Detail`]'s order instead, and if even the last of those overflows
/// it gives up on the middle rather than on the way out.
fn modal_lines(
    modal: &Modal,
    config: &ResolvedSet,
    width: usize,
    rows: usize,
) -> Vec<Line<'static>> {
    let mut tightest = Vec::new();
    for dropped in 0..=Detail::MOST {
        let (body, tail) = modal_body(modal, config, width, Detail::at(dropped));
        if body.len() + tail.len() <= rows {
            return [body, tail].concat();
        }
        tightest = vec![body, tail];
    }

    // Nothing fits. Keep the top of the box and all of the tail; the middle is
    // what gives way.
    let tail = tightest.pop().unwrap_or_default();
    let mut body = tightest.pop().unwrap_or_default();
    body.truncate(rows.saturating_sub(tail.len()));
    let mut lines = [body, tail].concat();
    lines.truncate(rows);
    lines
}

/// How much of the overlay's explanation survives.
///
/// Ordered by what is lost. **Decoration goes before prose**: the subject's help
/// is duplicated in the panel behind the box and in `daft config list`, and the
/// `preset` label and the rule above `unset` carry no information at all — so
/// those three go before the sentence explaining the state under the cursor,
/// which exists nowhere else and changes as the cursor moves. A classic 80×24
/// terminal lands on that boundary exactly: shedding the label and the rule is
/// what keeps the explanation on screen there.
///
/// Everything a keystroke needs — the list, the scope, the refusal, the way
/// out — is not in this struct at all.
#[derive(Debug, Clone, Copy)]
struct Detail {
    subject_help: bool,
    heading: bool,
    option_help: bool,
    table: bool,
    now: bool,
}

impl Detail {
    /// How many blocks there are to shed.
    const MOST: u8 = 5;

    fn at(dropped: u8) -> Self {
        Self {
            subject_help: dropped < 1,
            heading: dropped < 2,
            option_help: dropped < 3,
            table: dropped < 4,
            now: dropped < 5,
        }
    }
}

/// The overlay, split into what may be trimmed and what may not.
fn modal_body(
    modal: &Modal,
    config: &ResolvedSet,
    width: usize,
    detail: Detail,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let mut lines: Vec<Line> = vec![Line::from(vec![
        modal.subject.label().bold(),
        "  ".into(),
        modal.subject.name().dim(),
    ])];
    if detail.subject_help {
        lines.extend(prose(&modal.subject.help(), width, 0));
    }
    lines.push(Line::from(""));

    match &modal.subject {
        Subject::Behavior(spec) => {
            behavior_body(&mut lines, modal, spec, config, width, detail);
        }
        Subject::Setting(_) => setting_body(&mut lines, modal, width),
    }

    let mut tail = vec![Line::from("")];
    match &modal.error {
        Some(error) => tail.extend(
            wrapped(&format!("✗ {error}"), width, 2)
                .into_iter()
                .map(|line| Line::from(line.red())),
        ),
        None => tail.push(modal_hints(modal)),
    }

    (lines, tail)
}

/// A setting's editor: pick a value or type one, at the pill's scope.
fn setting_body(lines: &mut Vec<Line<'static>>, modal: &Modal, width: usize) {
    lines.push(scope_row(modal));
    if modal.scope_is_inert {
        lines.push(Line::from(
            "   daft reads this key from global config only".yellow(),
        ));
    }
    lines.push(Line::from(""));

    match &modal.field {
        Field::Options { cursor } => {
            for (index, option) in modal.options.iter().enumerate() {
                lines.extend(option_rows(option, index == *cursor, width));
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
                lines.extend(option_rows(unset, *on_unset, width));
            }
        }
    }
}

/// A behavior's editor: which named state, and what that state does.
///
/// Four blocks, in the order the questions get asked. What is it doing now, and
/// why is that `Custom` if it is. Where would a write go. Which state do I
/// want. And — the one a value editor never has to answer — what does choosing
/// that state actually set.
fn behavior_body(
    lines: &mut Vec<Line<'static>>,
    modal: &Modal,
    spec: &'static BehaviorSpec,
    config: &ResolvedSet,
    width: usize,
    detail: Detail,
) {
    let behavior = config.behavior(spec.name).filter(|_| detail.now);

    if let Some(behavior) = behavior {
        let mut now: Vec<Span> = vec!["now    ".dim()];
        now.push(if behavior.preset().is_some() {
            behavior.state_label().bold()
        } else {
            behavior.state_label().yellow()
        });
        // A named state nothing sets is not the same as one someone chose, and
        // the difference decides whether unset would change anything.
        if behavior.preset().is_some() && !behavior.is_set(&config.settings) {
            now.push("   nothing set — daft's default".dim());
        }
        lines.push(Line::from(now));

        // Why it is Custom, in the one phrasing the CLI and the panel also use.
        if let Some(note) = behavior.divergence_note(&config.settings) {
            lines.extend(divergence_lines(&note, width, 7));
        }
        lines.push(Line::from(""));
    }

    lines.push(scope_row(modal));
    lines.push(Line::from(""));

    // The list is a preset selector: the state's name leads, the word you would
    // type for it follows in the dim column. Unset is separated by a blank
    // line — it is not a third state, it is the way to stop having one.
    if detail.heading {
        lines.push(Line::from("preset".dim()));
    }
    for (index, option) in modal.options.iter().enumerate() {
        let selected = matches!(modal.field, Field::Options { cursor } if cursor == index);
        if matches!(option, Option_::Unset { .. }) && detail.heading {
            lines.push(Line::from(""));
        }
        lines.extend(preset_rows(option, selected, width));
    }

    // What the highlighted row means, in full.
    if detail.option_help {
        lines.push(Line::from(""));
        let help = match modal.selected_option() {
            Some(Option_::Value { value, .. }) => spec
                .preset(value)
                .map(|preset| format!("{} — {}", preset.label, preset.help)),
            Some(Option_::Unset { .. }) => Some(format!(
                "Removes {} from {} config. What the behavior reads then is \
                 whatever the remaining scopes and the defaults say.",
                spec.members.join(", "),
                modal.scope_label(modal.scope),
            )),
            None => None,
        };
        if let Some(help) = help {
            lines.extend(prose(&help, width, 0));
        }
    }

    // And what it sets, key by key.
    if detail.table {
        lines.push(Line::from(""));
        lines.extend(member_table(modal, spec, config, width));
    }
}

/// One row per member: what it reads now, where that came from, and what the
/// highlighted preset would write.
///
/// This is the part a value editor has no equivalent of, and the reason a
/// behavior needs its own box. A preset is only trustworthy if you can see what
/// it stands for — and the arrow is only honest if a write that will not change
/// what daft reads says so.
fn member_table(
    modal: &Modal,
    spec: &'static BehaviorSpec,
    config: &ResolvedSet,
    width: usize,
) -> Vec<Line<'static>> {
    let Some(behavior) = config.behavior(spec.name) else {
        return Vec::new();
    };

    let clearing = matches!(modal.selected_option(), Some(Option_::Unset { .. }));
    let preset = match modal.selected_option() {
        Some(Option_::Value { value, .. }) => spec.preset(value),
        _ => None,
    };
    if !clearing && preset.is_none() {
        return Vec::new();
    }

    let scope = modal.scope_label(modal.scope);
    let mut lines = vec![Line::from(
        if clearing {
            format!("clears from {scope}")
        } else {
            format!("writes to {scope}")
        }
        .dim(),
    )];

    let key_width = behavior
        .members
        .iter()
        .map(|index| config.settings[*index].spec.key.chars().count())
        .max()
        .unwrap_or(24)
        .min(width.saturating_sub(28));

    for index in &behavior.members {
        let member = &config.settings[*index];
        let now = member.effective_display().to_string();

        // What lands, and whether it is a change at all. Compared through the
        // member's own type, so a hand-typed `yes` counts as `true`.
        let target = preset.and_then(|preset| preset.value_for(&member.spec.key));
        let changes = if clearing {
            member.value_written_at(&member.spec, modal.scope).is_some()
        } else {
            !values_agree(&member.spec.ty, member.effective.as_deref(), target)
        };

        let target_span: Span = match (clearing, target) {
            (true, _) if changes => "unset".bold(),
            (true, _) => "—".dim(),
            (false, Some(value)) if changes => value.to_string().bold(),
            (false, Some(value)) => value.to_string().dim(),
            (false, None) => "—".dim(),
        };

        let mut spans = vec![
            "  ".into(),
            Span::from(pad(&truncate(&member.spec.key, key_width), key_width)),
            "  ".into(),
            Span::from(pad(&now, 6)),
            "  ".into(),
            pad(&member.origin.label(), 8).dim(),
            "→  ".dim(),
            target_span,
        ];
        // The qualification, only where it would otherwise be a false promise.
        if changes && let Some(layer) = member.masked_above(&member.spec, modal.scope) {
            spans.push(format!("  outranked by {}", layer.label()).yellow());
        }
        lines.push(Line::from(spans));
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

/// The column a radio row's gloss starts in, and so the hanging indent a
/// wrapped gloss lines up under.
const GLOSS_COLUMN: usize = 5 + 18 + 2;

/// A value row: the value, then what the registry says it means.
fn option_rows(option: &Option_, selected: bool, width: usize) -> Vec<Line<'static>> {
    let (value, gloss) = match option {
        Option_::Value { value, gloss } => (value.clone(), gloss.clone()),
        // Unset always names what it would reveal — otherwise it reads as
        // "delete" rather than "fall back to".
        Option_::Unset { reveals } => ("unset".to_string(), format!("inherit: {reveals}")),
    };
    radio(&value, &gloss, selected, width)
}

/// A preset row: the state's name, then the word you would type for it.
///
/// The other way round from a value row, and deliberately: the thing being
/// chosen is the state, and `off` / `on` in the primary column is what made a
/// preset selector read as a boolean's editor.
fn preset_rows(option: &Option_, selected: bool, width: usize) -> Vec<Line<'static>> {
    match option {
        Option_::Value { value, gloss } => radio(gloss, value, selected, width),
        Option_::Unset { reveals } => radio("unset", reveals, selected, width),
    }
}

fn radio(label: &str, gloss: &str, selected: bool, width: usize) -> Vec<Line<'static>> {
    let marker: Span = if selected {
        " (•) ".cyan()
    } else {
        " ( ) ".into()
    };
    let label: Span = if selected {
        pad(label, 18).bold()
    } else {
        Span::from(pad(label, 18))
    };

    let mut wrapped_gloss = wrapped(gloss, width.saturating_sub(GLOSS_COLUMN), 0).into_iter();
    let first = wrapped_gloss.next().unwrap_or_default();

    let mut lines = vec![Line::from(vec![marker, label, "  ".into(), first.dim()])];
    // A gloss longer than its column continues under itself rather than off the
    // edge of the box.
    lines.extend(
        wrapped_gloss.map(|line| Line::from(vec![" ".repeat(GLOSS_COLUMN).into(), line.dim()])),
    );
    lines
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

    // Drilled into a behavior's members, the list is three rows out of eighty
    // with nothing to say which three. The header is where "where am I" belongs,
    // and without it the only clue is the footer's way out.
    if let Some(behavior) = state
        .member_focus()
        .and_then(|name| state.config.behavior(name))
    {
        spans.push("   ".into());
        spans.push("inside ".dim());
        spans.push(behavior.spec.label.bold());
    }

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
    ];
    lines.extend(
        wrapped(&resolved.spec.help, usize::from(area.width), 0)
            .into_iter()
            .map(Line::from),
    );
    lines.push(Line::from(""));

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
        lines.extend(diagnostic_lines(diagnostic, usize::from(area.width)));
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
    ];
    lines.extend(
        wrapped(
            "Set in your config, read by nothing. daft ignores keys it does not know.",
            usize::from(width),
            0,
        )
        .into_iter()
        .map(Line::from),
    );
    lines.extend([
        Line::from(""),
        Line::from(vec!["   value:  ".dim(), entry.value.clone().into()]),
        Line::from(vec!["   scope:  ".dim(), entry.scope.label().into()]),
    ]);

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
    let advice = match crate::suggest::find_similar(&entry.key, &keys, 1).first() {
        Some(near) => format!("did you mean {near}?"),
        None => "remove it, or check the spelling against `daft config list`".to_string(),
    };
    let mut wrapped_advice = wrapped(&advice, usize::from(width).saturating_sub(5), 0).into_iter();
    lines.push(Line::from(vec![
        "   ! ".yellow(),
        wrapped_advice.next().unwrap_or_default().yellow(),
    ]));
    lines.extend(wrapped_advice.map(|line| Line::from(vec!["     ".into(), line.yellow()])));

    lines
}

/// A diagnostic, wrapped under its marker.
///
/// One of these carries a stored value verbatim (`the local value "…" is not
/// valid`), so its length is up to whoever typed it — the one line on this panel
/// that a user can make arbitrarily long.
fn diagnostic_lines(diagnostic: &Diagnostic, width: usize) -> Vec<Line<'static>> {
    const MARKER: usize = 5;

    let (marker, text) = match diagnostic {
        Diagnostic::Invalid { layer, value, .. } => (
            "   ✗ ".red(),
            format!("the {} value {value:?} is not valid", layer.label()),
        ),
        Diagnostic::Deprecated { alias, replacement } => (
            "   ! ".yellow(),
            format!("{alias} is retired — move the value to {replacement}"),
        ),
        Diagnostic::Inert { scope, .. } => (
            "   ! ".yellow(),
            format!("the {} value is set but never read", scope.label()),
        ),
        Diagnostic::EnvShadow { layer, .. } => (
            "   ! ".yellow(),
            match layer {
                Layer::Env(var) => format!("${var} outranks every config file"),
                _ => "a process-scoped value outranks every config file".to_string(),
            },
        ),
    };
    let colour = marker.style;

    let mut spans = wrapped(&text, width.saturating_sub(MARKER), 0).into_iter();
    let first = spans.next().unwrap_or_default();
    let mut lines = vec![Line::from(vec![marker, Span::styled(first, colour)])];
    lines.extend(
        spans.map(|line| Line::from(vec![" ".repeat(MARKER).into(), Span::styled(line, colour)])),
    );
    lines
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
    ];
    // Wrapped, not clipped: this sentence exists to name what the behavior
    // changes underneath, and the members are at the end of it.
    lines.extend(
        wrapped(behavior.spec.help, usize::from(width), 0)
            .into_iter()
            .map(Line::from),
    );
    lines.push(Line::from(""));
    lines.push(Line::from("What it sets".dim().underlined()));

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

    // The longest dynamic line on the screen — it names every member that is
    // out of step — and the answer to "why does this say Custom", so it wraps
    // rather than losing the members at the end of it.
    if let Some(note) = behavior.divergence_note(&config.settings) {
        lines.extend(divergence_lines(&note, usize::from(width), 4));
    }

    lines
}

/// The Custom explanation, wrapped, at a given indent.
///
/// Dim rather than yellow, in both of the places it is drawn: the *state* is
/// what carries the warning colour, and repeating it on the sentence underneath
/// would be two accents for one fact.
fn divergence_lines(note: &str, width: usize, indent: usize) -> Vec<Line<'static>> {
    let pad = " ".repeat(indent);
    wrapped(note, width.saturating_sub(indent), 0)
        .into_iter()
        .map(|line| Line::from(vec![pad.clone().into(), line.dim()]))
        .collect()
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
        // A behavior stands for several settings, and the way to reach them is
        // worth advertising while the cursor is on one — otherwise the only
        // route to a member is scrolling to its own category and knowing which
        // keys to look for.
        if state.member_focus().is_some() {
            wanted.push(("h", "leave".to_string()));
        } else if state.selected_behavior().is_some() {
            wanted.push(("l", "settings".to_string()));
        }
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

    /// The editor for a behavior is a *preset* selector: the state's name is
    /// what you pick, and the values it stands for are shown rather than
    /// offered. Presenting `off` / `on` / `unset` as the choices is presenting a
    /// boolean's editor for something that is not a boolean.
    #[test]
    fn the_editor_on_a_behavior_selects_a_preset_and_shows_what_it_writes() {
        let mut state = state_as_opened(vec![]);
        state.open_modal();
        let text = render(&state, 120, 40);

        // The states, by name, in the column the cursor moves down.
        assert!(text.contains("(•) Local only"), "{text}");
        assert!(text.contains("( ) Full sync"), "{text}");
        assert!(
            !text.contains("(•) true") && !text.contains("( ) true"),
            "a behavior takes states, not booleans: {text}"
        );
        // The word you would type for the highlighted state is still there —
        // the screen is where people learn the CLI's vocabulary.
        assert!(text.contains("off"), "the preset's own name: {text}");

        // And what the state stands for, key by key, with what each one reads
        // now: this is the part a value editor has no equivalent of.
        assert!(text.contains("writes to local"), "{text}");
        for member in state.config.behaviors[0].spec.members {
            assert!(text.contains(member), "member {member} is spelled out");
        }
        assert!(text.contains("→"), "each member's new value: {text}");
    }

    /// Custom is not a state you chose, so the editor has to say what put it
    /// there — and open on the preset that would resolve it.
    #[test]
    fn the_editor_says_why_a_behavior_reads_custom() {
        let mut state = state_as_opened(vec![
            entry(keys::CHECKOUT_FETCH, "true", ConfigScope::Local),
            entry(keys::BRANCH_DELETE_REMOTE, "true", ConfigScope::Local),
        ]);
        state.open_modal();
        let text = render(&state, 120, 40);

        assert!(text.contains("now"), "the state it is in now: {text}");
        assert!(text.contains("Custom"), "{text}");
        assert!(
            text.contains("closest to Full sync"),
            "and what it is closest to: {text}"
        );
        assert!(
            text.contains(&format!("{} is false", keys::CHECKOUT_PUSH)),
            "and the member that put it there, with its value: {text}"
        );
        assert!(
            text.contains("(•) Full sync"),
            "opening on the preset one keystroke away, not on the top of the \
             list, which would revert two deliberate settings: {text}"
        );
    }

    /// The whole reason the box was rebuilt: registry prose ends in the part
    /// that qualifies it, and truncation always takes the end.
    #[test]
    fn the_editor_never_truncates_its_explanations() {
        for width in [80, 120, 200] {
            let mut state = state_as_opened(vec![]);
            state.open_modal();
            let text = render(&state, width, 44);

            assert!(
                text.contains("along with the local one."),
                "the behavior's help is cut off at {width} columns: {text}"
            );
            assert!(
                text.contains("safe to run offline."),
                "the chosen state's help is cut off at {width} columns: {text}"
            );
        }
    }

    /// A classic terminal is where the shedding order earns its keep. The
    /// selected state's help exists nowhere else and changes as the cursor
    /// moves, so it outlives the `preset` label and the rule above `unset`,
    /// which say nothing.
    #[test]
    fn the_explanation_that_only_the_editor_has_outlives_its_decoration() {
        let mut state = state_as_opened(vec![]);
        state.open_modal();
        let text = render(&state, 80, 24);

        assert!(
            text.contains("safe to run offline."),
            "the chosen state's explanation is gone at 80x24: {text}"
        );
        assert!(
            text.contains("Local only") && text.contains("Full sync"),
            "and every state is still on the list: {text}"
        );
        assert!(text.contains("esc"), "and the way out: {text}");
    }

    /// A refusal that scrolled off the bottom is a keystroke that did nothing
    /// for no visible reason. The box sheds explanation before it sheds this.
    #[test]
    fn a_refusal_and_the_way_out_survive_a_short_terminal() {
        for (width, height) in [(80, 24), (80, 20), (100, 16)] {
            let mut state = state_as_opened(vec![]);
            state.open_modal();
            if let Some(modal) = state.modal.as_mut() {
                modal.error = Some("refused for a reason".to_string());
            }
            let text = render(&state, width, height);

            assert!(
                text.contains("refused for a reason"),
                "the refusal is off the bottom at {width}x{height}: {text}"
            );
        }

        // Without an error the hints take that line, and they matter as much:
        // a modal with no visible way out is a trap.
        for (width, height) in [(80, 24), (80, 20), (100, 16)] {
            let mut state = state_as_opened(vec![]);
            state.open_modal();
            let text = render(&state, width, height);
            assert!(
                text.contains("esc"),
                "no way out shown at {width}x{height}: {text}"
            );
        }
    }

    /// The arrow promises what the value will be. When the scope being written
    /// is outranked, that promise needs its qualification next to it.
    #[test]
    fn a_write_something_above_would_outrank_says_so() {
        let mut state =
            state_as_opened(vec![entry(keys::CHECKOUT_PUSH, "true", ConfigScope::Local)]);
        state.open_modal();
        if let Some(modal) = state.modal.as_mut() {
            modal.set_scope(WriteScope::Global);
        }
        let text = render(&state, 120, 40);

        assert!(text.contains("writes to global"), "{text}");
        assert!(
            text.contains("outranked by local"),
            "a global write under a local value changes nothing daft reads, and \
             the row has to say so: {text}"
        );
    }

    /// The members are reachable from the behavior's row, and a route nothing
    /// mentions is a route nobody takes.
    #[test]
    fn the_footer_offers_the_way_into_a_behaviors_settings_and_back_out() {
        // The footer only — "settings" also appears in a behavior row's third
        // column, and the question here is what the key hints advertise.
        let footer = |state: &ScreenState| {
            painted(state, 120, 40)
                .last()
                .cloned()
                .expect("a footer row")
        };

        let mut state = state_as_opened(vec![]);
        let on_behavior = footer(&state);
        assert!(on_behavior.contains(" l "), "{on_behavior}");
        assert!(on_behavior.contains("settings"), "{on_behavior}");

        let behavior = state.selected_behavior().cloned().expect("a behavior row");
        state.focus_members(&behavior);
        let drilled_in = footer(&state);
        assert!(
            drilled_in.contains("leave"),
            "the way back out: {drilled_in}"
        );

        // And the header says which behavior's settings these are — three rows
        // out of eighty, with nothing else to identify them.
        let header = painted(&state, 120, 40)[0].clone();
        assert!(
            header.contains("inside Remote sync"),
            "no orientation while drilled in: {header}"
        );

        let on_setting = footer(&state_with(vec![]));
        assert!(
            !on_setting.contains("settings") && !on_setting.contains("leave"),
            "a setting has no members to drill into: {on_setting}"
        );
    }

    /// Unset is not a third state — it is the way to stop having one, and what
    /// it removes is worth seeing before pressing Enter.
    #[test]
    fn the_unset_row_shows_what_it_would_clear() {
        let mut state = state_as_opened(vec![entry(
            keys::CHECKOUT_FETCH,
            "true",
            ConfigScope::Local,
        )]);
        state.open_modal();
        if let Some(modal) = state.modal.as_mut() {
            for _ in 0..5 {
                modal.move_down();
            }
        }
        let text = render(&state, 120, 40);

        assert!(text.contains("(•) unset"), "{text}");
        assert!(text.contains("clears from local"), "{text}");
        assert!(
            text.contains(keys::CHECKOUT_FETCH),
            "the members it would remove: {text}"
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

    // ── Wrapping ─────────────────────────────────────────────────────────

    #[test]
    fn prose_wraps_inside_its_width_and_keeps_every_word() {
        let text = "Whether worktree commands reach the remote: fetching before \
                    checkout, pushing branches as they are created.";
        for width in [12, 24, 40, 80, 200] {
            let lines = wrapped(text, width, 0);
            for line in &lines {
                assert!(
                    line.chars().count() <= width.max(8),
                    "{line:?} overflows {width}"
                );
            }
            assert_eq!(
                lines.join(" ").split_whitespace().collect::<Vec<_>>(),
                text.split_whitespace().collect::<Vec<_>>(),
                "wrapping at {width} lost or reordered a word"
            );
        }
    }

    #[test]
    fn a_hanging_indent_applies_to_continuations_only() {
        let lines = wrapped("one two three four five six seven", 20, 4);
        assert!(lines.len() > 1, "{lines:?}");
        assert!(!lines[0].starts_with(' '), "the first line is not indented");
        for line in &lines[1..] {
            assert!(line.starts_with("    "), "{line:?} is not indented");
            assert!(line.chars().count() <= 20, "{line:?} overflows");
        }
    }

    #[test]
    fn a_word_longer_than_the_box_is_split_rather_than_clipped() {
        // A key or a path can be longer than a narrow overlay, and dropping the
        // tail of one is how a "did you mean" suggestion becomes unreadable.
        let lines = wrapped("short daft.checkoutBranch.carryUntrackedChanges end", 16, 0);
        for line in &lines {
            assert!(line.chars().count() <= 16, "{line:?} overflows");
        }
        let rejoined: String = lines.concat();
        assert!(
            rejoined.contains("carryUntrackedChanges"),
            "the long word survived in pieces: {lines:?}"
        );
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
