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

use anyhow::{Context, Result, bail};

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

    /// What this scope is called for a particular setting.
    ///
    /// "local" and "global" are the git-config words. The layout row's two
    /// writable layers are a per-repo entry in the trust store and a default
    /// in the global TOML, and calling those "local" and "global" would send
    /// someone looking in `.git/config` for a value that is not there.
    pub fn label_for(self, spec: &SettingSpec) -> &'static str {
        match (spec.backend, self) {
            (Backend::LayoutChain, Self::Local) => "repo store",
            (Backend::LayoutChain, Self::Global) => "global toml",
            _ => self.label(),
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

/// Write the worktree layout at `scope`.
///
/// The two scopes map onto the two layers of the chain daft owns: local is the
/// per-repo choice in the trust store, global is `defaults.layout` in the TOML.
/// This records the *preference* — it does not move any worktrees, which is
/// `daft layout transform`'s job and a very different thing to set off from a
/// settings list.
fn set_layout(scope: WriteScope, value: &str) -> Result<()> {
    use crate::core::global_config::GlobalConfig;
    use crate::hooks::TrustDatabase;

    match scope {
        WriteScope::Global => GlobalConfig::set_default_layout(value),
        WriteScope::Local => {
            let git_dir = crate::get_git_common_dir()
                .context("Not inside a git repository — no repo to set a layout for")?;
            // Through `update_if`, which takes the store's lock — the same
            // path `daft layout` uses, so two daft processes cannot lose each
            // other's repo entries.
            let value = value.to_string();
            TrustDatabase::update_if(|db| {
                db.set_layout(std::path::Path::new(&git_dir), value.clone());
                Ok(true)
            })
        }
    }
}

/// Clear the worktree layout at `scope`. Returns whether anything was set.
fn unset_layout(scope: WriteScope) -> Result<bool> {
    use crate::core::global_config::GlobalConfig;
    use crate::hooks::TrustDatabase;

    match scope {
        WriteScope::Global => {
            let had = GlobalConfig::load()
                .unwrap_or_default()
                .defaults
                .layout
                .is_some();
            GlobalConfig::remove_default_layout()?;
            Ok(had)
        }
        WriteScope::Local => {
            let git_dir = crate::get_git_common_dir()
                .context("Not inside a git repository — no repo to clear a layout for")?;
            let mut removed = false;
            TrustDatabase::update_if(|db| {
                removed = db.remove_layout(std::path::Path::new(&git_dir));
                Ok(removed)
            })?;
            Ok(removed)
        }
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

/// Refuse a value that would loosen a policy an overlay may only tighten.
///
/// The merge-gate keys are the case: a repository commits `merge.ff: only` as
/// a boundary, and an untracked `daft.local.yml` that could switch it off
/// would not be a boundary at all. Their types carry a single variant for
/// exactly this reason, so the check is that the value is *that* variant —
/// anything else is someone trying to relax a gate through the back door.
fn check_tightening(spec: &SettingSpec, value: &str) -> Result<()> {
    let Backend::DaftYml {
        tighten_only: true,
        path,
    } = spec.backend
    else {
        return Ok(());
    };

    let allowed: Vec<&str> = spec
        .ty
        .variants()
        .map(|variants| variants.iter().map(|(v, _)| *v).collect())
        .unwrap_or_default();

    if allowed.iter().any(|v| v.eq_ignore_ascii_case(value)) {
        return Ok(());
    }

    bail!(
        "{path} is a merge-gate policy — it can only be tightened, to {}. \
         Relaxing it is a per-invocation decision, not a config change.",
        allowed.join(" or ")
    )
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
    check_tightening(spec, &value)?;
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
        Backend::LayoutChain => set_layout(scope, &value)?,
    }

    Ok(format!(
        "Set {} = {value} ({})",
        spec.key,
        scope.label_for(spec)
    ))
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
        Backend::LayoutChain => unset_layout(scope)?,
    };

    Ok(if removed {
        format!("Unset {} ({})", spec.key, scope.label_for(spec))
    } else {
        format!("{} was not set ({})", spec.key, scope.label_for(spec))
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
    fn a_merge_gate_can_be_tightened_but_not_relaxed() {
        // The gate keys carry one variant precisely so an overlay cannot
        // switch them off. The check has to consult `tighten_only`, not just
        // the type — the type would accept "only" at either file, and the
        // point is that "off" is not a value at all.
        let spec = find("merge.ff").unwrap();

        // The tightening value passes both checks and then fails on the
        // backend, which is what tells us it got that far.
        let err = set(&spec, WriteScope::Local, "only", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("daft.yml"),
            "tightening should reach the writer: {err}"
        );

        // Anything else is refused. Today the type gets there first, because
        // these enums carry one variant for exactly this reason.
        assert!(set(&spec, WriteScope::Local, "off", &empty_config()).is_err());

        // And the policy check stands behind it, so widening the enum later
        // cannot quietly make a gate relaxable.
        assert!(check_tightening(&spec, "only").is_ok());
        let err = check_tightening(&spec, "any").unwrap_err().to_string();
        assert!(err.contains("only be tightened"), "unhelpful: {err}");
        assert!(err.contains("merge.ff"), "the message names the policy");
    }

    #[test]
    fn an_ordinary_yml_row_has_no_tightening_rule() {
        let spec = find("log.retention").unwrap();
        let err = set(&spec, WriteScope::Local, "14d", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("daft.yml") && !err.contains("tightened"),
            "unhelpful: {err}"
        );
    }

    #[test]
    fn a_daft_yml_row_names_the_file_rather_than_failing_obscurely() {
        let spec = find("log.retention").unwrap();
        let err = set(&spec, WriteScope::Local, "14d", &empty_config())
            .unwrap_err()
            .to_string();
        assert!(err.contains("daft.yml"), "unhelpful: {err}");
    }

    /// The layout row writes real stores — the global TOML and the trust
    /// database — so there is deliberately no unit test that calls `set` on
    /// it. A test that did would edit the developer's own config the moment
    /// it ran outside a sandbox. Its write path is covered where the state
    /// dirs are redirected: the integration suite.
    ///
    /// What is safe to assert here is the shape: the row is writable, and it
    /// is not a git-config key.
    #[test]
    fn the_layout_row_is_writable_but_is_not_a_git_config_key() {
        let spec = find("layout").unwrap();
        assert!(spec.is_writable());
        assert_eq!(
            spec.backend,
            crate::core::settings_spec::Backend::LayoutChain
        );
        assert!(!spec.key.starts_with("daft."));
    }
}
