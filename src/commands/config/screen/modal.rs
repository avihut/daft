//! The value editor: a centred overlay over the dimmed screen.
//!
//! Every setting is edited the same way, which is the point — a bool, an enum,
//! a duration, and a column spec all open the same box with the same keys, so
//! there is one thing to learn rather than thirteen.
//!
//! Two things the overlay always shows that a bare prompt would not: **which
//! scope** the value is going to, and **unset** as a first-class choice rather
//! than an absence. Unsetting is how you reveal what a value was masking, and
//! for the tri-state settings it is a third meaningful answer, not a way to
//! give up.
//!
//! The editor decides *what to write* and hands it back as an [`Apply`]. It
//! never writes: that keeps the whole thing testable without a repository, and
//! keeps the one place that touches git in `write.rs` where the CLI shares it.

use crate::commands::config::resolve::Resolved;
use crate::commands::config::write::{WriteScope, canonical_value};
use crate::core::settings_spec::{SettingSpec, ValueType};
use crate::git::ConfigScope;

/// What the editor decided to do, for the caller to carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Apply {
    Set {
        key: String,
        scope: WriteScope,
        value: String,
    },
    Unset {
        key: String,
        scope: WriteScope,
    },
}

/// One row of the value list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Option_ {
    /// A concrete value, with the gloss the registry gives it.
    Value { value: String, gloss: String },
    /// Remove whatever is set at this scope. Always last.
    Unset { reveals: String },
}

/// How the value is being entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// Pick from a closed list.
    Options { cursor: usize },
    /// Type it. `on_unset` is the row below the input.
    Text {
        buffer: String,
        caret: usize,
        on_unset: bool,
    },
}

/// The open editor.
#[derive(Debug, Clone)]
pub struct Modal {
    pub spec: SettingSpec,
    /// Where the write goes. Starts at the screen's pill.
    pub scope: WriteScope,
    /// The scopes worth offering — one of them when there is no repository.
    pub scopes: Vec<WriteScope>,
    pub options: Vec<Option_>,
    pub field: Field,
    /// Why the last attempt was refused. Cleared on the next keystroke.
    pub error: Option<String>,
    /// What this setting resolves to today, for the "reveals" line.
    inherited: String,
    /// True when the chosen scope is one daft never reads for this key.
    pub scope_is_inert: bool,
}

impl Modal {
    /// Open the editor for a setting.
    pub fn open(resolved: &Resolved, scope: ConfigScope, in_repo: bool) -> Self {
        let spec = resolved.spec.clone();

        let scopes = if in_repo {
            vec![WriteScope::Local, WriteScope::Global]
        } else {
            vec![WriteScope::Global]
        };
        let scope = match scope {
            ConfigScope::Global => WriteScope::Global,
            _ if in_repo => WriteScope::Local,
            _ => WriteScope::Global,
        };

        // What unsetting at this scope would leave behind. Computing it up
        // front is what lets the unset row say "inherit: auto" instead of
        // making the user guess.
        let inherited = resolved
            .spec
            .default
            .value()
            .map(str::to_string)
            .unwrap_or_else(|| "nothing".to_string());

        let options = match spec.ty.variants() {
            Some(variants) => {
                let mut rows: Vec<Option_> = variants
                    .iter()
                    .map(|(value, gloss)| Option_::Value {
                        value: (*value).to_string(),
                        gloss: (*gloss).to_string(),
                    })
                    .collect();
                rows.push(Option_::Unset {
                    reveals: inherited.clone(),
                });
                rows
            }
            None => vec![Option_::Unset {
                reveals: inherited.clone(),
            }],
        };

        let field = match spec.ty.variants() {
            Some(_) => {
                // Start on whatever is set at this scope, so Enter without
                // moving is a no-op rather than a surprise.
                let current = resolved.value_written_at(&spec, scope);
                let cursor = current
                    .and_then(|value| {
                        options.iter().position(|row| match row {
                            Option_::Value { value: v, .. } => v.eq_ignore_ascii_case(value),
                            Option_::Unset { .. } => false,
                        })
                    })
                    .unwrap_or(0);
                Field::Options { cursor }
            }
            None => {
                let buffer = resolved
                    .value_written_at(&spec, scope)
                    .unwrap_or_default()
                    .to_string();
                let caret = buffer.chars().count();
                Field::Text {
                    buffer,
                    caret,
                    on_unset: false,
                }
            }
        };

        let mut modal = Self {
            spec,
            scope,
            scopes,
            options,
            field,
            error: None,
            inherited,
            scope_is_inert: false,
        };
        modal.refresh_scope_warning();
        modal
    }

    /// Whether the value is picked from a list rather than typed.
    pub fn is_picking(&self) -> bool {
        matches!(self.field, Field::Options { .. })
    }

    /// The row currently selected, when picking.
    pub fn selected_option(&self) -> std::option::Option<&Option_> {
        match &self.field {
            Field::Options { cursor } => self.options.get(*cursor),
            Field::Text { on_unset, .. } if *on_unset => self.options.last(),
            Field::Text { .. } => None,
        }
    }

    pub fn move_down(&mut self) {
        self.error = None;
        match &mut self.field {
            Field::Options { cursor } => {
                if *cursor + 1 < self.options.len() {
                    *cursor += 1;
                }
            }
            Field::Text { on_unset, .. } => *on_unset = true,
        }
    }

    pub fn move_up(&mut self) {
        self.error = None;
        match &mut self.field {
            Field::Options { cursor } => *cursor = cursor.saturating_sub(1),
            Field::Text { on_unset, .. } => *on_unset = false,
        }
    }

    /// Move to the next scope. Wraps, so one key reaches all of them.
    pub fn cycle_scope(&mut self) {
        self.error = None;
        if self.scopes.len() < 2 {
            return;
        }
        let index = self
            .scopes
            .iter()
            .position(|s| *s == self.scope)
            .unwrap_or(0);
        self.scope = self.scopes[(index + 1) % self.scopes.len()];
        self.refresh_scope_warning();
    }

    pub fn set_scope(&mut self, scope: WriteScope) {
        if self.scopes.contains(&scope) {
            self.scope = scope;
            self.error = None;
            self.refresh_scope_warning();
        }
    }

    /// A key daft reads only from global config is offered at local anyway,
    /// with the warning inline — hiding the scope would be more confusing than
    /// showing why it is a bad idea, and the apply still refuses.
    fn refresh_scope_warning(&mut self) {
        self.scope_is_inert = self.spec.global_only && self.scope == WriteScope::Local;
    }

    // ── Text entry ───────────────────────────────────────────────────────

    pub fn type_char(&mut self, ch: char) {
        self.error = None;
        if let Field::Text {
            buffer,
            caret,
            on_unset,
        } = &mut self.field
        {
            // Typing always means "I want a value", so it takes focus back
            // from the unset row rather than being swallowed.
            *on_unset = false;
            let index = char_to_byte(buffer, *caret);
            buffer.insert(index, ch);
            *caret += 1;
        }
    }

    pub fn backspace(&mut self) {
        self.error = None;
        if let Field::Text { buffer, caret, .. } = &mut self.field
            && *caret > 0
        {
            let index = char_to_byte(buffer, *caret - 1);
            buffer.remove(index);
            *caret -= 1;
        }
    }

    pub fn caret_left(&mut self) {
        if let Field::Text { caret, .. } = &mut self.field {
            *caret = caret.saturating_sub(1);
        }
    }

    pub fn caret_right(&mut self) {
        if let Field::Text { buffer, caret, .. } = &mut self.field {
            *caret = (*caret + 1).min(buffer.chars().count());
        }
    }

    /// The live format hint under a text field, or the current buffer's
    /// complaint once it stops parsing.
    pub fn text_feedback(&self) -> std::option::Option<String> {
        let Field::Text { buffer, .. } = &self.field else {
            return None;
        };
        if buffer.trim().is_empty() {
            return self.spec.ty.format_hint().map(str::to_string);
        }
        match self.spec.ty.validate(buffer) {
            Ok(()) => self.spec.ty.format_hint().map(str::to_string),
            Err(reason) => Some(reason),
        }
    }

    /// Whether what is typed would be accepted, for the live marker.
    pub fn text_is_valid(&self) -> bool {
        match &self.field {
            Field::Text {
                buffer, on_unset, ..
            } if !on_unset => !buffer.trim().is_empty() && self.spec.ty.validate(buffer).is_ok(),
            _ => true,
        }
    }

    // ── Applying ─────────────────────────────────────────────────────────

    /// What the current selection would do.
    ///
    /// `Err` is a refusal to show inside the overlay; the caller only carries
    /// out an `Ok`. Type validation happens here so the message lands next to
    /// the field rather than after the box has closed — the cross-key rules
    /// still run in `write.rs`, which is the one place that cannot be skipped.
    pub fn apply(&self) -> Result<Apply, String> {
        let key = self.spec.key.to_string();

        if self.scope_is_inert {
            return Err(format!(
                "daft reads {key} from global config only — press tab to switch scope"
            ));
        }

        let chosen = match &self.field {
            Field::Options { cursor } => match self.options.get(*cursor) {
                Some(Option_::Value { value, .. }) => Some(value.clone()),
                Some(Option_::Unset { .. }) | None => None,
            },
            Field::Text { on_unset: true, .. } => None,
            Field::Text { buffer, .. } => {
                if buffer.trim().is_empty() {
                    return Err("type a value, or choose unset below".to_string());
                }
                Some(buffer.clone())
            }
        };

        match chosen {
            Some(raw) => {
                let value = canonical_value(&self.spec.ty, &raw);
                self.spec.ty.validate(&value)?;
                Ok(Apply::Set {
                    key,
                    scope: self.scope,
                    value,
                })
            }
            None => Ok(Apply::Unset {
                key,
                scope: self.scope,
            }),
        }
    }

    /// What unsetting would leave behind, for the unset row's caption.
    pub fn inherited(&self) -> &str {
        &self.inherited
    }
}

/// Byte offset of the `index`-th character.
fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

/// The quick toggle: the other side of a boolean, for `space` in the list.
///
/// Returns `None` for anything that is not a two-state setting, so the key
/// reports "not a toggle" rather than guessing at what a duration's opposite
/// might be.
pub fn toggled_value(resolved: &Resolved, scope: ConfigScope) -> std::option::Option<String> {
    if !matches!(resolved.spec.ty, ValueType::Bool | ValueType::TriBool) {
        return None;
    }
    // Flip what the user can currently see, not what this scope holds: the
    // effective value is what they are looking at, and a toggle that inverts
    // an invisible lower layer would look like it did nothing.
    let current = resolved
        .value_at(scope)
        .map(str::to_string)
        .or_else(|| resolved.effective.clone())
        .unwrap_or_else(|| "false".to_string());

    match crate::core::settings_spec::parse_git_bool(&current) {
        Some(true) => Some("false".to_string()),
        Some(false) => Some("true".to_string()),
        None => Some("true".to_string()),
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

    fn resolved_for(key: &str, entries: Vec<ConfigEntry>) -> Resolved {
        let set = resolve_all(&Snapshot {
            entries,
            in_repo: true,
            ..Default::default()
        });
        set.get(key).expect("registry row").clone()
    }

    fn open(key: &str, entries: Vec<ConfigEntry>) -> Modal {
        Modal::open(&resolved_for(key, entries), ConfigScope::Local, true)
    }

    #[test]
    fn an_enum_opens_on_its_current_value() {
        let modal = open(
            keys::MERGE_STYLE,
            vec![entry(keys::MERGE_STYLE, "rebase", ConfigScope::Local)],
        );
        assert!(modal.is_picking());
        assert_eq!(
            modal.selected_option(),
            Some(&Option_::Value {
                value: "rebase".to_string(),
                gloss: "rebase onto the target".to_string(),
            }),
            "opening elsewhere makes Enter-without-moving a surprise"
        );
    }

    #[test]
    fn unset_is_always_the_last_choice_and_says_what_it_reveals() {
        let modal = open(keys::MERGE_STYLE, vec![]);
        match modal.options.last() {
            Some(Option_::Unset { reveals }) => assert_eq!(reveals, "merge"),
            other => panic!("expected unset last, got {other:?}"),
        }
    }

    #[test]
    fn picking_a_value_applies_it_at_the_current_scope() {
        let mut modal = open(keys::MERGE_STYLE, vec![]);
        modal.move_down();
        assert_eq!(
            modal.apply(),
            Ok(Apply::Set {
                key: keys::MERGE_STYLE.to_string(),
                scope: WriteScope::Local,
                value: "squash".to_string(),
            })
        );
    }

    #[test]
    fn the_unset_row_applies_as_an_unset() {
        let mut modal = open(keys::MERGE_STYLE, vec![]);
        for _ in 0..10 {
            modal.move_down();
        }
        assert_eq!(
            modal.apply(),
            Ok(Apply::Unset {
                key: keys::MERGE_STYLE.to_string(),
                scope: WriteScope::Local,
            })
        );
    }

    #[test]
    fn the_scope_cycles_and_the_write_follows_it() {
        let mut modal = open(keys::MERGE_STYLE, vec![]);
        assert_eq!(modal.scope, WriteScope::Local);
        modal.cycle_scope();
        assert_eq!(modal.scope, WriteScope::Global);
        modal.cycle_scope();
        assert_eq!(modal.scope, WriteScope::Local, "cycling wraps");

        modal.set_scope(WriteScope::Global);
        match modal.apply().unwrap() {
            Apply::Set { scope, .. } => assert_eq!(scope, WriteScope::Global),
            other => panic!("expected a set, got {other:?}"),
        }
    }

    #[test]
    fn outside_a_repository_only_global_is_offered() {
        let resolved = resolved_for(keys::MERGE_STYLE, vec![]);
        let mut modal = Modal::open(&resolved, ConfigScope::Global, false);

        assert_eq!(modal.scopes, vec![WriteScope::Global]);
        modal.cycle_scope();
        assert_eq!(
            modal.scope,
            WriteScope::Global,
            "there is no second scope to reach"
        );
    }

    #[test]
    fn a_global_only_key_warns_at_local_and_refuses_to_apply_there() {
        let mut modal = open(keys::UPDATE_CHECK, vec![]);
        assert!(
            modal.scope_is_inert,
            "the warning has to be visible before Enter, not after"
        );

        let err = modal.apply().unwrap_err();
        assert!(err.contains("global config only"), "unhelpful: {err}");
        assert!(err.contains("tab"), "the message names the way out");

        modal.cycle_scope();
        assert!(!modal.scope_is_inert);
        assert!(modal.apply().is_ok());
    }

    // ── Text entry ───────────────────────────────────────────────────────

    #[test]
    fn a_text_setting_opens_with_what_is_already_there() {
        let modal = open(
            keys::REMOTE,
            vec![entry(keys::REMOTE, "upstream", ConfigScope::Local)],
        );
        match &modal.field {
            Field::Text { buffer, caret, .. } => {
                assert_eq!(buffer, "upstream");
                assert_eq!(*caret, 8, "the caret starts where typing continues");
            }
            other => panic!("expected a text field, got {other:?}"),
        }
    }

    #[test]
    fn typing_editing_and_applying_a_text_value() {
        let mut modal = open(keys::REMOTE, vec![]);
        for ch in "upstreammm".chars() {
            modal.type_char(ch);
        }
        modal.backspace();
        modal.backspace();

        assert_eq!(
            modal.apply(),
            Ok(Apply::Set {
                key: keys::REMOTE.to_string(),
                scope: WriteScope::Local,
                value: "upstream".to_string(),
            })
        );
    }

    #[test]
    fn the_caret_moves_and_inserts_where_it_sits() {
        let mut modal = open(keys::REMOTE, vec![]);
        for ch in "orign".chars() {
            modal.type_char(ch);
        }
        modal.caret_left();
        modal.type_char('i');

        match modal.apply().unwrap() {
            Apply::Set { value, .. } => assert_eq!(value, "origin"),
            other => panic!("expected a set, got {other:?}"),
        }
    }

    #[test]
    fn text_editing_handles_multi_byte_characters() {
        // A byte-indexed insert would split these and panic.
        let mut modal = open(keys::REMOTE, vec![]);
        for ch in "naïve—ünïcode".chars() {
            modal.type_char(ch);
        }
        modal.caret_left();
        modal.caret_left();
        modal.type_char('ß');
        modal.backspace();
        modal.backspace();

        match modal.apply().unwrap() {
            Apply::Set { value, .. } => assert!(value.starts_with("naïve—ünïc")),
            other => panic!("expected a set, got {other:?}"),
        }
    }

    #[test]
    fn an_empty_text_field_refuses_rather_than_writing_nothing() {
        let modal = open(keys::REMOTE, vec![]);
        let err = modal.apply().unwrap_err();
        assert!(err.contains("unset"), "the message names the alternative");
    }

    #[test]
    fn a_text_field_reports_what_is_wrong_while_you_type() {
        let mut modal = open(keys::hooks::TIMEOUT, vec![]);
        assert_eq!(
            modal.text_feedback().as_deref(),
            Some("a whole number"),
            "an empty field shows the format"
        );

        for ch in "ninety".chars() {
            modal.type_char(ch);
        }
        assert!(!modal.text_is_valid());
        assert_eq!(
            modal.text_feedback().as_deref(),
            Some("expected a whole number")
        );

        for _ in 0..6 {
            modal.backspace();
        }
        modal.type_char('9');
        modal.type_char('0');
        assert!(modal.text_is_valid());
        assert_eq!(
            modal.apply().unwrap(),
            Apply::Set {
                key: keys::hooks::TIMEOUT.to_string(),
                scope: WriteScope::Local,
                value: "90".to_string(),
            }
        );
    }

    #[test]
    fn a_text_setting_can_still_be_unset() {
        let mut modal = open(
            keys::REMOTE,
            vec![entry(keys::REMOTE, "upstream", ConfigScope::Local)],
        );
        modal.move_down();
        assert_eq!(
            modal.apply(),
            Ok(Apply::Unset {
                key: keys::REMOTE.to_string(),
                scope: WriteScope::Local,
            })
        );

        // Typing takes focus back from the unset row.
        modal.type_char('x');
        assert!(matches!(modal.apply(), Ok(Apply::Set { .. })));
    }

    #[test]
    fn values_are_canonicalized_before_they_leave_the_editor() {
        let mut modal = open(keys::MERGE_GPG_SIGN, vec![]);
        for ch in "TRUE".chars() {
            modal.type_char(ch);
        }
        // boolOrKey passes its text through — a key id must survive verbatim.
        match modal.apply().unwrap() {
            Apply::Set { value, .. } => assert_eq!(value, "TRUE"),
            other => panic!("expected a set, got {other:?}"),
        }

        let mut modal = open(keys::MERGE_STYLE, vec![]);
        modal.move_down();
        match modal.apply().unwrap() {
            Apply::Set { value, .. } => assert_eq!(value, "squash"),
            other => panic!("expected a set, got {other:?}"),
        }
    }

    // ── Quick toggle ─────────────────────────────────────────────────────

    #[test]
    fn the_quick_toggle_flips_what_is_on_screen() {
        // Nothing set: flip the default the user is looking at.
        let resolved = resolved_for(keys::CHECKOUT_FETCH, vec![]);
        assert_eq!(
            toggled_value(&resolved, ConfigScope::Local).as_deref(),
            Some("true")
        );

        // Set locally: flip that.
        let resolved = resolved_for(
            keys::CHECKOUT_FETCH,
            vec![entry(keys::CHECKOUT_FETCH, "true", ConfigScope::Local)],
        );
        assert_eq!(
            toggled_value(&resolved, ConfigScope::Local).as_deref(),
            Some("false")
        );

        // Set globally, editing locally: flip what is effective, because that
        // is what is on screen.
        let resolved = resolved_for(
            keys::CHECKOUT_FETCH,
            vec![entry(keys::CHECKOUT_FETCH, "true", ConfigScope::Global)],
        );
        assert_eq!(
            toggled_value(&resolved, ConfigScope::Local).as_deref(),
            Some("false")
        );
    }

    #[test]
    fn the_quick_toggle_refuses_anything_that_is_not_two_state() {
        for key in [keys::MERGE_STYLE, keys::REMOTE, keys::hooks::TIMEOUT] {
            let resolved = resolved_for(key, vec![]);
            assert!(
                toggled_value(&resolved, ConfigScope::Local).is_none(),
                "{key} has no obvious opposite"
            );
        }
    }

    #[test]
    fn a_tri_state_toggles_and_still_offers_unset_in_the_editor() {
        let resolved = resolved_for(keys::MERGE_EDIT, vec![]);
        assert!(toggled_value(&resolved, ConfigScope::Local).is_some());

        let modal = Modal::open(&resolved, ConfigScope::Local, true);
        assert!(
            matches!(modal.options.last(), Some(Option_::Unset { .. })),
            "unset is the third state, not the absence of one"
        );
    }
}
