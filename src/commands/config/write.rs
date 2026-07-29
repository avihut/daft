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
use crate::core::settings_spec::{
    Backend, BehaviorSpec, Preset, SettingLookup, SettingSpec, ValueType, parse_git_bool,
};
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
            // Neither daft.yml file is user-wide: "global" here is the
            // repository's *committed* config, which every clone gets and
            // which lands in the writer's diff. Calling that "global" would
            // tell someone reaching for a personal preference that they had
            // set one, when they had in fact edited the team's file.
            (Backend::DaftYml { .. }, Self::Global) => "committed config",
            (Backend::DaftYml { .. }, Self::Local) => "local overlay",
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

/// Edit a scalar in one of the two `daft.yml` files. Returns whether
/// anything changed.
///
/// `daft.yml` is tracked, so a write here lands in the user's diff. Two things
/// follow from that: the edit is line surgery rather than a serializer
/// round-trip (see [`yaml_scalar_edit`]), and the file is replaced atomically
/// with its permissions preserved, so a crash mid-write cannot leave a
/// half-written config that fails to parse on the next hook.
fn write_yaml(scope: WriteScope, path: &str, value: Option<&str>) -> Result<bool> {
    use crate::hooks::{yaml_config_loader, yaml_scalar_edit};

    let worktree = crate::get_current_worktree_path()
        .context("Not inside a git repository — there is no daft.yml to edit")?;

    let main = yaml_config_loader::find_config_file(&worktree).map(|(path, _)| path);
    let target = match scope {
        // The overlay: this machine only, and created on demand. Its name
        // follows the main config's stem, so a repo using `daft.yaml` gets
        // `daft.local.yaml`.
        WriteScope::Local => match &main {
            Some(main) => {
                yaml_config_loader::find_local_config(main).unwrap_or_else(|| local_sibling(main))
            }
            // The overlay is discovered *from* the main config — the loader
            // derives its name from that file's stem and bails before the
            // merge when there is none. Writing one anyway produces a file
            // daft will never read, reported as a success. Refuse instead,
            // and only for a set: an unset with no file to edit is already a
            // truthful "was not set".
            None if value.is_some() => bail!(
                "This repository has no daft.yml, so there is nothing for a local \
                 overlay to override — daft would never read the file.\n\
                 Create daft.yml first, or use --global to write it there."
            ),
            None => worktree.join("daft.local.yml"),
        },
        // The committed convention.
        WriteScope::Global => main.clone().unwrap_or_else(|| worktree.join("daft.yml")),
    };

    let before = std::fs::read_to_string(&target).unwrap_or_default();
    let after = match value {
        Some(value) => Some(yaml_scalar_edit::set_scalar(&before, path, value)?),
        None => yaml_scalar_edit::unset_scalar(&before, path)?,
    };
    let Some(after) = after else {
        return Ok(false);
    };

    write_atomically(&target, &after)?;
    Ok(true)
}

/// `daft.yml` → `daft.local.yml`, keeping whichever extension is in use.
fn local_sibling(main: &std::path::Path) -> std::path::PathBuf {
    let extension = main
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "yml".to_string());
    main.with_file_name(format!("daft.local.{extension}"))
}

/// Replace a file's contents through a temporary in the same directory, so a
/// reader never sees a partial write.
fn write_atomically(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::io::Write;

    let directory = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::create_dir_all(directory).ok();

    let mut temp = tempfile::Builder::new()
        .prefix(".daft-yml-")
        .tempfile_in(directory)
        .with_context(|| format!("Failed to stage a write next to {}", path.display()))?;
    temp.write_all(contents.as_bytes())?;
    temp.flush()?;

    // Keep the file's own permissions: a repo may have tightened them, and a
    // rename would otherwise install the tempfile's 0600.
    #[cfg(unix)]
    if let Ok(existing) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mode = existing.permissions().mode();
        let _ = std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(mode));
    }

    temp.persist(path)
        .map_err(|e| anyhow::anyhow!("Failed to replace {}: {e}", path.display()))?;
    Ok(())
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

/// Refuse to delete a policy an overlay may only tighten.
///
/// [`check_tightening`] guards the value; this guards its absence. Removing
/// `merge.ff: only` leaves no policy at all, which is the same hole as setting
/// it to `any` and reached with one fewer keystroke — so `set` refusing to
/// relax the gate while `unset` deleted it outright would be a guard in name
/// only. Refused at both scopes: the committed line is the boundary itself,
/// and the overlay cannot hold a gate it is not allowed to loosen. Retiring a
/// gate is a reviewed edit to the tracked file, not a config operation.
fn check_removable(spec: &SettingSpec) -> Result<()> {
    let Backend::DaftYml {
        tighten_only: true,
        path,
    } = spec.backend
    else {
        return Ok(());
    };

    bail!(
        "{path} is a merge-gate policy — removing it would drop the gate entirely. \
         Retiring it is an edit to the file that declares it."
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

    store(spec, scope, &value)?;

    Ok(format!(
        "Set {} = {value} ({})",
        spec.key,
        scope.label_for(spec)
    ))
}

/// Put an already-validated value in its backend.
///
/// Split out so a behavior applying several members at once goes through the
/// identical write for each, rather than a second dispatch that could drift
/// from this one.
fn store(spec: &SettingSpec, scope: WriteScope, value: &str) -> Result<()> {
    let git = GitCommand::new(false);
    match spec.backend {
        Backend::GitConfig => match scope {
            WriteScope::Global => git.config_set_global(&spec.key, value)?,
            WriteScope::Local => git.config_set(&spec.key, value)?,
        },
        Backend::DaftYml { path, .. } => {
            write_yaml(scope, path, Some(value))?;
        }
        Backend::LayoutChain => set_layout(scope, value)?,
    }
    Ok(())
}

/// Remove `spec`'s value at `scope`, returning the line to narrate.
pub fn unset(spec: &SettingSpec, scope: WriteScope) -> Result<String> {
    check_writable(spec, scope)?;
    check_removable(spec)?;

    let git = GitCommand::new(false);
    let removed = match spec.backend {
        Backend::GitConfig => match scope {
            WriteScope::Global => git.config_unset_global(&spec.key)?,
            WriteScope::Local => git.config_unset(&spec.key)?,
        },
        Backend::DaftYml { path, .. } => write_yaml(scope, path, None)?,
        Backend::LayoutChain => unset_layout(scope)?,
    };

    Ok(if removed {
        format!("Unset {} ({})", spec.key, scope.label_for(spec))
    } else {
        format!("{} was not set ({})", spec.key, scope.label_for(spec))
    })
}

// ─────────────────────────────────────────────────────────────────────────
// Behaviors
// ─────────────────────────────────────────────────────────────────────────

/// What a behavior write did, and the configuration it left behind.
pub struct BehaviorWrite {
    /// The line — or lines — to narrate.
    pub message: String,
    /// Everything re-resolved after the write, so a caller that needs to
    /// redraw does not pay for a second resolution.
    pub config: ResolvedSet,
}

/// The configuration as it *would* read once a preset is applied.
///
/// Cross-key validators ask what other settings resolve to. A preset changes
/// several at once, so validating each member against the *current*
/// configuration judges it against values this same write is about to replace
/// — and would refuse a destination that is only contradictory half-way there.
struct Candidate<'a> {
    base: &'a ResolvedSet,
    pending: &'a [(SettingSpec, String)],
}

impl SettingLookup for Candidate<'_> {
    fn effective(&self, key: &str) -> Option<String> {
        self.pending
            .iter()
            .find(|(spec, _)| spec.key == key)
            .map(|(_, value)| value.clone())
            .or_else(|| self.base.effective(key))
    }
}

/// Apply every member of `preset` at `scope`.
///
/// Validates the whole destination before writing anything, so a preset that
/// would land in a state daft refuses is turned away with nothing touched. The
/// writes themselves are sequential and cannot be rolled back — a failure
/// part-way through says so, and says which members landed, because the
/// behavior is then genuinely Custom rather than either of its presets.
pub fn set_behavior(
    behavior: &BehaviorSpec,
    preset: &Preset,
    scope: WriteScope,
    config: &ResolvedSet,
    reresolve: impl FnOnce() -> Result<ResolvedSet>,
) -> Result<BehaviorWrite> {
    let mut plan: Vec<(SettingSpec, String)> = Vec::with_capacity(behavior.members.len());
    for key in behavior.members {
        let spec = crate::core::settings_spec::find(key)
            .with_context(|| format!("{}: member {key} is not a known setting", behavior.name))?;
        let raw = preset.value_for(key).with_context(|| {
            format!(
                "{}/{}: no value for member {key}",
                behavior.name, preset.name
            )
        })?;

        check_writable(&spec, scope)?;
        let value = canonical_value(&spec.ty, raw);
        if let Err(reason) = spec.ty.validate(&value) {
            bail!("{key}: {reason}");
        }
        check_tightening(&spec, &value)?;
        plan.push((spec, value));
    }

    // Every member judged against the whole destination, only once the whole
    // destination is known.
    let candidate = Candidate {
        base: config,
        pending: &plan,
    };
    for (spec, value) in &plan {
        if let Some(validate) = spec.validate
            && let Err(reason) = validate(&candidate, value)
        {
            bail!("{} cannot be applied: {reason}", behavior.name);
        }
    }

    for (done, (spec, value)) in plan.iter().enumerate() {
        store(spec, scope, value).map_err(|error| {
            let landed: Vec<&str> = plan[..done].iter().map(|(s, _)| s.key.as_ref()).collect();
            anyhow::anyhow!(
                "{error}\n\n{} is now half-applied: {}. It reads Custom until the \
                 rest land — re-run to finish, or set the members individually.",
                behavior.name,
                if landed.is_empty() {
                    "nothing was written".to_string()
                } else {
                    format!("wrote {}", landed.join(", "))
                }
            )
        })?;
    }

    let fresh = reresolve()?;
    let action = format!(
        "Set {} = {} ({}) — {} keys",
        behavior.name,
        preset.label,
        scope.label(),
        plan.len()
    );
    Ok(BehaviorWrite {
        message: with_resulting_state(&fresh, behavior, &action),
        config: fresh,
    })
}

/// Remove every member of `behavior` at `scope`.
///
/// Unsetting is not the same as applying the default preset, and the narration
/// has to say where it actually landed: clearing three local keys can *reveal*
/// a global Full sync, which is the opposite of what "unset remote sync"
/// sounds like.
pub fn unset_behavior(
    behavior: &BehaviorSpec,
    scope: WriteScope,
    reresolve: impl FnOnce() -> Result<ResolvedSet>,
) -> Result<BehaviorWrite> {
    let mut specs = Vec::with_capacity(behavior.members.len());
    for key in behavior.members {
        let spec = crate::core::settings_spec::find(key)
            .with_context(|| format!("{}: member {key} is not a known setting", behavior.name))?;
        check_writable(&spec, scope)?;
        check_removable(&spec)?;
        specs.push(spec);
    }

    let git = GitCommand::new(false);
    let mut removed = Vec::new();
    for spec in &specs {
        let gone = match spec.backend {
            Backend::GitConfig => match scope {
                WriteScope::Global => git.config_unset_global(&spec.key)?,
                WriteScope::Local => git.config_unset(&spec.key)?,
            },
            Backend::DaftYml { path, .. } => write_yaml(scope, path, None)?,
            Backend::LayoutChain => unset_layout(scope)?,
        };
        if gone {
            removed.push(spec.key.as_ref());
        }
    }

    let fresh = reresolve()?;
    let action = if removed.is_empty() {
        format!("{} was not set ({})", behavior.name, scope.label())
    } else {
        format!(
            "Unset {} ({}) — {} of {} keys",
            behavior.name,
            scope.label(),
            removed.len(),
            specs.len()
        )
    };
    Ok(BehaviorWrite {
        message: with_resulting_state(&fresh, behavior, &action),
        config: fresh,
    })
}

/// Append where the behavior actually landed, whenever that is not what the
/// action implies.
///
/// Two ways a write lands somewhere other than where it points, and both are
/// silent without this: a `--global` write that a local value still outranks,
/// and an unset that reveals a value from the scope below.
fn with_resulting_state(fresh: &ResolvedSet, behavior: &BehaviorSpec, action: &str) -> String {
    let Some(resolved) = fresh.behavior(behavior.name) else {
        return action.to_string();
    };

    let state = match resolved.divergence_note(&fresh.settings) {
        Some(note) => format!("Custom — {note}"),
        None => resolved.state_label().to_string(),
    };

    format!("{action}\n{} now reads {state}", behavior.name)
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

    fn config_with(pairs: &[(&str, &str)]) -> ResolvedSet {
        resolve_all(&Snapshot {
            in_repo: true,
            entries: pairs
                .iter()
                .map(|(key, value)| crate::git::ConfigEntry {
                    key: (*key).to_string(),
                    value: (*value).to_string(),
                    scope: ConfigScope::Local,
                    origin_path: None,
                })
                .collect(),
            ..Default::default()
        })
    }

    // ── Behaviors ────────────────────────────────────────────────────────

    /// The reason a preset validates against its own destination rather than
    /// against what is currently set.
    ///
    /// `merge.commit = false` is incompatible with `cleanup = remove-branch`.
    /// A preset moving *both* to a compatible pair is a legal destination —
    /// but judged one key at a time against the current configuration, the
    /// first member is refused for contradicting a value the same write is
    /// about to replace.
    #[test]
    fn a_preset_is_validated_against_where_it_lands_not_where_it_starts() {
        let base = config_with(&[(keys::MERGE_CLEANUP, "remove-branch")]);
        let commit = find(keys::MERGE_COMMIT).unwrap();
        let validate = commit.validate.expect("merge.commit is cross-key checked");

        // One key at a time, against what is set today: refused.
        assert!(
            validate(&base, "false").is_err(),
            "the pair really is incompatible as it stands"
        );

        // The same value, judged against the whole destination: fine.
        let plan = vec![
            (commit.clone(), "false".to_string()),
            (find(keys::MERGE_CLEANUP).unwrap(), "keep".to_string()),
        ];
        let candidate = Candidate {
            base: &base,
            pending: &plan,
        };
        assert!(
            validate(&candidate, "false").is_ok(),
            "a preset that moves both keys never passes through the bad pair"
        );
    }

    #[test]
    fn the_candidate_falls_through_to_the_real_configuration() {
        let base = config_with(&[(keys::MERGE_CLEANUP, "remove-branch")]);
        let plan = vec![(find(keys::MERGE_COMMIT).unwrap(), "false".to_string())];
        let candidate = Candidate {
            base: &base,
            pending: &plan,
        };

        assert_eq!(
            candidate.effective(keys::MERGE_COMMIT).as_deref(),
            Some("false")
        );
        assert_eq!(
            candidate.effective(keys::MERGE_CLEANUP).as_deref(),
            Some("remove-branch"),
            "a key the preset does not touch still reads from the real config"
        );
    }

    /// The narration's whole job: a write that points one way and lands
    /// another has to say so. `--global` under a local value is the case that
    /// used to lie.
    #[test]
    fn the_narration_reports_where_the_behavior_actually_landed() {
        let behavior = crate::core::settings_spec::find_behavior("remote-sync").unwrap();

        let full = config_with(&[
            (keys::CHECKOUT_FETCH, "true"),
            (keys::CHECKOUT_PUSH, "true"),
            (keys::BRANCH_DELETE_REMOTE, "true"),
        ]);
        let message = with_resulting_state(&full, behavior, "Set remote-sync = Full sync (global)");
        assert_eq!(
            message,
            "Set remote-sync = Full sync (global)\nremote-sync now reads Full sync"
        );

        // The same action, but a member the write did not reach still holds
        // the old value — the resulting line is what tells the user.
        let partial = config_with(&[
            (keys::CHECKOUT_FETCH, "true"),
            (keys::CHECKOUT_PUSH, "false"),
            (keys::BRANCH_DELETE_REMOTE, "true"),
        ]);
        let message =
            with_resulting_state(&partial, behavior, "Set remote-sync = Full sync (global)");
        assert!(
            message
                .contains("now reads Custom — closest to Full sync — daft.checkout.push is false"),
            "the resulting state must name what is out of step: {message}"
        );
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
    fn neither_daft_yml_scope_calls_itself_global() {
        // "global" means ~/.gitconfig everywhere else in this screen. For a
        // daft.yml row it would name the repository's *committed* file, so a
        // user reaching for a personal preference would be told they had set
        // one while editing the team's config.
        let spec = find("log.retention").unwrap();
        assert_eq!(WriteScope::Global.label_for(&spec), "committed config");
        assert_eq!(WriteScope::Local.label_for(&spec), "local overlay");

        // The git rows keep git's own words, where they are true.
        let git = find(keys::MERGE_STYLE).unwrap();
        assert_eq!(WriteScope::Global.label_for(&git), "global");
        assert_eq!(WriteScope::Local.label_for(&git), "local");
    }

    #[test]
    fn a_merge_gate_cannot_be_deleted_from_either_scope() {
        // `set` already refuses to relax the gate. Deleting it leaves no
        // policy at all — the same hole, one keystroke cheaper — so `unset`
        // has to refuse too, or the guard is decorative.
        let spec = find("merge.ff").unwrap();
        for scope in [WriteScope::Global, WriteScope::Local] {
            let err = unset(&spec, scope).unwrap_err().to_string();
            assert!(err.contains("merge.ff"), "unhelpful: {err}");
            assert!(
                err.contains("gate"),
                "the message must say what is being protected: {err}"
            );
        }
    }

    #[test]
    fn an_ordinary_yml_key_is_still_removable() {
        // The refusal above is about merge gates specifically. It must not
        // have quietly frozen every daft.yml row — but proving that cannot
        // run the write, which touches the worktree, so it stops at the guard.
        let spec = find("log.retention").unwrap();
        assert!(check_removable(&spec).is_ok());
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

    // No unit test calls `set` on a daft.yml row. It edits a real file in
    // whatever worktree the test happens to run in — this repository's own,
    // when run from here — and a test that did would leave a `daft.local.yml`
    // in a developer's tree. The write path is covered where the worktree is
    // a sandbox: the YAML scenarios. What is safe to assert here is the
    // policy, which is pure.

    #[test]
    fn a_merge_gate_can_be_tightened_but_not_relaxed() {
        // The gate keys carry one variant precisely so an overlay cannot
        // switch them off. Two layers enforce it: the type refuses anything
        // that is not a variant, and `check_tightening` stands behind that so
        // widening one of these enums later cannot quietly make a committed
        // boundary relaxable.
        let spec = find("merge.ff").unwrap();

        assert!(spec.ty.validate("off").is_err(), "the type refuses first");

        assert!(check_tightening(&spec, "only").is_ok());
        let err = check_tightening(&spec, "any").unwrap_err().to_string();
        assert!(err.contains("only be tightened"), "unhelpful: {err}");
        assert!(err.contains("merge.ff"), "the message names the policy");
    }

    #[test]
    fn an_ordinary_yml_row_has_no_tightening_rule() {
        let spec = find("log.retention").unwrap();
        assert!(check_tightening(&spec, "14d").is_ok());
        assert!(check_tightening(&spec, "anything at all").is_ok());
    }

    #[test]
    fn the_overlay_file_follows_the_main_config_extension() {
        // A repo that spells it `daft.yaml` gets `daft.local.yaml`, not a
        // second file with a different extension that nothing merges.
        assert_eq!(
            local_sibling(std::path::Path::new("/repo/daft.yml")),
            std::path::PathBuf::from("/repo/daft.local.yml")
        );
        assert_eq!(
            local_sibling(std::path::Path::new("/repo/daft.yaml")),
            std::path::PathBuf::from("/repo/daft.local.yaml")
        );
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
