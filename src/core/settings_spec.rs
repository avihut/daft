//! The settings registry: one machine-readable row per daft setting.
//!
//! daft's configuration knowledge used to live in three places that could
//! disagree — the `keys::` / `defaults::` const lists in
//! [`crate::core::settings`], the dozen free-standing `parse()` impls that
//! give each value its type, and the prose tables in `docs/`. Nothing could
//! answer "what is configurable?" without a human reading all three.
//!
//! This module is that answer. Every row carries the key, its type and
//! accepted values, its default, which backend stores it, and the prose a
//! user needs — enough for `daft config` to list, validate, complete, and
//! edit a setting without hardcoding it anywhere else.
//!
//! Two rules keep it honest:
//!
//! 1. **Keys are the `keys::` consts, never re-typed literals.** A renamed
//!    key breaks the build here rather than silently orphaning a row.
//! 2. **Enum value sets come from the parser's own `variants()`.** The
//!    registry and the code that parses the value cannot disagree about
//!    which spellings exist. Where two settings share a Rust type
//!    (`GovernorMode` backs both `daft.governor.mode` and
//!    `daft.governor.jobserver`), the variant gloss stays generic and the
//!    row's `help` carries the specifics.
//!
//! An xtask drift test enforces the converse — a `daft.*` key that exists in
//! the codebase but not here fails CI.
//!
//! This is a *read model*. It does not load or apply settings; the loaders in
//! [`crate::core::settings`] remain the runtime path.

use std::borrow::Cow;

use crate::core::settings::keys;
use crate::hooks::HookType;

/// Accepted values for an enum-typed setting: `(value, one-phrase gloss)`.
///
/// Always sourced from the parsing type's own `variants()` so the registry
/// cannot drift from what `parse()` accepts.
pub type Variants = &'static [(&'static str, &'static str)];

// ─────────────────────────────────────────────────────────────────────────
// Value types
// ─────────────────────────────────────────────────────────────────────────

/// Which duration spelling a setting accepts. The two dialects in daft are
/// genuinely different, and a user typing `300` at the wrong one deserves to
/// be told so rather than have it silently mean something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationDialect {
    /// Bare numbers are seconds and `off` / `0` disables the timeout —
    /// `daft.sync.pushTimeout`.
    BareSeconds,
    /// A `d` / `h` / `m` / `s` suffix is required — the `daft.yml` log
    /// durations.
    Suffixed,
}

/// The shape of a setting's value: what the TUI renders as an editor, what
/// completion offers, and what `daft config set` validates against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    /// A git boolean — `true`/`false` plus git's `yes`/`no`/`on`/`off`/`1`/`0`.
    Bool,
    /// A boolean whose *absence* is a third meaningful state, because
    /// something downstream (git, a hook) decides when daft says nothing.
    TriBool,
    /// A closed set of spellings.
    Enum(Variants),
    /// A boolean or an opaque signing-key id — `daft.merge.gpgSign`.
    BoolOrKey,
    /// Free-form text.
    Str,
    /// A filesystem path; `~` is expanded on read.
    Path,
    /// A non-negative integer.
    Int,
    /// A positive integer, or `auto` for the computed default.
    IntOrAuto,
    /// A duration in one of the two [`DurationDialect`]s.
    Duration(DurationDialect),
    /// A byte size: a plain integer, or one with a `B`/`KB`/`MB`/`GB` suffix.
    Size,
    /// A byte size, a percentage of total RAM, or `auto`.
    SizeOrPct,
    /// A comma-separated spec — column sets, sort keys, strategy options.
    ///
    /// Each spec has its own parser downstream that knows the valid tokens
    /// for its command; the registry only checks that the shape is non-empty.
    Spec,
    /// The composite worktree-layout setting: one row, six layers, three
    /// writable backends. Values are built-in layout names or an inline
    /// template.
    LayoutComposite,
}

impl ValueType {
    /// Check a candidate value, returning a user-facing reason on rejection.
    ///
    /// This is the gate every write goes through — `daft config set`, the
    /// modal editor's apply, and the S1 test that asserts each row's default
    /// parses under its own type.
    pub fn validate(&self, value: &str) -> Result<(), String> {
        match self {
            Self::Bool | Self::TriBool => parse_git_bool(value)
                .map(|_| ())
                .ok_or_else(|| "expected a boolean: true or false".to_string()),
            Self::Enum(variants) => {
                if variants
                    .iter()
                    .any(|(v, _)| v.eq_ignore_ascii_case(value.trim()))
                {
                    Ok(())
                } else {
                    let list: Vec<&str> = variants.iter().map(|(v, _)| *v).collect();
                    Err(format!("expected one of: {}", list.join(", ")))
                }
            }
            Self::BoolOrKey | Self::Str | Self::Path | Self::Spec | Self::LayoutComposite => {
                if value.trim().is_empty() {
                    Err("expected a value".to_string())
                } else {
                    Ok(())
                }
            }
            Self::Int => value
                .trim()
                .parse::<u64>()
                .map(|_| ())
                .map_err(|_| "expected a whole number".to_string()),
            Self::IntOrAuto => {
                if value.trim().eq_ignore_ascii_case("auto") {
                    Ok(())
                } else {
                    match value.trim().parse::<u64>() {
                        Ok(n) if n >= 1 => Ok(()),
                        _ => Err("expected auto or a positive whole number".to_string()),
                    }
                }
            }
            Self::Duration(DurationDialect::BareSeconds) => {
                crate::core::settings::parse_push_timeout(value)
                    .map(|_| ())
                    .ok_or_else(|| {
                        "expected a duration (30m, 2h, 90) or off to disable".to_string()
                    })
            }
            Self::Duration(DurationDialect::Suffixed) => {
                crate::coordinator::clean_policy::parse_duration_str(value)
                    .map(|_| ())
                    .map_err(|_| "expected a duration with a unit: 7d, 24h, 30m".to_string())
            }
            Self::Size => crate::coordinator::clean_policy::parse_size(value)
                .map(|_| ())
                .map_err(|_| "expected a size: 10MB, 2GB, 1024".to_string()),
            Self::SizeOrPct => crate::core::settings::MemoryReserve::parse(value)
                .map(|_| ())
                .ok_or_else(|| "expected auto, a size (2G, 512M), or a percentage".to_string()),
        }
    }

    /// The one-line format hint shown under a text editor, or `None` when the
    /// type is picked from a list and needs no hint.
    pub fn format_hint(&self) -> Option<&'static str> {
        match self {
            Self::Bool | Self::TriBool | Self::Enum(_) => None,
            Self::BoolOrKey => Some("true, false, or a signing key id"),
            Self::Str => Some("free-form text"),
            Self::Path => Some("a path; ~ is expanded"),
            Self::Int => Some("a whole number"),
            Self::IntOrAuto => Some("auto, or a positive whole number"),
            Self::Duration(DurationDialect::BareSeconds) => {
                Some("30m, 2h, 7d, bare seconds, or off")
            }
            Self::Duration(DurationDialect::Suffixed) => Some("7d, 24h, 30m — a unit is required"),
            Self::Size => Some("10MB, 2GB, or bytes"),
            Self::SizeOrPct => Some("auto, 2G, 512M, or 15%"),
            Self::Spec => Some("comma-separated; +add,-remove for a diff"),
            Self::LayoutComposite => Some("a layout name, or an inline template"),
        }
    }

    /// The accepted values, when this type is picked from a closed list.
    pub fn variants(&self) -> Option<Variants> {
        match self {
            Self::Enum(v) => Some(*v),
            Self::Bool | Self::TriBool => Some(BOOL_VARIANTS),
            _ => None,
        }
    }
}

/// The two spellings the editor offers for a boolean, so a bool row renders
/// as the same radio list an enum row does.
pub const BOOL_VARIANTS: Variants = &[("true", "enabled"), ("false", "disabled")];

/// Parse a git boolean strictly — unlike
/// [`crate::core::settings::parse_bool`], which folds an unparseable value
/// into a default because the loaders must always produce *something*. A
/// write has no such obligation and should refuse.
pub fn parse_git_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Defaults, backends, categories
// ─────────────────────────────────────────────────────────────────────────

/// What a setting does when nothing sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultDesc {
    /// A literal that parses under the row's [`ValueType`] — enforced by test.
    Fixed(&'static str),
    /// Resolved at runtime.
    ///
    /// Split in two on purpose: `value` is the spelling a user can type to
    /// ask for this back, and `rule` says what it works out to. Collapsing
    /// them into one "auto = max(2, cores/4)" string reads fine in a ladder
    /// and then leaks out of `daft config get` as a value nothing accepts.
    Computed {
        value: &'static str,
        rule: &'static str,
    },
    /// No default: unset is itself meaningful, and daft or git decides.
    Unset,
}

impl DefaultDesc {
    /// The value that applies when nothing is set — always something the user
    /// could type back. `None` when unset is the answer.
    pub fn value(&self) -> Option<&'static str> {
        match self {
            Self::Fixed(value) | Self::Computed { value, .. } => Some(value),
            Self::Unset => None,
        }
    }

    /// What a computed default works out to, for the ladder's detail line.
    pub fn rule(&self) -> Option<&'static str> {
        match self {
            Self::Computed { rule, .. } => Some(rule),
            Self::Fixed(_) | Self::Unset => None,
        }
    }
}

/// Where a setting is stored, which decides the scopes it can be written at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// `git config daft.*`: system / global / local / worktree scopes, plus
    /// read-only environment and command-line entries.
    GitConfig,
    /// A scalar in the repository's `daft.yml`, overlaid by `daft.local.yml`.
    DaftYml {
        /// Dotted path within the document — `log.retention`.
        path: &'static str,
        /// The overlay may only *tighten* this key, never relax it. The
        /// merge-gate policies: a committed boundary an untracked local file
        /// could switch off would not be a boundary.
        tighten_only: bool,
    },
    /// The six-layer worktree-layout chain, spanning three writable stores.
    LayoutChain,
}

/// The functional grouping a setting appears under. Storage is metadata —
/// people look for "the merge settings", not "the git-config settings".
///
/// Declaration order is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Checkout,
    PushSync,
    Remotes,
    Merge,
    Hooks,
    Output,
    Governor,
    Forge,
    Update,
    Layout,
    RepoFile,
}

impl Category {
    /// The rail label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Checkout => "Checkout",
            Self::PushSync => "Push & Sync",
            Self::Remotes => "Remotes",
            Self::Merge => "Merge",
            Self::Hooks => "Hooks",
            Self::Output => "Output",
            Self::Governor => "Governor",
            Self::Forge => "Forge",
            Self::Update => "Update",
            Self::Layout => "Layout",
            Self::RepoFile => "daft.yml",
        }
    }

    /// Every category, in display order.
    pub fn all() -> &'static [Category] {
        &[
            Self::Checkout,
            Self::PushSync,
            Self::Remotes,
            Self::Merge,
            Self::Hooks,
            Self::Output,
            Self::Governor,
            Self::Forge,
            Self::Update,
            Self::Layout,
            Self::RepoFile,
        ]
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Cross-key validation
// ─────────────────────────────────────────────────────────────────────────

/// What a cross-key validator may ask about the rest of the configuration.
///
/// A port, so the registry stays independent of the resolver that will
/// implement it: tests supply a map, `daft config` supplies real resolution.
pub trait SettingLookup {
    /// The effective value of another setting, as the string a user sees.
    /// `None` when nothing sets it and it has no fixed default.
    fn effective(&self, key: &str) -> Option<String>;
}

/// Rejects a candidate value, with the reason to show the user.
///
/// Runs *before* the write, against the value the user is proposing — so a
/// combination that the loaders would refuse can never be written in the
/// first place.
pub type Validator = fn(&dyn SettingLookup, &str) -> Result<(), String>;

/// The refusal shared by both halves of the merge-settings pair.
///
/// Mirrors [`crate::core::settings::validate_merge_settings`], which enforces
/// the same rule at load time — this one just gets there first, so the user
/// sees it in the editor instead of at the next `daft merge`.
fn merge_pair_ok(commit: bool, cleanup: &str) -> Result<(), String> {
    if !commit && cleanup.eq_ignore_ascii_case("remove-branch") {
        return Err(format!(
            "{} = false is incompatible with {} = remove-branch: \
             branch cleanup requires a committed merge to justify deletion",
            keys::MERGE_COMMIT,
            keys::MERGE_CLEANUP
        ));
    }
    Ok(())
}

/// Validator for `daft.merge.commit`.
fn validate_merge_commit(cfg: &dyn SettingLookup, candidate: &str) -> Result<(), String> {
    let commit = parse_git_bool(candidate).ok_or("expected a boolean: true or false")?;
    let cleanup = cfg
        .effective(keys::MERGE_CLEANUP)
        .unwrap_or_else(|| "keep".to_string());
    merge_pair_ok(commit, &cleanup)
}

/// Validator for `daft.merge.cleanup`.
fn validate_merge_cleanup(cfg: &dyn SettingLookup, candidate: &str) -> Result<(), String> {
    let commit = cfg
        .effective(keys::MERGE_COMMIT)
        .as_deref()
        .and_then(parse_git_bool)
        .unwrap_or(true);
    merge_pair_ok(commit, candidate)
}

// ─────────────────────────────────────────────────────────────────────────
// The spec
// ─────────────────────────────────────────────────────────────────────────

/// Which git-config setting a per-hook row addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSetting {
    /// `daft.hooks.<hook>.enabled`
    Enabled,
    /// `daft.hooks.<hook>.failMode`
    FailMode,
}

impl HookSetting {
    /// The git-config leaf, camelCase as git subsections require.
    fn git_suffix(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::FailMode => "failMode",
        }
    }
}

/// How a row's key is formed — its identity, separate from the key string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyForm {
    /// A fixed git-config key, one of the `keys::` consts.
    Static,
    /// `daft.hooks.<hook>.<setting>`, expanded over every [`HookType`].
    ///
    /// Mind the naming split this encodes: the **git** subsection is
    /// camelCase (`worktreePostCreate`, from [`HookType::config_key`]) while
    /// the same hook's **`daft.yml`** key is dash-case
    /// (`worktree-post-create`, from [`HookType::yaml_name`]).
    PerHook {
        hook: HookType,
        setting: HookSetting,
    },
    /// The synthetic layout row: no single backing key.
    Layout,
    /// A `daft.yml` scalar.
    Yaml,
}

/// One setting, fully described.
#[derive(Debug, Clone)]
pub struct SettingSpec {
    /// The key a user types: the git-config key, `layout`, or a `daft.yml`
    /// dotted path. Unambiguous across backends because every git key starts
    /// with `daft.` and no `daft.yml` path does.
    pub key: Cow<'static, str>,
    /// The key's identity, for consumers that must treat forms differently.
    pub form: KeyForm,
    /// The row label — what the setting is, in the user's words.
    pub label: Cow<'static, str>,
    /// One sentence: what it does, and what turning it on means.
    pub help: Cow<'static, str>,
    /// Functional grouping.
    pub category: Category,
    /// Value shape.
    pub ty: ValueType,
    /// What happens when nothing sets it.
    pub default: DefaultDesc,
    /// Where it is stored.
    pub backend: Backend,
    /// Only the global scope is ever read; a local value is inert.
    pub global_only: bool,
    /// An environment variable that outranks every config scope. Read-only —
    /// daft never writes the user's environment.
    pub env_override: Option<&'static str>,
    /// A retired spelling still honoured while this key is unset.
    pub deprecated_alias: Option<Cow<'static, str>>,
    /// The key this one falls back to when unset, rather than to its default.
    pub inherits: Option<&'static str>,
    /// Cross-key consistency check, run against the candidate before writing.
    pub validate: Option<Validator>,
    /// Owned by another command: the row renders read-only and points here.
    pub managed_by: Option<&'static str>,
}

impl SettingSpec {
    fn git(
        key: &'static str,
        label: &'static str,
        help: &'static str,
        category: Category,
        ty: ValueType,
        default: DefaultDesc,
    ) -> Self {
        Self {
            key: Cow::Borrowed(key),
            form: KeyForm::Static,
            label: Cow::Borrowed(label),
            help: Cow::Borrowed(help),
            category,
            ty,
            default,
            backend: Backend::GitConfig,
            global_only: false,
            env_override: None,
            deprecated_alias: None,
            inherits: None,
            validate: None,
            managed_by: None,
        }
    }

    /// One of the two per-hook git-config rows.
    ///
    /// Spells the key with the camelCase git subsection and the label with
    /// the dash-case hook name, because those are genuinely different
    /// surfaces — and hands the four hooks renamed in the `worktree-`
    /// migration their pre-rename key as the deprecated alias.
    fn per_hook(
        hook: HookType,
        setting: HookSetting,
        label_suffix: &str,
        help: String,
        ty: ValueType,
        default: DefaultDesc,
    ) -> Self {
        let suffix = setting.git_suffix();
        Self {
            key: Cow::Owned(keys::hooks::hook_key(hook.config_key(), suffix)),
            form: KeyForm::PerHook { hook, setting },
            label: Cow::Owned(format!("{} · {label_suffix}", hook.yaml_name())),
            help: Cow::Owned(help),
            category: Category::Hooks,
            ty,
            default,
            backend: Backend::GitConfig,
            global_only: false,
            env_override: None,
            deprecated_alias: hook
                .deprecated_config_key()
                .map(|dep| Cow::Owned(keys::hooks::hook_key(dep, suffix))),
            inherits: None,
            validate: None,
            managed_by: None,
        }
    }

    fn yml(
        path: &'static str,
        label: &'static str,
        help: &'static str,
        ty: ValueType,
        default: DefaultDesc,
    ) -> Self {
        Self {
            key: Cow::Borrowed(path),
            form: KeyForm::Yaml,
            label: Cow::Borrowed(label),
            help: Cow::Borrowed(help),
            category: Category::RepoFile,
            ty,
            default,
            backend: Backend::DaftYml {
                path,
                tighten_only: false,
            },
            global_only: false,
            env_override: None,
            deprecated_alias: None,
            inherits: None,
            validate: None,
            managed_by: None,
        }
    }

    fn global_only(mut self) -> Self {
        self.global_only = true;
        self
    }

    fn env(mut self, var: &'static str) -> Self {
        self.env_override = Some(var);
        self
    }

    fn alias(mut self, key: &'static str) -> Self {
        self.deprecated_alias = Some(Cow::Borrowed(key));
        self
    }

    fn inherits(mut self, key: &'static str) -> Self {
        self.inherits = Some(key);
        self
    }

    fn validated(mut self, validate: Validator) -> Self {
        self.validate = Some(validate);
        self
    }

    fn tighten_only(mut self) -> Self {
        if let Backend::DaftYml { path, .. } = self.backend {
            self.backend = Backend::DaftYml {
                path,
                tighten_only: true,
            };
        }
        self
    }

    fn managed_by(mut self, command: &'static str) -> Self {
        self.managed_by = Some(command);
        self
    }

    /// Whether `daft config` can write this row at all.
    pub fn is_writable(&self) -> bool {
        self.managed_by.is_none()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The table
// ─────────────────────────────────────────────────────────────────────────

/// Every setting daft has, in display order.
///
/// 63 fixed git-config keys, 14 per-hook keys expanded over the seven hook
/// types, the composite layout row, and the writable `daft.yml` scalars.
pub fn all_specs() -> Vec<SettingSpec> {
    let mut specs = git_specs();
    specs.extend(per_hook_specs());
    specs.push(layout_spec());
    specs.extend(yml_specs());
    specs
}

/// The spec for `key`, matched exactly. Case-insensitive lookup that honours
/// git's own rules is a separate concern, layered on by `daft config`.
pub fn find(key: &str) -> Option<SettingSpec> {
    all_specs().into_iter().find(|s| s.key == key)
}

fn git_specs() -> Vec<SettingSpec> {
    use Category::*;
    use DefaultDesc::{Computed, Fixed, Unset};
    use ValueType::*;

    vec![
        // ── Checkout ────────────────────────────────────────────────────
        SettingSpec::git(
            keys::AUTOCD,
            "Auto-cd into worktrees",
            "The shell wrapper cd's into worktrees daft creates.",
            Checkout,
            Bool,
            Fixed("true"),
        ),
        SettingSpec::git(
            keys::CHECKOUT_FETCH,
            "Fetch before checkout",
            "Fetch from the remote before creating a worktree, so new branches start from fresh refs.",
            Checkout,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::CHECKOUT_PUSH,
            "Push new branches",
            "Push newly created branches to the remote.",
            Checkout,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::CHECKOUT_UPSTREAM,
            "Set upstream tracking",
            "Set upstream tracking on newly created branches.",
            Checkout,
            Bool,
            Fixed("true"),
        ),
        SettingSpec::git(
            keys::CHECKOUT_CARRY,
            "Carry changes (checkout)",
            "Carry uncommitted changes over when checking out an existing branch.",
            Checkout,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::CHECKOUT_BRANCH_CARRY,
            "Carry changes (new branch)",
            "Carry uncommitted changes into a newly created branch.",
            Checkout,
            Bool,
            Fixed("true"),
        ),
        SettingSpec::git(
            keys::GO_AUTO_START,
            "Auto-start on go",
            "Create the worktree when daft go names a branch that does not exist yet.",
            Checkout,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::GO_FETCH_ON_MISS,
            "Fetch on completion miss",
            "Tab completion fetches when a typed prefix matches no local branch.",
            Checkout,
            Bool,
            Fixed("true"),
        ),
        SettingSpec::git(
            keys::START_FORK_NAMING,
            "Fork naming",
            "How daft start --fork names anonymous worktrees.",
            Checkout,
            Enum(crate::core::settings::ForkNaming::variants()),
            Fixed("derived"),
        ),
        // ── Push & Sync ─────────────────────────────────────────────────
        SettingSpec::git(
            keys::PUSH_VERIFY,
            "Push verify",
            "Whether pushes that are provably ref-only still run the repository's pre-push hook.",
            PushSync,
            Enum(crate::core::settings::PushVerify::variants()),
            Fixed("auto"),
        ),
        SettingSpec::git(
            keys::CHECKOUT_PUSH_VERIFY,
            "Push verify (checkout)",
            "Override of daft.pushVerify for the upstream push during branch creation.",
            PushSync,
            Enum(crate::core::settings::PushVerify::variants()),
            Unset,
        )
        .inherits(keys::PUSH_VERIFY),
        SettingSpec::git(
            keys::SYNC_PUSH_TIMEOUT,
            "Push timeout",
            "Wall-clock budget per push unit; 0 or off disables the timeout.",
            PushSync,
            Duration(DurationDialect::BareSeconds),
            Fixed("30m"),
        ),
        SettingSpec::git(
            keys::SYNC_PUSH_HOOK_STRATEGY,
            "Push hook strategy",
            "Pre-push hook cadence when sync pushes many branches.",
            PushSync,
            Enum(crate::core::settings::PushHookStrategy::variants()),
            Fixed("per-branch"),
        ),
        SettingSpec::git(
            keys::BRANCH_DELETE_REMOTE,
            "Delete remote branches",
            "Also delete the remote branch when removing a branch.",
            PushSync,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::OWNERSHIP_STRATEGY,
            "Ownership strategy",
            "How commit authorship maps to a worktree's owner.",
            PushSync,
            Enum(crate::core::ownership::OwnershipStrategy::variants()),
            Fixed("recency-plurality"),
        ),
        // ── Remotes ─────────────────────────────────────────────────────
        SettingSpec::git(
            keys::REMOTE,
            "Default remote",
            "Default remote daft talks to.",
            Remotes,
            Str,
            Fixed("origin"),
        ),
        SettingSpec::git(
            keys::multi_remote::ENABLED,
            "Multi-remote mode",
            "Organize worktrees per remote, for fork-based workflows.",
            Remotes,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::multi_remote::DEFAULT_REMOTE,
            "Multi-remote default",
            "Remote used when none is named in multi-remote mode.",
            Remotes,
            Str,
            Fixed("origin"),
        ),
        // ── Merge ───────────────────────────────────────────────────────
        SettingSpec::git(
            keys::MERGE_STYLE,
            "Merge style",
            "How daft merge lands the source branch.",
            Merge,
            Enum(crate::core::worktree::merge::MergeStyle::variants()),
            Fixed("merge"),
        ),
        SettingSpec::git(
            keys::MERGE_CLEANUP,
            "After-merge cleanup",
            "What happens to the source branch after a successful merge.",
            Merge,
            Enum(crate::core::worktree::merge::CleanupKind::variants()),
            Fixed("keep"),
        )
        .validated(validate_merge_cleanup),
        SettingSpec::git(
            keys::MERGE_COMMIT,
            "Create merge commit",
            "Create the commit; false stages the merge without committing.",
            Merge,
            Bool,
            Fixed("true"),
        )
        .validated(validate_merge_commit),
        SettingSpec::git(
            keys::MERGE_EDIT,
            "Edit merge message",
            "Open the editor for the merge message. Unset lets git decide.",
            Merge,
            TriBool,
            Unset,
        ),
        SettingSpec::git(
            keys::MERGE_SIGNOFF,
            "Sign-off",
            "Add Signed-off-by to merge commits.",
            Merge,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::MERGE_GPG_SIGN,
            "GPG-sign merges",
            "Sign merge commits: on, off, or a specific key id.",
            Merge,
            BoolOrKey,
            Unset,
        ),
        SettingSpec::git(
            keys::MERGE_VERIFY_SIGNATURES,
            "Verify signatures",
            "Verify the source tip's signature before merging.",
            Merge,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::MERGE_ALLOW_UNRELATED_HISTORIES,
            "Allow unrelated histories",
            "Permit merging histories without a common ancestor.",
            Merge,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::MERGE_STRATEGY,
            "Merge strategy",
            "Merge strategy passed to git (ort, octopus, resolve).",
            Merge,
            Str,
            Unset,
        ),
        SettingSpec::git(
            keys::MERGE_STRATEGY_OPTION,
            "Strategy options",
            "Comma-separated strategy options, each passed to git as -X.",
            Merge,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::MERGE_ADOPT_TARGET_ON_DEMAND,
            "Adopt target on demand",
            "Create the target branch's worktree on demand when it is missing.",
            Merge,
            Enum(crate::core::worktree::merge::AdoptPreset::variants()),
            Fixed("prompt"),
        ),
        SettingSpec::git(
            keys::MERGE_REQUIRE_CLEAN_TARGET,
            "Require clean target",
            "Refuse to merge into a dirty target worktree.",
            Merge,
            Bool,
            Fixed("true"),
        ),
        // ── Hooks ───────────────────────────────────────────────────────
        SettingSpec::git(
            keys::hooks::ENABLED,
            "Hooks enabled",
            "Master switch for lifecycle hooks.",
            Hooks,
            Bool,
            Fixed("true"),
        ),
        SettingSpec::git(
            keys::hooks::DEFAULT_TRUST,
            "Default trust",
            "Trust level for repositories with no explicit trust decision.",
            Hooks,
            Enum(crate::hooks::TrustLevel::variants()),
            Fixed("deny"),
        ),
        SettingSpec::git(
            keys::hooks::USER_DIRECTORY,
            "User hooks directory",
            "Directory of user-level hooks that run for every repository.",
            Hooks,
            Path,
            Computed {
                value: "~/.config/daft/hooks",
                rule: "the XDG config directory",
            },
        ),
        SettingSpec::git(
            keys::hooks::TIMEOUT,
            "Hook timeout",
            "Seconds a hook may run before daft gives up.",
            Hooks,
            Int,
            Fixed("300"),
        ),
        SettingSpec::git(
            keys::hooks::TRUST_PRUNE,
            "Trust auto-prune",
            "Background-prune trust entries for repositories that no longer exist.",
            Hooks,
            Bool,
            Fixed("true"),
        )
        .global_only(),
        SettingSpec::git(
            keys::hooks::OUTPUT_QUIET,
            "Quiet hook output",
            "Suppress hook output unless a hook fails.",
            Hooks,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::hooks::OUTPUT_TIMER_DELAY,
            "Timer delay",
            "Seconds before a running hook shows its elapsed timer.",
            Hooks,
            Int,
            Fixed("5"),
        ),
        SettingSpec::git(
            keys::hooks::OUTPUT_TAIL_LINES,
            "Tail lines",
            "Trailing output lines shown per running hook; 0 hides them.",
            Hooks,
            Int,
            Fixed("6"),
        ),
        SettingSpec::git(
            keys::hooks::OUTPUT_VERBOSE,
            "Verbose hook output",
            "Stream full hook output instead of the live summary.",
            Hooks,
            Bool,
            Fixed("false"),
        ),
        SettingSpec::git(
            keys::hooks::OUTPUT_PARSE_MANAGERS,
            "Parse hook managers",
            "Render lefthook and husky output as first-class structured hook rows.",
            Hooks,
            Bool,
            Fixed("true"),
        ),
        // ── Output ──────────────────────────────────────────────────────
        SettingSpec::git(
            keys::LIST_STAT,
            "list · change stat",
            "Change-stat detail in list tables.",
            Output,
            Enum(crate::core::worktree::list::Stat::variants()),
            Fixed("summary"),
        ),
        SettingSpec::git(
            keys::SYNC_STAT,
            "sync · change stat",
            "Change-stat detail in sync tables.",
            Output,
            Enum(crate::core::worktree::list::Stat::variants()),
            Fixed("summary"),
        ),
        SettingSpec::git(
            keys::PRUNE_STAT,
            "prune · change stat",
            "Change-stat detail in prune tables.",
            Output,
            Enum(crate::core::worktree::list::Stat::variants()),
            Fixed("summary"),
        ),
        SettingSpec::git(
            keys::LIST_COLUMNS,
            "list · columns",
            "Column spec: a full list (branch,path,age) or a diff (+size,-annotation).",
            Output,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::SYNC_COLUMNS,
            "sync · columns",
            "Column spec for sync tables.",
            Output,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::PRUNE_COLUMNS,
            "prune · columns",
            "Column spec for prune tables.",
            Output,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::LIST_SORT,
            "list · sort",
            "Sort spec, such as +name, -activity, or +owner,-size.",
            Output,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::SYNC_SORT,
            "sync · sort",
            "Sort spec for sync tables.",
            Output,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::PRUNE_SORT,
            "prune · sort",
            "Sort spec for prune tables.",
            Output,
            Spec,
            Unset,
        ),
        SettingSpec::git(
            keys::LIST_SIZE_CONCURRENCY,
            "Size-walk concurrency",
            "Parallel jobs for the worktree size walk.",
            Output,
            IntOrAuto,
            Computed {
                value: "auto",
                rule: "the CPU count",
            },
        )
        .env(crate::core::size_walk::JOBS_ENV),
        SettingSpec::git(
            keys::PRUNE_CD_TARGET,
            "Prune cd target",
            "Where your shell lands when prune removes the worktree you are standing in.",
            Output,
            Enum(crate::core::settings::PruneCdTarget::variants()),
            Fixed("root"),
        ),
        SettingSpec::git(
            keys::completions::BRANCHES_COLUMNS,
            "Completion columns",
            "Columns shown in rich branch tab-completion; unset uses the built-in set.",
            Output,
            Spec,
            Unset,
        ),
        // ── Governor ────────────────────────────────────────────────────
        SettingSpec::git(
            keys::GOVERNOR_MODE,
            "Governor",
            "Resource governor for parallel sync pushes.",
            Governor,
            Enum(crate::core::settings::GovernorMode::variants()),
            Fixed("auto"),
        ),
        SettingSpec::git(
            keys::GOVERNOR_JOBS,
            "Governor jobs",
            "Maximum concurrent hook-bearing pushes.",
            Governor,
            IntOrAuto,
            Computed {
                value: "auto",
                rule: "max(2, cores/4)",
            },
        ),
        SettingSpec::git(
            keys::GOVERNOR_MEMORY_RESERVE,
            "Memory reserve",
            "Memory headroom the governor keeps free, as a size or a percentage.",
            Governor,
            SizeOrPct,
            Computed {
                value: "auto",
                rule: "max(10% of RAM, 2G)",
            },
        ),
        SettingSpec::git(
            keys::GOVERNOR_JOBSERVER,
            "Jobserver",
            "Export a shared POSIX jobserver to hooks through MAKEFLAGS.",
            Governor,
            Enum(crate::core::settings::GovernorMode::variants()),
            Fixed("auto"),
        ),
        // ── Forge ───────────────────────────────────────────────────────
        SettingSpec::git(
            keys::FORGE_PLATFORM,
            "Forge platform",
            "Force the forge platform when the remote is ambiguous; unset detects it from the remote.",
            Forge,
            Enum(crate::forge::PLATFORM_VARIANTS),
            Unset,
        ),
        SettingSpec::git(
            keys::FORGE_GITHUB_CLI,
            "GitHub CLI",
            "Override the gh binary, for Enterprise wrappers.",
            Forge,
            Path,
            Fixed("gh"),
        ),
        SettingSpec::git(
            keys::FORGE_GITLAB_CLI,
            "GitLab CLI",
            "Override the glab binary.",
            Forge,
            Path,
            Fixed("glab"),
        ),
        SettingSpec::git(
            keys::FORGE_HOSTNAME,
            "Forge hostname",
            "Self-hosted forge hostname, passed to the CLI as --hostname.",
            Forge,
            Str,
            Unset,
        ),
        // ── Update ──────────────────────────────────────────────────────
        SettingSpec::git(
            keys::UPDATE_CHECK,
            "Update check",
            "Background check for new daft releases.",
            Update,
            Bool,
            Fixed("true"),
        )
        .global_only(),
        SettingSpec::git(
            keys::UPDATE_ARGS,
            "Update pull args",
            "Arguments daft update passes to git pull.",
            Update,
            Str,
            Fixed("--ff-only"),
        )
        .alias(keys::FETCH_ARGS_DEPRECATED),
        SettingSpec::git(
            keys::GITOXIDE,
            "gitoxide backend",
            "In-process gitoxide backend; off forces git subprocesses.",
            Update,
            Bool,
            Fixed("true"),
        ),
    ]
}

/// The two per-hook git-config settings, expanded over every hook type.
///
/// Seven hook types × two settings = the 14 dynamic keys. The four renamed
/// hooks still honour their pre-`worktree-` spelling while the new key is
/// unset, which the row records as its deprecated alias.
fn per_hook_specs() -> Vec<SettingSpec> {
    let mut specs = Vec::with_capacity(HookType::all().len() * 2);

    for &hook in HookType::all() {
        let name = hook.yaml_name();

        specs.push(SettingSpec::per_hook(
            hook,
            HookSetting::Enabled,
            "enabled",
            format!("Toggle the {name} hook without deleting its definition."),
            ValueType::Bool,
            DefaultDesc::Fixed("true"),
        ));

        specs.push(SettingSpec::per_hook(
            hook,
            HookSetting::FailMode,
            "fail mode",
            format!(
                "Whether a failing {name} hook aborts. Git config beats a committed fail_mode."
            ),
            ValueType::Enum(crate::hooks::FailMode::variants()),
            DefaultDesc::Fixed(hook.default_fail_mode().as_str()),
        ));
    }

    specs
}

/// The composite worktree-layout row.
///
/// Not a git-config key: six layers across three stores decide where
/// worktrees land, and the row exists so a user can see that whole chain —
/// including the repo store outranking the committed `daft.yml` — in one
/// place. The `daft.yml layout:` key is one of its rungs, not a row of its
/// own.
fn layout_spec() -> SettingSpec {
    let mut spec = SettingSpec::git(
        "layout",
        "Worktree layout",
        "Where worktrees are placed, resolved across six layers.",
        Category::Layout,
        ValueType::LayoutComposite,
        DefaultDesc::Fixed("sibling"),
    );
    spec.form = KeyForm::Layout;
    spec.backend = Backend::LayoutChain;
    spec
}

/// The writable `daft.yml` scalars, plus the one display-only row.
///
/// Structured blocks — `hooks:`, `tasks:`, `relations:`, `extends:` — keep
/// their own commands and are deliberately absent.
fn yml_specs() -> Vec<SettingSpec> {
    use DefaultDesc::{Fixed, Unset};
    use ValueType::*;

    vec![
        SettingSpec::yml(
            "rc",
            "Hook rc file",
            "Shell rc file sourced before every hook runs.",
            Str,
            Unset,
        ),
        SettingSpec::yml(
            "source_dir",
            "Hook source dir",
            "Directory of committed hook scripts.",
            Path,
            Fixed(".daft"),
        ),
        SettingSpec::yml(
            "shared",
            "Shared files",
            "Files symlinked across every worktree, such as .env or .idea.",
            Spec,
            Unset,
        )
        .managed_by("daft shared"),
        SettingSpec::yml(
            "log.retention",
            "Log retention",
            "How long finished job logs are kept.",
            Duration(DurationDialect::Suffixed),
            Fixed("7d"),
        ),
        SettingSpec::yml(
            "log.max_log_size",
            "Log size cap",
            "Per-log size cap; unset keeps logs whole.",
            Size,
            Unset,
        ),
        SettingSpec::yml(
            "log.max_total_size",
            "Log budget",
            "Total bytes all of this repository's job logs may occupy.",
            Size,
            Fixed("500MB"),
        ),
        SettingSpec::yml(
            "log.keep_last",
            "Log floor",
            "Invocations always retained per worktree, whatever retention says.",
            Int,
            Fixed("3"),
        ),
        SettingSpec::yml(
            "log.stale_running_after",
            "Stale-running cutoff",
            "A running job older than this with no live coordinator counts as cancelled.",
            Duration(DurationDialect::Suffixed),
            Fixed("24h"),
        ),
        SettingSpec::yml(
            "merge.ff",
            "Gate · fast-forward only",
            "Committed merge-gate policy; an overlay may only tighten it.",
            Enum(&[("only", "refuse merges that cannot fast-forward")]),
            Unset,
        )
        .tighten_only(),
        SettingSpec::yml(
            "merge.source_worktree",
            "Gate · source clean",
            "Committed merge-gate policy; an overlay may only tighten it.",
            Enum(&[("clean", "refuse merging a missing or dirty source worktree")]),
            Unset,
        )
        .tighten_only(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    /// One enum-typed key, the variant list the registry offers for it, and
    /// the parser that must accept every one of them.
    type ParserCase = (&'static str, Variants, fn(&str) -> bool);

    /// A [`SettingLookup`] backed by a map, for validator tests.
    struct FakeConfig(HashMap<String, String>);

    impl FakeConfig {
        fn new(pairs: &[(&str, &str)]) -> Self {
            Self(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        }
    }

    impl SettingLookup for FakeConfig {
        fn effective(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
    }

    #[test]
    fn every_declared_default_parses_under_its_own_type() {
        // Computed defaults are held to the same bar as fixed ones: whatever
        // a row falls back to has to be a value the user could type back, or
        // `daft config get` prints something `daft config set` refuses.
        for spec in all_specs() {
            let Some(value) = spec.default.value() else {
                continue;
            };
            assert!(
                spec.ty.validate(value).is_ok(),
                "{}: default {value:?} does not parse as {:?}: {:?}",
                spec.key,
                spec.ty,
                spec.ty.validate(value)
            );
        }
    }

    #[test]
    fn a_computed_default_states_its_rule_separately_from_its_value() {
        for spec in all_specs() {
            match spec.default {
                DefaultDesc::Computed { value, rule } => {
                    assert!(!value.is_empty(), "{}: computed value is empty", spec.key);
                    assert!(!rule.is_empty(), "{}: computed rule is empty", spec.key);
                    assert!(
                        !value.contains('='),
                        "{}: {value:?} reads like a rule, not a value a user can type",
                        spec.key
                    );
                }
                DefaultDesc::Fixed(_) | DefaultDesc::Unset => {
                    assert!(spec.default.rule().is_none());
                }
            }
        }
    }

    #[test]
    fn every_enum_variant_round_trips_through_the_real_parser() {
        // The registry points at each type's `variants()`, so this asserts
        // the pairing is live: a variant the parser rejects means the two
        // lists have drifted apart.
        let cases: Vec<ParserCase> = vec![
            (
                keys::START_FORK_NAMING,
                crate::core::settings::ForkNaming::variants(),
                |v| crate::core::settings::ForkNaming::parse(v).is_some(),
            ),
            (
                keys::PUSH_VERIFY,
                crate::core::settings::PushVerify::variants(),
                |v| crate::core::settings::PushVerify::parse(v).is_some(),
            ),
            (
                keys::SYNC_PUSH_HOOK_STRATEGY,
                crate::core::settings::PushHookStrategy::variants(),
                |v| crate::core::settings::PushHookStrategy::parse(v).is_some(),
            ),
            (
                keys::GOVERNOR_MODE,
                crate::core::settings::GovernorMode::variants(),
                |v| crate::core::settings::GovernorMode::parse(v).is_some(),
            ),
            (
                keys::PRUNE_CD_TARGET,
                crate::core::settings::PruneCdTarget::variants(),
                |v| crate::core::settings::PruneCdTarget::parse(v).is_some(),
            ),
            (
                keys::OWNERSHIP_STRATEGY,
                crate::core::ownership::OwnershipStrategy::variants(),
                |v| crate::core::ownership::OwnershipStrategy::parse(v).is_some(),
            ),
            (
                keys::LIST_STAT,
                crate::core::worktree::list::Stat::variants(),
                |v| crate::core::worktree::list::Stat::parse(v).is_some(),
            ),
            (
                keys::MERGE_ADOPT_TARGET_ON_DEMAND,
                crate::core::worktree::merge::AdoptPreset::variants(),
                |v| crate::core::worktree::merge::AdoptPreset::parse(v).is_some(),
            ),
            (
                keys::hooks::DEFAULT_TRUST,
                crate::hooks::TrustLevel::variants(),
                |v| crate::hooks::TrustLevel::parse(v).is_some(),
            ),
        ];

        for (key, variants, parses) in cases {
            assert!(!variants.is_empty(), "{key}: no variants");
            for (value, gloss) in variants {
                assert!(
                    parses(value),
                    "{key}: parser rejects its own variant {value:?}"
                );
                assert!(!gloss.is_empty(), "{key}: variant {value:?} has no gloss");
            }
        }

        // MergeStyle and CleanupKind round-trip through `as_str` rather than
        // a `parse`, so check that direction instead.
        for (value, _) in crate::core::worktree::merge::MergeStyle::variants() {
            assert!(
                crate::core::worktree::merge::MergeStyle::variants()
                    .iter()
                    .any(|(v, _)| v == value),
                "merge style {value:?} missing"
            );
        }
        for (value, _) in crate::hooks::FailMode::variants() {
            assert!(
                crate::hooks::FailMode::parse(value).is_some(),
                "fail mode {value:?} does not parse"
            );
        }
    }

    #[test]
    fn enum_rows_accept_their_variants_and_reject_others() {
        for spec in all_specs() {
            let ValueType::Enum(variants) = spec.ty else {
                continue;
            };
            for (value, _) in variants {
                assert!(
                    spec.ty.validate(value).is_ok(),
                    "{}: rejects its own variant {value:?}",
                    spec.key
                );
            }
            assert!(
                spec.ty.validate("definitely-not-a-variant").is_err(),
                "{}: accepts a bogus value",
                spec.key
            );
        }
    }

    #[test]
    fn per_hook_expansion_covers_every_hook_and_setting() {
        let specs = per_hook_specs();
        assert_eq!(specs.len(), 14, "7 hook types x 2 settings");

        for &hook in HookType::all() {
            for (setting, suffix) in [
                (HookSetting::Enabled, "enabled"),
                (HookSetting::FailMode, "failMode"),
            ] {
                let expected = format!("daft.hooks.{}.{suffix}", hook.config_key());
                let spec = specs
                    .iter()
                    .find(|s| s.key == expected)
                    .unwrap_or_else(|| panic!("missing per-hook row {expected}"));
                assert_eq!(spec.form, KeyForm::PerHook { hook, setting });
                assert_eq!(spec.category, Category::Hooks);
            }
        }
    }

    #[test]
    fn per_hook_git_keys_are_camel_case_and_yml_labels_are_dash_case() {
        // The naming split is the trap this encodes: the git subsection is
        // camelCase while the same hook's daft.yml key is dash-case. A row
        // that labels itself with the git spelling would send a user looking
        // for `worktreePostCreate:` in a file that spells it
        // `worktree-post-create:`.
        for spec in per_hook_specs() {
            let KeyForm::PerHook { hook, .. } = spec.form else {
                unreachable!()
            };
            assert!(
                spec.key.contains(hook.config_key()),
                "{}: git key must use the camelCase subsection",
                spec.key
            );
            assert!(
                spec.label.starts_with(hook.yaml_name()),
                "{}: label must use the dash-case hook name, got {:?}",
                spec.key,
                spec.label
            );
        }
    }

    #[test]
    fn per_hook_fail_mode_defaults_match_the_hook_types() {
        for spec in per_hook_specs() {
            let KeyForm::PerHook { hook, setting } = spec.form else {
                unreachable!()
            };
            if setting == HookSetting::FailMode {
                assert_eq!(
                    spec.default,
                    DefaultDesc::Fixed(hook.default_fail_mode().as_str()),
                    "{}: default drifted from HookType::default_fail_mode",
                    spec.key
                );
            }
        }
    }

    #[test]
    fn renamed_hooks_carry_their_deprecated_alias() {
        for spec in per_hook_specs() {
            let KeyForm::PerHook { hook, .. } = spec.form else {
                unreachable!()
            };
            match hook.deprecated_config_key() {
                Some(dep) => {
                    let alias = spec
                        .deprecated_alias
                        .as_deref()
                        .unwrap_or_else(|| panic!("{}: expected an alias", spec.key));
                    assert!(
                        alias.contains(dep),
                        "{}: alias {alias} does not name {dep}",
                        spec.key
                    );
                }
                None => assert!(
                    spec.deprecated_alias.is_none(),
                    "{}: hook was never renamed but carries an alias",
                    spec.key
                ),
            }
        }
    }

    #[test]
    fn registry_totals_hold() {
        let specs = all_specs();
        let git = specs
            .iter()
            .filter(|s| s.backend == Backend::GitConfig)
            .count();
        assert_eq!(git, 77, "63 fixed git keys + 14 per-hook");
        assert_eq!(
            specs.iter().filter(|s| s.form == KeyForm::Layout).count(),
            1
        );
        assert_eq!(specs.iter().filter(|s| s.form == KeyForm::Yaml).count(), 10);
    }

    #[test]
    fn keys_are_unique() {
        let mut seen = HashSet::new();
        for spec in all_specs() {
            assert!(
                seen.insert(spec.key.to_string()),
                "duplicate registry key {}",
                spec.key
            );
        }
    }

    #[test]
    fn every_category_has_rows_and_every_row_has_a_known_category() {
        let specs = all_specs();
        for category in Category::all() {
            assert!(
                specs.iter().any(|s| s.category == *category),
                "{} has no rows",
                category.label()
            );
        }
        // Every row's category is in the display order, so nothing is
        // invisible in the rail.
        for spec in &specs {
            assert!(
                Category::all().contains(&spec.category),
                "{}: category missing from display order",
                spec.key
            );
        }
    }

    #[test]
    fn labels_and_help_read_like_prose() {
        for spec in all_specs() {
            assert!(!spec.label.trim().is_empty(), "{}: empty label", spec.key);
            assert!(
                !spec.label.ends_with('.'),
                "{}: label is a fragment, not a sentence — drop the period",
                spec.key
            );
            assert!(
                spec.label.chars().count() <= 40,
                "{}: label too long for the list column",
                spec.key
            );

            assert!(!spec.help.trim().is_empty(), "{}: empty help", spec.key);
            assert!(
                spec.help.ends_with('.'),
                "{}: help is a sentence and needs a period",
                spec.key
            );
            assert!(
                spec.help
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit()),
                "{}: help should start with a capital",
                spec.key
            );
            assert!(
                spec.help.chars().count() <= 120,
                "{}: help should stay one line",
                spec.key
            );
        }
    }

    #[test]
    fn git_keys_are_namespaced_and_yml_paths_are_not() {
        for spec in all_specs() {
            match spec.backend {
                Backend::GitConfig => assert!(
                    spec.key.starts_with("daft."),
                    "{}: git keys live under daft.",
                    spec.key
                ),
                Backend::DaftYml { path, .. } => {
                    assert_eq!(spec.key, path);
                    assert!(
                        !spec.key.starts_with("daft."),
                        "{}: a daft.yml path must not look like a git key",
                        spec.key
                    );
                }
                Backend::LayoutChain => assert_eq!(spec.key, "layout"),
            }
        }
    }

    #[test]
    fn global_only_rows_are_the_two_that_read_global_config() {
        let global: Vec<String> = all_specs()
            .into_iter()
            .filter(|s| s.global_only)
            .map(|s| s.key.to_string())
            .collect();
        assert_eq!(
            global,
            vec![
                keys::hooks::TRUST_PRUNE.to_string(),
                keys::UPDATE_CHECK.to_string()
            ]
        );
    }

    #[test]
    fn merge_pair_validation_refuses_the_incompatible_combination() {
        let spec = find(keys::MERGE_COMMIT).unwrap();
        let validate = spec.validate.unwrap();

        let removing = FakeConfig::new(&[(keys::MERGE_CLEANUP, "remove-branch")]);
        let err = validate(&removing, "false").unwrap_err();
        assert!(err.contains("remove-branch"), "unhelpful refusal: {err}");
        assert!(validate(&removing, "true").is_ok());

        let keeping = FakeConfig::new(&[(keys::MERGE_CLEANUP, "keep")]);
        assert!(validate(&keeping, "false").is_ok());
    }

    #[test]
    fn merge_pair_validation_refuses_from_the_cleanup_side_too() {
        let spec = find(keys::MERGE_CLEANUP).unwrap();
        let validate = spec.validate.unwrap();

        let no_commit = FakeConfig::new(&[(keys::MERGE_COMMIT, "false")]);
        assert!(validate(&no_commit, "remove-branch").is_err());
        assert!(validate(&no_commit, "keep").is_ok());

        // Nothing set: merge.commit defaults to true, so cleanup is free.
        let empty = FakeConfig::new(&[]);
        assert!(validate(&empty, "remove-branch").is_ok());
    }

    #[test]
    fn inherited_and_aliased_rows_name_real_keys() {
        let specs = all_specs();
        let known: HashSet<&str> = specs.iter().map(|s| s.key.as_ref()).collect();

        for spec in &specs {
            if let Some(parent) = spec.inherits {
                assert!(
                    known.contains(parent),
                    "{}: inherits from unknown key {parent}",
                    spec.key
                );
            }
            if let Some(alias) = &spec.deprecated_alias {
                assert!(
                    !known.contains(alias.as_ref()),
                    "{}: alias {alias} is also a live row",
                    spec.key
                );
            }
        }
    }

    #[test]
    fn value_type_validation_rejects_the_obvious_mistakes() {
        assert!(ValueType::Bool.validate("yes").is_ok());
        assert!(ValueType::Bool.validate("maybe").is_err());

        assert!(ValueType::Int.validate("300").is_ok());
        assert!(ValueType::Int.validate("-1").is_err());

        assert!(ValueType::IntOrAuto.validate("auto").is_ok());
        assert!(ValueType::IntOrAuto.validate("0").is_err());

        assert!(
            ValueType::Duration(DurationDialect::BareSeconds)
                .validate("90")
                .is_ok()
        );
        assert!(
            ValueType::Duration(DurationDialect::BareSeconds)
                .validate("off")
                .is_ok()
        );
        // The yml dialect requires a unit — a bare number there would silently
        // mean something else.
        assert!(
            ValueType::Duration(DurationDialect::Suffixed)
                .validate("90")
                .is_err()
        );
        assert!(
            ValueType::Duration(DurationDialect::Suffixed)
                .validate("7d")
                .is_ok()
        );

        assert!(ValueType::Size.validate("10MB").is_ok());
        assert!(ValueType::Size.validate("10 gigs").is_err());

        assert!(ValueType::SizeOrPct.validate("15%").is_ok());
        assert!(ValueType::SizeOrPct.validate("2G").is_ok());
        assert!(ValueType::SizeOrPct.validate("half").is_err());

        assert!(ValueType::Str.validate("  ").is_err());
    }

    #[test]
    fn text_types_offer_a_format_hint_and_pick_lists_do_not() {
        for spec in all_specs() {
            match spec.ty {
                ValueType::Bool | ValueType::TriBool | ValueType::Enum(_) => assert!(
                    spec.ty.format_hint().is_none(),
                    "{}: a pick list needs no format hint",
                    spec.key
                ),
                _ => assert!(
                    spec.ty.format_hint().is_some(),
                    "{}: a typed text field needs a format hint",
                    spec.key
                ),
            }
        }
    }

    #[test]
    fn only_shared_is_managed_elsewhere() {
        let managed: Vec<String> = all_specs()
            .into_iter()
            .filter(|s| !s.is_writable())
            .map(|s| s.key.to_string())
            .collect();
        assert_eq!(managed, vec!["shared".to_string()]);
    }

    #[test]
    fn tighten_only_is_reserved_for_the_merge_gate() {
        let tighten: Vec<String> = all_specs()
            .into_iter()
            .filter(|s| {
                matches!(
                    s.backend,
                    Backend::DaftYml {
                        tighten_only: true,
                        ..
                    }
                )
            })
            .map(|s| s.key.to_string())
            .collect();
        assert_eq!(
            tighten,
            vec!["merge.ff".to_string(), "merge.source_worktree".to_string()]
        );
    }

    #[test]
    fn find_matches_keys_from_every_backend() {
        assert!(find(keys::AUTOCD).is_some());
        assert!(find("daft.hooks.worktreePostCreate.failMode").is_some());
        assert!(find("layout").is_some());
        assert!(find("log.retention").is_some());
        // Hyphenated on purpose: the xtask drift gate treats every
        // key-shaped `daft.*` literal in src/ as a setting that owes the
        // registry a row, and a hyphen is how a fixture opts out.
        assert!(find("daft.no-such-key").is_none());
    }
}
