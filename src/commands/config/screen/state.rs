//! What the settings screen is showing, and everything you can do to it.
//!
//! Pure state: no terminal, no ratatui, no git. Every key the screen accepts
//! ends up as a method here, so the whole navigation model is testable without
//! drawing a frame — which matters because the interesting behaviour (skipping
//! group headers, keeping a selection through a filter change, what the rail
//! does versus what it filters) is exactly the part a screenshot cannot check.

use super::modal::Modal;
use crate::commands::config::resolve::{Resolved, ResolvedSet};
use crate::core::settings_spec::Category;
use crate::git::ConfigScope;

/// What the list is showing.
///
/// The rail's top three entries pick one of these. They are modes rather than
/// filters-among-equals: "Modified" and "Issues" answer the two questions
/// people actually arrive with — what did I change, and what is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    All,
    Modified,
    Issues,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Modified => "Modified",
            Self::Issues => "Issues",
        }
    }
}

/// One entry in the left rail.
///
/// The split is deliberate: the modes change *what is in* the list, the
/// categories change *where you are* in it. A category that also filtered
/// would hide the neighbouring settings a user is often comparing against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RailEntry {
    Mode(Mode),
    Category(Category),
}

/// A row in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    Header(Category),
    Setting(usize),
    /// The blank line above a heading. Carries nothing.
    ///
    /// It is a row rather than something the renderer inserts because the
    /// scroll offset indexes this list: a line the list draws but does not
    /// count would slide the viewport out from under the cursor.
    Spacer,
    /// A heading for the `daft.*` keys that are not settings.
    StrayHeader,
    /// A `daft.*` key in the config that matches no registry row — a typo, or
    /// a key from a newer daft. Index into `ResolvedSet::unrecognized`.
    ///
    /// These have no setting to attach a diagnostic to, so without a row of
    /// their own they would be counted in the header and then be unfindable.
    Stray(usize),
}

/// Which pane the keyboard is driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Rail,
    List,
}

/// Whether the cursor may rest on a row. Headings are signposts, not stops.
fn is_landable(row: &Row) -> bool {
    matches!(row, Row::Setting(_) | Row::Stray(_))
}

/// Put a blank line above the heading about to be pushed.
///
/// Every section but the first gets one: the gap is what makes the categories
/// read as groups, and it costs a row where a rule would cost a row and a
/// line of ink.
fn open_section(rows: &mut Vec<Row>) {
    if !rows.is_empty() {
        rows.push(Row::Spacer);
    }
}

/// The whole screen.
pub struct ScreenState {
    /// Everything, resolved. Re-resolved after a write.
    pub config: ResolvedSet,
    /// The scope a write goes to — the header pill, flipped with `s`.
    pub write_scope: ConfigScope,
    /// Whether a local scope exists to write at all.
    pub in_repo: bool,
    /// What the repository is called, for the header.
    pub repo_label: Option<String>,

    pub mode: Mode,
    pub focus: Focus,
    /// The text currently narrowing the list; `None` when nothing is.
    pub filter: Option<String>,
    /// Whether keystrokes are going into the filter prompt.
    ///
    /// Separate from `filter` on purpose: `Enter` closes the prompt but keeps
    /// the text, so the list stays narrowed while the letter keys go back to
    /// being commands. Conflating the two makes `s` type an 's' forever.
    prompt_open: bool,

    /// The rows currently listed, rebuilt whenever mode or filter changes.
    rows: Vec<Row>,
    /// Index into `rows`. Always points at a `Setting` when one exists.
    cursor: usize,
    /// First visible row.
    pub scroll: usize,
    /// The rail entries, in display order.
    rail: Vec<RailEntry>,
    rail_cursor: usize,

    /// Whether the terminal is wide enough to show the rail. Set from the
    /// frame each draw, because a pane you cannot see must not be a pane you
    /// can drive — `tab` into an invisible rail loses the cursor entirely.
    pub rail_visible: bool,

    /// The value editor, when one is open. While it is, it owns the keyboard.
    pub modal: Option<Modal>,

    /// The last thing that happened, narrated.
    pub status: Option<Status>,
}

/// A one-line report of the last action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub text: String,
    pub kind: StatusKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Success,
    Error,
    Info,
}

impl ScreenState {
    pub fn new(config: ResolvedSet, in_repo: bool, repo_label: Option<String>) -> Self {
        // Outside a repository there is nowhere local to write, so the pill
        // starts — and stays — on global rather than offering a scope that
        // does not exist.
        let write_scope = if in_repo {
            ConfigScope::Local
        } else {
            ConfigScope::Global
        };

        let mut state = Self {
            config,
            write_scope,
            in_repo,
            repo_label,
            mode: Mode::All,
            focus: Focus::List,
            filter: None,
            prompt_open: false,
            rows: Vec::new(),
            cursor: 0,
            scroll: 0,
            rail: Vec::new(),
            rail_cursor: 0,
            rail_visible: true,
            modal: None,
            status: None,
        };
        state.rebuild();
        state
    }

    // ── Derived views ────────────────────────────────────────────────────

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn rail(&self) -> &[RailEntry] {
        &self.rail
    }

    pub fn rail_cursor(&self) -> usize {
        self.rail_cursor
    }

    /// The setting under the cursor, if one is.
    pub fn selected(&self) -> Option<&Resolved> {
        match self.rows.get(self.cursor) {
            Some(Row::Setting(index)) => self.config.settings.get(*index),
            _ => None,
        }
    }

    /// The stray key under the cursor, if one is.
    pub fn selected_stray(&self) -> Option<&crate::git::ConfigEntry> {
        match self.rows.get(self.cursor) {
            Some(Row::Stray(index)) => self.config.unrecognized.get(*index),
            _ => None,
        }
    }

    /// How many settings match the current mode and filter.
    pub fn visible_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, Row::Setting(_)))
            .count()
    }

    /// Everything worth flagging, for the header count and the Issues mode.
    pub fn issue_count(&self) -> usize {
        self.config.issue_count()
    }

    // ── Rebuilding ───────────────────────────────────────────────────────

    /// Recompute the visible rows and the rail, keeping the selection on the
    /// same setting where possible.
    ///
    /// Holding the selection through a filter keystroke is what makes `/`
    /// usable: typing narrows the list under a cursor that stays put rather
    /// than snapping to the top on every character.
    pub fn rebuild(&mut self) {
        let held = self.selected().map(|r| r.spec.key.to_string());

        self.rows.clear();
        let mut current: Option<Category> = None;

        for (index, resolved) in self.config.settings.iter().enumerate() {
            if !self.matches_mode(resolved) || !self.matches_filter(resolved) {
                continue;
            }
            if current != Some(resolved.spec.category) {
                open_section(&mut self.rows);
                self.rows.push(Row::Header(resolved.spec.category));
                current = Some(resolved.spec.category);
            }
            self.rows.push(Row::Setting(index));
        }

        // Stray keys belong to the mode whose job is "what is wrong". In the
        // other modes the header count points here.
        if self.mode == Mode::Issues && !self.config.unrecognized.is_empty() {
            let strays: Vec<usize> = (0..self.config.unrecognized.len())
                .filter(|index| self.matches_filter_text(&self.config.unrecognized[*index].key))
                .collect();
            if !strays.is_empty() {
                open_section(&mut self.rows);
                self.rows.push(Row::StrayHeader);
                self.rows.extend(strays.into_iter().map(Row::Stray));
            }
        }

        self.rail = self.build_rail();
        self.rail_cursor = self.rail_cursor.min(self.rail.len().saturating_sub(1));

        match held.and_then(|key| self.row_of(&key)) {
            Some(index) => self.cursor = index,
            None => {
                self.cursor = 0;
                self.settle_forward();
            }
        }
        self.clamp();
    }

    fn matches_mode(&self, resolved: &Resolved) -> bool {
        match self.mode {
            Mode::All => true,
            Mode::Modified => resolved.is_set(),
            Mode::Issues => !resolved.diagnostics.is_empty(),
        }
    }

    /// Whether a single piece of text survives the filter — for rows that
    /// have nothing but a key.
    fn matches_filter_text(&self, text: &str) -> bool {
        match self.filter.as_deref() {
            None | Some("") => true,
            Some(needle) => text.to_lowercase().contains(&needle.to_lowercase()),
        }
    }

    fn matches_filter(&self, resolved: &Resolved) -> bool {
        let Some(needle) = self.filter.as_deref() else {
            return true;
        };
        if needle.is_empty() {
            return true;
        }
        let needle = needle.to_lowercase();
        // Label, key, and help together: people search for the word they
        // remember, and which of the three it lives in is not something they
        // should have to know.
        resolved.spec.label.to_lowercase().contains(&needle)
            || resolved.spec.key.to_lowercase().contains(&needle)
            || resolved.spec.help.to_lowercase().contains(&needle)
    }

    fn build_rail(&self) -> Vec<RailEntry> {
        let mut rail = vec![RailEntry::Mode(Mode::All), RailEntry::Mode(Mode::Modified)];
        // The Issues entry appears only when there are issues — a permanent
        // "Issues 0" trains people to ignore it.
        if self.issue_count() > 0 {
            rail.push(RailEntry::Mode(Mode::Issues));
        }
        for category in Category::all() {
            if self
                .rows
                .iter()
                .any(|row| matches!(row, Row::Header(c) if c == category))
            {
                rail.push(RailEntry::Category(*category));
            }
        }
        rail
    }

    fn row_of(&self, key: &str) -> Option<usize> {
        self.rows.iter().position(|row| match row {
            Row::Setting(index) => self
                .config
                .settings
                .get(*index)
                .is_some_and(|r| r.spec.key == key),
            Row::Header(_) | Row::Spacer | Row::StrayHeader | Row::Stray(_) => false,
        })
    }

    /// How many settings a rail entry stands for.
    pub fn rail_count(&self, entry: RailEntry) -> usize {
        match entry {
            RailEntry::Mode(Mode::All) => self.config.settings.len(),
            RailEntry::Mode(Mode::Modified) => {
                self.config.settings.iter().filter(|r| r.is_set()).count()
            }
            RailEntry::Mode(Mode::Issues) => self.issue_count(),
            RailEntry::Category(category) => self
                .rows
                .iter()
                .filter(|row| match row {
                    Row::Setting(index) => self
                        .config
                        .settings
                        .get(*index)
                        .is_some_and(|r| r.spec.category == category),
                    Row::Header(_) | Row::Spacer | Row::StrayHeader | Row::Stray(_) => false,
                })
                .count(),
        }
    }

    // ── Movement ─────────────────────────────────────────────────────────

    pub fn move_down(&mut self) {
        match self.focus {
            Focus::Rail => {
                if self.rail_cursor + 1 < self.rail.len() {
                    self.rail_cursor += 1;
                    self.activate_rail();
                }
            }
            Focus::List => {
                let mut next = self.cursor;
                while next + 1 < self.rows.len() {
                    next += 1;
                    if is_landable(&self.rows[next]) {
                        self.cursor = next;
                        break;
                    }
                }
                self.clamp();
            }
        }
    }

    pub fn move_up(&mut self) {
        match self.focus {
            Focus::Rail => {
                if self.rail_cursor > 0 {
                    self.rail_cursor -= 1;
                    self.activate_rail();
                }
            }
            Focus::List => {
                let mut next = self.cursor;
                while next > 0 {
                    next -= 1;
                    if is_landable(&self.rows[next]) {
                        self.cursor = next;
                        break;
                    }
                }
                self.clamp();
            }
        }
    }

    /// The first row you can land on in each section, in order.
    fn section_starts(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, Row::Header(_) | Row::StrayHeader))
            .filter_map(|(index, _)| {
                self.rows[index + 1..]
                    .iter()
                    .position(is_landable)
                    .map(|offset| index + 1 + offset)
            })
            .collect()
    }

    /// Jump to the top of the next section, or of this one and then the one
    /// before it.
    ///
    /// The categories are the screen's structure and they are a screenful
    /// apart, so reaching the next one costs a dozen `j`s or a trip through
    /// the rail and back. Backwards lands on the current section's own first
    /// row before leaving it, the way vim's `{` does: from the middle of a
    /// section the useful move is nearly always "take me to the top of this",
    /// and the other one is a second press away.
    pub fn jump_section(&mut self, forward: bool) {
        let starts = self.section_starts();
        let target = if forward {
            starts.into_iter().find(|start| *start > self.cursor)
        } else {
            starts.into_iter().rfind(|start| *start < self.cursor)
        };
        if let Some(index) = target {
            self.cursor = index;
            self.clamp();
        }
    }

    pub fn move_to_top(&mut self) {
        self.cursor = 0;
        self.settle_forward();
        self.clamp();
    }

    pub fn move_to_bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
        self.settle_backward();
        self.clamp();
    }

    pub fn page(&mut self, delta: isize, height: usize) {
        let height = height.max(1) as isize;
        let target = (self.cursor as isize + delta * height)
            .clamp(0, self.rows.len().saturating_sub(1) as isize);
        self.cursor = target as usize;
        if delta > 0 {
            self.settle_backward_then_forward();
        } else {
            self.settle_forward_then_backward();
        }
        self.clamp();
    }

    /// Move the cursor forward to the next setting row, if it is on a header.
    fn settle_forward(&mut self) {
        while self.cursor < self.rows.len() && !is_landable(&self.rows[self.cursor]) {
            self.cursor += 1;
        }
        if self.cursor >= self.rows.len() {
            self.cursor = self.rows.len().saturating_sub(1);
            self.settle_backward();
        }
    }

    fn settle_backward(&mut self) {
        while self.cursor > 0 && !is_landable(&self.rows[self.cursor]) {
            self.cursor -= 1;
        }
    }

    fn settle_forward_then_backward(&mut self) {
        let start = self.cursor;
        self.settle_forward();
        if !self.rows.get(self.cursor).is_some_and(is_landable) {
            self.cursor = start;
            self.settle_backward();
        }
    }

    fn settle_backward_then_forward(&mut self) {
        let start = self.cursor;
        self.settle_backward();
        if !self.rows.get(self.cursor).is_some_and(is_landable) {
            self.cursor = start;
            self.settle_forward();
        }
    }

    fn clamp(&mut self) {
        if self.rows.is_empty() {
            self.cursor = 0;
            self.scroll = 0;
            return;
        }
        self.cursor = self.cursor.min(self.rows.len() - 1);
    }

    /// Keep the cursor on screen for a viewport `height` rows tall.
    pub fn follow_cursor(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + height {
            self.scroll = self.cursor + 1 - height;
        }
        // A blank line at the top of the viewport says nothing — a gap only
        // means something with a section either side of it. Skipping it can
        // only move the viewport towards the cursor, which never sits on one.
        if matches!(self.rows.get(self.scroll), Some(Row::Spacer)) {
            self.scroll += 1;
        }
        // Show a setting's own heading when it is the first thing on screen,
        // so a scrolled list rarely leaves a value without its category — but
        // only when there is room. Stealing the row costs the bottom line, and
        // a visible cursor outranks a visible heading.
        if self.scroll > 0
            && matches!(self.rows.get(self.scroll), Some(Row::Setting(_)))
            && matches!(self.rows.get(self.scroll - 1), Some(Row::Header(_)))
            && self.cursor + 1 < self.scroll + height
        {
            self.scroll -= 1;
        }
    }

    // ── Rail ─────────────────────────────────────────────────────────────

    pub fn focus_rail(&mut self) {
        self.focus = Focus::Rail;
    }

    pub fn focus_list(&mut self) {
        self.focus = Focus::List;
    }

    pub fn toggle_focus(&mut self) {
        if !self.rail_visible {
            self.focus = Focus::List;
            return;
        }
        self.focus = match self.focus {
            Focus::Rail => Focus::List,
            Focus::List => Focus::Rail,
        };
    }

    /// Tell the screen whether the rail fits, from the frame it is about to
    /// draw into. Focus follows: a hidden rail cannot hold the cursor.
    pub fn set_rail_visible(&mut self, visible: bool) {
        self.rail_visible = visible;
        if !visible {
            self.focus = Focus::List;
        }
    }

    /// Apply whatever the rail cursor is sitting on.
    fn activate_rail(&mut self) {
        match self.rail.get(self.rail_cursor).copied() {
            Some(RailEntry::Mode(mode)) if self.mode != mode => {
                self.mode = mode;
                self.rebuild();
            }
            Some(RailEntry::Mode(_)) => {}
            Some(RailEntry::Category(category)) => self.jump_to(category),
            None => {}
        }
    }

    /// Put the cursor on a category's first setting.
    pub fn jump_to(&mut self, category: Category) {
        if let Some(index) = self.rows.iter().position(|row| match row {
            Row::Setting(index) => self
                .config
                .settings
                .get(*index)
                .is_some_and(|r| r.spec.category == category),
            Row::Header(_) | Row::Spacer | Row::StrayHeader | Row::Stray(_) => false,
        }) {
            self.cursor = index;
            self.clamp();
        }
    }

    // ── Filter ───────────────────────────────────────────────────────────

    pub fn start_filter(&mut self) {
        if self.filter.is_none() {
            self.filter = Some(String::new());
        }
        self.prompt_open = true;
        self.focus = Focus::List;
    }

    pub fn filter_push(&mut self, ch: char) {
        if let Some(filter) = &mut self.filter {
            filter.push(ch);
            self.rebuild();
        }
    }

    pub fn filter_pop(&mut self) {
        if let Some(filter) = &mut self.filter {
            filter.pop();
            self.rebuild();
        }
    }

    /// Close the prompt but keep the text narrowing the list — `Enter`.
    ///
    /// An empty prompt closes to nothing: a filter line reading `/` with no
    /// text is a row of the screen that says nothing.
    pub fn commit_filter(&mut self) {
        self.prompt_open = false;
        if self.filter.as_deref() == Some("") {
            self.filter = None;
            self.rebuild();
        }
    }

    /// Drop the filter entirely — `Esc`.
    pub fn clear_filter(&mut self) {
        self.prompt_open = false;
        if self.filter.is_some() {
            self.filter = None;
            self.rebuild();
        }
    }

    /// Whether the list is narrowed, which the filter line reports.
    pub fn is_filtering(&self) -> bool {
        self.filter.is_some()
    }

    /// Whether keystrokes belong to the prompt rather than to the commands.
    pub fn is_prompt_open(&self) -> bool {
        self.prompt_open
    }

    // ── Write scope ──────────────────────────────────────────────────────

    /// Flip the scope writes go to.
    ///
    /// Outside a repository there is only one, so the key reports why rather
    /// than appearing to do nothing.
    pub fn cycle_write_scope(&mut self) {
        if !self.in_repo {
            self.status = Some(Status {
                text: "Not inside a repository — writes go to global config".to_string(),
                kind: StatusKind::Info,
            });
            return;
        }
        self.write_scope = match self.write_scope {
            ConfigScope::Local => ConfigScope::Global,
            _ => ConfigScope::Local,
        };
        self.status = None;
    }

    // ── The editor ───────────────────────────────────────────────────────

    /// Open the value editor on the selected setting.
    ///
    /// Rows another command owns do not open one: the editor's whole promise
    /// is that Enter changes the value, and a box that can only refuse breaks
    /// it. The status line points at the command that can.
    pub fn open_modal(&mut self) {
        if let Some(stray) = self.selected_stray() {
            let key = stray.key.clone();
            self.set_status(
                format!("{key} is not a daft setting — there is nothing to edit"),
                StatusKind::Info,
            );
            return;
        }
        let Some(resolved) = self.selected() else {
            return;
        };
        if let Some(owner) = resolved.spec.managed_by {
            let key = resolved.spec.key.to_string();
            self.set_status(
                format!("{key} is managed by `{owner}` — change it there"),
                StatusKind::Info,
            );
            return;
        }
        self.modal = Some(Modal::open(resolved, self.write_scope, self.in_repo));
        self.status = None;
    }

    pub fn close_modal(&mut self) {
        self.modal = None;
    }

    pub fn modal_open(&self) -> bool {
        self.modal.is_some()
    }

    /// Swap in freshly resolved config after a write, keeping the cursor on
    /// the setting the user was looking at.
    pub fn reload(&mut self, config: ResolvedSet) {
        self.config = config;
        self.rebuild();
    }

    pub fn set_status(&mut self, text: impl Into<String>, kind: StatusKind) {
        self.status = Some(Status {
            text: text.into(),
            kind,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::resolve::{Snapshot, resolve_all};
    use crate::core::settings::keys;
    use crate::git::ConfigEntry;

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

    fn state() -> ScreenState {
        state_with(vec![])
    }

    #[test]
    fn the_cursor_starts_on_a_setting_not_a_heading() {
        let state = state();
        assert!(matches!(state.rows()[0], Row::Header(_)));
        assert!(state.selected().is_some());
    }

    #[test]
    fn moving_skips_the_headings() {
        let mut state = state();
        let mut seen = 0;
        for _ in 0..40 {
            state.move_down();
            assert!(
                state.selected().is_some(),
                "the cursor landed on a heading at row {}",
                state.cursor()
            );
            seen += 1;
        }
        assert!(seen > 0);
    }

    #[test]
    fn a_blank_line_opens_every_section_but_the_first() {
        let state = state();
        let rows = state.rows();

        assert!(
            !matches!(rows[0], Row::Spacer),
            "a gap above the first section is a wasted row"
        );

        let mut headings = 0;
        for (index, row) in rows.iter().enumerate() {
            if !matches!(row, Row::Header(_) | Row::StrayHeader) {
                continue;
            }
            headings += 1;
            if index > 0 {
                assert!(
                    matches!(rows[index - 1], Row::Spacer),
                    "the heading at row {index} runs straight on from the section above"
                );
            }
        }
        assert!(headings > 1, "there is more than one section to separate");
    }

    #[test]
    fn the_cursor_walks_over_the_gaps() {
        let mut state = state();
        for _ in 0..state.rows().len() + 5 {
            state.move_down();
            assert!(
                !matches!(state.rows()[state.cursor()], Row::Spacer),
                "the cursor stopped in a gap at row {}",
                state.cursor()
            );
        }
        for _ in 0..state.rows().len() + 5 {
            state.move_up();
            assert!(!matches!(state.rows()[state.cursor()], Row::Spacer));
        }
    }

    #[test]
    fn the_section_motion_walks_the_headings_in_both_directions() {
        let mut state = state();
        let first = state.selected().unwrap().spec.category;

        state.jump_section(true);
        let second = state.selected().unwrap().spec.category;
        assert_ne!(second, first, "forward lands in the next section");
        assert!(
            state.rows()[state.cursor() - 1].eq(&Row::Header(second)),
            "and on its first row, right under the heading"
        );

        // Back from a section's own first row is the section before it.
        state.jump_section(false);
        assert_eq!(state.selected().unwrap().spec.category, first);
    }

    #[test]
    fn going_back_lands_on_this_section_before_leaving_it() {
        let mut state = state();
        state.jump_section(true);
        let section = state.selected().unwrap().spec.category;
        let top = state.cursor();

        state.move_down();
        state.move_down();
        assert!(state.cursor() > top);

        state.jump_section(false);
        assert_eq!(
            state.cursor(),
            top,
            "from the middle, go to the top of this"
        );
        assert_eq!(state.selected().unwrap().spec.category, section);
    }

    #[test]
    fn the_section_motion_stops_at_the_ends() {
        let mut state = state();
        for _ in 0..40 {
            state.jump_section(true);
            assert!(state.selected().is_some());
        }
        let last = state.cursor();
        state.jump_section(true);
        assert_eq!(state.cursor(), last, "there is nowhere further to go");

        for _ in 0..40 {
            state.jump_section(false);
            assert!(state.selected().is_some());
        }
        assert_eq!(state.cursor(), 1, "the first section's first row");
    }

    #[test]
    fn the_section_motion_survives_a_filter_that_matches_nothing() {
        let mut state = state();
        state.start_filter();
        for ch in "zzzznothing".chars() {
            state.filter_push(ch);
        }
        state.jump_section(true);
        state.jump_section(false);
        assert!(state.selected().is_none());
    }

    #[test]
    fn the_viewport_never_opens_on_a_gap() {
        let mut state = state();
        let height = 8;

        let check = |state: &ScreenState| {
            assert!(
                !matches!(state.rows().get(state.scroll), Some(Row::Spacer)),
                "the top line of the viewport is blank"
            );
            assert!(
                state.cursor() >= state.scroll && state.cursor() < state.scroll + height,
                "cursor {} outside viewport [{}, {})",
                state.cursor(),
                state.scroll,
                state.scroll + height
            );
        };

        for _ in 0..state.rows().len() + 5 {
            state.move_down();
            state.follow_cursor(height);
            check(&state);
        }
        for _ in 0..state.rows().len() + 5 {
            state.move_up();
            state.follow_cursor(height);
            check(&state);
        }
    }

    #[test]
    fn moving_up_from_the_first_setting_stays_put() {
        let mut state = state();
        let first = state.selected().unwrap().spec.key.to_string();
        state.move_up();
        state.move_up();
        assert_eq!(state.selected().unwrap().spec.key, first);
    }

    #[test]
    fn the_bottom_of_the_list_is_a_setting() {
        let mut state = state();
        state.move_to_bottom();
        assert!(state.selected().is_some());
        state.move_down();
        assert!(state.selected().is_some());
    }

    #[test]
    fn modified_mode_shows_only_what_something_sets() {
        let mut state = state_with(vec![
            entry(keys::MERGE_STYLE, "squash", ConfigScope::Local),
            entry(keys::REMOTE, "upstream", ConfigScope::Global),
        ]);

        assert!(state.visible_count() > 2, "All shows everything");

        state.mode = Mode::Modified;
        state.rebuild();
        assert_eq!(state.visible_count(), 2);
        assert!(
            state
                .rows()
                .iter()
                .filter_map(|row| match row {
                    Row::Setting(i) => state.config.settings.get(*i),
                    _ => None,
                })
                .all(|r| r.is_set())
        );
    }

    #[test]
    fn issues_mode_shows_only_settings_with_diagnostics() {
        let mut state = state_with(vec![
            entry(keys::MERGE_STYLE, "squash", ConfigScope::Local),
            entry(keys::CHECKOUT_FETCH, "maybe", ConfigScope::Local),
        ]);

        state.mode = Mode::Issues;
        state.rebuild();

        assert_eq!(state.visible_count(), 1);
        assert_eq!(state.selected().unwrap().spec.key, keys::CHECKOUT_FETCH);
    }

    #[test]
    fn the_issues_rail_entry_appears_only_when_there_are_issues() {
        let clean = state_with(vec![entry(keys::MERGE_STYLE, "squash", ConfigScope::Local)]);
        assert!(
            !clean.rail().contains(&RailEntry::Mode(Mode::Issues)),
            "a permanent 'Issues 0' teaches people to ignore it"
        );

        let messy = state_with(vec![entry(
            keys::CHECKOUT_FETCH,
            "maybe",
            ConfigScope::Local,
        )]);
        assert!(messy.rail().contains(&RailEntry::Mode(Mode::Issues)));
    }

    #[test]
    fn filtering_searches_label_key_and_help_together() {
        let mut state = state();

        state.start_filter();
        for ch in "pushverify".chars() {
            state.filter_push(ch);
        }
        // Matches the key.
        assert!(state.visible_count() >= 2);

        state.clear_filter();
        state.start_filter();
        for ch in "octopus".chars() {
            state.filter_push(ch);
        }
        // Matches only inside help text — daft.merge.strategy's.
        assert_eq!(state.visible_count(), 1);
        assert_eq!(state.selected().unwrap().spec.key, keys::MERGE_STRATEGY);
    }

    #[test]
    fn filtering_keeps_the_selection_when_it_still_matches() {
        let mut state = state();
        // Land on a setting whose help mentions "merge".
        state.start_filter();
        for ch in "merge co".chars() {
            state.filter_push(ch);
        }
        let held = state.selected().map(|r| r.spec.key.to_string());
        state.filter_push('m');
        assert_eq!(state.selected().map(|r| r.spec.key.to_string()), held);
    }

    #[test]
    fn a_filter_that_matches_nothing_leaves_no_selection() {
        let mut state = state();
        state.start_filter();
        for ch in "zzzzznothing".chars() {
            state.filter_push(ch);
        }
        assert_eq!(state.visible_count(), 0);
        assert!(state.selected().is_none());
        // And moving around must not panic on the empty list.
        state.move_down();
        state.move_up();
        state.move_to_bottom();
        assert!(state.selected().is_none());
    }

    #[test]
    fn clearing_the_filter_restores_the_full_list() {
        let mut state = state();
        let total = state.visible_count();
        state.start_filter();
        state.filter_push('z');
        assert!(state.visible_count() < total);
        state.clear_filter();
        assert_eq!(state.visible_count(), total);
        assert!(!state.is_filtering());
    }

    #[test]
    fn backspacing_the_last_character_widens_the_list_again() {
        let mut state = state();
        state.start_filter();
        for ch in "merge".chars() {
            state.filter_push(ch);
        }
        let narrow = state.visible_count();
        state.filter_pop();
        assert!(state.visible_count() >= narrow);
    }

    #[test]
    fn the_rail_indexes_categories_without_filtering_them_out() {
        let mut state = state();
        let before = state.visible_count();

        state.jump_to(Category::Merge);
        assert_eq!(state.selected().unwrap().spec.category, Category::Merge);
        assert_eq!(
            state.visible_count(),
            before,
            "a category jump moves the cursor; it does not hide the neighbours"
        );
    }

    #[test]
    fn walking_the_rail_switches_modes_and_jumps_to_categories() {
        let mut state = state_with(vec![entry(keys::MERGE_STYLE, "squash", ConfigScope::Local)]);
        state.focus_rail();

        // All → Modified
        state.move_down();
        assert_eq!(state.mode, Mode::Modified);
        assert_eq!(state.visible_count(), 1);

        // Modified → the first category still present
        state.move_down();
        assert!(matches!(
            state.rail()[state.rail_cursor()],
            RailEntry::Category(_)
        ));
        assert_eq!(
            state.mode,
            Mode::Modified,
            "a category does not change mode"
        );
    }

    #[test]
    fn a_stray_key_is_reachable_in_issues_mode_and_nowhere_else() {
        let mut state = state_with(vec![entry(
            "daft.checkoutbranch.carry",
            "false",
            ConfigScope::Local,
        )]);

        assert!(
            !state.rows().iter().any(|r| matches!(r, Row::Stray(_))),
            "All mode lists settings; the header count points at the problem"
        );

        state.mode = Mode::Issues;
        state.rebuild();

        let strays: Vec<&Row> = state
            .rows()
            .iter()
            .filter(|r| matches!(r, Row::Stray(_)))
            .collect();
        assert_eq!(strays.len(), 1, "a counted issue has to be findable");

        // And the cursor can actually reach it.
        state.move_to_bottom();
        assert_eq!(
            state.selected_stray().map(|e| e.key.as_str()),
            Some("daft.checkoutbranch.carry")
        );
        assert!(state.selected().is_none(), "it is not a setting");
    }

    #[test]
    fn editing_a_stray_key_reports_instead_of_opening_an_editor() {
        let mut state = state_with(vec![entry(
            "daft.checkoutbranch.carry",
            "false",
            ConfigScope::Local,
        )]);
        state.mode = Mode::Issues;
        state.rebuild();
        state.move_to_bottom();

        state.open_modal();
        assert!(!state.modal_open(), "there is no setting to edit");
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|s| s.text.contains("not a daft setting"))
        );
    }

    #[test]
    fn filtering_reaches_stray_keys_by_name() {
        let mut state = state_with(vec![entry(
            "daft.checkoutbranch.carry",
            "false",
            ConfigScope::Local,
        )]);
        state.mode = Mode::Issues;
        state.rebuild();

        state.start_filter();
        for ch in "checkoutbranch".chars() {
            state.filter_push(ch);
        }
        assert!(state.rows().iter().any(|r| matches!(r, Row::Stray(_))));

        state.clear_filter();
        state.start_filter();
        for ch in "zzzz".chars() {
            state.filter_push(ch);
        }
        assert!(
            !state.rows().iter().any(|r| matches!(r, Row::Stray(_))),
            "a filter that matches nothing must not leave the stray behind"
        );
    }

    #[test]
    fn the_write_scope_starts_local_in_a_repo_and_flips() {
        let mut state = state();
        assert_eq!(state.write_scope, ConfigScope::Local);
        state.cycle_write_scope();
        assert_eq!(state.write_scope, ConfigScope::Global);
        state.cycle_write_scope();
        assert_eq!(state.write_scope, ConfigScope::Local);
    }

    #[test]
    fn outside_a_repository_the_scope_is_global_and_says_why() {
        let config = resolve_all(&Snapshot::default());
        let mut state = ScreenState::new(config, false, None);

        assert_eq!(state.write_scope, ConfigScope::Global);
        state.cycle_write_scope();
        assert_eq!(
            state.write_scope,
            ConfigScope::Global,
            "there is no local scope to flip to"
        );
        assert!(
            state
                .status
                .as_ref()
                .is_some_and(|s| s.text.contains("global")),
            "a key that does nothing must say why"
        );
    }

    #[test]
    fn scrolling_follows_the_cursor_and_keeps_its_heading_visible() {
        let mut state = state();
        let height = 10;

        for _ in 0..30 {
            state.move_down();
            state.follow_cursor(height);
            assert!(
                state.cursor() >= state.scroll && state.cursor() < state.scroll + height,
                "cursor {} outside viewport [{}, {})",
                state.cursor(),
                state.scroll,
                state.scroll + height
            );
        }

        // A heading directly above the viewport gets pulled in — unless doing
        // so would push the cursor off the bottom, which is the one case where
        // the value shows without its category.
        if state.scroll > 0
            && matches!(state.rows().get(state.scroll), Some(Row::Setting(_)))
            && matches!(state.rows().get(state.scroll - 1), Some(Row::Header(_)))
        {
            assert_eq!(
                state.cursor() + 1,
                state.scroll + height,
                "the heading was left off screen with room to show it"
            );
        }
    }

    #[test]
    fn the_cursor_stays_visible_even_when_that_costs_the_heading() {
        let mut state = state();
        let height = 6;

        // Walk the whole list; the viewport must never lose the cursor,
        // whatever the heading logic decides.
        for _ in 0..state.rows().len() + 5 {
            state.move_down();
            state.follow_cursor(height);
            assert!(
                state.cursor() >= state.scroll && state.cursor() < state.scroll + height,
                "cursor {} outside viewport [{}, {})",
                state.cursor(),
                state.scroll,
                state.scroll + height
            );
        }
    }

    #[test]
    fn paging_lands_on_a_setting_in_both_directions() {
        let mut state = state();
        for _ in 0..5 {
            state.page(1, 12);
            assert!(state.selected().is_some());
        }
        for _ in 0..8 {
            state.page(-1, 12);
            assert!(state.selected().is_some());
        }
    }

    #[test]
    fn focus_toggles_between_the_two_panes() {
        let mut state = state();
        assert_eq!(state.focus, Focus::List);
        state.toggle_focus();
        assert_eq!(state.focus, Focus::Rail);
        state.toggle_focus();
        assert_eq!(state.focus, Focus::List);
    }

    #[test]
    fn a_hidden_rail_cannot_take_focus() {
        let mut state = state();
        state.focus_rail();
        assert_eq!(state.focus, Focus::Rail);

        // The terminal narrows: focus has to come back, or j/k would drive a
        // pane that is not on screen.
        state.set_rail_visible(false);
        assert_eq!(state.focus, Focus::List);

        state.toggle_focus();
        assert_eq!(state.focus, Focus::List, "tab must not reach a hidden pane");

        state.set_rail_visible(true);
        state.toggle_focus();
        assert_eq!(state.focus, Focus::Rail);
    }

    #[test]
    fn rail_counts_describe_what_each_entry_stands_for() {
        let state = state_with(vec![
            entry(keys::MERGE_STYLE, "squash", ConfigScope::Local),
            entry(keys::CHECKOUT_FETCH, "maybe", ConfigScope::Local),
        ]);

        assert_eq!(
            state.rail_count(RailEntry::Mode(Mode::All)),
            state.config.settings.len()
        );
        assert_eq!(state.rail_count(RailEntry::Mode(Mode::Modified)), 2);
        assert!(state.rail_count(RailEntry::Category(Category::Merge)) > 0);
    }
}
