//! The machine-readable form of every `daft config` verb.
//!
//! Two properties hold across all of them, and both exist so a consumer never
//! has to know which flags were passed to know what it is looking at:
//!
//! 1. **A read's document always carries the full ladder.** `--origin` changes
//!    how much a *person* is shown and nothing else. A machine that had to
//!    remember to pass a verbosity flag to get the provenance would eventually
//!    forget to, and get a document that looked complete.
//! 2. **`--format` never changes the exit code.** A narrowed read that finds
//!    nothing still exits 1 and still emits its document, so the status and the
//!    `value` field always agree and a script may use either.
//!
//! `daft config list` is the exception to neither but belongs elsewhere: it is
//! tabular, so it goes through [`crate::output::emit::Table`] rather than here.

use serde_json::{Value, json};

use super::resolve::{
    BehaviorState, Diagnostic, Layer, Resolved, ResolvedBehavior, ResolvedSet, Rung,
};
use super::write::{WriteScope, Written};
use crate::core::settings_spec::{Backend, BehaviorSpec, SettingSpec, ValueType};

/// One setting: what it is, what it reads, and every layer that had a say.
///
/// `scope` narrows `value` to that layer's own — the same narrowing the text
/// output does — and fills in `outranked_by`. `effective` is always the
/// resolved value regardless, because a consumer asking about one layer usually
/// also wants to know whether that layer is the one in force, and making it
/// re-run the command to find out would be a poor trade for one field.
pub fn setting(row: &Resolved, scope: Option<WriteScope>) -> Value {
    let at_scope = scope.and_then(|s| row.value_written_at(&row.spec, s));

    json!({
        "kind": "setting",
        "key": row.spec.key.as_ref(),
        "label": row.spec.label.as_ref(),
        "category": row.spec.category.label(),
        "help": row.spec.help.as_ref(),
        "type": type_name(&row.spec.ty),
        "backend": backend_name(&row.spec.backend),
        "values": row.spec.ty.variants().map(|variants| {
            variants
                .iter()
                .map(|(value, gloss)| json!({ "value": value, "help": gloss }))
                .collect::<Vec<_>>()
        }),
        "default": row.spec.default.value(),
        "default_rule": row.spec.default.rule(),
        "scope": scope.map(WriteScope::label),
        "value": match scope {
            Some(_) => at_scope,
            None => row.effective.as_deref(),
        },
        "effective": row.effective.as_deref(),
        "is_set": row.is_set(),
        "origin": row.origin.label(),
        "outranked_by": scope
            .and_then(|s| row.masked_above(&row.spec, s))
            .map(Layer::label),
        "writable_scopes": writable_scopes(&row.spec),
        "layers": row.rungs.iter().enumerate().map(|(index, rung)| {
            layer(rung, row.reads_from() == Some(index))
        }).collect::<Vec<_>>(),
        "diagnostics": row.diagnostics.iter().map(diagnostic).collect::<Vec<_>>(),
    })
}

/// One rung of the ladder.
///
/// `reads` rather than `winner`: the mark has to agree with the effective value
/// above it, and where a layer sets something unparseable the loaders fall back
/// past it to the default rather than to the layer below — so the layer that
/// "won" is not the one the value comes from. Both facts are here, since
/// `inert` and `value` together say what happened.
fn layer(rung: &Rung, reads: bool) -> Value {
    json!({
        "layer": rung.layer.label(),
        "value": rung.value.as_deref(),
        "path": rung.origin_path.as_ref().map(|p| p.display().to_string()),
        "reads": reads,
        "writable": rung.writable,
        "inert": rung.inert.is_some() && rung.value.is_some(),
    })
}

fn diagnostic(diagnostic: &Diagnostic) -> Value {
    // `message` is the same sentence the text output prints, so a consumer that
    // just wants to show the user something does not have to re-word `kind`.
    let message = super::describe(diagnostic);
    match diagnostic {
        Diagnostic::Invalid {
            layer,
            value,
            reason,
        } => json!({
            "kind": "invalid",
            "layer": layer.label(),
            "value": value,
            "reason": reason,
            "message": message,
        }),
        Diagnostic::Deprecated { alias, replacement } => json!({
            "kind": "deprecated",
            "alias": alias,
            "replacement": replacement,
            "message": message,
        }),
        Diagnostic::Inert { scope, value, .. } => json!({
            "kind": "inert",
            "layer": scope.label(),
            "value": value,
            "message": message,
        }),
        Diagnostic::EnvShadow { layer, value } => json!({
            "kind": "env-shadow",
            "layer": layer.label(),
            "value": value,
            "message": message,
        }),
    }
}

/// A behavior: the state it is in, the states it could be in, and its members.
///
/// With `scope`, `state` is the state that layer's own values name and is
/// `null` when they name none — which is also when the command exits 1. The
/// members carry the same narrowing, so a null state next to three member
/// values that do not add up to a preset is a readable answer rather than a
/// contradiction.
pub fn behavior(
    resolved: &ResolvedBehavior,
    set: &ResolvedSet,
    scope: Option<WriteScope>,
    state: Option<&BehaviorState>,
) -> Value {
    let spec = resolved.spec;
    let (nearest, diverging) = match state {
        Some(BehaviorState::Custom { nearest, diverging }) => (
            Some(spec.presets[*nearest].name),
            Some(diverging.iter().map(String::as_str).collect::<Vec<_>>()),
        ),
        _ => (None, None),
    };

    json!({
        "kind": "behavior",
        "name": spec.name,
        "label": spec.label,
        "help": spec.help,
        "scope": scope.map(WriteScope::label),
        "state": state.map(|state| state.name(spec)),
        "state_label": state.map(|state| state.label(spec)),
        "is_set": resolved.is_set(&set.settings),
        // Both only apply to `custom`, and naming them is the difference
        // between a state a script can act on and a shrug.
        "nearest": nearest,
        "diverging": diverging,
        "divergence": state
            .filter(|state| matches!(state, BehaviorState::Custom { .. }))
            .and(resolved.divergence_note(&set.settings)),
        "presets": spec.presets.iter().map(|preset| json!({
            "name": preset.name,
            "label": preset.label,
            "help": preset.help,
            "values": preset.values.iter()
                .map(|(key, value)| json!({ "key": key, "value": value }))
                .collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "members": resolved.members.iter()
            .map(|index| setting(&set.settings[*index], scope))
            .collect::<Vec<_>>(),
    })
}

/// What one write did, whether it touched one key or a behavior's worth.
///
/// The shape does not change with the number of keys: a setting write is a
/// `written` array of one. A consumer that special-cased the singular form
/// would break the first time it was pointed at a behavior.
pub fn write_record(
    action: Action,
    target: Target<'_>,
    scope: WriteScope,
    store: &str,
    written: &[Written],
    message: &str,
) -> Value {
    let mut value = json!({
        "action": match action {
            Action::Set => "set",
            Action::Unset => "unset",
        },
        "scope": scope.label(),
        // Which of daft's stores it landed in, which is not always what the
        // scope is called: a daft.yml setting's "global" is the repository's
        // committed file, and calling that global would send someone looking
        // in their own config for a change that is in the team's diff.
        "store": store,
        "written": written.iter().map(|entry| json!({
            "key": entry.key,
            "value": entry.value.as_deref(),
            // An unset of something already absent is a success that changed
            // nothing, and the two are worth telling apart.
            "changed": entry.changed,
            "file": entry.file.as_ref().map(|p| p.display().to_string()),
        })).collect::<Vec<_>>(),
        "message": message,
    });

    let map = value.as_object_mut().expect("json! built an object");
    match target {
        Target::Setting(spec) => {
            map.insert("kind".into(), "setting".into());
            map.insert("key".into(), spec.key.as_ref().into());
        }
        Target::Behavior { spec, state } => {
            map.insert("kind".into(), "behavior".into());
            map.insert("name".into(), spec.name.into());
            // The state it *ended* in, which is the point of narrating a
            // behavior write at all: a sequence that failed part-way through
            // leaves it custom, and this is where that shows up.
            map.insert("state".into(), state.into());
        }
    }
    value
}

#[derive(Clone, Copy)]
pub enum Action {
    Set,
    Unset,
}

pub enum Target<'a> {
    Setting(&'a SettingSpec),
    Behavior {
        spec: &'a BehaviorSpec,
        state: &'a str,
    },
}

/// The scopes a write to this setting would be accepted at.
///
/// Saves a consumer from rediscovering the rules by trying: global-only keys
/// take no local write, and a merge-gate key in a tracked file takes neither.
fn writable_scopes(spec: &SettingSpec) -> Vec<&'static str> {
    [WriteScope::Global, WriteScope::Local]
        .into_iter()
        .filter(|scope| super::write::would_accept(spec, *scope))
        .map(WriteScope::label)
        .collect()
}

fn type_name(ty: &ValueType) -> &'static str {
    match ty {
        ValueType::Bool => "bool",
        ValueType::TriBool => "bool-or-unset",
        ValueType::Enum(_) => "enum",
        ValueType::BoolOrKey => "bool-or-key",
        ValueType::Str => "string",
        ValueType::Path => "path",
        ValueType::Int => "int",
        ValueType::IntOrAuto => "int-or-auto",
        ValueType::Duration(_) => "duration",
        ValueType::Size => "size",
        ValueType::SizeOrPct => "size-or-percent",
        ValueType::Spec(_) => "spec",
        ValueType::LayoutComposite => "layout",
    }
}

fn backend_name(backend: &Backend) -> &'static str {
    match backend {
        Backend::GitConfig => "git-config",
        Backend::DaftYml { .. } => "daft.yml",
        Backend::LayoutChain => "layout-chain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::resolve::{Snapshot, resolve_all};
    use crate::core::settings::keys;
    use crate::git::{ConfigEntry, ConfigScope};
    use std::collections::HashMap;

    fn entry(key: &str, value: &str, scope: ConfigScope) -> ConfigEntry {
        ConfigEntry {
            key: key.to_string(),
            value: value.to_string(),
            scope,
            origin_path: None,
        }
    }

    fn resolved(entries: Vec<ConfigEntry>) -> ResolvedSet {
        resolve_all(&Snapshot {
            entries,
            env: HashMap::new(),
            in_repo: true,
            yaml: None,
            layout: None,
        })
    }

    /// The property that lets a consumer skip `--origin`: the ladder is always
    /// there. Asserted on a setting nothing sets, which is the case where a
    /// "only include it when interesting" shortcut would look harmless.
    #[test]
    fn a_settings_document_carries_its_ladder_even_when_nothing_sets_it() {
        let set = resolved(vec![]);
        let row = set.get(keys::REMOTE).expect("registry row");
        let document = setting(row, None);

        let layers = document["layers"].as_array().expect("layers is an array");
        assert!(
            layers.len() > 1,
            "an unset setting still has every layer that could have spoken: {layers:?}"
        );
        assert!(
            layers.iter().any(|layer| layer["reads"] == true),
            "exactly one layer is read, and on a default that is the default rung: {layers:?}"
        );
        assert_eq!(document["is_set"], false);
        assert_eq!(document["origin"], "default");
    }

    /// Narrowing changes `value` to the layer's own and fills in the warning.
    /// `effective` stays put, so the document says both what that layer holds
    /// and whether it is what daft reads.
    #[test]
    fn narrowing_reports_the_layers_own_value_and_names_what_outranks_it() {
        let set = resolved(vec![
            entry(keys::REMOTE, "shared", ConfigScope::Global),
            entry(keys::REMOTE, "mine", ConfigScope::Local),
        ]);
        let row = set.get(keys::REMOTE).expect("registry row");

        let global = setting(row, Some(WriteScope::Global));
        assert_eq!(global["scope"], "global");
        assert_eq!(global["value"], "shared");
        assert_eq!(global["effective"], "mine");
        assert_eq!(global["outranked_by"], "local");

        let local = setting(row, Some(WriteScope::Local));
        assert_eq!(local["value"], "mine");
        assert_eq!(local["outranked_by"], Value::Null);

        // Unnarrowed there is no layer to be outranked, so the field is absent
        // rather than misleadingly false.
        assert_eq!(setting(row, None)["outranked_by"], Value::Null);
    }

    /// A key daft only ever reads from global config takes no local write, and
    /// the document says so rather than leaving a consumer to try one.
    #[test]
    fn writable_scopes_reports_what_the_write_path_would_actually_accept() {
        let set = resolved(vec![]);

        let ordinary = setting(set.get(keys::REMOTE).expect("registry row"), None);
        assert_eq!(ordinary["writable_scopes"], json!(["global", "local"]));

        let global_only = setting(set.get(keys::UPDATE_CHECK).expect("registry row"), None);
        assert_eq!(global_only["writable_scopes"], json!(["global"]));
    }

    /// A behavior narrowed to a layer that names no state reports null — the
    /// same fact the exit code carries — while still listing the members, which
    /// is what shows *why* it names none.
    #[test]
    fn a_behavior_narrowed_to_a_partial_layer_has_no_state_but_still_has_members() {
        let set = resolved(vec![entry(keys::CHECKOUT_PUSH, "true", ConfigScope::Local)]);
        let behaviour = set.behavior("remote-sync").expect("registry row");
        let state = behaviour.state_at(WriteScope::Local, &set.settings);

        let document = behavior(behaviour, &set, Some(WriteScope::Local), state.as_ref());
        assert_eq!(document["state"], Value::Null);
        assert_eq!(document["scope"], "local");

        let members = document["members"].as_array().expect("members is an array");
        assert_eq!(members.len(), 3);
        assert_eq!(members[1]["value"], "true");
        assert_eq!(
            members[0]["value"],
            Value::Null,
            "the member local leaves alone is what makes the state unnameable"
        );
    }

    /// Unnarrowed, a behavior always has a state — and `custom` comes with the
    /// two fields that make it actionable.
    #[test]
    fn a_custom_behavior_names_its_nearest_preset_and_the_members_that_differ() {
        let set = resolved(vec![
            entry(keys::CHECKOUT_FETCH, "true", ConfigScope::Local),
            entry(keys::CHECKOUT_PUSH, "false", ConfigScope::Local),
            entry(keys::BRANCH_DELETE_REMOTE, "true", ConfigScope::Local),
        ]);
        let behaviour = set.behavior("remote-sync").expect("registry row");

        let document = behavior(behaviour, &set, None, Some(&behaviour.state));
        assert_eq!(document["state"], "custom");
        assert_eq!(document["nearest"], "on");
        assert_eq!(document["diverging"], json!([keys::CHECKOUT_PUSH]));
        assert!(
            document["divergence"]
                .as_str()
                .expect("custom carries the sentence")
                .contains("closest to Full sync")
        );
    }

    /// A preset's own state carries no divergence — the fields that only apply
    /// to `custom` stay absent rather than arriving empty.
    #[test]
    fn a_behavior_in_a_preset_carries_no_divergence_fields() {
        let set = resolved(vec![]);
        let behaviour = set.behavior("remote-sync").expect("registry row");

        let document = behavior(behaviour, &set, None, Some(&behaviour.state));
        assert_eq!(document["state"], "off");
        assert_eq!(document["nearest"], Value::Null);
        assert_eq!(document["diverging"], Value::Null);
        assert_eq!(document["divergence"], Value::Null);
    }

    /// One shape for one key and for many, so a consumer written against a
    /// setting write reads a behavior write without changing.
    #[test]
    fn a_write_record_has_the_same_shape_for_one_key_and_for_a_behaviors_worth() {
        let spec = crate::core::settings_spec::find(keys::REMOTE).expect("registry row");
        let one = write_record(
            Action::Set,
            Target::Setting(&spec),
            WriteScope::Local,
            "local",
            &[Written {
                key: keys::REMOTE.to_string(),
                value: Some("upstream".to_string()),
                changed: true,
                file: None,
            }],
            "Set daft.remote = upstream (local)",
        );
        assert_eq!(one["kind"], "setting");
        assert_eq!(one["key"], keys::REMOTE);
        assert_eq!(one["written"].as_array().expect("an array").len(), 1);
        assert_eq!(one["written"][0]["changed"], true);

        let behaviour = crate::core::settings_spec::BEHAVIORS
            .iter()
            .find(|spec| spec.name == "remote-sync")
            .expect("registry row");
        let many = write_record(
            Action::Unset,
            Target::Behavior {
                spec: behaviour,
                state: "off",
            },
            WriteScope::Local,
            "local",
            &[Written {
                key: keys::CHECKOUT_PUSH.to_string(),
                value: None,
                changed: false,
                file: None,
            }],
            "remote-sync was not set (local)",
        );
        assert_eq!(many["kind"], "behavior");
        assert_eq!(many["name"], "remote-sync");
        // The state it ended in, not the one asked for: an unset can reveal a
        // value from the scope below.
        assert_eq!(many["state"], "off");
        assert_eq!(many["written"][0]["value"], Value::Null);
        assert_eq!(
            many["written"][0]["changed"], false,
            "nothing was there to remove, which is not the same as removing it"
        );

        // The spine is common: everything a setting write reports, a behavior
        // write reports too, under the same name. A behavior adds `state`, and
        // a setting deliberately has none rather than a null one — a field
        // present and empty invites a consumer to look for a state that cannot
        // exist. What differs is only the key that names the target.
        let one_keys = one.as_object().expect("an object").clone();
        let many_keys = many.as_object().expect("an object").clone();
        for key in one_keys.keys().filter(|key| *key != "key") {
            assert!(
                many_keys.contains_key(key),
                "{key} is part of the shape both must have"
            );
        }
        let mut extra: Vec<&str> = many_keys
            .keys()
            .map(String::as_str)
            .filter(|key| *key != "name" && !one_keys.contains_key(*key))
            .collect();
        extra.sort_unstable();
        assert_eq!(extra, ["state"], "a behavior adds its resulting state");
    }
}
