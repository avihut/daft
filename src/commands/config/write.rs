//! The write path every setting change goes through.
//!
//! `daft config set`, the modal editor's apply, and the list-level quick
//! toggle all land here, so the order of checks is the same wherever a value
//! comes from: validate the type, run any cross-key rule, canonicalize the
//! spelling, then write once.
//!
//! Canonicalizing before the write is not cosmetic. Git normalizes a config
//! key's section and value name but compares the subsection between them
//! case-**sensitively**, so writing the user's spelling of
//! `daft.CHECKOUTBRANCH.carry` would mint a second, inert subsection — the
//! exact typo the unrecognized-key diagnostic exists to report. The registry's
//! spelling is the one that goes in the file.

use anyhow::{Result, bail};

use super::resolve::ResolvedSet;
use crate::core::settings_spec::{Backend, SettingSpec, ValueType, parse_git_bool};
use crate::git::{ConfigScope, GitCommand};

/// Where a change is written.
///
/// The two scopes git gives a flag for. The others are readable but not
/// writable: system needs privileges daft should not ask for, worktree needs
/// an extension most repos have off, and the process scope is not a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScope {
    Global,
    Local,
}

impl WriteScope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Local => "local",
        }
    }

    pub fn as_config_scope(self) -> ConfigScope {
        match self {
            Self::Global => ConfigScope::Global,
            Self::Local => ConfigScope::Local,
        }
    }
}

/// The spelling a value is stored as.
///
/// Bools become `true`/`false` whichever of git's six spellings was typed, and
/// an enum takes the registry's casing. Keeps config files readable and means
/// the value that comes back out matches the variant list.
pub fn canonical_value(ty: &ValueType, input: &str) -> String {
    let trimmed = input.trim();
    match ty {
        ValueType::Bool | ValueType::TriBool => match parse_git_bool(trimmed) {
            Some(true) => "true".to_string(),
            Some(false) => "false".to_string(),
            None => trimmed.to_string(),
        },
        ValueType::Enum(variants) => variants
            .iter()
            .find(|(variant, _)| variant.eq_ignore_ascii_case(trimmed))
            .map(|(variant, _)| (*variant).to_string())
            .unwrap_or_else(|| trimmed.to_string()),
        _ => trimmed.to_string(),
    }
}

/// Refuse a write that could not take effect, before touching anything.
///
/// A write daft accepts and then ignores is worse than a refusal: the user
/// walks away believing the setting is live.
fn check_writable(spec: &SettingSpec, scope: WriteScope) -> Result<()> {
    if let Some(owner) = spec.managed_by {
        bail!(
            "{} is managed by `{owner}` — change it there so the two cannot disagree",
            spec.key
        );
    }

    if spec.global_only && scope == WriteScope::Local {
        bail!(
            "{} is read from global config only — a local value would be set and ignored.\n\
             Use --global to change it.",
            spec.key
        );
    }

    Ok(())
}

/// Set `spec` to `raw` at `scope`, returning the line to narrate.
///
/// `config` supplies the rest of the configuration to any cross-key rule —
/// the merge pair refuses here rather than at the next `daft merge`.
pub fn set(
    spec: &SettingSpec,
    scope: WriteScope,
    raw: &str,
    config: &ResolvedSet,
) -> Result<String> {
    check_writable(spec, scope)?;

    let value = canonical_value(&spec.ty, raw);
    if let Err(reason) = spec.ty.validate(&value) {
        bail!("{}: {reason}", spec.key);
    }
    if let Some(validate) = spec.validate
        && let Err(reason) = validate(config, &value)
    {
        bail!("{reason}");
    }

    let git = GitCommand::new(false);
    match spec.backend {
        Backend::GitConfig => match scope {
            WriteScope::Global => git.config_set_global(&spec.key, &value)?,
            WriteScope::Local => git.config_set(&spec.key, &value)?,
        },
        Backend::DaftYml { path, .. } => bail!(
            "{path} lives in daft.yml, which `daft config` cannot edit yet — \
             change it in the file directly"
        ),
        Backend::LayoutChain => bail!(
            "the worktree layout has its own command: `daft layout default <name>` \
             for the global default, `daft layout transform <name>` for this repo"
        ),
    }

    Ok(format!("Set {} = {value} ({})", spec.key, scope.label()))
}

/// Remove `spec`'s value at `scope`, returning the line to narrate.
pub fn unset(spec: &SettingSpec, scope: WriteScope) -> Result<String> {
    check_writable(spec, scope)?;

    let git = GitCommand::new(false);
    let removed = match spec.backend {
        Backend::GitConfig => match scope {
            WriteScope::Global => git.config_unset_global(&spec.key)?,
            WriteScope::Local => git.config_unset(&spec.key)?,
        },
        Backend::DaftYml { path, .. } => bail!(
            "{path} lives in daft.yml, which `daft config` cannot edit yet — \
             change it in the file directly"
        ),
        Backend::LayoutChain => bail!(
            "the worktree layout has its own command: `daft layout reset` clears \
             this repo's stored layout"
        ),
    };

    Ok(if removed {
        format!("Unset {} ({})", spec.key, scope.label())
    } else {
        format!("{} was not set ({})", spec.key, scope.label())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::config::resolve::{Snapshot, resolve_all};
    use crate::core::settings::keys;
    use crate::core::settings_spec::find;

    fn empty_config() -> ResolvedSet {
        resolve_all(&Snapshot {
            in_repo: true,
            ..Default::default()
        })
    }

    #[test]
    fn booleans_are_stored_in_one_spelling() {
        let bool_ty = ValueType::Bool;
        for spelling in ["true", "TRUE", "yes", "on", "1"] {
            assert_eq!(canonical_value(&bool_ty, spelling), "true");
        }
        for spelling in ["false", "FALSE", "no", "off", "0"] {
            assert_eq!(canonical_value(&bool_ty, spelling), "false");
        }
    }

    #[test]
    fn an_enum_takes_the_registrys_casing() {
        let spec = find(keys::MERGE_STYLE).unwrap();
        assert_eq!(canonical_value(&spec.ty, "REBASE-MERGE"), "rebase-merge");
        assert_eq!(canonical_value(&spec.ty, "  squash "), "squash");
    }

    #[test]
    fn a_signing_key_is_not_mangled_into_a_boolean() {
        // `merge.gpgSign` takes a bool *or* a key id, so the bool
        // canonicalization must not touch it — "0xDEADBEEF" is a key.
        let spec = find(keys::MERGE_GPG_SIGN).unwrap();
        assert_eq!(canonical_value(&spec.ty, "0xDEADBEEF"), "0xDEADBEEF");
    }

    #[test]
    fn a_global_only_key_refuses_a_local_write_before_touching_git() {
        let spec = find(keys::UPDATE_CHECK).unwrap();
        let err = set(&spec, WriteScope::Local, "false", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(err.contains("global config only"), "unhelpful: {err}");
        assert!(err.contains("--global"), "the fix must be in the message");
    }

    #[test]
    fn a_row_owned_by_another_command_points_at_it() {
        let spec = find("shared").unwrap();
        let err = set(&spec, WriteScope::Local, ".env", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(err.contains("daft shared"), "unhelpful: {err}");
    }

    #[test]
    fn an_invalid_value_is_refused_with_the_reason() {
        let spec = find(keys::MERGE_STYLE).unwrap();
        let err = set(&spec, WriteScope::Local, "octopus-ish", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected one of"), "unhelpful: {err}");
        assert!(err.contains("squash"), "the message must list the options");
    }

    #[test]
    fn a_bad_column_spec_is_refused_where_it_is_typed() {
        // Not at the next `daft list`, which is where it used to surface.
        let spec = find(keys::LIST_COLUMNS).unwrap();
        let err = set(
            &spec,
            WriteScope::Local,
            "+definitelyNotAColumn",
            &empty_config(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains(keys::LIST_COLUMNS), "unhelpful: {err}");
    }

    #[test]
    fn the_cross_key_rule_refuses_before_the_write() {
        let config = resolve_all(&Snapshot {
            entries: vec![crate::git::ConfigEntry {
                key: keys::MERGE_CLEANUP.to_string(),
                value: "remove-branch".to_string(),
                scope: ConfigScope::Local,
                origin_path: None,
            }],
            in_repo: true,
            ..Default::default()
        });

        let spec = find(keys::MERGE_COMMIT).unwrap();
        let err = set(&spec, WriteScope::Local, "false", &config)
            .unwrap_err()
            .to_string();
        assert!(err.contains("remove-branch"), "unhelpful: {err}");
    }

    #[test]
    fn a_daft_yml_row_names_the_file_rather_than_failing_obscurely() {
        let spec = find("log.retention").unwrap();
        let err = set(&spec, WriteScope::Local, "14d", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(err.contains("daft.yml"), "unhelpful: {err}");
    }

    #[test]
    fn the_layout_row_points_at_the_layout_command() {
        let spec = find("layout").unwrap();
        let err = set(&spec, WriteScope::Global, "nested", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(err.contains("daft layout"), "unhelpful: {err}");
    }
}
